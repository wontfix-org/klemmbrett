//! Records clipboard changes into [`HistoryStore`] as [`ClipboardSync`] ticks.

use super::{Action, Backend, ClipboardSync, HistoryStore, Selection};

/// Text that should be stored after `action`, if any.
pub fn history_candidate<'a>(action: &'a Action, last_text: &'a str) -> Option<&'a str> {
    match action {
        Action::Copied { text, .. } => Some(text.as_str()),
        Action::InSync if !last_text.is_empty() => Some(last_text),
        _ => None,
    }
}

/// The sync engine plus the SQLite history it should feed.
pub struct ClipboardHistory<B: Backend> {
    sync: ClipboardSync<B>,
    history: HistoryStore,
}

impl<B: Backend> ClipboardHistory<B> {
    pub fn new(sync: ClipboardSync<B>, history: HistoryStore) -> Self {
        Self { sync, history }
    }

    pub fn backend_mut(&mut self) -> &mut B {
        self.sync.backend_mut()
    }

    pub fn last_text(&self) -> &str {
        self.sync.last_text()
    }

    pub fn entries(&self) -> rusqlite::Result<Vec<String>> {
        self.history.entries()
    }

    /// Seeds history from whatever the engine already observed at startup.
    pub fn record_current(&mut self) {
        let text = self.sync.last_text().to_owned();
        let _ = self.history.record(&text);
    }

    pub fn poll(&mut self) {
        let action = self.sync.tick();
        let last = self.sync.last_text().to_owned();
        if let Some(text) = history_candidate(&action, &last) {
            let _ = self.history.record(text);
        }
    }

    /// Puts `text` on the clipboard and records it. The next poll copies it
    /// to the primary selection the same way a user copy would.
    pub fn restore(&mut self, text: &str) -> Result<(), String> {
        self.sync
            .backend_mut()
            .write(Selection::Clipboard, text)?;
        self.history.record(text).map_err(|err| err.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Action;
    use std::time::Duration;

    #[derive(Default)]
    struct Fake {
        clipboard: String,
        primary: String,
        writes: Vec<(Selection, String)>,
    }

    impl Fake {
        fn new(clipboard: &str, primary: &str) -> Self {
            Self {
                clipboard: clipboard.to_owned(),
                primary: primary.to_owned(),
                writes: Vec::new(),
            }
        }

        fn user_sets(&mut self, selection: Selection, text: &str) {
            match selection {
                Selection::Clipboard => self.clipboard = text.to_owned(),
                Selection::Primary => self.primary = text.to_owned(),
            }
        }
    }

    impl Backend for Fake {
        fn read(&mut self, selection: Selection) -> Option<String> {
            Some(match selection {
                Selection::Clipboard => self.clipboard.clone(),
                Selection::Primary => self.primary.clone(),
            })
        }

        fn write(&mut self, selection: Selection, text: &str) -> Result<(), String> {
            self.writes.push((selection, text.to_owned()));
            match selection {
                Selection::Clipboard => self.clipboard = text.to_owned(),
                Selection::Primary => self.primary = text.to_owned(),
            }
            Ok(())
        }

        fn sleep(&mut self, _duration: Duration) {}
    }

    fn history(backend: Fake) -> ClipboardHistory<Fake> {
        let sync = ClipboardSync::new(backend).with_settle(Duration::from_millis(1), 4);
        ClipboardHistory::new(sync, HistoryStore::open_in_memory(10).unwrap())
    }

    #[test]
    fn copied_text_is_a_history_candidate() {
        let action = Action::Copied {
            target: Selection::Primary,
            text: "hello".into(),
            confirmed: true,
        };
        assert_eq!(history_candidate(&action, "hello"), Some("hello"));
    }

    #[test]
    fn in_sync_records_the_shared_text() {
        assert_eq!(history_candidate(&Action::InSync, "hello"), Some("hello"));
    }

    #[test]
    fn empty_in_sync_is_not_recorded() {
        assert_eq!(history_candidate(&Action::InSync, ""), None);
    }

    #[test]
    fn stalled_and_unreadable_are_not_recorded() {
        assert_eq!(history_candidate(&Action::Stalled, "hello"), None);
        assert_eq!(history_candidate(&Action::Unreadable, "hello"), None);
    }

    #[test]
    fn a_copy_is_appended_to_history() {
        let mut hist = history(Fake::new("", ""));
        hist.backend_mut().user_sets(Selection::Clipboard, "hello");
        hist.poll();
        assert_eq!(hist.entries().unwrap(), vec!["hello".to_owned()]);
    }

    #[test]
    fn a_primary_selection_is_appended_to_history() {
        let mut hist = history(Fake::new("", ""));
        hist.backend_mut().user_sets(Selection::Primary, "from-mouse");
        hist.poll();
        assert_eq!(hist.entries().unwrap(), vec!["from-mouse".to_owned()]);
    }

    #[test]
    fn a_growing_primary_selection_replaces_the_last_history_item() {
        let mut hist = history(Fake::new("", ""));
        for text in ["h", "he", "hel", "hell", "hello"] {
            hist.backend_mut().user_sets(Selection::Primary, text);
            hist.poll();
        }
        assert_eq!(hist.entries().unwrap(), vec!["hello".to_owned()]);
    }

    #[test]
    fn polling_while_in_sync_does_not_duplicate_the_current_item() {
        let mut hist = history(Fake::new("hello", "hello"));
        hist.record_current();
        hist.poll();
        hist.poll();
        assert_eq!(hist.entries().unwrap(), vec!["hello".to_owned()]);
    }

    #[test]
    fn startup_text_is_recorded_once() {
        let mut hist = history(Fake::new("already-there", "already-there"));
        hist.record_current();
        assert_eq!(hist.entries().unwrap(), vec!["already-there".to_owned()]);
    }

    #[test]
    fn restoring_an_older_item_writes_the_clipboard_and_moves_it_to_the_top() {
        let mut hist = history(Fake::new("", ""));
        hist.backend_mut().user_sets(Selection::Clipboard, "one");
        hist.poll();
        hist.backend_mut().user_sets(Selection::Clipboard, "two");
        hist.poll();

        hist.restore("one").unwrap();
        assert_eq!(hist.backend_mut().clipboard, "one");
        assert_eq!(
            hist.entries().unwrap(),
            vec!["one".to_owned(), "two".to_owned()]
        );

        hist.poll();
        assert_eq!(hist.backend_mut().primary, "one");
        assert_eq!(hist.backend_mut().writes.last().unwrap().0, Selection::Primary);
    }

    #[test]
    fn emptying_a_selection_does_not_wipe_history() {
        let mut hist = history(Fake::new("hello", "hello"));
        hist.record_current();
        hist.backend_mut().user_sets(Selection::Primary, "");
        hist.poll();
        assert_eq!(hist.entries().unwrap(), vec!["hello".to_owned()]);
    }
}
