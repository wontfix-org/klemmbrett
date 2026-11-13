//! Daemon messages for the journal.
//!
//! Info and above are always written. There is no filter: a recovered
//! failure should be visible without extra configuration. When systemd has
//! connected stderr to the journal (`JOURNAL_STREAM`), each line is prefixed
//! with its syslog priority so the journal records the level. A terminal
//! keeps the plain text.

use std::io::{self, Write};

/// `<6>` — a rejection we expected and handled.
pub fn info(message: &str) {
    emit(6, message);
}

/// `<4>` — one client, read, or connection failed; the daemon kept serving.
pub fn warn(message: &str) {
    emit(4, message);
}

/// `<3>` — a user action or an invariant was lost, and we kept running.
pub fn error(message: &str) {
    emit(3, message);
}

fn emit(priority: u8, message: &str) {
    let journal = std::env::var_os("JOURNAL_STREAM").is_some();
    let mut stderr = io::stderr().lock();
    let _ = writeln!(stderr, "{}", format_line(priority, message, journal));
}

fn format_line(priority: u8, message: &str, journal: bool) -> String {
    if journal {
        format!("<{priority}>{message}")
    } else {
        message.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_lines_carry_a_syslog_priority() {
        assert_eq!(format_line(3, "boom", true), "<3>boom");
        assert_eq!(format_line(4, "careful", true), "<4>careful");
        assert_eq!(format_line(6, "noted", true), "<6>noted");
        assert_eq!(format_line(3, "boom", false), "boom");
    }
}
