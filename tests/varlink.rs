//! The Varlink server end to end over a real socket, with the UI replaced
//! by a closure that records each request and answers prompts.

use klemmbrett::notifications::{NotificationCenter, NotificationConfig, ToastCmd};
use klemmbrett::varlink::{self, Server};
use klemmbrett::widget::StaticSource;
use klemmbrett::{CallError, Client, ClientError, Handlers, Record, ShowRequest, WidgetDef};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct Stashes;

impl Handlers for Stashes {
    fn select(&self, _widget: &str, _records: Vec<Record>) {}

    fn highlight(&self, _widget: &str, _record: Record) {}

    fn call(&self, method: &str, parameters: Value) -> Result<Value, CallError> {
        match (method, parameters["stash"].as_str()) {
            ("StashList", Some("inbox")) => Ok(json!({ "records": [{ "value": "hi" }] })),
            ("StashMarkRead", Some("inbox")) => Ok(json!({ "marked": 2 })),
            (_, stash) => Err(CallError::new(
                "UnknownStash",
                json!({ "stash": stash, "known": ["inbox"] }),
            )),
        }
    }
}

type Shown = (String, Option<(i32, i32)>, Option<Vec<Record>>);

struct Fixture {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    shown: Arc<Mutex<Vec<Shown>>>,
    toasts: Arc<Mutex<Vec<u32>>>,
}

impl Fixture {
    fn start(notifications: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("klemmbrett").join("varlink.sock");
        let listener = varlink::bind(&socket).unwrap();
        let mut picker = WidgetDef::new(
            "picker",
            Some(Arc::new(StaticSource(vec![
                Record::new("alpha").with_data(json!({ "n": 1 })),
                Record::new("beta"),
            ]))),
        );
        picker.multi = None;
        let widgets = vec![picker, WidgetDef::new("bare", None)];
        let handlers: Arc<dyn Handlers> = Arc::new(Stashes);
        let toasts = Arc::new(Mutex::new(Vec::new()));
        let center = notifications.then(|| {
            let toasts = toasts.clone();
            NotificationCenter::new(
                NotificationConfig::default(),
                handlers.clone(),
                move |cmd| {
                    if let ToastCmd::Show(notification) = cmd {
                        toasts.lock().unwrap().push(notification.id);
                    }
                },
            )
        });
        let shown = Arc::new(Mutex::new(Vec::new()));
        let log = shown.clone();
        Server::new(widgets, handlers, center, move |request: ShowRequest| {
            log.lock().unwrap().push((
                request.widget.clone(),
                request.pointer,
                request.records.clone(),
            ));
            if let Some(reply) = request.reply {
                let picked = request
                    .records
                    .unwrap_or_else(|| vec![Record::new("alpha")])
                    .into_iter()
                    .map(|record| record.value)
                    .take(1)
                    .collect();
                reply.send(Ok(picked)).unwrap();
            }
            Ok(())
        })
        .serve(listener);
        Self {
            _dir: dir,
            socket,
            shown,
            toasts,
        }
    }

    fn client(&self) -> Client {
        Client::connect(&self.socket).unwrap()
    }
}

fn remote(result: Result<Value, ClientError>) -> (String, Value) {
    match result {
        Err(ClientError::Remote { error, parameters }) => (error, parameters),
        other => panic!("expected an error reply, got {other:?}"),
    }
}

#[test]
fn show_and_prompt_reach_the_ui() {
    let fixture = Fixture::start(false);
    let mut client = fixture.client();
    client
        .call(
            "Show",
            json!({ "widget": "picker", "pointer": { "x": 3, "y": -4 } }),
        )
        .unwrap();
    let reply = client
        .call(
            "Prompt",
            json!({ "widget": "bare", "records": [{ "value": "v", "data": { "deep": [1] } }] }),
        )
        .unwrap();
    assert_eq!(reply, json!({ "values": ["v"] }));
    let shown = fixture.shown.lock().unwrap();
    assert_eq!(shown[0], ("picker".into(), Some((3, -4)), None));
    let records = shown[1].2.as_ref().unwrap();
    assert_eq!(records[0].data, json!({ "deep": [1] }));
}

#[test]
fn errors_name_the_problem() {
    let fixture = Fixture::start(false);
    let mut client = fixture.client();
    let (error, parameters) = remote(client.call("Show", json!({ "widget": "nope" })));
    assert_eq!(error, "org.wontfix.klemmbrett.UnknownWidget");
    assert_eq!(parameters["known"], json!(["bare", "picker"]));
    let (error, _) = remote(client.call("Show", json!({ "widget": "bare" })));
    assert_eq!(error, "org.wontfix.klemmbrett.NoProvider");
    let (error, parameters) = remote(client.call(
        "Show",
        json!({ "widget": "bare", "records": [{ "value": "a" }, { "value": 3 }] }),
    ));
    assert_eq!(error, "org.wontfix.klemmbrett.InvalidRecord");
    assert_eq!(parameters["path"], "records[1]");
    let (error, _) = remote(client.call("Show", json!({})));
    assert_eq!(error, "org.varlink.service.InvalidParameter");
    let (error, _) = remote(client.call("Frobnicate", json!({})));
    assert_eq!(error, "org.varlink.service.MethodNotFound");
    let (error, _) = remote(client.call("Notify", json!({ "app_name": "a", "summary": "s" })));
    assert_eq!(error, "org.wontfix.klemmbrett.NotificationsDisabled");
    let (error, _) = remote(client.call("Subscribe", json!({})));
    assert_eq!(error, "org.varlink.service.ExpectedMore");
}

#[test]
fn widgets_records_and_service_info() {
    let fixture = Fixture::start(false);
    let mut client = fixture.client();
    let widgets = client.call("Widgets", json!({})).unwrap();
    assert_eq!(
        widgets["widgets"],
        json!([
            { "name": "picker", "provider": true, "multi": false },
            { "name": "bare", "provider": false, "multi": false },
        ])
    );
    let records = client
        .call("Records", json!({ "widget": "picker" }))
        .unwrap();
    assert_eq!(
        records["records"][0],
        json!({ "key": "alpha", "label": "alpha", "value": "alpha", "data": { "n": 1 } })
    );
    let info = client
        .call("org.varlink.service.GetInfo", json!({}))
        .unwrap();
    assert_eq!(info["product"], "klemmbrett");
    let description = client
        .call(
            "org.varlink.service.GetInterfaceDescription",
            json!({ "interface": "org.wontfix.klemmbrett" }),
        )
        .unwrap();
    assert!(description["description"]
        .as_str()
        .unwrap()
        .contains("method Subscribe"));
}

#[test]
fn stash_calls_go_to_the_handlers() {
    let fixture = Fixture::start(false);
    let mut client = fixture.client();
    let listed = client
        .call("StashList", json!({ "stash": "inbox" }))
        .unwrap();
    assert_eq!(listed["records"][0]["value"], "hi");
    let marked = client
        .call("StashMarkRead", json!({ "stash": "inbox" }))
        .unwrap();
    assert_eq!(marked["marked"], 2);
    let (error, parameters) = remote(client.call("StashList", json!({ "stash": "x" })));
    assert_eq!(error, "org.wontfix.klemmbrett.UnknownStash");
    assert_eq!(parameters["known"], json!(["inbox"]));
}

#[test]
fn notify_shows_a_toast_and_reaches_subscribers() {
    let fixture = Fixture::start(true);
    let mut watcher = fixture.client();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = watcher.stream("Subscribe", json!({}), |reply| {
            tx.send(reply["event"].clone()).is_ok()
        });
    });
    assert_eq!(rx.recv().unwrap()["kind"], "ready");
    let id = fixture
        .client()
        .call(
            "Notify",
            json!({ "app_name": "test", "summary": "Hello", "urgency": 2 }),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap() as u32;
    let event = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(event["kind"], "notification");
    assert_eq!(event["notification"]["summary"], "Hello");
    assert_eq!(event["notification"]["urgency"], 2);
    assert_eq!(*fixture.toasts.lock().unwrap(), vec![id]);
}

#[test]
fn a_second_daemon_is_refused_and_a_stale_socket_replaced() {
    let fixture = Fixture::start(false);
    let error = varlink::bind(&fixture.socket).unwrap_err();
    assert!(error.contains("already running"), "{error}");

    let dir = tempfile::tempdir().unwrap();
    let stale = dir.path().join("stale.sock");
    drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
    varlink::bind(&stale).unwrap();
}

/// systemd's client agrees with the server, when it is installed.
#[test]
fn varlinkctl_can_introspect_and_call() {
    let Ok(status) = std::process::Command::new("varlinkctl")
        .arg("--version")
        .output()
    else {
        eprintln!("skipping: varlinkctl is not installed");
        return;
    };
    assert!(status.status.success());
    let fixture = Fixture::start(false);
    let socket = fixture.socket.to_str().unwrap();
    let introspect = std::process::Command::new("varlinkctl")
        .args(["introspect", socket, "org.wontfix.klemmbrett"])
        .output()
        .unwrap();
    assert!(introspect.status.success(), "{introspect:?}");
    assert!(String::from_utf8_lossy(&introspect.stdout).contains("method Prompt"));
    let call = std::process::Command::new("varlinkctl")
        .args([
            "call",
            "--json=short",
            socket,
            "org.wontfix.klemmbrett.Records",
            r#"{"widget":"picker"}"#,
        ])
        .output()
        .unwrap();
    assert!(call.status.success(), "{call:?}");
    let reply: Value = serde_json::from_slice(&call.stdout).unwrap();
    assert_eq!(reply["records"][1]["value"], "beta");
}

#[test]
fn messages_are_nul_terminated_json() {
    let fixture = Fixture::start(false);
    let mut stream = UnixStream::connect(&fixture.socket).unwrap();
    stream
        .write_all(b"{\"method\":\"org.wontfix.klemmbrett.Widgets\"}\0")
        .unwrap();
    let mut reply = Vec::new();
    BufReader::new(&stream).read_until(0, &mut reply).unwrap();
    assert_eq!(reply.pop(), Some(0));
    let reply: Value = serde_json::from_slice(&reply).unwrap();
    assert_eq!(reply["parameters"]["widgets"][1]["name"], "bare");
}
