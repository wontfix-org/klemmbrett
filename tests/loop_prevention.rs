//! Drives the sync engine against a scripted fake compositor.
//!
//! These cover the cases that make a clipboard syncer loop: the echo of the
//! tool's own write, a write that lands late, and a value that was already
//! seen before. A real compositor applies selections asynchronously, so these
//! races are only reproducible on demand with a fake.

use std::time::Duration;
use klemmbrett::{Action, Backend, ClipboardSync, Selection};

/// A pair of selections whose writes can be delayed by a fixed number of
/// reads, imitating a compositor that applies a selection asynchronously.
#[derive(Default)]
struct FakeSelections {
    clipboard: String,
    primary: String,
    /// Writes become visible only after this many reads of that selection.
    write_lag: u32,
    pending: Option<(Selection, String, u32)>,
    unreadable: Option<Selection>,
    write_error: Option<Selection>,
    writes: Vec<(Selection, String)>,
    slept: Duration,
}

impl FakeSelections {
    fn new(clipboard: &str, primary: &str) -> Self {
        Self {
            clipboard: clipboard.to_owned(),
            primary: primary.to_owned(),
            ..Default::default()
        }
    }

    fn with_write_lag(mut self, reads: u32) -> Self {
        self.write_lag = reads;
        self
    }

    fn slot(&mut self, selection: Selection) -> &mut String {
        match selection {
            Selection::Clipboard => &mut self.clipboard,
            Selection::Primary => &mut self.primary,
        }
    }

    /// A change made by the user, rather than by the sync engine.
    fn user_sets(&mut self, selection: Selection, text: &str) {
        *self.slot(selection) = text.to_owned();
    }

    fn apply_pending(&mut self, being_read: Selection) {
        let Some((selection, text, remaining)) = self.pending.take() else {
            return;
        };
        if selection != being_read {
            self.pending = Some((selection, text, remaining));
            return;
        }
        if remaining == 0 {
            *self.slot(selection) = text;
        } else {
            self.pending = Some((selection, text, remaining - 1));
        }
    }
}

impl Backend for FakeSelections {
    fn read(&mut self, selection: Selection) -> Option<String> {
        if self.unreadable == Some(selection) {
            return None;
        }
        self.apply_pending(selection);
        Some(match selection {
            Selection::Clipboard => self.clipboard.clone(),
            Selection::Primary => self.primary.clone(),
        })
    }

    fn write(&mut self, selection: Selection, text: &str) -> Result<(), String> {
        if self.write_error == Some(selection) {
            return Err("write refused".to_owned());
        }
        self.writes.push((selection, text.to_owned()));
        if self.write_lag == 0 {
            *self.slot(selection) = text.to_owned();
        } else {
            self.pending = Some((selection, text.to_owned(), self.write_lag));
        }
        Ok(())
    }

    fn sleep(&mut self, duration: Duration) {
        self.slept += duration;
    }
}

fn sync(backend: FakeSelections) -> ClipboardSync<FakeSelections> {
    ClipboardSync::new(backend).with_settle(Duration::from_millis(1), 40)
}

fn copied(target: Selection, text: &str) -> Action {
    Action::Copied {
        target,
        text: text.to_owned(),
        confirmed: true,
    }
}

#[test]
fn copy_propagates_to_the_primary_selection() {
    let mut sync = sync(FakeSelections::new("", ""));
    sync.backend_mut().user_sets(Selection::Clipboard, "hello");

    assert_eq!(sync.tick(), copied(Selection::Primary, "hello"));
    assert_eq!(sync.backend_mut().primary, "hello");
}

#[test]
fn selection_propagates_to_the_clipboard() {
    let mut sync = sync(FakeSelections::new("", ""));
    sync.backend_mut().user_sets(Selection::Primary, "hello");

    assert_eq!(sync.tick(), copied(Selection::Clipboard, "hello"));
    assert_eq!(sync.backend_mut().clipboard, "hello");
}

/// The echo of the engine's own write must not be copied back.
#[test]
fn a_synced_value_is_not_echoed_back() {
    let mut sync = sync(FakeSelections::new("", ""));
    sync.backend_mut().user_sets(Selection::Clipboard, "hello");
    sync.tick();

    for _ in 0..20 {
        assert_eq!(sync.tick(), Action::InSync);
    }
    assert_eq!(sync.backend_mut().writes.len(), 1);
}

/// The case that breaks a syncer sharing one "last value" across both sides:
/// the write is still in flight, so the target still reads its old value.
#[test]
fn a_write_still_in_flight_is_not_mistaken_for_a_user_edit() {
    let mut sync = sync(FakeSelections::new("old", "old").with_write_lag(3));
    sync.backend_mut().user_sets(Selection::Primary, "new");

    assert_eq!(sync.tick(), copied(Selection::Clipboard, "new"));

    // The stale "old" on the clipboard must not be pushed back to the primary
    // selection, which would revert the user and start a ping-pong.
    for _ in 0..20 {
        assert_eq!(sync.tick(), Action::InSync);
    }
    assert_eq!(sync.backend_mut().primary, "new");
    assert_eq!(
        sync.backend_mut().writes,
        vec![(Selection::Clipboard, "new".to_owned())]
    );
}

/// Re-copying something that was synced earlier is a real edit, not an echo.
#[test]
fn a_previously_seen_value_can_be_copied_again() {
    let mut sync = sync(FakeSelections::new("", ""));

    sync.backend_mut().user_sets(Selection::Clipboard, "aaa");
    assert_eq!(sync.tick(), copied(Selection::Primary, "aaa"));

    sync.backend_mut().user_sets(Selection::Primary, "bbb");
    assert_eq!(sync.tick(), copied(Selection::Clipboard, "bbb"));

    sync.backend_mut().user_sets(Selection::Clipboard, "aaa");
    assert_eq!(sync.tick(), copied(Selection::Primary, "aaa"));
    assert_eq!(sync.backend_mut().primary, "aaa");
}

#[test]
fn an_explicit_copy_wins_when_both_sides_change_at_once() {
    let mut sync = sync(FakeSelections::new("", ""));
    sync.backend_mut().user_sets(Selection::Clipboard, "copied");
    sync.backend_mut().user_sets(Selection::Primary, "selected");

    assert_eq!(sync.tick(), copied(Selection::Primary, "copied"));

    for _ in 0..10 {
        assert_eq!(sync.tick(), Action::InSync);
    }
}

/// Clearing one side should not wipe the other.
#[test]
fn an_emptied_selection_is_not_propagated() {
    let mut sync = sync(FakeSelections::new("hello", "hello"));
    sync.backend_mut().user_sets(Selection::Primary, "");

    assert_eq!(sync.tick(), Action::Stalled);
    assert_eq!(sync.backend_mut().clipboard, "hello");
    assert!(sync.backend_mut().writes.is_empty());
}

/// An image or file list reads as empty and must leave the other side alone.
#[test]
fn non_text_content_leaves_the_other_side_alone() {
    let mut sync = sync(FakeSelections::new("hello", "hello"));
    // Non-text content surfaces as an empty read from the backend.
    sync.backend_mut().user_sets(Selection::Clipboard, "");

    assert_eq!(sync.tick(), Action::Stalled);
    assert_eq!(sync.backend_mut().primary, "hello");

    sync.backend_mut()
        .user_sets(Selection::Clipboard, "text-after");
    assert_eq!(sync.tick(), copied(Selection::Primary, "text-after"));
}

#[test]
fn an_unreadable_selection_leaves_both_sides_untouched() {
    let mut sync = sync(FakeSelections::new("hello", "hello"));
    sync.backend_mut().user_sets(Selection::Primary, "new");
    sync.backend_mut().unreadable = Some(Selection::Clipboard);

    assert_eq!(sync.tick(), Action::Unreadable);
    assert!(sync.backend_mut().writes.is_empty());

    sync.backend_mut().unreadable = None;
    assert_eq!(sync.tick(), copied(Selection::Clipboard, "new"));
}

/// A failing write must not turn into an endless retry loop.
#[test]
fn a_failing_write_is_not_retried_forever() {
    let mut sync = sync(FakeSelections::new("", ""));
    sync.backend_mut().write_error = Some(Selection::Primary);
    sync.backend_mut().user_sets(Selection::Clipboard, "hello");

    assert_eq!(
        sync.tick(),
        Action::Copied {
            target: Selection::Primary,
            text: "hello".to_owned(),
            confirmed: false,
        }
    );

    for _ in 0..20 {
        assert_eq!(sync.tick(), Action::Stalled);
    }
    assert!(sync.backend_mut().writes.is_empty());
}

/// Nothing above should depend on how long the engine waits for a write.
#[test]
fn settling_gives_up_after_the_configured_attempts() {
    let backend = FakeSelections::new("", "").with_write_lag(u32::MAX);
    let mut sync = ClipboardSync::new(backend).with_settle(Duration::from_millis(5), 3);
    sync.backend_mut().user_sets(Selection::Clipboard, "hello");

    assert_eq!(
        sync.tick(),
        Action::Copied {
            target: Selection::Primary,
            text: "hello".to_owned(),
            confirmed: false,
        }
    );
    assert_eq!(sync.backend_mut().slept, Duration::from_millis(15));
}
