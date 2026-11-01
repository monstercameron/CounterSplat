//! Bounded outstanding work with completion-driven permits.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

/// Limits concurrent submissions without requiring their completion on the submitting thread.
#[derive(Clone)]
pub struct InFlight {
    count: Arc<AtomicUsize>,
    limit: usize,
}

impl InFlight {
    pub fn new(limit: usize) -> Result<Self, &'static str> {
        if limit == 0 {
            return Err("in-flight limit must be positive");
        }
        Ok(Self {
            count: Arc::new(AtomicUsize::new(0)),
            limit,
        })
    }

    pub fn count(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }
    pub fn limit(&self) -> usize {
        self.limit
    }
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }
    pub fn is_full(&self) -> bool {
        self.count() >= self.limit
    }

    /// The slot is returned when the permit is dropped, including on error paths.
    pub fn try_acquire(&self) -> Option<InFlightPermit> {
        self.count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                if count < self.limit {
                    Some(count + 1)
                } else {
                    None
                }
            })
            .ok()
            .map(|_| InFlightPermit {
                count: Arc::clone(&self.count),
            })
    }
}

pub struct InFlightPermit {
    count: Arc<AtomicUsize>,
}
impl Drop for InFlightPermit {
    fn drop(&mut self) {
        let previous = self.count.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_permits_release_on_completion_and_survive_the_owner() {
        assert!(InFlight::new(0).is_err());
        let gate = InFlight::new(2).unwrap();
        assert!(gate.is_empty());
        let first = gate.try_acquire().unwrap();
        let second = gate.try_acquire().unwrap();
        assert!(gate.is_full());
        assert!(gate.try_acquire().is_none());
        drop(first);
        assert_eq!(gate.count(), 1);
        let third = gate.try_acquire().unwrap();
        assert!(gate.is_full());
        drop(second);
        drop(gate); // A completion can arrive after the host releases its handle.
        drop(third);
    }

    #[test]
    fn submissions_across_threads_never_exceed_the_limit() {
        use std::sync::{atomic::AtomicBool, Barrier};
        let gate = InFlight::new(2).unwrap();
        let barrier = Arc::new(Barrier::new(9));
        let exceeded = Arc::new(AtomicBool::new(false));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let gate = gate.clone();
            let barrier = barrier.clone();
            let exceeded = exceeded.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..1000 {
                    if let Some(_permit) = gate.try_acquire() {
                        if gate.count() > gate.limit() {
                            exceeded.store(true, Ordering::Relaxed);
                        }
                        std::thread::yield_now();
                    }
                }
            }));
        }
        barrier.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(!exceeded.load(Ordering::Relaxed));
        assert!(gate.is_empty());
    }
}
