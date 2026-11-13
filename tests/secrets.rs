//! Secret questions end to end: `AskSecret` over a real Varlink socket, as
//! `klemmbrett askpass` and `pinentry` ask it, and the systemd password
//! agent answering `ask.*` files. A fake asker stands in for the popup.

use klemmbrett::secret::{
    AgentKind, AskError, Echo, Mode, SecretAsker, SecretReply, SecretRequest, SecretTicket,
};
use klemmbrett::varlink::{self, Server};
use klemmbrett::{ask_password, askpass, pinentry, Client, ClientError, NoHandlers, ShowRequest};
use serde_json::json;
use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use zeroize::Zeroizing;

/// Answers every question with `answer`, or holds it open when `None`.
struct FakeAsker {
    answer: Option<&'static str>,
    asked: Mutex<Vec<SecretRequest>>,
    open: Mutex<Vec<(u64, mpsc::Sender<SecretReply>)>>,
    withdrawn: Mutex<Vec<u64>>,
}

impl FakeAsker {
    fn new(answer: Option<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            answer,
            asked: Mutex::new(Vec::new()),
            open: Mutex::new(Vec::new()),
            withdrawn: Mutex::new(Vec::new()),
        })
    }
}

impl SecretAsker for FakeAsker {
    fn begin(&self, request: SecretRequest) -> Result<SecretTicket, AskError> {
        let (reply, answer) = mpsc::channel();
        let mut asked = self.asked.lock().unwrap();
        let id = asked.len() as u64 + 1;
        let reply_with = match (&request.mode, self.answer) {
            (Mode::Secret, Some(text)) => Some(SecretReply::Secret(Zeroizing::new(text.into()))),
            (_, Some(_)) => Some(SecretReply::Confirmed),
            (_, None) => None,
        };
        asked.push(request);
        match reply_with {
            Some(answer) => reply.send(answer).unwrap(),
            None => self.open.lock().unwrap().push((id, reply)),
        }
        Ok(SecretTicket::new(id, answer))
    }

    fn withdraw(&self, id: u64) {
        self.withdrawn.lock().unwrap().push(id);
        let mut open = self.open.lock().unwrap();
        if let Some(index) = open.iter().position(|(open_id, _)| *open_id == id) {
            let (_, reply) = open.remove(index);
            let _ = reply.send(SecretReply::Cancelled);
        }
    }
}

fn serve(asker: Arc<FakeAsker>, open: Arc<AtomicBool>) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("varlink.sock");
    let listener = varlink::bind(&socket).unwrap();
    let server = Server::new(
        vec![klemmbrett::WidgetDef::new("bare", None)],
        Arc::new(NoHandlers),
        None,
        |_request: ShowRequest| Ok(()),
    );
    server.answer_secrets(open, move |request| {
        if request.widget.as_deref() == Some("gone") {
            return Err(AskError::UnknownWidget("gone".into()));
        }
        asker.ask(request)
    });
    server.serve(listener);
    (dir, socket)
}

#[test]
fn askpass_and_pinentry_questions_come_back_to_the_asker() {
    let asker = FakeAsker::new(Some("hunter2"));
    let (_dir, socket) = serve(asker.clone(), Arc::new(AtomicBool::new(false)));

    let request = askpass::request("[sudo] password for me: ", None, Some("bare".into()));
    let answer = askpass::ask_daemon(&socket, &request).unwrap();
    assert_eq!(
        answer,
        SecretReply::Secret(Zeroizing::new("hunter2".into()))
    );
    let confirm = askpass::request("Allow?", Some("confirm"), None);
    assert_eq!(
        askpass::ask_daemon(&socket, &confirm).unwrap(),
        SecretReply::Confirmed
    );

    let mut gpg = Vec::new();
    let mut ask = |request: SecretRequest| {
        askpass::ask_daemon(&socket, &request).map_err(|err| err.to_string())
    };
    pinentry::serve(
        &mut "SETDESC Unlock%0Akey\nSETTIMEOUT 5\nGETPIN\nBYE\n".as_bytes(),
        &mut gpg,
        None,
        &mut ask,
    )
    .unwrap();
    assert!(String::from_utf8(gpg)
        .unwrap()
        .contains("\nD hunter2\nOK\n"));

    let asked = asker.asked.lock().unwrap();
    assert_eq!(asked.len(), 3);
    assert_eq!(asked[0].kind, AgentKind::Askpass);
    assert_eq!(asked[0].widget.as_deref(), Some("bare"));
    assert_eq!(asked[0].message, "[sudo] password for me:");
    assert_eq!(asked[1].mode, Mode::Confirm);
    assert_eq!(
        (
            asked[2].kind,
            asked[2].message.as_str(),
            asked[2].timeout,
            asked[2].echo
        ),
        (
            AgentKind::Pinentry,
            "Unlock\nkey",
            Some(Duration::from_secs(5)),
            Echo::Masked
        )
    );
}

#[test]
fn bad_kinds_unknown_widgets_and_busy_popups_are_errors() {
    let open = Arc::new(AtomicBool::new(false));
    let (_dir, socket) = serve(FakeAsker::new(Some("x")), open.clone());
    let mut client = Client::connect(&socket).unwrap();
    let error = client
        .call("AskSecret", json!({ "kind": "polkit" }))
        .unwrap_err();
    assert!(
        matches!(&error, ClientError::Remote { error, .. } if error.ends_with("InvalidParameter")),
        "{error}"
    );

    let request = askpass::request("pw", None, Some("gone".into()));
    let error = askpass::ask_daemon(&socket, &request).unwrap_err();
    assert!(
        error.to_string().contains("unknown widget \"gone\""),
        "{error}"
    );

    open.store(true, Ordering::Relaxed);
    let show = json!({ "widget": "bare", "records": [{ "value": "v" }] });
    let error = client.call("Show", show.clone()).unwrap_err();
    assert_eq!(error.to_string(), "a password prompt is open");
    open.store(false, Ordering::Relaxed);
    client.call("Show", show).unwrap();
}

#[test]
fn cancelling_is_an_answer() {
    let asker = FakeAsker::new(None);
    let (_dir, socket) = serve(asker.clone(), Arc::new(AtomicBool::new(false)));
    let waiter = {
        let socket = socket.clone();
        std::thread::spawn(move || {
            askpass::ask_daemon(&socket, &askpass::request("pw", None, None)).unwrap()
        })
    };
    while asker.open.lock().unwrap().is_empty() {
        std::thread::sleep(Duration::from_millis(5));
    }
    asker.withdraw(1);
    assert_eq!(waiter.join().unwrap(), SecretReply::Cancelled);
}

fn write_ask(
    dir: &std::path::Path,
    name: &str,
    socket: &std::path::Path,
    message: &str,
) -> PathBuf {
    let path = dir.join(name);
    let staging = dir.join(format!(".{name}"));
    std::fs::write(
        &staging,
        format!(
            "[Ask]\nPID={}\nSocket={}\nAcceptCached=0\nEcho=0\nNotAfter=0\nMessage={message}\nId=test:{name}\n",
            std::process::id(),
            socket.display()
        ),
    )
    .unwrap();
    std::fs::rename(&staging, &path).unwrap();
    path
}

#[test]
fn the_systemd_agent_answers_and_withdraws_ask_files() {
    let dir = tempfile::tempdir().unwrap();
    let asks = dir.path().join("ask-password");
    std::fs::create_dir(&asks).unwrap();
    let reply_socket = dir.path().join("sck.existing");
    let replies = UnixDatagram::bind(&reply_socket).unwrap();
    replies
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write_ask(
        &asks,
        "ask.existing",
        &reply_socket,
        "Key for \\\"backup\\\"",
    );

    let answering = FakeAsker::new(Some("s3cret"));
    {
        let asker: Arc<dyn SecretAsker> = answering.clone();
        let asks = asks.clone();
        std::thread::spawn(move || ask_password::watch(asker, &[asks]));
    }
    let mut buffer = [0u8; 64];
    let length = replies.recv(&mut buffer).unwrap();
    assert_eq!(&buffer[..length], b"+s3cret");
    let asked = answering.asked.lock().unwrap().remove(0);
    assert_eq!(asked.kind, AgentKind::Systemd);
    assert_eq!(asked.message, "Key for \"backup\"");
    assert_eq!(asked.id, "test:ask.existing");

    let asks = dir.path().join("ask-password-later");
    let waiting = FakeAsker::new(None);
    {
        let asker: Arc<dyn SecretAsker> = waiting.clone();
        let asks = asks.clone();
        std::thread::spawn(move || ask_password::watch(asker, &[asks]));
    }
    while !asks.exists() {
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(100));
    let later_socket = dir.path().join("sck.later");
    let later = UnixDatagram::bind(&later_socket).unwrap();
    later
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let path = write_ask(&asks, "ask.later", &later_socket, "Later");
    while waiting.open.lock().unwrap().is_empty() {
        std::thread::sleep(Duration::from_millis(5));
    }
    std::fs::remove_file(path).unwrap();
    let length = later.recv(&mut buffer).unwrap();
    assert_eq!(&buffer[..length], b"-");
    assert_eq!(*waiting.withdrawn.lock().unwrap(), vec![1]);
}
