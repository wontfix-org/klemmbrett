//! Keeps the Wayland clipboard and primary selection holding the same text.
//!
//! Copying between the two is inherently circular: every write to one side
//! shows up as a change on the next poll, which would be copied straight back.
//! [`ClipboardSync`] breaks that cycle by remembering, per side, the text it
//! last observed there, and by recording a value it wrote for *both* sides
//! before the next poll can see it.

use std::time::Duration;

pub const POLL_INTERVAL: Duration = Duration::from_millis(250);
pub const SETTLE_INTERVAL: Duration = Duration::from_millis(25);
pub const SETTLE_ATTEMPTS: u32 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Clipboard,
    Primary,
}

impl Selection {
    pub fn other(self) -> Self {
        match self {
            Selection::Clipboard => Selection::Primary,
            Selection::Primary => Selection::Clipboard,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Selection::Clipboard => "clipboard",
            Selection::Primary => "primary selection",
        }
    }
}

/// The selections as the sync engine sees them.
///
/// Sleeping is part of the trait so that tests can drive the engine without
/// spending real time in the settle loop.
pub trait Backend {
    /// Returns `None` if the selection could not be read at all. A selection
    /// that is empty or holds something other than text reads as `Some("")`.
    fn read(&mut self, selection: Selection) -> Option<String>;

    fn write(&mut self, selection: Selection, text: &str) -> Result<(), String>;

    fn sleep(&mut self, duration: Duration);
}

/// What a single [`ClipboardSync::tick`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Both sides already hold the same text.
    InSync,
    /// A selection could not be read, so both sides were left untouched.
    Unreadable,
    /// The sides differ, but neither changed since the last poll.
    Stalled,
    /// `text` was copied to `target`. `confirmed` reports whether it was
    /// observed to actually land there.
    Copied {
        target: Selection,
        text: String,
        confirmed: bool,
    },
}

pub struct ClipboardSync<B: Backend> {
    backend: B,
    last_clipboard: String,
    last_primary: String,
    settle_interval: Duration,
    settle_attempts: u32,
}

impl<B: Backend> ClipboardSync<B> {
    /// Seeds the remembered values from the current selections so that
    /// whatever is already on the clipboard is not treated as a fresh edit.
    pub fn new(mut backend: B) -> Self {
        let last_clipboard = backend.read(Selection::Clipboard).unwrap_or_default();
        let last_primary = backend.read(Selection::Primary).unwrap_or_default();
        Self {
            backend,
            last_clipboard,
            last_primary,
            settle_interval: SETTLE_INTERVAL,
            settle_attempts: SETTLE_ATTEMPTS,
        }
    }

    pub fn with_settle(mut self, interval: Duration, attempts: u32) -> Self {
        self.settle_interval = interval;
        self.settle_attempts = attempts;
        self
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Text last recorded for the clipboard side. After a successful tick that
    /// copied or found both sides in sync, this is the common value.
    pub fn last_text(&self) -> &str {
        &self.last_clipboard
    }

    fn remember(&mut self, text: &str) {
        self.last_clipboard.clear();
        self.last_clipboard.push_str(text);
        self.last_primary.clear();
        self.last_primary.push_str(text);
    }

    /// Writes `text` and waits for it to read back. A compositor applies a new
    /// selection asynchronously, so without this the next poll could still see
    /// the previous value and mistake it for a user edit.
    fn write_and_settle(&mut self, target: Selection, text: &str) -> bool {
        if let Err(err) = self.backend.write(target, text) {
            eprintln!("Failed to write the {}: {err}", target.name());
            return false;
        }

        for _ in 0..self.settle_attempts {
            if self.backend.read(target).as_deref() == Some(text) {
                return true;
            }
            let interval = self.settle_interval;
            self.backend.sleep(interval);
        }

        eprintln!("The {} did not take the new value in time.", target.name());
        false
    }

    /// Runs one poll cycle.
    pub fn tick(&mut self) -> Action {
        let (Some(clipboard), Some(primary)) = (
            self.backend.read(Selection::Clipboard),
            self.backend.read(Selection::Primary),
        ) else {
            return Action::Unreadable;
        };

        if clipboard == primary {
            self.last_clipboard = clipboard;
            self.last_primary = primary;
            return Action::InSync;
        }

        // If a single poll observes both sides changed, an explicit copy wins
        // over a mouse selection. Two changes within one interval can still be
        // seen in either order, so the winner only settles once both have been
        // observed.
        let source = if clipboard != self.last_clipboard && !clipboard.is_empty() {
            Some((Selection::Clipboard, clipboard.clone()))
        } else if primary != self.last_primary && !primary.is_empty() {
            Some((Selection::Primary, primary.clone()))
        } else {
            None
        };

        let Some((source, text)) = source else {
            self.last_clipboard = clipboard;
            self.last_primary = primary;
            return Action::Stalled;
        };

        let target = source.other();
        let confirmed = self.write_and_settle(target, &text);
        // Recorded for both sides even when unconfirmed: a value this tool
        // wrote must never be read back as a user edit, and retrying forever
        // would be worse than letting the next real change resolve it.
        self.remember(&text);

        Action::Copied {
            target,
            text,
            confirmed,
        }
    }

    /// Polls forever.
    pub fn run(&mut self, poll_interval: Duration) -> ! {
        loop {
            self.backend.sleep(poll_interval);
            self.tick();
        }
    }
}

mod activate_api;
mod clipboard_history;
mod config;
mod executables;
mod history;
mod launch;
mod popup;
mod source;
mod theme;
mod windows;

pub use activate_api::{
    request_show_menu, request_show_menu_on, ActivateApi, ACTIVATE_INTERFACE, ACTIVATE_OBJECT_PATH,
    HISTORY_BUS_NAME,
};
pub use clipboard_history::{history_candidate, ClipboardHistory};
pub use config::{
    config_path_from_env, default_config_path, load_config, parse_config, Config, FileFormat,
    PopupConfig, SelectAction, SourceConfig,
};
pub use history::{
    db_path_from_env, default_db_path, shorten_label, HistoryStore, DEFAULT_COALESCE_GROWING,
    DEFAULT_HISTORY_LIMIT, DEFAULT_LABEL_MAX, DEFAULT_SKIP_WHITESPACE_ONLY,
};
pub use launch::{running_as_systemd_unit, spawn_detached, Launch, ResolvedLaunch};
pub use popup::{
    accelerator_for, filter_records, index_for_accelerator, menu_item_spec, place_at_location,
    place_on_outputs, position_on_outputs, MenuItemSpec, OutputGeom,
};
pub use source::{Record, Sources};
pub use theme::{
    parse_document, parse_theme, Edges, HclDocument, HclValue, Location, Orientation, Presentation,
    RawSource, Rgba, Style, Theme, Widget,
};

#[cfg(target_os = "linux")]
pub use arboard_backend::ArboardBackend;

#[cfg(target_os = "linux")]
mod arboard_backend {
    use super::{Backend, Selection};
    use arboard::{Clipboard, Error, GetExtLinux, LinuxClipboardKind, SetExtLinux};
    use std::thread;
    use std::time::Duration;

    fn kind(selection: Selection) -> LinuxClipboardKind {
        match selection {
            Selection::Clipboard => LinuxClipboardKind::Clipboard,
            Selection::Primary => LinuxClipboardKind::Primary,
        }
    }

    pub struct ArboardBackend {
        clipboard: Clipboard,
    }

    impl ArboardBackend {
        pub fn new() -> Result<Self, Error> {
            Ok(Self {
                clipboard: Clipboard::new()?,
            })
        }
    }

    impl Backend for ArboardBackend {
        fn read(&mut self, selection: Selection) -> Option<String> {
            match self.clipboard.get().clipboard(kind(selection)).text() {
                Ok(text) => Some(text),
                // Empty, or holding something that is not text.
                Err(Error::ContentNotAvailable) => Some(String::new()),
                Err(_) => None,
            }
        }

        fn write(&mut self, selection: Selection, text: &str) -> Result<(), String> {
            self.clipboard
                .set()
                .clipboard(kind(selection))
                .text(text.to_owned())
                .map_err(|err| err.to_string())
        }

        fn sleep(&mut self, duration: Duration) {
            thread::sleep(duration);
        }
    }
}
