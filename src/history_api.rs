//! Session-bus object that asks the UI thread to pop the history menu.

use zbus::interface;

pub const HISTORY_BUS_NAME: &str = "org.klemmbrett";
pub const HISTORY_OBJECT_PATH: &str = "/org/klemmbrett/History";
pub const HISTORY_INTERFACE: &str = "org.klemmbrett.History";

pub struct HistoryApi {
    send: Box<dyn Fn(i32, i32) + Send + Sync>,
}

impl HistoryApi {
    pub fn new<F>(send: F) -> Self
    where
        F: Fn(i32, i32) + Send + Sync + 'static,
    {
        Self {
            send: Box::new(send),
        }
    }
}

#[interface(name = "org.klemmbrett.History")]
impl HistoryApi {
    fn show_menu(&self, x: i32, y: i32) {
        (self.send)(x, y);
    }
}
