//! The `org.freedesktop.Notifications` server on a private bus, driven the
//! way `notify-send` and other senders drive it.

use klemmbrett::notifications::{self, NotificationCenter, NotificationConfig, ToastCmd};
use klemmbrett::{CloseReason, Handlers, Notification, Record};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

/// Takes notifications whose summary starts with "routed", as a routing
/// function would, and leaves the rest to be shown.
struct Router(Arc<Mutex<Vec<Notification>>>);

impl Handlers for Router {
    fn select(&self, _widget: &str, _records: Vec<Record>) {}

    fn highlight(&self, _widget: &str, _record: Record) {}

    fn notification(&self, notification: &Notification) -> bool {
        if !notification.summary.starts_with("routed") {
            return false;
        }
        self.0.lock().unwrap().push(notification.clone());
        true
    }
}

struct Bus(Child, String);

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn private_bus() -> Option<Bus> {
    let mut child = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut address = String::new();
    BufReader::new(child.stdout.take()?)
        .read_line(&mut address)
        .ok()?;
    Some(Bus(child, address.trim().to_owned()))
}

fn proxy(conn: &Connection) -> Proxy<'static> {
    Proxy::new(
        conn,
        notifications::BUS_NAME,
        notifications::OBJECT_PATH,
        notifications::INTERFACE,
    )
    .unwrap()
}

type NotifyArgs<'a> = (
    &'a str,
    u32,
    &'a str,
    &'a str,
    &'a str,
    Vec<&'a str>,
    HashMap<&'a str, Value<'a>>,
    i32,
);

fn notify(proxy: &Proxy<'_>, replaces: u32, summary: &str, hints: HashMap<&str, Value<'_>>) -> u32 {
    let args: NotifyArgs = (
        "tester",
        replaces,
        "",
        summary,
        "the body",
        vec!["default", "Open", "later", "Later"],
        hints,
        -1,
    );
    proxy.call("Notify", &args).unwrap()
}

#[test]
fn senders_get_ids_toasts_and_close_signals() {
    let Some(bus) = private_bus() else {
        eprintln!("skipping: dbus-daemon is not installed");
        return;
    };
    let toasts = Arc::new(Mutex::new(Vec::new()));
    let routed = Arc::new(Mutex::new(Vec::new()));
    let sink = toasts.clone();
    let center = NotificationCenter::new(
        NotificationConfig::default(),
        Arc::new(Router(routed.clone())),
        move |cmd| sink.lock().unwrap().push(cmd),
    );
    notifications::serve(center.clone(), Some(bus.1.clone()));

    let conn = zbus::blocking::connection::Builder::address(bus.1.as_str())
        .unwrap()
        .build()
        .unwrap();
    let proxy = proxy(&conn);
    let deadline = Instant::now() + Duration::from_secs(10);
    let info: (String, String, String, String) = loop {
        match proxy.call("GetServerInformation", &()) {
            Ok(info) => break info,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Err(err) => panic!("the server never claimed the name: {err}"),
        }
    };
    assert_eq!((info.0.as_str(), info.3.as_str()), ("klemmbrett", "1.2"));
    let capabilities: Vec<String> = proxy.call("GetCapabilities", &()).unwrap();
    assert!(
        capabilities.contains(&"actions".to_owned()),
        "{capabilities:?}"
    );

    let (closed_tx, closed) = mpsc::channel();
    let signals = proxy.receive_signal("NotificationClosed").unwrap();
    std::thread::spawn(move || {
        for message in signals {
            let body: (u32, u32) = message.body().deserialize().unwrap();
            if closed_tx.send(body).is_err() {
                return;
            }
        }
    });

    let hints = HashMap::from([
        ("urgency", Value::U8(2)),
        ("category", Value::from("im.received")),
        ("image-data", Value::from(vec![0u8; 64])),
    ]);
    let id = notify(&proxy, 0, "hello", hints);
    let replaced = notify(&proxy, id, "hello again", HashMap::new());
    assert_eq!(replaced, id);
    assert!(notify(&proxy, 0, "next", HashMap::new()) > id);
    {
        let toasts = toasts.lock().unwrap();
        let ToastCmd::Show(first) = &toasts[0] else {
            panic!("expected a toast, got {toasts:?}");
        };
        assert_eq!(first.urgency, 2);
        assert_eq!(first.hints["category"], "im.received");
        assert_eq!(
            first.hints["image-data"],
            serde_json::json!({ "bytes": 64 })
        );
        assert_eq!(
            first.actions,
            [
                ("default".into(), "Open".into()),
                ("later".into(), "Later".into())
            ]
        );
        assert!(first.has_default_action());
        let ToastCmd::Show(second) = &toasts[1] else {
            panic!("expected a replacing toast");
        };
        assert!(second.replaces);
    }

    let _: () = proxy.call("CloseNotification", &(id,)).unwrap();
    let wait = || closed.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(wait(), (id, CloseReason::Closed as u32));
    assert!(
        matches!(toasts.lock().unwrap().last(), Some(ToastCmd::Close(closed)) if *closed == id)
    );

    let count = toasts.lock().unwrap().len();
    let stashed = notify(&proxy, 0, "routed away", HashMap::new());
    assert_eq!(
        toasts.lock().unwrap().len(),
        count,
        "routed notifications wait for the route"
    );
    assert_eq!(routed.lock().unwrap()[0].id, stashed);
    center.closed(stashed, CloseReason::Expired);
    center.closed(stashed, CloseReason::Expired);
    assert_eq!(wait(), (stashed, CloseReason::Expired as u32));
    assert!(
        closed.recv_timeout(Duration::from_millis(300)).is_err(),
        "closed twice"
    );
}
