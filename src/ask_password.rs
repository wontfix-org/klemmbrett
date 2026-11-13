//! A systemd password agent: answers `systemd-ask-password` in the popup.
//!
//! `systemd-ask-password --user` (and services using it) write an
//! `ask.<random>` file into `$XDG_RUNTIME_DIR/systemd/ask-password`, an
//! INI file with an `[Ask]` section naming a datagram socket. The agent
//! watches the directory, asks the question, and sends `+<password>` (or
//! `-` to cancel) to that socket. A question is dropped when its file goes
//! away, its `NotAfter` deadline passes, or the asking process exits.
//! See <https://systemd.io/PASSWORD_AGENTS/>.

use crate::journal;
use crate::secret::{unescape_c, AgentKind, Echo, SecretAsker, SecretReply, SecretRequest};
use inotify::{Inotify, WatchMask};
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

/// How often deadlines and asking processes are checked.
const CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// `$XDG_RUNTIME_DIR/systemd/ask-password`, where `--user` questions go.
pub fn user_dirs() -> Vec<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|dir| !dir.is_empty())
        .map(|dir| vec![PathBuf::from(dir).join("systemd").join("ask-password")])
        .unwrap_or_default()
}

/// One question, as its `ask.*` file states it.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub socket: PathBuf,
    pub message: String,
    pub id: String,
    pub icon: String,
    pub echo: Echo,
    /// CLOCK_MONOTONIC microseconds; 0 for no deadline.
    pub not_after: u64,
    pub pid: Option<u32>,
}

impl Ask {
    /// Reads the `[Ask]` section. `None` without a `Socket=`.
    pub fn parse(text: &str) -> Option<Self> {
        let mut section = "";
        let mut keys = HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(['#', ';']) {
                continue;
            }
            if let Some(name) = line
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
            {
                section = name;
                continue;
            }
            if section == "Ask" {
                if let Some((key, value)) = line.split_once('=') {
                    keys.insert(key.trim(), value.trim());
                }
            }
        }
        let flag = |key: &str| {
            keys.get(key)
                .is_some_and(|value| matches!(*value, "1" | "yes" | "true" | "on"))
        };
        let echo = if flag("Silent") {
            Echo::No
        } else if flag("Echo") {
            Echo::Yes
        } else {
            Echo::Masked
        };
        Some(Self {
            socket: PathBuf::from(keys.get("Socket").filter(|path| !path.is_empty())?),
            message: unescape_c(keys.get("Message").copied().unwrap_or_default()),
            id: keys.get("Id").copied().unwrap_or_default().to_owned(),
            icon: keys.get("Icon").copied().unwrap_or_default().to_owned(),
            echo,
            not_after: keys
                .get("NotAfter")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            pid: keys.get("PID").and_then(|value| value.parse().ok()),
        })
    }

    pub fn request(&self) -> SecretRequest {
        let mut request = SecretRequest::new(AgentKind::Systemd);
        request.id = self.id.clone();
        request.title = "Password required".to_owned();
        request.message = self.message.clone();
        request.echo = self.echo;
        request.pid = self.pid;
        if !self.icon.is_empty() {
            request.details.push(("icon".to_owned(), self.icon.clone()));
        }
        request
    }

    fn expired(&self, now: u64) -> bool {
        self.not_after != 0 && now > self.not_after
    }
}

/// Watches `dirs` on a background thread for as long as the daemon runs.
pub fn spawn(asker: Arc<dyn SecretAsker>, dirs: Vec<PathBuf>) {
    if dirs.is_empty() {
        journal::warn("XDG_RUNTIME_DIR is not set; systemd password questions are not answered");
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("klemmbrett-ask-password".into())
        .spawn(move || {
            if let Err(err) = watch(asker, &dirs) {
                journal::error(&format!("the systemd password agent stopped: {err}"));
            }
        });
    if let Err(err) = spawned {
        journal::error(&format!(
            "failed to start the systemd password agent: {err}"
        ));
    }
}

/// Answers questions in `dirs` until watching fails.
pub fn watch(asker: Arc<dyn SecretAsker>, dirs: &[PathBuf]) -> Result<(), String> {
    let mut inotify = Inotify::init().map_err(|err| format!("inotify: {err}"))?;
    let mut watched = HashMap::new();
    for dir in dirs {
        if let Err(err) = std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(dir)
        {
            journal::warn(&format!("cannot create {}: {err}", dir.display()));
            continue;
        }
        let mask = WatchMask::CLOSE_WRITE
            | WatchMask::MOVED_TO
            | WatchMask::DELETE
            | WatchMask::MOVED_FROM;
        match inotify.watches().add(dir, mask) {
            Ok(descriptor) => {
                watched.insert(descriptor, dir.clone());
            }
            Err(err) => journal::warn(&format!("cannot watch {}: {err}", dir.display())),
        }
    }
    if watched.is_empty() {
        return Err("no password directory could be watched".into());
    }
    let mut open: HashMap<PathBuf, (u64, Ask)> = HashMap::new();
    for dir in watched.values() {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            arrived(&*asker, &mut open, &entry.path());
        }
    }
    let mut buffer = [0u8; 4096];
    loop {
        if wait_readable(inotify.as_raw_fd(), CHECK_INTERVAL) {
            let events = match inotify.read_events(&mut buffer) {
                Ok(events) => events,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(err) => return Err(format!("inotify: {err}")),
            };
            for event in events {
                let (Some(dir), Some(name)) = (watched.get(&event.wd), event.name) else {
                    continue;
                };
                let path = dir.join(name);
                if event
                    .mask
                    .intersects(inotify::EventMask::DELETE | inotify::EventMask::MOVED_FROM)
                {
                    gone(&*asker, &mut open, &path);
                } else {
                    arrived(&*asker, &mut open, &path);
                }
            }
        }
        let now = monotonic_micros();
        let stale: Vec<PathBuf> = open
            .iter()
            .filter(|(path, (_, ask))| {
                ask.expired(now) || !path.exists() || ask.pid.is_some_and(|pid| !alive(pid))
            })
            .map(|(path, _)| path.clone())
            .collect();
        for path in stale {
            gone(&*asker, &mut open, &path);
        }
    }
}

fn arrived(asker: &dyn SecretAsker, open: &mut HashMap<PathBuf, (u64, Ask)>, path: &Path) {
    let is_ask = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("ask."));
    if !is_ask || open.contains_key(path) {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Some(ask) = Ask::parse(&text) else {
        journal::warn(&format!("ignoring {}: no Socket=", path.display()));
        return;
    };
    if ask.expired(monotonic_micros()) {
        return;
    }
    let ticket = match asker.begin(ask.request()) {
        Ok(ticket) => ticket,
        Err(err) => {
            journal::warn(&format!("cannot ask {:?}: {err}", ask.id));
            return;
        }
    };
    open.insert(path.to_owned(), (ticket.id, ask.clone()));
    let spawned = std::thread::Builder::new()
        .name("klemmbrett-ask-password-reply".into())
        .spawn(move || reply(&ask.socket, ticket.wait()));
    if let Err(err) = spawned {
        journal::error(&format!("failed to wait for a password: {err}"));
    }
}

fn gone(asker: &dyn SecretAsker, open: &mut HashMap<PathBuf, (u64, Ask)>, path: &Path) {
    if let Some((id, _)) = open.remove(path) {
        asker.withdraw(id);
    }
}

/// Sends the answer to the asker's socket: `+password`, or `-` to cancel.
pub fn reply(socket: &Path, answer: SecretReply) {
    let message = match answer {
        SecretReply::Secret(secret) => {
            let mut message = Zeroizing::new(Vec::with_capacity(secret.len() + 1));
            message.push(b'+');
            message.extend_from_slice(secret.as_bytes());
            message
        }
        SecretReply::Confirmed | SecretReply::Chose(_) => Zeroizing::new(b"+".to_vec()),
        SecretReply::Cancelled => Zeroizing::new(b"-".to_vec()),
    };
    let sent = UnixDatagram::unbound().and_then(|out| out.send_to(&message, socket));
    if let Err(err) = sent {
        journal::warn(&format!("cannot answer {}: {err}", socket.display()));
    }
}

fn wait_readable(fd: i32, timeout: Duration) -> bool {
    let mut poll = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd.
    let ready = unsafe { libc::poll(&mut poll, 1, timeout.as_millis() as libc::c_int) };
    ready > 0 && poll.revents & libc::POLLIN != 0
}

fn monotonic_micros() -> u64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime fills the timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    now.tv_sec as u64 * 1_000_000 + now.tv_nsec as u64 / 1_000
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    let status = unsafe { libc::kill(pid as libc::pid_t, 0) };
    status == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_files_are_read_like_systemd_writes_them() {
        let ask = Ask::parse(
            "[Ask]\nPID=4242\nSocket=/run/user/1000/systemd/ask-password/sck.1\n\
             AcceptCached=0\nEcho=0\nNotAfter=123456\nMessage=Key for \\\"backup\\\"\n\
             Icon=drive-harddisk\nId=backup:key\n",
        )
        .unwrap();
        assert_eq!(
            ask.socket,
            PathBuf::from("/run/user/1000/systemd/ask-password/sck.1")
        );
        assert_eq!(ask.message, "Key for \"backup\"");
        assert_eq!(
            (ask.pid, ask.not_after, ask.echo),
            (Some(4242), 123456, Echo::Masked)
        );
        assert!(ask.expired(123457) && !ask.expired(123456));
        let request = ask.request();
        assert_eq!(request.kind, AgentKind::Systemd);
        assert_eq!(request.detail("icon"), Some("drive-harddisk"));

        let echo = Ask::parse("[Ask]\nSocket=/s\nEcho=1\n").unwrap();
        assert_eq!((echo.echo, echo.not_after, echo.pid), (Echo::Yes, 0, None));
        assert!(!echo.expired(u64::MAX));
        assert_eq!(
            Ask::parse("[Ask]\nSocket=/s\nSilent=1\n").unwrap().echo,
            Echo::No
        );
        assert!(Ask::parse("[Ask]\nMessage=no socket\n").is_none());
        assert!(Ask::parse("[Other]\nSocket=/s\n").is_none());
    }

    #[test]
    fn this_process_is_alive() {
        assert!(alive(std::process::id()));
        assert!(monotonic_micros() > 0);
    }
}
