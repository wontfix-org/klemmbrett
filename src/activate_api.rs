//! Session-bus object that asks the UI thread to activate a popup source.

use zbus::interface;

pub const HISTORY_BUS_NAME: &str = "org.wontfix.klemmbrett";
pub const ACTIVATE_OBJECT_PATH: &str = "/org/wontfix/klemmbrett/Activate";
pub const ACTIVATE_INTERFACE: &str = "org.wontfix.klemmbrett.Activate";

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
