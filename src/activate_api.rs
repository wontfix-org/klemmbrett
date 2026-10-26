//! Session-bus object that asks the UI thread to show a popup source.

use zbus::interface;

pub const HISTORY_BUS_NAME: &str = "org.wontfix.klemmbrett";
pub const OBJECT_PATH: &str = "/org/wontfix/klemmbrett";
pub const INTERFACE: &str = HISTORY_BUS_NAME;

/// Calls `Show` on an already-open connection.
pub fn request_show_on(
    conn: &zbus::blocking::Connection,
    destination: &str,
    source: &str,
    pointer: Option<(i32, i32)>,
) -> zbus::Result<()> {
    let pointer: Vec<(i32, i32)> = pointer.into_iter().collect();
    conn.call_method(
        Some(destination),
        OBJECT_PATH,
        Some(INTERFACE),
        "Show",
        &(source, pointer),
    )
    .map(|_| ())
}

/// Calls `Show` on `destination` over the session bus.
pub fn request_show(
    destination: &str,
    source: &str,
    pointer: Option<(i32, i32)>,
) -> zbus::Result<()> {
    let conn = zbus::blocking::Connection::session()?;
    request_show_on(&conn, destination, source, pointer)
}

/// `[]` is no pointer. One pair is a global coordinate. More than one is an error.
pub fn pointer_from_pairs(pairs: &[(i32, i32)]) -> Result<Option<(i32, i32)>, String> {
    match pairs {
        [] => Ok(None),
        [xy] => Ok(Some(*xy)),
        _ => Err("Show accepts at most one pointer coordinate pair".to_string()),
    }
}

pub struct BusInterface {
    send: Box<dyn Fn(String, Option<(i32, i32)>) -> Result<(), String> + Send + Sync>,
}

impl BusInterface {
    pub fn new<F>(send: F) -> Self
    where
        F: Fn(String, Option<(i32, i32)>) -> Result<(), String> + Send + Sync + 'static,
    {
        Self {
            send: Box::new(send),
        }
    }
}

#[interface(name = "org.wontfix.klemmbrett")]
impl BusInterface {
    fn show(&self, source: &str, pointer: Vec<(i32, i32)>) -> zbus::fdo::Result<()> {
        let pointer = pointer_from_pairs(&pointer).map_err(zbus::fdo::Error::InvalidArgs)?;
        (self.send)(source.to_owned(), pointer).map_err(zbus::fdo::Error::InvalidArgs)
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
