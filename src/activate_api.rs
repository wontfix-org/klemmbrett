//! Session-bus object that asks the UI thread to show a popup widget.

use std::collections::BTreeMap;
use std::sync::mpsc;

use zbus::interface;

use crate::widget::records_from_maps;
use crate::Record;

pub const HISTORY_BUS_NAME: &str = "org.wontfix.klemmbrett";
pub const OBJECT_PATH: &str = "/org/wontfix/klemmbrett";
pub const INTERFACE: &str = HISTORY_BUS_NAME;

/// One `Show` or `Prompt` after the record list has been decoded.
///
/// `records` is `None` when the caller passed an empty array and the widget
/// provider should load the rows. A widget with no provider rejects that
/// case. `reply` is set only for `Prompt`.
#[derive(Debug)]
pub struct ShowRequest {
    pub widget: String,
    pub pointer: Option<(i32, i32)>,
    pub records: Option<Vec<Record>>,
    pub reply: Option<mpsc::Sender<Result<Vec<String>, String>>>,
}

/// Calls `Show` on an already-open connection.
///
/// An empty `records` slice asks the daemon to use the widget provider.
/// A widget with no provider rejects an empty slice.
pub fn request_show_on(
    conn: &zbus::blocking::Connection,
    destination: &str,
    widget: &str,
    pointer: Option<(i32, i32)>,
    records: &[Record],
) -> zbus::Result<()> {
    let pointer: Vec<(i32, i32)> = pointer.into_iter().collect();
    conn.call_method(
        Some(destination),
        OBJECT_PATH,
        Some(INTERFACE),
        "Show",
        &(widget, pointer, record_maps(records)),
    )
    .map(|_| ())
}

/// Calls `Show` on `destination` over the session bus.
pub fn request_show(
    destination: &str,
    widget: &str,
    pointer: Option<(i32, i32)>,
    records: &[Record],
) -> zbus::Result<()> {
    let conn = zbus::blocking::Connection::session()?;
    request_show_on(&conn, destination, widget, pointer, records)
}

/// Calls `Prompt` and waits until the daemon returns the selected values.
pub fn request_prompt_on(
    conn: &zbus::blocking::Connection,
    destination: &str,
    widget: &str,
    pointer: Option<(i32, i32)>,
    records: &[Record],
) -> zbus::Result<Vec<String>> {
    let pointer: Vec<(i32, i32)> = pointer.into_iter().collect();
    let reply = conn.call_method(
        Some(destination),
        OBJECT_PATH,
        Some(INTERFACE),
        "Prompt",
        &(widget, pointer, record_maps(records)),
    )?;
    reply.body().deserialize()
}

/// Calls `Prompt` on `destination` over the session bus.
pub fn request_prompt(
    destination: &str,
    widget: &str,
    pointer: Option<(i32, i32)>,
    records: &[Record],
) -> zbus::Result<Vec<String>> {
    let conn = zbus::blocking::Connection::session()?;
    request_prompt_on(&conn, destination, widget, pointer, records)
}

fn record_maps(records: &[Record]) -> Vec<BTreeMap<String, String>> {
    records
        .iter()
        .map(|record| {
            BTreeMap::from([
                ("key".into(), record.key.clone()),
                ("label".into(), record.label.clone()),
                ("value".into(), record.value.clone()),
            ])
        })
        .collect()
}

/// `[]` is no pointer. One pair is a global coordinate. More than one is an error.
pub fn pointer_from_pairs(pairs: &[(i32, i32)]) -> Result<Option<(i32, i32)>, String> {
    match pairs {
        [] => Ok(None),
        [xy] => Ok(Some(*xy)),
        _ => Err("at most one pointer coordinate pair is accepted".to_string()),
    }
}

pub struct BusInterface {
    send: Box<dyn Fn(ShowRequest) -> Result<(), String> + Send + Sync>,
}

impl BusInterface {
    pub fn new<F>(send: F) -> Self
    where
        F: Fn(ShowRequest) -> Result<(), String> + Send + Sync + 'static,
    {
        Self {
            send: Box::new(send),
        }
    }

    fn dispatch(
        &self,
        widget: &str,
        pointer: Vec<(i32, i32)>,
        records: Vec<BTreeMap<String, String>>,
        reply: Option<mpsc::Sender<Result<Vec<String>, String>>>,
    ) -> zbus::fdo::Result<()> {
        let pointer = pointer_from_pairs(&pointer).map_err(zbus::fdo::Error::InvalidArgs)?;
        let records = records_from_maps(widget, records).map_err(zbus::fdo::Error::InvalidArgs)?;
        (self.send)(ShowRequest {
            widget: widget.to_owned(),
            pointer,
            records,
            reply,
        })
        .map_err(zbus::fdo::Error::InvalidArgs)
    }
}

#[interface(name = "org.wontfix.klemmbrett")]
impl BusInterface {
    fn show(
        &self,
        widget: &str,
        pointer: Vec<(i32, i32)>,
        records: Vec<BTreeMap<String, String>>,
    ) -> zbus::fdo::Result<()> {
        self.dispatch(widget, pointer, records, None)
    }

    fn prompt(
        &self,
        widget: &str,
        pointer: Vec<(i32, i32)>,
        records: Vec<BTreeMap<String, String>>,
    ) -> zbus::fdo::Result<Vec<String>> {
        let (tx, rx) = mpsc::channel();
        self.dispatch(widget, pointer, records, Some(tx))?;
        match rx.recv() {
            Ok(Ok(values)) => Ok(values),
            Ok(Err(err)) => Err(zbus::fdo::Error::Failed(err)),
            Err(_) => Err(zbus::fdo::Error::Failed(
                "prompt closed without a result".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_pointer_array_means_no_coordinates() {
        assert_eq!(pointer_from_pairs(&[]), Ok(None));
    }

    #[test]
    fn one_pair_is_a_pointer() {
        assert_eq!(pointer_from_pairs(&[(120, -40)]), Ok(Some((120, -40))));
        assert_eq!(pointer_from_pairs(&[(0, 0)]), Ok(Some((0, 0))));
    }

    #[test]
    fn two_pairs_are_rejected() {
        assert!(pointer_from_pairs(&[(1, 2), (3, 4)]).is_err());
    }
}
