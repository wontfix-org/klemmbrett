//! klemmbrett's own RPC: Varlink on a Unix socket.
//!
//! Varlink messages are JSON objects, each terminated by a NUL byte. A call
//! is `{"method", "parameters", "more", "oneway"}`; a reply carries either
//! `parameters` or `error` and `parameters`, plus `continues` while a `more`
//! call keeps streaming. The interface is `org.wontfix.klemmbrett.varlink`.

use crate::events;
use crate::notifications::{NotificationCenter, NotifyRequest};
use crate::widget::{fetch_records, CallError, Handlers, Record, WidgetDef};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::Duration;

pub const INTERFACE: &str = "org.wontfix.klemmbrett";
pub const INTERFACE_DESCRIPTION: &str = include_str!("org.wontfix.klemmbrett.varlink");
const SERVICE: &str = "org.varlink.service";
const SERVICE_DESCRIPTION: &str = include_str!("org.varlink.service.varlink");
const SOCKET_ENV: &str = "KLEMMBRETT_SOCKET";

type Reply = mpsc::Sender<Result<Vec<String>, String>>;

/// One `Show` or `Prompt` after the records have been decoded. `reply` is
/// set only for `Prompt`.
#[derive(Debug)]
pub struct ShowRequest {
    pub widget: String,
    pub pointer: Option<(i32, i32)>,
    pub records: Option<Vec<Record>>,
    pub reply: Option<Reply>,
}

/// `explicit`, then `$KLEMMBRETT_SOCKET`, then
/// `$XDG_RUNTIME_DIR/klemmbrett/varlink.sock`.
pub fn socket_path(explicit: Option<&Path>) -> PathBuf {
    if let Some(path) = explicit {
        return path.to_owned();
    }
    if let Some(path) = std::env::var_os(SOCKET_ENV).filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            // SAFETY: getuid has no preconditions.
            std::env::temp_dir().join(format!("klemmbrett-{}", unsafe { libc::getuid() }))
        });
    runtime.join("klemmbrett").join("varlink.sock")
}

/// The socket systemd passed (`LISTEN_FDS`), or `path` bound by this
/// process. A path another daemon still answers on is an error, and a stale
/// one is replaced.
pub fn bind(path: &Path) -> Result<UnixListener, String> {
    if let Some(listener) = activated_listener() {
        return Ok(listener);
    }
    if let Some(dir) = path.parent() {
        if !dir.exists() {
            std::fs::create_dir_all(dir)
                .map_err(|err| format!("failed to create {}: {err}", dir.display()))?;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            return Err(format!(
                "another klemmbrett is already running on {}",
                path.display()
            ));
        }
        std::fs::remove_file(path)
            .map_err(|err| format!("failed to remove stale {}: {err}", path.display()))?;
    }
    let listener = UnixListener::bind(path)
        .map_err(|err| format!("failed to listen on {}: {err}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|err| format!("failed to restrict {}: {err}", path.display()))?;
    Ok(listener)
}

fn activated_listener() -> Option<UnixListener> {
    let pid: u32 = std::env::var("LISTEN_PID").ok()?.parse().ok()?;
    let fds: i32 = std::env::var("LISTEN_FDS").ok()?.parse().ok()?;
    if pid != std::process::id() || fds < 1 {
        return None;
    }
    for name in ["LISTEN_PID", "LISTEN_FDS", "LISTEN_FDNAMES"] {
        std::env::remove_var(name);
    }
    // SAFETY: systemd hands over fd 3 as the first listening socket and
    // nothing else in this process owns it.
    Some(unsafe { UnixListener::from_raw_fd(3) })
}

/// Answers Varlink calls. `show` hands requests to the UI; tests pass their own.
pub struct Server {
    widgets: Vec<WidgetDef>,
    handlers: Arc<dyn Handlers>,
    notifications: Option<Arc<NotificationCenter>>,
    show: Box<dyn Fn(ShowRequest) -> Result<(), String> + Send + Sync>,
}

impl Server {
    pub fn new(
        widgets: Vec<WidgetDef>,
        handlers: Arc<dyn Handlers>,
        notifications: Option<Arc<NotificationCenter>>,
        show: impl Fn(ShowRequest) -> Result<(), String> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            widgets,
            handlers,
            notifications,
            show: Box::new(show),
        })
    }

    /// Accepts connections on a background thread, one thread per client.
    /// Clients running as another user are turned away.
    pub fn serve(self: Arc<Self>, listener: UnixListener) {
        std::thread::Builder::new()
            .name("klemmbrett-varlink".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else {
                        continue;
                    };
                    if !same_user(&stream) {
                        continue;
                    }
                    let server = self.clone();
                    let _ = std::thread::Builder::new()
                        .name("klemmbrett-varlink-client".into())
                        .spawn(move || server.connection(stream));
                }
            })
            .expect("spawn Varlink thread");
    }

    fn connection(&self, stream: UnixStream) {
        let Ok(read) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(read);
        let mut out = stream;
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(0, &mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if buffer.last() == Some(&0) {
                buffer.pop();
            }
            let call: Value = match serde_json::from_slice(&buffer) {
                Ok(call) => call,
                Err(_) => return,
            };
            let oneway = call["oneway"].as_bool().unwrap_or(false);
            let mut send = |reply: Value| -> bool {
                if oneway {
                    return true;
                }
                write_message(&mut out, &reply).is_ok()
            };
            if !self.dispatch(&call, &mut send) {
                return;
            }
        }
    }

    /// Runs one call. Returns `false` when the client went away.
    fn dispatch(&self, call: &Value, send: &mut dyn FnMut(Value) -> bool) -> bool {
        let method = call["method"].as_str().unwrap_or_default();
        let parameters = match &call["parameters"] {
            Value::Null => Value::Object(Map::new()),
            other => other.clone(),
        };
        let more = call["more"].as_bool().unwrap_or(false);
        let Some((interface, name)) = method.rsplit_once('.') else {
            return send(error_reply(&service_error(
                "InterfaceNotFound",
                json!({ "interface": method }),
            )));
        };
        let result = match (interface, name) {
            (SERVICE, "GetInfo") => Ok(json!({
                "vendor": "wontfix",
                "product": "klemmbrett",
                "version": env!("CARGO_PKG_VERSION"),
                "url": "https://github.com/wontfix-org/klemmbrett",
                "interfaces": [SERVICE, INTERFACE],
            })),
            (SERVICE, "GetInterfaceDescription") => match parameters["interface"].as_str() {
                Some(SERVICE) => Ok(json!({ "description": SERVICE_DESCRIPTION })),
                Some(INTERFACE) => Ok(json!({ "description": INTERFACE_DESCRIPTION })),
                Some(other) => Err(service_error(
                    "InterfaceNotFound",
                    json!({ "interface": other }),
                )),
                None => Err(invalid("interface")),
            },
            (INTERFACE, "Subscribe") => return self.subscribe(&parameters, more, send),
            (INTERFACE, "Show") => self.show(&parameters, false),
            (INTERFACE, "Prompt") => self.show(&parameters, true),
            (INTERFACE, "Widgets") => Ok(self.widget_info()),
            (INTERFACE, "Records") => self.records(&parameters),
            (INTERFACE, "StashList" | "StashMarkRead") => self.handlers.call(name, parameters),
            (INTERFACE, "Notify") => self.notify(&parameters),
            (SERVICE | INTERFACE, _) => {
                Err(service_error("MethodNotFound", json!({ "method": method })))
            }
            _ => Err(service_error(
                "InterfaceNotFound",
                json!({ "interface": interface }),
            )),
        };
        send(match result {
            Ok(parameters) => json!({ "parameters": parameters }),
            Err(error) => error_reply(&error),
        })
    }

    fn widget(&self, name: &str) -> Result<&WidgetDef, CallError> {
        self.widgets
            .iter()
            .find(|def| def.name == name)
            .ok_or_else(|| {
                let mut known: Vec<_> = self.widgets.iter().map(|def| def.name.clone()).collect();
                known.sort();
                CallError::new("UnknownWidget", json!({ "widget": name, "known": known }))
            })
    }

    fn show(&self, parameters: &Value, prompt: bool) -> Result<Value, CallError> {
        let widget = parameters["widget"]
            .as_str()
            .ok_or_else(|| invalid("widget"))?;
        let def = self.widget(widget)?;
        let pointer = match &parameters["pointer"] {
            Value::Null => None,
            point => {
                let coordinate = |name: &str| {
                    point[name]
                        .as_i64()
                        .and_then(|value| i32::try_from(value).ok())
                        .ok_or_else(|| invalid("pointer"))
                };
                Some((coordinate("x")?, coordinate("y")?))
            }
        };
        let records = match &parameters["records"] {
            Value::Null => None,
            Value::Array(rows) if rows.is_empty() => None,
            Value::Array(rows) => Some(decode_records(widget, rows)?),
            _ => return Err(invalid("records")),
        };
        if records.is_none() && def.source.is_none() {
            return Err(CallError::new("NoProvider", json!({ "widget": widget })));
        }
        let (reply, answer) = if prompt {
            let (tx, rx) = mpsc::channel();
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        (self.show)(ShowRequest {
            widget: widget.to_owned(),
            pointer,
            records,
            reply,
        })
        .map_err(CallError::failed)?;
        let Some(answer) = answer else {
            return Ok(json!({}));
        };
        match answer.recv() {
            Ok(Ok(values)) => Ok(json!({ "values": values })),
            Ok(Err(err)) => Err(CallError::failed(err)),
            Err(_) => Err(CallError::failed("the prompt closed without a result")),
        }
    }

    fn widget_info(&self) -> Value {
        let widgets: Vec<_> = self
            .widgets
            .iter()
            .map(|def| {
                json!({
                    "name": def.name,
                    "provider": def.source.is_some(),
                    "multi": def.multi.is_some(),
                })
            })
            .collect();
        json!({ "widgets": widgets })
    }

    fn records(&self, parameters: &Value) -> Result<Value, CallError> {
        let widget = parameters["widget"]
            .as_str()
            .ok_or_else(|| invalid("widget"))?;
        let def = self.widget(widget)?;
        if def.source.is_none() {
            return Err(CallError::new("NoProvider", json!({ "widget": widget })));
        }
        let records = fetch_records(def).map_err(CallError::failed)?;
        Ok(json!({ "records": records.iter().map(Record::to_json).collect::<Vec<_>>() }))
    }

    fn notify(&self, parameters: &Value) -> Result<Value, CallError> {
        let center = self
            .notifications
            .as_ref()
            .ok_or_else(|| CallError::new("NotificationsDisabled", json!({})))?;
        let text = |name: &str| parameters[name].as_str().unwrap_or_default().to_owned();
        if !parameters["app_name"].is_string() {
            return Err(invalid("app_name"));
        }
        if !parameters["summary"].is_string() {
            return Err(invalid("summary"));
        }
        let urgency = match &parameters["urgency"] {
            Value::Null => 1,
            value => value
                .as_u64()
                .filter(|urgency| *urgency <= 2)
                .ok_or_else(|| invalid("urgency"))?,
        };
        let timeout = match &parameters["timeout"] {
            Value::Null => -1,
            value => value
                .as_i64()
                .and_then(|ms| i32::try_from(ms).ok())
                .ok_or_else(|| invalid("timeout"))?,
        };
        let id = center.notify(NotifyRequest {
            app_name: text("app_name"),
            summary: text("summary"),
            body: text("body"),
            hints: json!({ "urgency": urgency }),
            expire_timeout: timeout,
            ..NotifyRequest::default()
        });
        Ok(json!({ "id": id }))
    }

    fn subscribe(
        &self,
        parameters: &Value,
        more: bool,
        send: &mut dyn FnMut(Value) -> bool,
    ) -> bool {
        if !more {
            return send(error_reply(&service_error("ExpectedMore", json!({}))));
        }
        let filter = match &parameters["widgets"] {
            Value::Null => None,
            Value::Array(names) => {
                let mut wanted = HashSet::new();
                for name in names {
                    let Some(name) = name.as_str() else {
                        return send(error_reply(&invalid("widgets")));
                    };
                    if let Err(error) = self.widget(name) {
                        return send(error_reply(&error));
                    }
                    wanted.insert(name.to_owned());
                }
                Some(wanted)
            }
            _ => return send(error_reply(&invalid("widgets"))),
        };
        let events = events::subscribe(filter);
        let event = |event: Value| json!({ "parameters": { "event": event }, "continues": true });
        if !send(event(json!({ "kind": "ready" }))) {
            return false;
        }
        loop {
            match events.recv_timeout(Duration::from_secs(1)) {
                Ok(next) => {
                    if !send(event(next)) {
                        return false;
                    }
                }
                // A heartbeat would break the reply stream, so probe the socket instead.
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if !send(Value::Null) {
                        return false;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return false,
            }
        }
    }
}

fn decode_records(widget: &str, rows: &[Value]) -> Result<Vec<Record>, CallError> {
    let mut records = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let record = Record::from_json(row).map_err(|message| {
            CallError::new(
                "InvalidRecord",
                json!({ "path": format!("records[{index}]"), "message": message }),
            )
        })?;
        records.push(record);
    }
    crate::widget::validate_records(widget, &records).map_err(|message| {
        CallError::new(
            "InvalidRecord",
            json!({ "path": "records", "message": message }),
        )
    })?;
    Ok(records)
}

fn invalid(parameter: &str) -> CallError {
    service_error("InvalidParameter", json!({ "parameter": parameter }))
}

fn service_error(name: &str, parameters: Value) -> CallError {
    CallError::new(format!("{SERVICE}.{name}"), parameters)
}

fn error_reply(error: &CallError) -> Value {
    let name = if error.error.contains('.') {
        error.error.clone()
    } else {
        format!("{INTERFACE}.{}", error.error)
    };
    json!({ "error": name, "parameters": error.parameters })
}

/// Writes one message. `null` writes nothing and only checks that the peer
/// is still there.
fn write_message(out: &mut UnixStream, message: &Value) -> io::Result<()> {
    if message.is_null() {
        return if peer_closed(out) {
            Err(io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        };
    }
    let mut bytes = serde_json::to_vec(message)?;
    bytes.push(0);
    out.write_all(&bytes)?;
    out.flush()
}

fn peer_closed(stream: &UnixStream) -> bool {
    let mut poll = libc::pollfd {
        fd: stream.as_raw_fd(),
        events: libc::POLLRDHUP,
        revents: 0,
    };
    // SAFETY: one valid pollfd, no timeout.
    let ready = unsafe { libc::poll(&mut poll, 1, 0) };
    ready > 0 && poll.revents & (libc::POLLRDHUP | libc::POLLHUP | libc::POLLERR) != 0
}

fn same_user(stream: &UnixStream) -> bool {
    let mut cred = libc::ucred {
        pid: 0,
        uid: u32::MAX,
        gid: u32::MAX,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: SO_PEERCRED fills a ucred of the given size.
    let status = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    // SAFETY: getuid has no preconditions.
    status == 0 && cred.uid == unsafe { libc::getuid() }
}

/// What went wrong talking to the daemon.
#[derive(Debug)]
pub enum ClientError {
    /// No daemon answers on the socket.
    NotRunning(PathBuf, io::Error),
    Io(io::Error),
    Protocol(String),
    /// The daemon answered with a Varlink error.
    Remote {
        error: String,
        parameters: Value,
    },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning(path, err) => write!(
                f,
                "klemmbrett is not running (no daemon on {}: {err})",
                path.display()
            ),
            Self::Io(err) => write!(f, "{err}"),
            Self::Protocol(message) => write!(f, "protocol error: {message}"),
            Self::Remote { error, parameters } => {
                let short = error.rsplit('.').next().unwrap_or(error);
                match short {
                    "UnknownWidget" => {
                        let known: Vec<_> = parameters["known"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .collect();
                        if known.is_empty() {
                            write!(
                                f,
                                "unknown widget {}: no popup widgets are configured",
                                parameters["widget"]
                            )
                        } else {
                            write!(
                                f,
                                "unknown widget {}; known: {}",
                                parameters["widget"],
                                known.join(", ")
                            )
                        }
                    }
                    "NoProvider" => write!(
                        f,
                        "widget {} has no provider; pass records",
                        parameters["widget"]
                    ),
                    "InvalidRecord" => write!(
                        f,
                        "{}: {}",
                        parameters["path"].as_str().unwrap_or("records"),
                        parameters["message"].as_str().unwrap_or_default()
                    ),
                    "Failed" => write!(f, "{}", parameters["message"].as_str().unwrap_or(error)),
                    _ if parameters.as_object().is_some_and(|map| !map.is_empty()) => {
                        write!(f, "{error} {parameters}")
                    }
                    _ => write!(f, "{error}"),
                }
            }
        }
    }
}

impl std::error::Error for ClientError {}

impl From<io::Error> for ClientError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// A blocking Varlink connection to the daemon.
pub struct Client {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Client {
    pub fn connect(path: &Path) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(path)
            .map_err(|err| ClientError::NotRunning(path.to_owned(), err))?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Self {
            writer: stream,
            reader,
        })
    }

    /// Calls `method` (unqualified names are in `org.wontfix.klemmbrett`)
    /// and returns the reply parameters.
    pub fn call(&mut self, method: &str, parameters: Value) -> Result<Value, ClientError> {
        self.send(method, parameters, false)?;
        self.receive().map(|(parameters, _)| parameters)
    }

    /// Calls a streaming method and passes every reply to `each` until it
    /// returns `false` or the daemon ends the stream.
    pub fn stream(
        &mut self,
        method: &str,
        parameters: Value,
        mut each: impl FnMut(Value) -> bool,
    ) -> Result<(), ClientError> {
        self.send(method, parameters, true)?;
        loop {
            let (parameters, continues) = self.receive()?;
            if !each(parameters) || !continues {
                return Ok(());
            }
        }
    }

    fn send(&mut self, method: &str, parameters: Value, more: bool) -> Result<(), ClientError> {
        let method = if method.contains('.') {
            method.to_owned()
        } else {
            format!("{INTERFACE}.{method}")
        };
        let mut call = json!({ "method": method, "parameters": parameters });
        if more {
            call["more"] = json!(true);
        }
        write_message(&mut self.writer, &call)?;
        Ok(())
    }

    fn receive(&mut self) -> Result<(Value, bool), ClientError> {
        let mut buffer = Vec::new();
        if self.reader.read_until(0, &mut buffer)? == 0 {
            return Err(ClientError::Protocol(
                "the daemon closed the connection".into(),
            ));
        }
        if buffer.last() == Some(&0) {
            buffer.pop();
        }
        let mut reply: Value = serde_json::from_slice(&buffer)
            .map_err(|err| ClientError::Protocol(format!("invalid reply: {err}")))?;
        let parameters = reply["parameters"].take();
        if let Some(error) = reply["error"].as_str() {
            return Err(ClientError::Remote {
                error: error.to_owned(),
                parameters,
            });
        }
        let continues = reply["continues"].as_bool().unwrap_or(false);
        Ok((parameters, continues))
    }
}

/// Records as Varlink parameters.
pub fn records_json(records: &[Record]) -> Value {
    Value::Array(records.iter().map(Record::to_json).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_socket_path_wins() {
        assert_eq!(
            socket_path(Some(Path::new("/x/y.sock"))),
            PathBuf::from("/x/y.sock")
        );
    }

    #[test]
    fn errors_are_qualified_unless_they_already_are() {
        let reply = error_reply(&CallError::new("NoProvider", json!({ "widget": "w" })));
        assert_eq!(reply["error"], "org.wontfix.klemmbrett.NoProvider");
        let reply = error_reply(&invalid("widget"));
        assert_eq!(reply["error"], "org.varlink.service.InvalidParameter");
        assert_eq!(reply["parameters"]["parameter"], "widget");
    }

    #[test]
    fn remote_errors_read_like_the_old_messages() {
        let error = ClientError::Remote {
            error: "org.wontfix.klemmbrett.UnknownWidget".into(),
            parameters: json!({ "widget": "missing", "known": ["history", "pick"] }),
        };
        assert_eq!(
            error.to_string(),
            r#"unknown widget "missing"; known: history, pick"#
        );
        let error = ClientError::Remote {
            error: "org.wontfix.klemmbrett.NoProvider".into(),
            parameters: json!({ "widget": "pick" }),
        };
        assert!(error.to_string().contains("has no provider; pass records"));
    }
}
