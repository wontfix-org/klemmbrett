//! A polkit authentication agent: asks for the password when an action
//! needs administrator (or other user) authentication.
//!
//! The agent registers for the login session with polkitd. polkitd calls
//! `BeginAuthentication` with a cookie and the identities that may
//! authenticate; the agent runs polkit's helper for the chosen user, relays
//! its PAM conversation through the popup, and the helper reports the result
//! to polkitd. The agent never sees whether the password was right except
//! through the helper's `SUCCESS` / `FAILURE` line.

use crate::journal;
use crate::secret::{AgentKind, AskError, Echo, Mode, SecretAsker, SecretReply, SecretRequest};
use std::collections::HashMap;
use std::ffi::CStr;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use zbus::interface;
use zbus::zvariant::{OwnedValue, Value as Variant};
use zeroize::Zeroizing;

const AUTHORITY: &str = "org.freedesktop.PolicyKit1";
const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const AUTHORITY_INTERFACE: &str = "org.freedesktop.PolicyKit1.Authority";
const OBJECT_PATH: &str = "/org/wontfix/klemmbrett/PolkitAgent";
const ATTEMPTS: usize = 3;

/// Where polkit and the agent's session live.
#[derive(Debug, Clone)]
pub struct Options {
    /// The bus polkitd is on; `None` is the system bus.
    pub address: Option<String>,
    /// The login session to register for; `None` asks logind.
    pub session_id: Option<String>,
    /// polkit ≥ 121 starts its helper through this socket.
    pub helper_socket: PathBuf,
    /// The setuid helper older polkit installs.
    pub helper_binary: PathBuf,
}

impl Options {
    pub fn system() -> Self {
        let helper_binary = [
            "/usr/lib/polkit-1/polkit-agent-helper-1",
            "/usr/libexec/polkit-agent-helper-1",
            "/usr/lib/policykit-1/polkit-agent-helper-1",
        ]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("/usr/lib/polkit-1/polkit-agent-helper-1"));
        Self {
            address: None,
            session_id: std::env::var("XDG_SESSION_ID")
                .ok()
                .filter(|id| !id.is_empty()),
            helper_socket: PathBuf::from("/run/polkit/agent-helper.socket"),
            helper_binary,
        }
    }
}

/// Registers on a background thread for as long as the daemon runs.
pub fn spawn(asker: Arc<dyn SecretAsker>, options: Options) {
    let spawned = std::thread::Builder::new()
        .name("klemmbrett-polkit".into())
        .spawn(move || match register(asker, &options) {
            Ok(_conn) => loop {
                std::thread::park();
            },
            Err(err) => journal::error(&format!("the polkit agent is off: {err}")),
        });
    if let Err(err) = spawned {
        journal::error(&format!("failed to start the polkit agent: {err}"));
    }
}

/// Exports the agent and registers it with polkitd. The agent answers for
/// as long as the returned connection lives.
pub fn register(
    asker: Arc<dyn SecretAsker>,
    options: &Options,
) -> Result<zbus::blocking::Connection, String> {
    let agent = Agent(Arc::new(Shared {
        asker,
        helper_socket: options.helper_socket.clone(),
        helper_binary: options.helper_binary.clone(),
        sessions: Mutex::new(HashMap::new()),
    }));
    let conn = match &options.address {
        Some(address) => zbus::blocking::connection::Builder::address(address.as_str()),
        None => zbus::blocking::connection::Builder::system(),
    }
    .and_then(|builder| builder.serve_at(OBJECT_PATH, agent))
    .and_then(|builder| builder.build())
    .map_err(|err| format!("no system bus: {err}"))?;
    let session_id = match &options.session_id {
        Some(id) => id.clone(),
        None => logind_session(&conn)?,
    };
    let subject = (
        "unix-session",
        HashMap::from([("session-id", Variant::from(session_id.as_str()))]),
    );
    let locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_else(|_| "C".into());
    conn.call_method(
        Some(AUTHORITY),
        AUTHORITY_PATH,
        Some(AUTHORITY_INTERFACE),
        "RegisterAuthenticationAgent",
        &(subject, locale.as_str(), OBJECT_PATH),
    )
    .map_err(|err| format!("polkit refused the agent for session {session_id}: {err}"))?;
    journal::info(&format!("polkit agent registered for session {session_id}"));
    Ok(conn)
}

/// The graphical session of this user, as logind knows it.
fn logind_session(conn: &zbus::blocking::Connection) -> Result<String, String> {
    let uid = unsafe { libc::getuid() };
    let user: zbus::zvariant::OwnedObjectPath = conn
        .call_method(
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1",
            Some("org.freedesktop.login1.Manager"),
            "GetUser",
            &(uid,),
        )
        .and_then(|reply| reply.body().deserialize())
        .map_err(|err| format!("logind does not know uid {uid}: {err}"))?;
    let proxy = zbus::blocking::Proxy::new(
        conn,
        "org.freedesktop.login1",
        user,
        "org.freedesktop.login1.User",
    )
    .map_err(|err| format!("logind: {err}"))?;
    let (id, _): (String, zbus::zvariant::OwnedObjectPath) = proxy
        .get_property("Display")
        .map_err(|err| format!("logind has no session for uid {uid}: {err}"))?;
    if id.is_empty() {
        return Err(format!(
            "uid {uid} has no graphical session; set XDG_SESSION_ID"
        ));
    }
    Ok(id)
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.PolicyKit1.Error")]
enum PolkitError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Failed(String),
    Cancelled(String),
}

impl From<Stop> for PolkitError {
    fn from(stop: Stop) -> Self {
        match stop {
            Stop::Cancelled => Self::Cancelled("authentication was cancelled".into()),
            Stop::Failed(message) => Self::Failed(message),
        }
    }
}

struct Agent(Arc<Shared>);

struct Shared {
    asker: Arc<dyn SecretAsker>,
    helper_socket: PathBuf,
    helper_binary: PathBuf,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

#[interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl Agent {
    async fn begin_authentication(
        &self,
        action_id: String,
        message: String,
        icon_name: String,
        details: HashMap<String, String>,
        cookie: String,
        identities: Vec<(String, HashMap<String, OwnedValue>)>,
    ) -> Result<(), PolkitError> {
        let session = Arc::new(Session::default());
        self.0
            .sessions
            .lock()
            .unwrap()
            .insert(cookie.clone(), session.clone());
        let shared = self.0.clone();
        let users = users(&identities);
        let auth = Auth {
            action_id,
            message,
            icon_name,
            details,
            cookie: cookie.clone(),
        };
        let result = blocking::unblock(move || {
            let result = shared.authenticate(&session, &auth, users);
            shared.sessions.lock().unwrap().remove(&auth.cookie);
            result
        })
        .await;
        result.map_err(PolkitError::from)
    }

    fn cancel_authentication(&self, cookie: String) -> Result<(), PolkitError> {
        let session = self.0.sessions.lock().unwrap().get(&cookie).cloned();
        match session {
            Some(session) => {
                session.cancel(&*self.0.asker);
                Ok(())
            }
            None => Err(PolkitError::Failed(format!(
                "no authentication for cookie {cookie}"
            ))),
        }
    }
}

struct Auth {
    action_id: String,
    message: String,
    icon_name: String,
    details: HashMap<String, String>,
    cookie: String,
}

/// Why a conversation stopped without a result.
#[derive(Debug, PartialEq)]
pub enum Stop {
    Cancelled,
    Failed(String),
}

impl From<AskError> for Stop {
    fn from(err: AskError) -> Self {
        Self::Failed(err.to_string())
    }
}

/// One `BeginAuthentication` call: its open question and helper, so that
/// `CancelAuthentication` can end both.
#[derive(Default)]
struct Session {
    cancelled: AtomicBool,
    ticket: Mutex<Option<u64>>,
    closer: Mutex<Option<Closer>>,
}

impl Session {
    fn ask(&self, asker: &dyn SecretAsker, request: SecretRequest) -> Result<SecretReply, Stop> {
        let ticket = {
            let mut slot = self.ticket.lock().unwrap();
            if self.cancelled.load(Ordering::SeqCst) {
                return Err(Stop::Cancelled);
            }
            let ticket = asker.begin(request)?;
            *slot = Some(ticket.id);
            ticket
        };
        let reply = ticket.wait();
        self.ticket.lock().unwrap().take();
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(Stop::Cancelled);
        }
        Ok(reply)
    }

    fn cancel(&self, asker: &dyn SecretAsker) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(id) = self.ticket.lock().unwrap().take() {
            asker.withdraw(id);
        }
        if let Some(closer) = self.closer.lock().unwrap().take() {
            closer.close();
        }
    }
}

enum Closer {
    Socket(UnixStream),
    Child(u32),
}

impl Closer {
    fn close(self) {
        match self {
            Self::Socket(stream) => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            // SAFETY: signals our own child, which has not been reaped yet.
            Self::Child(pid) => unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            },
        }
    }
}

/// polkit's helper, talking PAM for one user.
struct Helper {
    reader: Box<dyn BufRead + Send>,
    writer: Box<dyn Write + Send>,
    child: Option<Child>,
}

impl Shared {
    fn authenticate(&self, session: &Session, auth: &Auth, users: Vec<String>) -> Result<(), Stop> {
        let user = match users.as_slice() {
            [] => return Err(Stop::Failed("no identity to authenticate as".into())),
            [user] => user.clone(),
            _ => {
                let mut request = self.request(auth, "");
                request.prompt = Some("Authenticate as".into());
                request.mode = Mode::Choose(users.clone());
                match session.ask(&*self.asker, request)? {
                    SecretReply::Chose(index) if index < users.len() => users[index].clone(),
                    _ => return Err(Stop::Cancelled),
                }
            }
        };
        let mut error = None;
        for _ in 0..ATTEMPTS {
            let mut helper = self.open(session, &user, &auth.cookie)?;
            let base = self.request(auth, &user);
            let outcome = converse(
                &mut helper.reader,
                &mut helper.writer,
                error.take(),
                |prompt| {
                    let mut request = base.clone();
                    request.prompt = Some(prompt.text);
                    request.error = prompt.error;
                    request.echo = if prompt.echo { Echo::Yes } else { Echo::Masked };
                    match session.ask(&*self.asker, request)? {
                        SecretReply::Secret(secret) => Ok(secret),
                        _ => Err(Stop::Cancelled),
                    }
                },
            );
            session.closer.lock().unwrap().take();
            drop(helper.writer);
            if let Some(mut child) = helper.child.take() {
                if outcome.is_err() {
                    let _ = child.kill();
                }
                let _ = child.wait();
            }
            match outcome? {
                true => return Ok(()),
                false => error = Some("Sorry, that did not work. Try again.".into()),
            }
        }
        Err(Stop::Failed("authentication failed".into()))
    }

    fn request(&self, auth: &Auth, user: &str) -> SecretRequest {
        let mut request = SecretRequest::new(AgentKind::Polkit);
        request.id = auth.action_id.clone();
        request.title = "Authentication required".into();
        request.message = auth.message.clone();
        request
            .details
            .push(("action_id".into(), auth.action_id.clone()));
        if !user.is_empty() {
            request.details.push(("user".into(), user.into()));
        }
        if !auth.icon_name.is_empty() {
            request
                .details
                .push(("icon".into(), auth.icon_name.clone()));
        }
        let mut details: Vec<_> = auth.details.iter().collect();
        details.sort();
        request.details.extend(
            details
                .into_iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        request
    }

    /// Starts the helper through polkit's socket, or the setuid binary.
    fn open(&self, session: &Session, user: &str, cookie: &str) -> Result<Helper, Stop> {
        let failed = |err: std::io::Error| Stop::Failed(format!("polkit's helper: {err}"));
        let helper = if self.helper_socket.exists() {
            let mut stream = UnixStream::connect(&self.helper_socket).map_err(failed)?;
            stream
                .write_all(format!("{user}\n{cookie}\n").as_bytes())
                .map_err(failed)?;
            *session.closer.lock().unwrap() =
                Some(Closer::Socket(stream.try_clone().map_err(failed)?));
            Helper {
                reader: Box::new(BufReader::new(stream.try_clone().map_err(failed)?)),
                writer: Box::new(stream),
                child: None,
            }
        } else {
            let mut child = Command::new(&self.helper_binary)
                .arg(user)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(failed)?;
            let mut stdin = child.stdin.take().expect("piped stdin");
            let stdout = child.stdout.take().expect("piped stdout");
            stdin
                .write_all(format!("{cookie}\n").as_bytes())
                .map_err(failed)?;
            *session.closer.lock().unwrap() = Some(Closer::Child(child.id()));
            Helper {
                reader: Box::new(BufReader::new(stdout)),
                writer: Box::new(stdin),
                child: Some(child),
            }
        };
        if session.cancelled.load(Ordering::SeqCst) {
            return Err(Stop::Cancelled);
        }
        Ok(helper)
    }
}

/// A PAM question from the helper.
#[derive(Debug, PartialEq)]
pub struct Prompt {
    pub text: String,
    pub echo: bool,
    /// Error and info lines PAM sent since the last question.
    pub error: Option<String>,
}

/// Relays the helper's PAM conversation: each prompt goes to `ask`, its
/// answer back to the helper. `Ok(true)` when the helper reports success.
pub fn converse(
    reader: &mut dyn BufRead,
    writer: &mut dyn Write,
    mut error: Option<String>,
    mut ask: impl FnMut(Prompt) -> Result<Zeroizing<String>, Stop>,
) -> Result<bool, Stop> {
    let failed = |err: std::io::Error| Stop::Failed(format!("polkit's helper: {err}"));
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).map_err(failed)? == 0 {
            return Err(Stop::Failed(
                "polkit's helper exited without a result".into(),
            ));
        }
        let line = line.trim_end_matches('\n');
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let rest = crate::secret::unescape_c(rest);
        let echo = match word {
            "SUCCESS" => return Ok(true),
            "FAILURE" => return Ok(false),
            "PAM_PROMPT_ECHO_OFF" => false,
            "PAM_PROMPT_ECHO_ON" => true,
            "PAM_ERROR_MSG" | "PAM_TEXT_INFO" => {
                let text = rest.trim_end().to_owned();
                error = Some(match error.take() {
                    Some(previous) => format!("{previous}\n{text}"),
                    None => text,
                });
                continue;
            }
            _ => {
                journal::warn(&format!("polkit's helper said something odd: {word}"));
                continue;
            }
        };
        let answer = ask(Prompt {
            text: rest,
            echo,
            error: error.take(),
        })?;
        let mut reply = Zeroizing::new(Vec::with_capacity(answer.len() + 1));
        reply.extend_from_slice(answer.as_bytes());
        reply.push(b'\n');
        writer.write_all(&reply).map_err(failed)?;
        writer.flush().map_err(failed)?;
    }
}

/// The user names that may authenticate, this user first.
fn users(identities: &[(String, HashMap<String, OwnedValue>)]) -> Vec<String> {
    let me = unsafe { libc::getuid() };
    let mut uids = Vec::new();
    let mut names = Vec::new();
    for (kind, fields) in identities {
        let id = fields
            .get(if kind == "unix-user" { "uid" } else { "gid" })
            .and_then(|value| value.downcast_ref::<u32>().ok());
        match (kind.as_str(), id) {
            ("unix-user", Some(uid)) => uids.push(uid),
            ("unix-group", Some(gid)) => names.extend(group_members(gid)),
            _ => {}
        }
    }
    let mut users: Vec<(bool, String)> = uids
        .into_iter()
        .filter_map(|uid| user_name(uid).map(|name| (uid != me, name)))
        .collect();
    let my_name = user_name(me);
    users.extend(
        names
            .into_iter()
            .map(|name| (Some(&name) != my_name.as_ref(), name)),
    );
    users.sort_by_key(|(other, _)| *other);
    let mut seen = std::collections::HashSet::new();
    users
        .into_iter()
        .filter_map(|(_, name)| seen.insert(name.clone()).then_some(name))
        .collect()
}

fn user_name(uid: u32) -> Option<String> {
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: getpwuid_r writes into pwd and buffer, which outlive the read.
    unsafe {
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut found = std::ptr::null_mut();
        let rc = libc::getpwuid_r(uid, &mut pwd, buffer.as_mut_ptr(), buffer.len(), &mut found);
        if rc != 0 || found.is_null() || pwd.pw_name.is_null() {
            return None;
        }
        CStr::from_ptr(pwd.pw_name).to_str().ok().map(str::to_owned)
    }
}

fn group_members(gid: u32) -> Vec<String> {
    let mut buffer = vec![0 as libc::c_char; 64 * 1024];
    // SAFETY: getgrgid_r writes into grp and buffer, which outlive the reads.
    unsafe {
        let mut grp: libc::group = std::mem::zeroed();
        let mut found = std::ptr::null_mut();
        let rc = libc::getgrgid_r(gid, &mut grp, buffer.as_mut_ptr(), buffer.len(), &mut found);
        if rc != 0 || found.is_null() || grp.gr_mem.is_null() {
            return Vec::new();
        }
        let mut members = Vec::new();
        let mut member = grp.gr_mem;
        while !(*member).is_null() {
            if let Ok(name) = CStr::from_ptr(*member).to_str() {
                members.push(name.to_owned());
            }
            member = member.add(1);
        }
        members
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn run(transcript: &str, answers: &[&str]) -> (Result<bool, Stop>, Vec<Prompt>, String) {
        let mut reader = Cursor::new(transcript.as_bytes().to_vec());
        let mut written = Vec::new();
        let mut prompts = Vec::new();
        let mut answers = answers.iter();
        let result = converse(&mut reader, &mut written, None, |prompt| {
            prompts.push(prompt);
            answers
                .next()
                .map(|answer| Zeroizing::new((*answer).to_owned()))
                .ok_or(Stop::Cancelled)
        });
        (result, prompts, String::from_utf8(written).unwrap())
    }

    #[test]
    fn the_pam_conversation_is_relayed() {
        let (result, prompts, written) = run(
            "PAM_TEXT_INFO Touch\\040the key\nPAM_PROMPT_ECHO_OFF Password: \nSUCCESS\n",
            &["hunter2"],
        );
        assert_eq!(result, Ok(true));
        assert_eq!(written, "hunter2\n");
        assert_eq!(
            prompts,
            vec![Prompt {
                text: "Password: ".into(),
                echo: false,
                error: Some("Touch the key".into()),
            }]
        );
    }

    #[test]
    fn failure_cancel_and_eof_are_told_apart() {
        let (result, prompts, _) = run(
            "PAM_PROMPT_ECHO_ON Login: \nPAM_ERROR_MSG Bad\nFAILURE\n",
            &["me"],
        );
        assert_eq!(result, Ok(false));
        assert!(prompts[0].echo);
        assert_eq!(
            run("PAM_PROMPT_ECHO_OFF Password: \n", &[]).0,
            Err(Stop::Cancelled)
        );
        assert!(matches!(
            run("PAM_PROMPT_ECHO_OFF Password: \n", &["x"]).0,
            Err(Stop::Failed(_))
        ));
    }

    #[test]
    fn this_user_comes_first() {
        let me = unsafe { libc::getuid() };
        let identities = vec![
            (
                "unix-user".to_owned(),
                HashMap::from([("uid".to_owned(), OwnedValue::from(0u32))]),
            ),
            (
                "unix-user".to_owned(),
                HashMap::from([("uid".to_owned(), OwnedValue::from(me))]),
            ),
        ];
        let users = users(&identities);
        assert_eq!(users.first(), user_name(me).as_ref());
        assert_eq!(users.len(), if me == 0 { 1 } else { 2 });
    }
}
