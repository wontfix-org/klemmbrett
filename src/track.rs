//! Whether an environment subscription is required.
//!
//! `None` probes and, when the source is missing, logs a warning and keeps
//! running. `Some(true)` aborts startup. `Some(false)` does not subscribe;
//! callers check that before probing.

use crate::journal;

/// Turns a failed probe into a warning, or into a startup error when the
/// subscription was requested.
pub(crate) fn finish(
    requested: Option<bool>,
    what: &str,
    result: Result<(), String>,
) -> Result<(), String> {
    match result {
        Ok(()) => Ok(()),
        Err(err) if requested == Some(true) => {
            Err(format!("{what} was requested but is not available: {err}"))
        }
        Err(err) => {
            journal::warn(&format!("{what} is off: {err}"));
            Ok(())
        }
    }
}
