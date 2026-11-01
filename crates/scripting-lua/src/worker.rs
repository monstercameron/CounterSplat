use crate::{LuaRuntime, ScriptOptions, WorldBridge};
use serde_json::Value;
use std::{
    sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError},
    thread::{self, JoinHandle},
};

/// A response to one submitted script request.
///
/// The host may wait or poll without borrowing the scripting worker. An error
/// from receiving means the scripting worker exited before sending its reply.
pub type ScriptReply<T> = Receiver<Result<T, String>>;

enum Work {
    Execute(String, Sender<Result<(), String>>),
    RunSystem(String, f64, Sender<Result<(), String>>),
    RunSystems(f64, Sender<Result<(), String>>),
    Systems(Sender<Result<Vec<String>, String>>),
    Dispatch(String, Value, Sender<Result<(), String>>),
    Shutdown,
}

/// Owns a dedicated native scripting thread and submits work to its Lua VM.
///
/// Submission is asynchronous; the caller decides when to receive each reply.
/// Keep the ECS owner servicing bridge requests until all scripts have finished.
pub struct ScriptWorker {
    sender: SyncSender<Work>,
    thread: Option<JoinHandle<()>>,
}

impl ScriptWorker {
    /// Start a worker, constructing the Lua VM inside the new thread.
    pub fn spawn<B: WorldBridge + Send + 'static>(
        bridge: B,
        options: ScriptOptions,
    ) -> Result<Self, String> {
        if options.worker_queue_capacity == 0 {
            return Err("worker queue capacity must be greater than zero".into());
        }
        let (sender, receiver) = mpsc::sync_channel(options.worker_queue_capacity);
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("countersplat-lua".into())
            .spawn(move || {
                let runtime = match LuaRuntime::new(bridge, options) {
                    Ok(runtime) => {
                        let _ = startup_sender.send(Ok(()));
                        runtime
                    }
                    Err(error) => {
                        let _ = startup_sender.send(Err(error.to_string()));
                        return;
                    }
                };
                while let Ok(work) = receiver.recv() {
                    match work {
                        Work::Execute(source, reply) => {
                            let _ = reply.send(runtime.execute(&source).map_err(|e| e.to_string()));
                        }
                        Work::RunSystem(name, dt, reply) => {
                            let _ = reply
                                .send(runtime.run_system(&name, dt).map_err(|e| e.to_string()));
                        }
                        Work::RunSystems(dt, reply) => {
                            let _ = reply.send(runtime.run_systems(dt).map_err(|e| e.to_string()));
                        }
                        Work::Systems(reply) => {
                            let _ = reply.send(Ok(runtime.systems()));
                        }
                        Work::Dispatch(name, payload, reply) => {
                            let _ = reply
                                .send(runtime.dispatch(&name, &payload).map_err(|e| e.to_string()));
                        }
                        Work::Shutdown => break,
                    }
                }
            })
            .map_err(|error| format!("could not start Lua worker: {error}"))?;
        match startup_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                sender,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err("Lua worker exited during startup".into())
            }
        }
    }

    fn submit<T>(
        &self,
        make: impl FnOnce(Sender<Result<T, String>>) -> Work,
    ) -> Result<ScriptReply<T>, String> {
        let (sender, receiver) = mpsc::channel();
        self.sender
            .try_send(make(sender))
            .map_err(|error| match error {
                TrySendError::Full(_) => "Lua worker queue is full".to_string(),
                TrySendError::Disconnected(_) => "Lua worker is unavailable".to_string(),
            })?;
        Ok(receiver)
    }

    /// Queue a script for execution.
    pub fn execute(&self, source: impl Into<String>) -> Result<ScriptReply<()>, String> {
        self.submit(|reply| Work::Execute(source.into(), reply))
    }

    /// Queue a named system, passing elapsed seconds.
    pub fn run_system(&self, name: impl Into<String>, dt: f64) -> Result<ScriptReply<()>, String> {
        self.submit(|reply| Work::RunSystem(name.into(), dt, reply))
    }

    /// Queue all registered systems in registration order.
    pub fn run_systems(&self, dt: f64) -> Result<ScriptReply<()>, String> {
        self.submit(|reply| Work::RunSystems(dt, reply))
    }

    /// Request the registered system names.
    pub fn systems(&self) -> Result<ScriptReply<Vec<String>>, String> {
        self.submit(Work::Systems)
    }

    /// Queue a host event for the registered Lua event handlers.
    pub fn dispatch(
        &self,
        name: impl Into<String>,
        payload: Value,
    ) -> Result<ScriptReply<()>, String> {
        self.submit(|reply| Work::Dispatch(name.into(), payload, reply))
    }

    /// Request shutdown after queued work and wait for the scripting thread.
    ///
    /// The ECS owner must continue servicing requests until this returns.
    pub fn shutdown(mut self) -> Result<(), String> {
        let _ = self.sender.send(Work::Shutdown);
        self.thread
            .take()
            .expect("worker thread exists")
            .join()
            .map_err(|_| "Lua worker panicked".to_string())
    }
}

impl Drop for ScriptWorker {
    fn drop(&mut self) {
        // Explicit shutdown joins the thread. Drop only requests shutdown so
        // it cannot deadlock an owner which is still serving an ECS request.
        if self.thread.is_some() {
            let _ = self.sender.try_send(Work::Shutdown);
        }
    }
}
