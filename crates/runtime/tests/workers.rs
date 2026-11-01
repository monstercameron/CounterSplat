#![cfg(not(target_arch = "wasm32"))]

use countersplat_runtime::native::{
    ReadyError, ShutdownError, SubmitError, Subsystem, Supervisor, TaskError, Worker,
};
use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[test]
fn state_may_be_non_send_and_is_created_used_and_dropped_on_its_owner_thread() {
    struct Owned {
        value: Rc<Cell<u32>>,
        owner: thread::ThreadId,
        dropped: mpsc::Sender<thread::ThreadId>,
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            self.dropped.send(thread::current().id()).unwrap();
        }
    }
    let (drop_tx, drop_rx) = mpsc::channel();
    let caller = thread::current().id();
    let mut worker = Worker::spawn(
        "state-owner",
        2,
        move || {
            Ok(Owned {
                value: Rc::new(Cell::new(0)),
                owner: thread::current().id(),
                dropped: drop_tx,
            })
        },
        |state: &mut Owned, increment: u32| {
            assert_eq!(state.owner, thread::current().id());
            state.value.set(state.value.get() + increment);
            Ok((thread::current().id(), state.value.get()))
        },
    )
    .unwrap();
    let owner = worker.wait_ready(TIMEOUT).unwrap();
    assert_ne!(owner, caller);
    let client = worker.client();
    assert_eq!(
        client
            .try_submit(3)
            .ok()
            .unwrap()
            .wait_timeout(TIMEOUT)
            .unwrap(),
        (owner, 3)
    );
    assert_eq!(
        client
            .try_submit(4)
            .ok()
            .unwrap()
            .wait_timeout(TIMEOUT)
            .unwrap(),
        (owner, 7)
    );
    worker.shutdown().unwrap();
    assert_eq!(drop_rx.recv_timeout(TIMEOUT).unwrap(), owner);
}

#[test]
fn bounded_queue_reports_backpressure_and_shutdown_drains_accepted_work() {
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut worker = Worker::spawn(
        "bounded",
        1,
        || Ok(()),
        move |_, n: u32| {
            if n == 1 {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            }
            Ok(n * 2)
        },
    )
    .unwrap();
    worker.wait_ready(TIMEOUT).unwrap();
    let client = worker.client();
    let first = client.try_submit(1).ok().unwrap();
    started_rx.recv_timeout(TIMEOUT).unwrap();
    let second = client.try_submit(2).ok().unwrap();
    assert!(matches!(client.try_submit(3), Err(SubmitError::Full(3))));
    worker.request_stop();
    assert!(matches!(client.try_submit(4), Err(SubmitError::Stopped(4))));
    release_tx.send(()).unwrap();
    worker.shutdown().unwrap();
    assert_eq!(first.wait_timeout(TIMEOUT).unwrap(), 2);
    assert_eq!(second.wait_timeout(TIMEOUT).unwrap(), 4);
}

#[test]
fn supervisor_runs_five_subsystems_on_distinct_named_threads() {
    let mut supervisor = Supervisor::new();
    let roles = [
        Subsystem::Assets,
        Subsystem::Rendering,
        Subsystem::Physics,
        Subsystem::Simulation,
        Subsystem::Scripting,
    ];
    let mut clients = Vec::new();
    for role in roles {
        let client = supervisor
            .start(
                role,
                2,
                || Ok(thread::current().name().unwrap().to_owned()),
                |name: &mut String, ()| Ok((thread::current().id(), name.clone())),
            )
            .unwrap();
        clients.push(client);
    }
    let ready = supervisor.wait_ready(TIMEOUT).unwrap();
    let ids: HashSet<_> = ready.iter().map(|(_, id)| *id).collect();
    assert_eq!(ids.len(), 5);
    assert!(!ids.contains(&thread::current().id()));
    for ((_, expected_id), client) in ready.iter().zip(clients) {
        let (id, name) = client
            .try_submit(())
            .ok()
            .unwrap()
            .wait_timeout(TIMEOUT)
            .unwrap();
        assert_eq!(&id, expected_id);
        assert!(name.starts_with("countersplat-"));
    }
    supervisor.shutdown().unwrap();
}

#[test]
fn handler_error_is_reported_without_stopping_worker() {
    let mut worker = Worker::spawn(
        "recoverable",
        2,
        || Ok(()),
        |_, value: u32| {
            if value == 1 {
                Err("bad command".into())
            } else {
                Ok(value)
            }
        },
    )
    .unwrap();
    worker.wait_ready(TIMEOUT).unwrap();
    let client = worker.client();
    assert_eq!(
        client.try_submit(1).ok().unwrap().wait(),
        Err(TaskError::Handler("bad command".into()))
    );
    assert_eq!(client.try_submit(2).ok().unwrap().wait(), Ok(2));
    worker.shutdown().unwrap();
}

#[test]
fn initialization_failure_and_handler_panics_are_reported() {
    let mut failed = Worker::<(), ()>::spawn(
        "bad-init",
        1,
        || -> Result<(), String> { Err("no device".into()) },
        |_, ()| Ok(()),
    )
    .unwrap();
    assert_eq!(
        failed.wait_ready(TIMEOUT),
        Err(ReadyError::InitializationFailed("no device".into()))
    );
    assert_eq!(
        failed.shutdown(),
        Err(ShutdownError::InitializationFailed("no device".into()))
    );

    let mut panicked = Worker::spawn(
        "panic",
        1,
        || Ok(()),
        |_, ()| -> Result<(), String> { panic!("broken handler") },
    )
    .unwrap();
    panicked.wait_ready(TIMEOUT).unwrap();
    let client = panicked.client();
    assert_eq!(
        client.try_submit(()).ok().unwrap().wait_timeout(TIMEOUT),
        Err(TaskError::Panicked("broken handler".into()))
    );
    assert!(matches!(
        client.try_submit(()),
        Err(SubmitError::Stopped(()))
    ));
    assert_eq!(
        panicked.shutdown(),
        Err(ShutdownError::Panicked("broken handler".into()))
    );
}

#[test]
fn timeouts_can_be_retried_and_dropping_tickets_does_not_block_worker() {
    let (release_tx, release_rx) = mpsc::channel();
    let mut worker = Worker::spawn(
        "timeout",
        2,
        || Ok(()),
        move |_, value: u32| {
            if value == 1 {
                release_rx.recv().unwrap();
            }
            Ok(value)
        },
    )
    .unwrap();
    worker.wait_ready(TIMEOUT).unwrap();
    let client = worker.client();
    let pending = client.try_submit(1).ok().unwrap();
    assert_eq!(
        pending.wait_timeout(Duration::from_millis(10)),
        Err(TaskError::Timeout)
    );
    assert_eq!(pending.try_receive(), Ok(None));
    drop(client.try_submit(2).ok().unwrap());
    release_tx.send(()).unwrap();
    assert_eq!(pending.wait_timeout(TIMEOUT), Ok(1));
    assert_eq!(
        client.try_submit(3).ok().unwrap().wait_timeout(TIMEOUT),
        Ok(3)
    );
    worker.shutdown().unwrap();
}
