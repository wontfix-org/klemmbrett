//! Session-bus object that asks the UI thread to activate a popup source.

use zbus::interface;

pub const HISTORY_BUS_NAME: &str = "org.wontfix.klemmbrett";
pub const ACTIVATE_OBJECT_PATH: &str = "/org/wontfix/klemmbrett/Activate";
pub const ACTIVATE_INTERFACE: &str = "org.wontfix.klemmbrett.Activate";

/// Calls `ShowMenu` on an already-open connection.
pub fn request_show_menu_on(
    conn: &zbus::blocking::Connection,
    destination: &str,
    source: &str,
    x: i32,
    y: i32,
) -> zbus::Result<()> {
    conn.call_method(
        Some(destination),
        ACTIVATE_OBJECT_PATH,
        Some(ACTIVATE_INTERFACE),
        "ShowMenu",
        &(source, x, y),
    )
    .map(|_| ())
}

/// Calls `ShowMenu` on `destination` over the session bus.
pub fn request_show_menu(destination: &str, source: &str, x: i32, y: i32) -> zbus::Result<()> {
    let conn = zbus::blocking::Connection::session()?;
    request_show_menu_on(&conn, destination, source, x, y)
}

pub struct ActivateApi {
    send: Box<dyn Fn(String, i32, i32) -> Result<(), String> + Send + Sync>,
}

impl ActivateApi {
    pub fn new<F>(send: F) -> Self
    where
        F: Fn(String, i32, i32) -> Result<(), String> + Send + Sync + 'static,
    {
        Self {
            send: Box::new(send),
        }
    }
}

#[interface(name = "org.wontfix.klemmbrett.Activate")]
impl ActivateApi {
    fn show_menu(&self, source: &str, x: i32, y: i32) -> zbus::fdo::Result<()> {
        (self.send)(source.to_owned(), x, y).map_err(zbus::fdo::Error::InvalidArgs)
    }
}
