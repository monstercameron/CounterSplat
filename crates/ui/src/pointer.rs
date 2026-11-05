//! Gameplay pointer state from browser Pointer Events and pointer lock, independent of the DOM: the host passes the
//! event fields and acts on the returned transitions.

/// The fields of a Pointer Event the gameplay buttons need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerSample<'a> {
    /// `pointerdown`, `pointermove`, `pointerup`, ...
    pub kind: &'a str,
    /// The button that changed (`-1` for none).
    pub button: i32,
    /// Bit mask of the buttons held.
    pub buttons: u32,
    /// `mouse`, `touch` or `pen`.
    pub pointer_type: &'a str,
}

/// One mouse button held for gameplay (right: zoom; left: fire). A press that begins over UI stays blocked until it
/// is released, even when dragged into play; browsers report a second button changing while another is held as a
/// `pointermove`, which counts too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameplayButton {
    button: i32,
    mask: u32,
    pub down: bool,
    blocked: bool,
    /// Whether `reset` reports a release for a held button (fire does; zoom does not).
    release_on_reset: bool,
}

impl GameplayButton {
    pub fn right() -> Self {
        Self { button: 2, mask: 2, down: false, blocked: false, release_on_reset: false }
    }

    pub fn left() -> Self {
        Self { button: 0, mask: 1, down: false, blocked: false, release_on_reset: true }
    }

    /// Applies a pointer event; returns `Some(pressed)` when the gameplay state changes. `allow_press` is false while
    /// the pointer is over UI.
    pub fn update(&mut self, event: PointerSample<'_>, allow_press: bool) -> Option<bool> {
        if event.pointer_type != "mouse" {
            return None;
        }
        if event.buttons & self.mask == 0 {
            let was_down = self.down;
            self.down = false;
            self.blocked = false;
            return was_down.then_some(false);
        }
        // A fresh pointerdown is a new physical press, even after a focus reset.
        if event.kind == "pointerdown" && event.button == self.button && !self.down {
            self.blocked = false;
        }
        if !self.down && !self.blocked && event.button == self.button {
            if !allow_press {
                self.blocked = true;
                return None;
            }
            self.down = true;
            return Some(true);
        }
        None
    }

    /// After pause, focus loss or pointer cancel: a held button is not reactivated by movement until it is pressed
    /// again. Returns `Some(false)` when the release should be reported.
    pub fn reset(&mut self) -> Option<bool> {
        let was_down = self.down;
        self.blocked = self.blocked || self.down;
        self.down = false;
        (was_down && self.release_on_reset).then_some(false)
    }
}

/// What a pointer-lock change means for the game (see `PointerLock::changed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockChange {
    Locked,
    /// A late lock arrived after the request was withdrawn; the host must exit it.
    Cancelled,
    Unlocked,
    /// The game released it.
    Released,
    /// The browser released it (Escape), not the game.
    Unexpected,
}

impl LockChange {
    pub fn name(self) -> &'static str {
        match self {
            Self::Locked => "locked",
            Self::Cancelled => "cancelled",
            Self::Unlocked => "unlocked",
            Self::Released => "released",
            Self::Unexpected => "unexpected",
        }
    }
}

/// Pointer-lock bookkeeping, including cancelling a request still in flight.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PointerLock {
    pub requested: bool,
    pub failed: bool,
    had_lock: bool,
    expected_unlock: bool,
    pub sequence: u64,
}

impl PointerLock {
    /// A lock request was issued; returns its sequence number (a rejection only counts for the latest one).
    pub fn request(&mut self) -> u64 {
        self.sequence += 1;
        self.requested = true;
        self.sequence
    }

    /// The request failed or the browser cannot lock.
    pub fn error(&mut self) {
        self.failed = true;
        self.requested = false;
    }

    /// Rejection of request `sequence`; ignored when a newer request or a release came after it.
    pub fn rejected(&mut self, sequence: u64) {
        if self.sequence == sequence {
            self.error();
        }
    }

    /// The game releases the lock; returns whether the host must exit pointer lock now.
    pub fn release(&mut self, locked: bool) -> bool {
        self.sequence += 1;
        self.requested = false;
        if locked {
            self.expected_unlock = true;
        }
        locked
    }

    /// On `pointerlockchange`: `locked` is whether the canvas holds the lock now, `allow_capture` whether gameplay
    /// may capture. For `Cancelled` the host must exit pointer lock.
    pub fn changed(&mut self, locked: bool, allow_capture: bool) -> LockChange {
        if locked {
            if !self.requested || !allow_capture {
                self.expected_unlock = true;
                return LockChange::Cancelled;
            }
            self.had_lock = true;
            self.failed = false;
            return LockChange::Locked;
        }
        self.requested = false;
        if !self.had_lock {
            self.expected_unlock = false;
            return LockChange::Unlocked;
        }
        self.had_lock = false;
        if self.expected_unlock {
            self.expected_unlock = false;
            return LockChange::Released;
        }
        LockChange::Unexpected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pointer<'a>(kind: &'a str, button: i32, buttons: u32, pointer_type: &'a str) -> PointerSample<'a> {
        PointerSample { kind, button, buttons, pointer_type }
    }

    #[test]
    fn right_button_ui_gating_chords_and_reset() {
        let mut events = Vec::new();
        let mut b = GameplayButton::right();
        let mut run = |b: &mut GameplayButton, e: PointerSample<'_>, allow: bool| {
            if let Some(pressed) = b.update(e, allow) {
                events.push(pressed);
            }
        };
        // A UI press never turns into zoom when dragged out of the UI.
        run(&mut b, pointer("pointerdown", 2, 2, "mouse"), false);
        run(&mut b, pointer("pointermove", -1, 2, "mouse"), true);
        run(&mut b, pointer("pointerup", 2, 0, "mouse"), false);
        // Normal hold/release.
        run(&mut b, pointer("pointerdown", 2, 2, "mouse"), true);
        run(&mut b, pointer("pointermove", -1, 2, "mouse"), true);
        run(&mut b, pointer("pointerup", 2, 0, "mouse"), false);
        // With another button held, changes arrive as pointermove.
        run(&mut b, pointer("pointerdown", 0, 1, "mouse"), true);
        run(&mut b, pointer("pointermove", 2, 3, "mouse"), true);
        run(&mut b, pointer("pointermove", 2, 1, "mouse"), true);
        // A focus reset cancels the hold; movement cannot reactivate it until a new press.
        run(&mut b, pointer("pointerdown", 2, 2, "mouse"), true);
        assert_eq!(b.reset(), None);
        run(&mut b, pointer("pointermove", -1, 2, "mouse"), true);
        run(&mut b, pointer("pointerup", 2, 0, "mouse"), false);
        run(&mut b, pointer("pointerdown", 2, 2, "mouse"), true);
        run(&mut b, pointer("pointerup", 2, 0, "mouse"), false);
        run(&mut b, pointer("pointerdown", 2, 2, "touch"), true);
        assert!(!b.down);
        assert_eq!(events, [true, false, true, false, true, true, false]);
    }

    #[test]
    fn fire_button_reports_release_on_reset() {
        let mut fire = GameplayButton::left();
        assert_eq!(fire.update(pointer("pointerdown", 0, 1, "mouse"), true), Some(true));
        assert_eq!(fire.reset(), Some(false));
        assert_eq!(fire.update(pointer("pointermove", -1, 1, "mouse"), true), None);
    }

    #[test]
    fn pointer_lock_requests_releases_and_cancellations() {
        let mut lock = PointerLock::default();
        lock.request();
        assert!(lock.requested);
        assert_eq!(lock.changed(true, true), LockChange::Locked);
        assert!(lock.release(true));
        assert_eq!(lock.changed(false, true), LockChange::Released);
        // A request completing after release is cancelled immediately.
        lock.request();
        lock.release(false);
        assert_eq!(lock.changed(true, true), LockChange::Cancelled);
        lock.changed(false, true);
        // An unrequested browser release (Escape) is distinguishable.
        lock.request();
        lock.changed(true, true);
        assert_eq!(lock.changed(false, true), LockChange::Unexpected);
        // A rejected request fails only if it is still the latest, and stays retryable.
        let stale = lock.request();
        lock.request();
        lock.rejected(stale);
        assert!(!lock.failed);
        let latest = lock.sequence;
        lock.rejected(latest);
        assert!(lock.failed && !lock.requested);
        lock.request();
        assert_eq!(lock.changed(true, true), LockChange::Locked);
        assert!(!lock.failed);
    }
}
