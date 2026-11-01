//! Native thread-owned workers and a supervisor for independent engine subsystems.
//!
//! A worker's initializer runs on its own named thread, and only that thread touches or drops the returned state.
//! The state therefore need not be `Send`; only the initializer, handler, commands, and replies cross threads.
//! Each accepted command is handled in FIFO order, unless initialization fails or the handler panics. A bounded
//! queue and nonblocking submission make overload visible to the host. Use [`crate::latest::Latest`] for snapshots
//! that may be replaced, and this queue for commands that must not be silently discarded.
//!
//! Do not block a window event loop on a ticket. Avoid synchronous dependency cycles between workers; for example,
//! a simulation worker must remain free to answer script requests while a script is running. Stop producing new
//! cross-worker requests before shutting down the supervisor. OS threads cannot be forcibly terminated safely, so
//! a handler that never returns can prevent an explicit join from completing.
//!
//! ```
//! use countersplat_runtime::native::Worker;
//! use std::{cell::Cell, rc::Rc, time::Duration};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut worker = Worker::spawn(
//!     "simulation", 32,
//!     || Ok(Rc::new(Cell::new(0))), // Created on the worker, so Rc need not be Send.
//!     |state: &mut Rc<Cell<u32>>, increment| {
//!         state.set(state.get() + increment);
//!         Ok(state.get())
//!     },
//! )?;
//! worker.wait_ready(Duration::from_secs(1))?;
//! let ticket = worker.client().try_submit(3)?;
//! assert_eq!(ticket.wait()?, 3);
//! worker.shutdown()?;
//! # Ok(())
//! # }
//! ```

use std::any::Any;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle, ThreadId};
use std::time::Duration;

// A short timeout wakes an idle worker for shutdown, which is independent of queue capacity.
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(20);

type ReplySender<R> = mpsc::SyncSender<Result<R, TaskError>>;

struct Envelope<C, R> {
    command: C,
    reply: ReplySender<R>,
}

struct Shared<C, R> {
    queue: mpsc::SyncSender<Envelope<C, R>>,
    accepting: Mutex<bool>,
    stopping: AtomicBool,
}

impl<C, R> Shared<C, R> {
    fn stop(&self) {
        // Serialize the change with submitters, so no command can arrive after draining starts.
        let mut accepting = self.accepting.lock().unwrap_or_else(|e| e.into_inner());
        *accepting = false;
        self.stopping.store(true, Ordering::Release);
    }
}

/// A nonblocking producer for one worker's bounded, ordered command queue.
/// Cloning a client does not duplicate worker state.
pub struct Client<C, R> {
    shared: Arc<Shared<C, R>>,
}

impl<C, R> Clone for Client<C, R> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<C, R> Client<C, R> {
    /// Enqueue a command without blocking. Full and closed errors return the original command to the caller.
    /// A ticket can be polled, or waited on from a thread that is allowed to block.
    pub fn try_submit(&self, command: C) -> Result<Ticket<R>, SubmitError<C>> {
        let accepting = self
            .shared
            .accepting
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !*accepting {
            return Err(SubmitError::Stopped(command));
        }
        let (reply, receiver) = mpsc::sync_channel(1);
        match self.shared.queue.try_send(Envelope { command, reply }) {
            Ok(()) => Ok(Ticket { receiver }),
            Err(mpsc::TrySendError::Full(envelope)) => Err(SubmitError::Full(envelope.command)),
            Err(mpsc::TrySendError::Disconnected(envelope)) => {
                Err(SubmitError::Disconnected(envelope.command))
            }
        }
    }

    /// Whether this worker is still accepting submissions. The state can change immediately after this check.
    pub fn is_accepting(&self) -> bool {
        *self
            .shared
            .accepting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
}

/// Submission failed, with the original command preserved for a retry or an explicit fallback.
#[derive(Debug, PartialEq, Eq)]
pub enum SubmitError<C> {
    /// All queue slots are in use. No command was sent.
    Full(C),
    /// Shutdown has begun or the worker terminated. No command was sent.
    Stopped(C),
    /// The receiver closed before the command could be sent. No command was sent.
    Disconnected(C),
}

impl<C> fmt::Display for SubmitError<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Full(_) => "worker queue is full",
            Self::Stopped(_) => "worker has stopped",
            Self::Disconnected(_) => "worker is disconnected",
        })
    }
}
impl<C: fmt::Debug> std::error::Error for SubmitError<C> {}

/// A single response to one accepted command. Dropping a ticket never blocks the worker.
pub struct Ticket<R> {
    receiver: mpsc::Receiver<Result<R, TaskError>>,
}

impl<R> Ticket<R> {
    /// Wait until the handler responds or the worker stops.
    pub fn wait(self) -> Result<R, TaskError> {
        self.receiver
            .recv()
            .unwrap_or(Err(TaskError::WorkerStopped))
    }

    /// Wait at most `timeout`; a timeout leaves the ticket usable for a later retry.
    pub fn wait_timeout(&self, timeout: Duration) -> Result<R, TaskError> {
        match self.receiver.recv_timeout(timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(TaskError::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(TaskError::WorkerStopped),
        }
    }

    /// Poll for a response without waiting. `Ok(None)` means the command is still pending.
    pub fn try_receive(&self) -> Result<Option<R>, TaskError> {
        match self.receiver.try_recv() {
            Ok(Ok(value)) => Ok(Some(value)),
            Ok(Err(error)) => Err(error),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(TaskError::WorkerStopped),
        }
    }
}

/// A command failed or its worker did not provide a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskError {
    /// The handler rejected this command. The worker remains available.
    Handler(String),
    /// The handler panicked. Its worker is stopped; queued tickets subsequently report `WorkerStopped`.
    Panicked(String),
    /// The worker stopped before responding, including when initialization failed.
    WorkerStopped,
    /// No response was available before the deadline. The ticket may still be used.
    Timeout,
}
impl fmt::Display for TaskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Handler(e) => write!(f, "worker handler failed: {e}"),
            Self::Panicked(e) => write!(f, "worker handler panicked: {e}"),
            Self::WorkerStopped => f.write_str("worker stopped before responding"),
            Self::Timeout => f.write_str("timed out waiting for worker"),
        }
    }
}
impl std::error::Error for TaskError {}

/// Failure to create a worker thread.
#[derive(Debug)]
pub enum SpawnError {
    /// A queue must have room for at least one waiting command.
    ZeroCapacity,
    /// The operating system refused to start the thread.
    Thread(std::io::Error),
}
impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroCapacity => f.write_str("worker queue capacity must be nonzero"),
            Self::Thread(e) => write!(f, "cannot start worker thread: {e}"),
        }
    }
}
impl std::error::Error for SpawnError {}

/// Failure or timeout while waiting for the initializer to finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadyError {
    InitializationFailed(String),
    Timeout,
    Disconnected,
}
impl fmt::Display for ReadyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InitializationFailed(e) => write!(f, "worker initialization failed: {e}"),
            Self::Timeout => f.write_str("worker initialization timed out"),
            Self::Disconnected => f.write_str("worker disconnected before initialization"),
        }
    }
}
impl std::error::Error for ReadyError {}

/// Failure reported while joining a stopped worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShutdownError {
    InitializationFailed(String),
    Panicked(String),
}
impl fmt::Display for ShutdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InitializationFailed(e) => write!(f, "worker initialization failed: {e}"),
            Self::Panicked(e) => write!(f, "worker panicked: {e}"),
        }
    }
}
impl std::error::Error for ShutdownError {}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else {
        "non-string panic payload".to_owned()
    }
}

/// One named, long-lived thread with state owned exclusively by that thread.
///
/// Construct one worker per active subsystem. Starting a worker does not itself connect an engine subsystem;
/// the host supplies its initializer and message handler. Requests submitted before readiness wait in the queue.
/// Explicitly call [`Worker::shutdown`] to wait for accepted work and report initialization or panic failures.
/// Dropping this handle requests shutdown but detaches the thread rather than blocking the caller.
pub struct Worker<C, R> {
    client: Client<C, R>,
    ready: mpsc::Receiver<Result<ThreadId, String>>,
    ready_result: Option<Result<ThreadId, String>>,
    join: Option<JoinHandle<Result<(), ShutdownError>>>,
}

impl<C: Send + 'static, R: Send + 'static> Worker<C, R> {
    /// Start a worker immediately. Its initializer runs concurrently on the newly started thread.
    /// `S` is created and destroyed on that thread and is deliberately not required to be `Send`.
    pub fn spawn<S: 'static>(
        name: impl Into<String>,
        capacity: usize,
        initialize: impl FnOnce() -> Result<S, String> + Send + 'static,
        mut handle: impl FnMut(&mut S, C) -> Result<R, String> + Send + 'static,
    ) -> Result<Self, SpawnError> {
        if capacity == 0 {
            return Err(SpawnError::ZeroCapacity);
        }
        let (sender, receiver) = mpsc::sync_channel::<Envelope<C, R>>(capacity);
        let shared = Arc::new(Shared {
            queue: sender,
            accepting: Mutex::new(true),
            stopping: AtomicBool::new(false),
        });
        let worker_shared = Arc::clone(&shared);
        let (ready_sender, ready) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let initialized =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(initialize));
                let mut state = match initialized {
                    Ok(Ok(state)) => state,
                    Ok(Err(error)) => {
                        worker_shared.stop();
                        let _ = ready_sender.try_send(Err(error.clone()));
                        return Err(ShutdownError::InitializationFailed(error));
                    }
                    Err(payload) => {
                        let error = panic_message(payload);
                        worker_shared.stop();
                        let _ =
                            ready_sender.try_send(Err(format!("initializer panicked: {error}")));
                        return Err(ShutdownError::Panicked(error));
                    }
                };
                let _ = ready_sender.try_send(Ok(thread::current().id()));
                loop {
                    // Once submissions are closed, drain every accepted command before dropping the owned state.
                    let envelope = if worker_shared.stopping.load(Ordering::Acquire) {
                        match receiver.try_recv() {
                            Ok(envelope) => envelope,
                            Err(_) => break,
                        }
                    } else {
                        match receiver.recv_timeout(STOP_POLL_INTERVAL) {
                            Ok(envelope) => envelope,
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    };
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        handle(&mut state, envelope.command)
                    }));
                    match result {
                        Ok(result) => {
                            let _ = envelope.reply.try_send(result.map_err(TaskError::Handler));
                        }
                        Err(payload) => {
                            let message = panic_message(payload);
                            worker_shared.stop();
                            let _ = envelope
                                .reply
                                .try_send(Err(TaskError::Panicked(message.clone())));
                            return Err(ShutdownError::Panicked(message));
                        }
                    }
                }
                worker_shared.stop();
                Ok(())
            })
            .map_err(SpawnError::Thread)?;
        Ok(Self {
            client: Client { shared },
            ready,
            ready_result: None,
            join: Some(join),
        })
    }

    /// Create another producer for the worker's queue.
    pub fn client(&self) -> Client<C, R> {
        self.client.clone()
    }

    /// Wait for initialization. Timeouts can be retried; a completed result is retained.
    pub fn wait_ready(&mut self, timeout: Duration) -> Result<ThreadId, ReadyError> {
        if self.ready_result.is_none() {
            match self.ready.recv_timeout(timeout) {
                Ok(result) => self.ready_result = Some(result),
                Err(mpsc::RecvTimeoutError::Timeout) => return Err(ReadyError::Timeout),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(ReadyError::Disconnected),
            }
        }
        match self
            .ready_result
            .as_ref()
            .expect("ready result was just set")
        {
            Ok(id) => Ok(*id),
            Err(error) => Err(ReadyError::InitializationFailed(error.clone())),
        }
    }
}

impl<C, R> Worker<C, R> {
    /// Reject new submissions and allow already accepted commands to finish.
    pub fn request_stop(&self) {
        self.client.shared.stop();
    }

    /// Request a graceful stop, drain accepted commands, then join and report failures.
    pub fn shutdown(mut self) -> Result<(), ShutdownError> {
        self.request_stop();
        match self.join.take() {
            Some(join) => match join.join() {
                Ok(result) => result,
                Err(payload) => Err(ShutdownError::Panicked(panic_message(payload))),
            },
            None => Ok(()),
        }
    }
}

impl<C, R> Drop for Worker<C, R> {
    fn drop(&mut self) {
        self.client.shared.stop();
    }
}

/// Common subsystem roles. Custom roles keep extensions independent of the runtime crate.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Subsystem {
    Assets,
    Rendering,
    Physics,
    Simulation,
    Scripting,
    Other(String),
}
impl Subsystem {
    fn thread_name(&self) -> String {
        match self {
            Self::Assets => "countersplat-assets".into(),
            Self::Rendering => "countersplat-rendering".into(),
            Self::Physics => "countersplat-physics".into(),
            Self::Simulation => "countersplat-simulation".into(),
            Self::Scripting => "countersplat-scripting".into(),
            Self::Other(name) => format!("countersplat-{name}"),
        }
    }
}

trait ManagedWorker: Send {
    fn ready(&mut self, timeout: Duration) -> Result<ThreadId, ReadyError>;
    fn stop(&self);
    fn join(self: Box<Self>) -> Result<(), ShutdownError>;
}
impl<C: Send + 'static, R: Send + 'static> ManagedWorker for Worker<C, R> {
    fn ready(&mut self, timeout: Duration) -> Result<ThreadId, ReadyError> {
        self.wait_ready(timeout)
    }
    fn stop(&self) {
        self.request_stop();
    }
    fn join(self: Box<Self>) -> Result<(), ShutdownError> {
        (*self).shutdown()
    }
}

/// Starts independently typed subsystem workers and coordinates their lifetime.
/// All workers are asked to stop before any join begins. Stop external producers and quiesce cross-worker
/// calls first: a worker shutting down cannot promise to answer requests sent by another worker afterward.
#[derive(Default)]
pub struct Supervisor {
    workers: Vec<(Subsystem, Box<dyn ManagedWorker>)>,
}

/// A subsystem could not be registered.
#[derive(Debug)]
pub enum SupervisorStartError {
    Duplicate(Subsystem),
    Spawn(SpawnError),
}
impl fmt::Display for SupervisorStartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate(role) => write!(f, "subsystem already started: {role:?}"),
            Self::Spawn(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for SupervisorStartError {}

/// Identifies the worker that failed to become ready.
#[derive(Debug)]
pub struct SupervisorReadyError {
    pub subsystem: Subsystem,
    pub error: ReadyError,
}
impl fmt::Display for SupervisorReadyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.subsystem, self.error)
    }
}
impl std::error::Error for SupervisorReadyError {}

/// Errors from all workers that failed while stopping.
#[derive(Debug)]
pub struct SupervisorShutdownError {
    pub failures: Vec<(Subsystem, ShutdownError)>,
}
impl fmt::Display for SupervisorShutdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} subsystem(s) failed during shutdown",
            self.failures.len()
        )
    }
}
impl std::error::Error for SupervisorShutdownError {}

impl Supervisor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register and start a named subsystem. The returned typed client is safe to move to other workers.
    pub fn start<C: Send + 'static, R: Send + 'static, S: 'static>(
        &mut self,
        subsystem: Subsystem,
        capacity: usize,
        initialize: impl FnOnce() -> Result<S, String> + Send + 'static,
        handle: impl FnMut(&mut S, C) -> Result<R, String> + Send + 'static,
    ) -> Result<Client<C, R>, SupervisorStartError> {
        if self
            .workers
            .iter()
            .any(|(existing, _)| existing == &subsystem)
        {
            return Err(SupervisorStartError::Duplicate(subsystem));
        }
        let worker = Worker::spawn(subsystem.thread_name(), capacity, initialize, handle)
            .map_err(SupervisorStartError::Spawn)?;
        let client = worker.client();
        self.workers.push((subsystem, Box::new(worker)));
        Ok(client)
    }

    /// Wait for each registered initializer, allowing up to `timeout_per_worker` for each one.
    pub fn wait_ready(
        &mut self,
        timeout_per_worker: Duration,
    ) -> Result<Vec<(Subsystem, ThreadId)>, SupervisorReadyError> {
        let mut ready = Vec::with_capacity(self.workers.len());
        for (subsystem, worker) in &mut self.workers {
            let id = worker
                .ready(timeout_per_worker)
                .map_err(|error| SupervisorReadyError {
                    subsystem: subsystem.clone(),
                    error,
                })?;
            ready.push((subsystem.clone(), id));
        }
        Ok(ready)
    }

    /// Close every command queue before joining any worker, then report all failures.
    pub fn shutdown(mut self) -> Result<(), SupervisorShutdownError> {
        for (_, worker) in &self.workers {
            worker.stop();
        }
        let mut failures = Vec::new();
        for (subsystem, worker) in self.workers.drain(..) {
            if let Err(error) = worker.join() {
                failures.push((subsystem, error));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(SupervisorShutdownError { failures })
        }
    }
}
