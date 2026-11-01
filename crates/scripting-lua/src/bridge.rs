use countersplat_ecs::{WorldCommand, WorldOperation, WorldOutcome, WorldReply};
use std::sync::mpsc::{self, Sender, SyncSender, TrySendError};
use std::time::Duration;

/// An owned-data boundary between Lua and the thread that owns the ECS.
///
/// Implementations may use channels, a scheduler's request/reply interface, or
/// a local test harness. Calls are synchronous: an ECS worker must remain
/// available to serve requests while Lua runs. Do not hold an ECS lock while
/// waiting for a script.
pub trait WorldBridge {
    /// Execute one operation on the ECS owner and return its result.
    fn request(&mut self, operation: WorldOperation) -> Result<WorldOutcome, String>;
}

/// A request sent to the ECS owner. Each request has its own reply channel.
///
/// The owner should reply using the result of world.apply_command(command).
/// If the requester has timed out or shut down, sending the reply can fail; the
/// operation may nevertheless already have changed the world.
pub struct WorldRequest {
    /// An owned, serializable ECS command.
    pub command: WorldCommand,
    /// A single-use channel for the matching reply.
    pub reply: Sender<WorldReply>,
}

/// A synchronous channel-based implementation of [WorldBridge].
///
/// The receiver must be polled by an independently running ECS owner. The
/// optional timeout limits waiting, not execution: timed-out commands are not
/// cancelled and may still be applied.
pub struct ChannelBridge {
    sender: SyncSender<WorldRequest>,
    next_id: u64,
    timeout: Option<Duration>,
}

impl ChannelBridge {
    /// Create a bridge which waits until each request is answered or disconnected.
    pub fn new(sender: SyncSender<WorldRequest>) -> Self {
        Self {
            sender,
            next_id: 1,
            timeout: None,
        }
    }

    /// Set a reply timeout. Timed-out requests may still be applied by the ECS.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

impl WorldBridge for ChannelBridge {
    fn request(&mut self, operation: WorldOperation) -> Result<WorldOutcome, String> {
        let request_id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| "ECS request ID space exhausted".to_string())?;
        let (reply, receiver) = mpsc::channel();
        self.sender
            .try_send(WorldRequest {
                command: WorldCommand {
                    request_id,
                    operation,
                },
                reply,
            })
            .map_err(|error| match error {
                TrySendError::Full(_) => "ECS request queue is full".to_string(),
                TrySendError::Disconnected(_) => "ECS worker is unavailable".to_string(),
            })?;
        let response = match self.timeout {
            Some(timeout) => receiver
                .recv_timeout(timeout)
                .map_err(|error| format!("ECS reply unavailable: {error}"))?,
            None => receiver
                .recv()
                .map_err(|_| "ECS worker disconnected before replying".to_string())?,
        };
        if response.request_id != request_id {
            return Err(format!(
                "ECS reply ID mismatch: expected {request_id}, got {}",
                response.request_id
            ));
        }
        response.outcome
    }
}
