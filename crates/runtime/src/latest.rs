//! A latest-value mailbox for replaceable snapshots such as render state.
//!
//! Unlike the reliable FIFO command queues, publishing replaces the previous value. A slow consumer can skip old
//! frames instead of forcing the simulation to wait. Use commands, not this mailbox, for work that must not be lost.

use std::sync::{Arc, Mutex};

struct State<T> {
    sequence: u64,
    value: Option<Arc<T>>,
}

/// A cloneable single-slot mailbox. Sequence numbers increase whenever a value is published.
pub struct Latest<T> {
    inner: Arc<Mutex<State<T>>>,
}

impl<T> Clone for Latest<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> Default for Latest<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Latest<T> {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(State {
                sequence: 0,
                value: None,
            })),
        }
    }

    /// Replaces the current snapshot and returns its sequence number. Old readers retain their `Arc` safely.
    pub fn publish(&self, value: T) -> u64 {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.sequence = state
            .sequence
            .checked_add(1)
            .expect("snapshot sequence exhausted");
        state.value = Some(Arc::new(value));
        state.sequence
    }

    /// Returns the current snapshot only when its sequence is newer than `last_seen`.
    pub fn after(&self, last_seen: u64) -> Option<(u64, Arc<T>)> {
        let state = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.sequence <= last_seen {
            return None;
        }
        state
            .value
            .as_ref()
            .map(|value| (state.sequence, Arc::clone(value)))
    }
}

#[cfg(test)]
mod tests {
    use super::Latest;
    #[test]
    fn readers_see_only_the_newest_snapshot_and_can_keep_an_older_one() {
        let mailbox = Latest::new();
        assert!(mailbox.after(0).is_none());
        assert_eq!(mailbox.publish("first"), 1);
        let (_, held) = mailbox.after(0).unwrap();
        assert_eq!(mailbox.publish("second"), 2);
        assert_eq!(mailbox.publish("third"), 3);
        let (version, latest) = mailbox.after(1).unwrap();
        assert_eq!(version, 3);
        assert_eq!(*latest, "third");
        assert_eq!(*held, "first");
        assert!(mailbox.after(3).is_none());
    }
}
