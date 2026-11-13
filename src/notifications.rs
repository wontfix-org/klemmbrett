//! A freedesktop notification server (`org.freedesktop.Notifications`).
//!
//! `Notify` answers with an id right away. The configuration then decides,
//! on the handler thread, whether the notification is shown as a toast,
//! stashed, or dropped; without a routing function every notification is
//! shown. Toasts are drawn by the UI thread, which this module reaches
//! through a [`ToastCmd`] sink.

use crate::events;
use crate::journal;
use crate::theme::Location;
use crate::widget::Handlers;
use serde_json::{json, Map, Number, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::interface;
use zbus::zvariant::{OwnedValue, Value as Variant};

pub const BUS_NAME: &str = "org.freedesktop.Notifications";
pub const OBJECT_PATH: &str = "/org/freedesktop/Notifications";
pub const INTERFACE: &str = "org.freedesktop.Notifications";
pub const DEFAULT_TIMEOUT_MS: u32 = 5000;

/// Why a notification closed, as `NotificationClosed` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    Expired = 1,
    Dismissed = 2,
    Closed = 3,
    Undefined = 4,
}

impl CloseReason {
    pub fn from_code(code: u32) -> Option<Self> {
        Some(match code {
            1 => Self::Expired,
            2 => Self::Dismissed,
            3 => Self::Closed,
            4 => Self::Undefined,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NotificationConfig {
    /// Used when a sender passes `-1`.
    pub timeout_ms: u32,
    /// The corner or edge toasts stack from.
    pub location: Location,
    /// Take the bus name from a running notification daemon.
    pub replace: bool,
    /// More toasts wait until one closes.
    pub max_visible: usize,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            timeout_ms: DEFAULT_TIMEOUT_MS,
            location: Location::NorthEast,
            replace: false,
            max_visible: 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    /// `(key, label)` pairs; `default` is what clicking the toast invokes.
    pub actions: Vec<(String, String)>,
    /// Hints as JSON. Byte arrays such as `image-data` become `{"bytes": n}`.
    pub hints: Value,
    /// 0 low, 1 normal, 2 critical.
    pub urgency: u8,
    /// Milliseconds; `-1` means the server default and `0` never expires.
    pub expire_timeout: i32,
    /// Whether this replaced an earlier notification with the same id.
    pub replaces: bool,
}

impl Notification {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "app_name": self.app_name,
            "app_icon": self.app_icon,
            "summary": self.summary,
            "body": self.body,
            "actions": self
                .actions
                .iter()
                .map(|(key, label)| json!([key, label]))
                .collect::<Vec<_>>(),
            "hints": self.hints,
            "urgency": self.urgency,
            "timeout": self.expire_timeout,
            "replaces": self.replaces,
        })
    }

    /// How long a toast stays, or `None` to stay until clicked.
    pub fn lifetime_ms(&self, default_ms: u32) -> Option<u32> {
        if self.urgency >= 2 {
            return None;
        }
        match self.expire_timeout {
            0 => None,
            ms if ms < 0 => Some(default_ms),
            ms => Some(ms as u32),
        }
    }

    pub fn has_default_action(&self) -> bool {
        self.actions.iter().any(|(key, _)| key == "default")
    }
}

/// What the UI thread is asked to do with toasts.
#[derive(Debug, Clone, PartialEq)]
pub enum ToastCmd {
    /// Show, or update in place when the id is already shown.
    Show(Box<Notification>),
    /// Remove without telling the sender; the center already did.
    Close(u32),
}

/// What a sender passes to `Notify`, before an id is assigned.
#[derive(Debug, Clone, Default)]
pub struct NotifyRequest {
    pub app_name: String,
    pub replaces_id: u32,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<String>,
    pub hints: Value,
    pub expire_timeout: i32,
}

/// Open notifications, their ids, and the bus connection signals go out on.
pub struct NotificationCenter {
    config: NotificationConfig,
    next_id: AtomicU32,
    open: Mutex<HashMap<u32, Notification>>,
    conn: OnceLock<zbus::blocking::Connection>,
    handlers: Arc<dyn Handlers>,
    toasts: Box<dyn Fn(ToastCmd) + Send + Sync>,
}

impl NotificationCenter {
    pub fn new(
        config: NotificationConfig,
        handlers: Arc<dyn Handlers>,
        toasts: impl Fn(ToastCmd) + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            next_id: AtomicU32::new(1),
            open: Mutex::new(HashMap::new()),
            conn: OnceLock::new(),
            handlers,
            toasts: Box::new(toasts),
        })
    }

    pub fn config(&self) -> &NotificationConfig {
        &self.config
    }

    /// Assigns the id (reusing `replaces_id`), then routes the notification.
    pub fn notify(&self, request: NotifyRequest) -> u32 {
        let id = if request.replaces_id != 0 {
            self.next_id
                .fetch_max(request.replaces_id.saturating_add(1), Ordering::Relaxed);
            request.replaces_id
        } else {
            self.next_id.fetch_add(1, Ordering::Relaxed)
        };
        let notification = Notification {
            id,
            app_name: request.app_name,
            app_icon: request.app_icon,
            summary: request.summary,
            body: request.body,
            actions: request
                .actions
                .chunks(2)
                .map(|pair| {
                    let key = pair[0].clone();
                    let label = pair.get(1).cloned().unwrap_or_else(|| key.clone());
                    (key, label)
                })
                .collect(),
            urgency: request
                .hints
                .get("urgency")
                .and_then(Value::as_u64)
                .map_or(1, |urgency| urgency.min(2) as u8),
            hints: request.hints,
            expire_timeout: request.expire_timeout,
            replaces: request.replaces_id != 0,
        };
        if let Ok(mut open) = self.open.lock() {
            open.insert(id, notification.clone());
        }
        events::publish(
            json!({ "kind": "notification", "notification": notification.to_json() }),
            None,
        );
        if !self.handlers.notification(&notification) {
            (self.toasts)(ToastCmd::Show(Box::new(notification)));
        }
        id
    }

    /// Shows an open notification as a toast. The routing function calls this.
    pub fn show(&self, id: u32) -> Result<(), String> {
        let notification = self
            .open
            .lock()
            .ok()
            .and_then(|open| open.get(&id).cloned())
            .ok_or_else(|| format!("notification {id} is not open"))?;
        (self.toasts)(ToastCmd::Show(Box::new(notification)));
        Ok(())
    }

    /// Closes a notification that is not on screen (dropped or stashed), or
    /// that the UI closed. Tells the sender once; later calls do nothing.
    pub fn closed(&self, id: u32, reason: CloseReason) {
        let removed = self.open.lock().ok().and_then(|mut open| open.remove(&id));
        if removed.is_some() {
            self.emit("NotificationClosed", &(id, reason as u32));
        }
    }

    /// `CloseNotification` from the sender: remove the toast too.
    pub fn close_by_sender(&self, id: u32) {
        let was_open = self
            .open
            .lock()
            .ok()
            .and_then(|mut open| open.remove(&id))
            .is_some();
        if was_open {
            (self.toasts)(ToastCmd::Close(id));
            self.emit("NotificationClosed", &(id, CloseReason::Closed as u32));
        }
    }

    /// The user picked `action` on a toast.
    pub fn invoke(&self, id: u32, action: &str) {
        self.emit("ActionInvoked", &(id, action));
    }

    pub fn is_open(&self, id: u32) -> bool {
        self.open.lock().is_ok_and(|open| open.contains_key(&id))
    }

    fn emit<B>(&self, signal: &str, body: &B)
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        if let Some(conn) = self.conn.get() {
            if let Err(err) = conn.emit_signal(None::<&str>, OBJECT_PATH, INTERFACE, signal, body) {
                journal::error(&format!("klemmbrett: failed to emit {signal}: {err}"));
            }
        }
    }
}

struct Server {
    center: Arc<NotificationCenter>,
}

#[interface(name = "org.freedesktop.Notifications")]
impl Server {
    fn get_capabilities(&self) -> Vec<String> {
        ["actions", "body", "persistence"]
            .map(String::from)
            .to_vec()
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let hints = hints
            .iter()
            .map(|(name, value)| (name.clone(), variant_json(value)))
            .collect::<Map<_, _>>();
        self.center.notify(NotifyRequest {
            app_name,
            replaces_id,
            app_icon,
            summary,
            body,
            actions,
            hints: Value::Object(hints),
            expire_timeout,
        })
    }

    fn close_notification(&self, id: u32) {
        self.center.close_by_sender(id);
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "klemmbrett".into(),
            "wontfix".into(),
            env!("CARGO_PKG_VERSION").into(),
            "1.2".into(),
        )
    }
}

/// Claims the bus name on a background thread. `address` selects a bus for
/// tests; `None` is the session bus. Failing to get the name is logged, and
/// klemmbrett keeps running without notifications.
pub fn serve(center: Arc<NotificationCenter>, address: Option<String>) {
    std::thread::Builder::new()
        .name("klemmbrett-notifications".into())
        .spawn(move || {
            if let Err(err) = claim(&center, address.as_deref()) {
                journal::error(&format!("klemmbrett: notifications are off: {err}"));
                return;
            }
            std::thread::park();
        })
        .expect("spawn notification thread");
}

fn claim(center: &Arc<NotificationCenter>, address: Option<&str>) -> Result<(), String> {
    let builder = match address {
        Some(address) => zbus::blocking::connection::Builder::address(address),
        None => zbus::blocking::connection::Builder::session(),
    }
    .map_err(|err| format!("no session bus: {err}"))?;
    let conn = builder
        .serve_at(
            OBJECT_PATH,
            Server {
                center: center.clone(),
            },
        )
        .and_then(|builder| builder.build())
        .map_err(|err| format!("failed to connect to the session bus: {err}"))?;
    let mut flags = RequestNameFlags::DoNotQueue.into();
    if center.config.replace {
        flags |= RequestNameFlags::ReplaceExisting;
    }
    match conn.request_name_with_flags(BUS_NAME, flags) {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {}
        Ok(_) => {
            return Err(format!(
                "another notification daemon owns {BUS_NAME}; stop it (mako, dunst, ...) \
                 or pass replace=True to notifications.serve"
            ))
        }
        Err(err) => return Err(format!("failed to request {BUS_NAME}: {err}")),
    }
    let _ = center.conn.set(conn);
    Ok(())
}

/// D-Bus variants as JSON. Byte arrays are only counted, never copied.
pub fn variant_json(value: &Variant<'_>) -> Value {
    match value {
        Variant::U8(n) => json!(n),
        Variant::Bool(b) => json!(b),
        Variant::I16(n) => json!(n),
        Variant::U16(n) => json!(n),
        Variant::I32(n) => json!(n),
        Variant::U32(n) => json!(n),
        Variant::I64(n) => json!(n),
        Variant::U64(n) => json!(n),
        Variant::F64(n) => Number::from_f64(*n).map_or(Value::Null, Value::Number),
        Variant::Str(text) => json!(text.as_str()),
        Variant::Signature(signature) => json!(signature.to_string()),
        Variant::ObjectPath(path) => json!(path.as_str()),
        Variant::Value(inner) => variant_json(inner),
        Variant::Array(array) if array.element_signature().to_string() == "y" => {
            json!({ "bytes": array.len() })
        }
        Variant::Array(array) => Value::Array(array.inner().iter().map(variant_json).collect()),
        Variant::Dict(dict) => Value::Object(
            dict.iter()
                .map(|(key, value)| {
                    let key = match key {
                        Variant::Str(text) => text.as_str().to_owned(),
                        other => variant_json(other).to_string(),
                    };
                    (key, variant_json(value))
                })
                .collect(),
        ),
        Variant::Structure(structure) => {
            Value::Array(structure.fields().iter().map(variant_json).collect())
        }
        #[allow(unreachable_patterns)]
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::NoHandlers;
    use std::sync::mpsc;

    fn center() -> (Arc<NotificationCenter>, mpsc::Receiver<ToastCmd>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let center = NotificationCenter::new(
            NotificationConfig::default(),
            Arc::new(NoHandlers),
            move |cmd| {
                let _ = tx.lock().unwrap().send(cmd);
            },
        );
        (center, rx)
    }

    fn request(summary: &str, replaces_id: u32) -> NotifyRequest {
        NotifyRequest {
            app_name: "test".into(),
            replaces_id,
            summary: summary.into(),
            actions: vec!["default".into(), "Open".into(), "later".into()],
            hints: json!({"urgency": 2}),
            expire_timeout: -1,
            ..NotifyRequest::default()
        }
    }

    #[test]
    fn ids_count_up_and_replacing_keeps_the_id() {
        let (center, toasts) = center();
        let first = center.notify(request("one", 0));
        let second = center.notify(request("two", 0));
        assert_eq!((first, second), (1, 2));
        assert_eq!(center.notify(request("again", first)), first);
        assert_eq!(center.notify(request("far", 40)), 40);
        assert_eq!(center.notify(request("next", 0)), 41);

        let shown: Vec<_> = toasts.try_iter().collect();
        assert_eq!(shown.len(), 5, "without routing, everything is shown");
        let ToastCmd::Show(replaced) = &shown[2] else {
            panic!("expected a toast, got {:?}", shown[2]);
        };
        assert!(replaced.replaces);
        assert_eq!(replaced.summary, "again");
        assert_eq!(
            replaced.actions,
            [
                ("default".to_string(), "Open".to_string()),
                ("later".to_string(), "later".to_string())
            ]
        );
        assert_eq!(replaced.urgency, 2);
    }

    #[test]
    fn closing_happens_once_and_sender_closes_remove_the_toast() {
        let (center, toasts) = center();
        let id = center.notify(request("one", 0));
        let _ = toasts.try_iter().count();
        assert!(center.is_open(id));
        center.close_by_sender(id);
        assert!(!center.is_open(id));
        assert_eq!(toasts.try_iter().collect::<Vec<_>>(), [ToastCmd::Close(id)]);
        center.close_by_sender(id);
        assert!(toasts.try_iter().next().is_none());
        assert!(center.show(id).is_err());
    }

    #[test]
    fn lifetimes_follow_the_spec() {
        let mut notification = Notification {
            id: 1,
            app_name: String::new(),
            app_icon: String::new(),
            summary: String::new(),
            body: String::new(),
            actions: Vec::new(),
            hints: Value::Null,
            urgency: 1,
            expire_timeout: -1,
            replaces: false,
        };
        assert_eq!(notification.lifetime_ms(5000), Some(5000));
        notification.expire_timeout = 1200;
        assert_eq!(notification.lifetime_ms(5000), Some(1200));
        notification.expire_timeout = 0;
        assert_eq!(notification.lifetime_ms(5000), None);
        notification.expire_timeout = 1200;
        notification.urgency = 2;
        assert_eq!(notification.lifetime_ms(5000), None);
    }

    #[test]
    fn variants_become_json_and_bytes_are_counted() {
        let image = Variant::from((2i32, 1i32, 8i32, true, 8i32, 4i32, vec![0u8; 8]));
        assert_eq!(
            variant_json(&image),
            json!([2, 1, 8, true, 8, 4, {"bytes": 8}])
        );
        assert_eq!(variant_json(&Variant::from("x")), json!("x"));
        assert_eq!(
            variant_json(&Variant::Value(Box::new(Variant::from(3u8)))),
            json!(3),
            "nested variants unwrap"
        );
        assert_eq!(
            variant_json(&Variant::from(vec!["a", "b"])),
            json!(["a", "b"])
        );
    }
}
