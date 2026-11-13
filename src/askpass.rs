//! `klemmbrett askpass`: an `SSH_ASKPASS` / `SUDO_ASKPASS` / `GIT_ASKPASS`
//! program that asks the running daemon.
//!
//! The prompt is the first argument. The answer goes to stdout with a
//! newline; the exit status is 1 when the user cancels or no daemon runs.
//! `SSH_ASKPASS_PROMPT=confirm` asks for Enter or Escape instead, and
//! `=none` only shows the message. Neither reads the config: the daemon
//! decides which widget asks, and `KLEMMBRETT_WIDGET` names one.

use crate::secret::{AgentKind, Echo, Mode, SecretReply, SecretRequest};
use crate::varlink::{Client, ClientError};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use zeroize::Zeroizing;

/// The widget `KLEMMBRETT_WIDGET` names, if any.
pub fn widget_from_env() -> Option<String> {
    std::env::var("KLEMMBRETT_WIDGET")
        .ok()
        .filter(|name| !name.is_empty())
}

/// What `prompt` asks for. ssh's host key question wants `yes` typed out,
/// and git asks for user names; those are not secret.
pub fn request(prompt: &str, ssh_prompt: Option<&str>, widget: Option<String>) -> SecretRequest {
    let mut request = SecretRequest::new(AgentKind::Askpass);
    request.widget = widget;
    request.message = prompt.trim_end().to_owned();
    if request.message.is_empty() {
        request.message = "Password:".into();
    }
    let lower = prompt.to_lowercase();
    if lower.contains("(yes/no") || lower.trim_start().starts_with("username") {
        request.echo = Echo::Yes;
    }
    request.mode = match ssh_prompt {
        Some("confirm") => Mode::Confirm,
        Some("none") => Mode::Message,
        _ => Mode::Secret,
    };
    request
}

/// Asks the daemon on `socket`. Cancelling is an answer, not an error.
pub fn ask_daemon(socket: &Path, request: &SecretRequest) -> Result<SecretReply, ClientError> {
    let mode = match request.mode {
        Mode::Secret => "secret",
        Mode::Confirm => "confirm",
        Mode::Message => "message",
        Mode::Choose(_) => {
            return Err(ClientError::Protocol(
                "choices are not asked over Varlink".into(),
            ))
        }
    };
    let echo = match request.echo {
        Echo::No => "no",
        Echo::Masked => "masked",
        Echo::Yes => "yes",
    };
    let mut client = Client::connect(socket)?;
    let parameters = json!({
        "kind": request.kind.name(),
        "widget": request.widget,
        "id": (!request.id.is_empty()).then_some(&request.id),
        "title": (!request.title.is_empty()).then_some(&request.title),
        "message": request.message,
        "prompt": request.prompt,
        "error": request.error,
        "echo": echo,
        "mode": mode,
        "timeout": request.timeout.map(|timeout| timeout.as_secs().max(1)),
    });
    match client.call("AskSecret", parameters) {
        Ok(mut reply) => Ok(match reply["secret"].take() {
            Value::String(secret) => SecretReply::Secret(Zeroizing::new(secret)),
            _ => SecretReply::Confirmed,
        }),
        Err(ClientError::Remote { error, .. }) if error.ends_with(".Cancelled") => {
            Ok(SecretReply::Cancelled)
        }
        Err(err) => Err(err),
    }
}

/// Runs `klemmbrett askpass [prompt]` and returns the exit status.
pub fn run(socket: &Path, prompt: Option<&str>) -> i32 {
    let ssh_prompt = std::env::var("SSH_ASKPASS_PROMPT").ok();
    let request = request(
        prompt.unwrap_or_default(),
        ssh_prompt.as_deref(),
        widget_from_env(),
    );
    match ask_daemon(socket, &request) {
        Ok(SecretReply::Secret(secret)) => {
            let mut line = Zeroizing::new(Vec::with_capacity(secret.len() + 1));
            line.extend_from_slice(secret.as_bytes());
            line.push(b'\n');
            let mut out = std::io::stdout().lock();
            if out.write_all(&line).and_then(|()| out.flush()).is_err() {
                return 1;
            }
            0
        }
        Ok(SecretReply::Confirmed | SecretReply::Chose(_)) => 0,
        Ok(SecretReply::Cancelled) => 1,
        Err(err) => {
            eprintln!("klemmbrett askpass: {err}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_pick_echo_and_mode() {
        let password = request(
            "Enter passphrase for key '/home/me/.ssh/id_ed25519': ",
            None,
            None,
        );
        assert_eq!(
            (password.echo, &password.mode),
            (Echo::Masked, &Mode::Secret)
        );
        assert_eq!(
            password.message,
            "Enter passphrase for key '/home/me/.ssh/id_ed25519':"
        );

        let host = request(
            "Are you sure you want to continue connecting (yes/no/[fingerprint])? ",
            None,
            None,
        );
        assert_eq!(host.echo, Echo::Yes);
        assert_eq!(
            request("Username for 'https://example.org': ", None, None).echo,
            Echo::Yes
        );

        assert_eq!(
            request("Allow use of key?", Some("confirm"), None).mode,
            Mode::Confirm
        );
        assert_eq!(
            request("Touch your key", Some("none"), None).mode,
            Mode::Message
        );
        let named = request("", None, Some("ssh".into()));
        assert_eq!(
            (named.message.as_str(), named.widget.as_deref()),
            ("Password:", Some("ssh"))
        );
    }
}
