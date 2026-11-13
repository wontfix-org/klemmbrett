//! `klemmbrett pinentry`: a pinentry for gpg-agent that asks the daemon.
//!
//! gpg-agent talks the Assuan protocol on stdin and stdout: it sets the
//! texts with `SET*` commands, then sends `GETPIN`, `CONFIRM`, or `MESSAGE`.
//! See <https://www.gnupg.org/documentation/manuals/assuan/>.

use crate::askpass::{ask_daemon, widget_from_env};
use crate::secret::{AgentKind, Echo, Mode, SecretReply, SecretRequest};
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::time::Duration;
use zeroize::Zeroizing;

const CANCELLED: &str = "ERR 83886179 Operation cancelled <Pinentry>";
const NOT_CONFIRMED: &str = "ERR 83886194 Not confirmed <Pinentry>";
const UNKNOWN_COMMAND: &str = "ERR 536871187 Unknown IPC command <User defined source 1>";
/// Assuan lines are at most 1000 bytes; data lines leave room for `D `
/// and escapes.
const DATA_CHUNK: usize = 300;

/// What `SET*` commands configured so far.
#[derive(Debug, Default, Clone)]
struct State {
    title: String,
    description: String,
    prompt: String,
    error: String,
    key: String,
    repeat: Option<String>,
    repeat_error: String,
    timeout: Option<Duration>,
}

impl State {
    fn request(&self, mode: Mode, widget: Option<String>) -> SecretRequest {
        let mut request = SecretRequest::new(AgentKind::Pinentry);
        request.widget = widget;
        request.id = self.key.clone();
        request.title = self.title.clone();
        request.message = self.description.clone();
        request.prompt = (!self.prompt.is_empty()).then(|| self.prompt.clone());
        request.error = (!self.error.is_empty()).then(|| self.error.clone());
        request.echo = Echo::Masked;
        request.mode = mode;
        request.timeout = self.timeout;
        request
    }
}

/// Answers Assuan commands from `reader` until `BYE` or end of input.
/// `ask` puts each question on screen; an `Err` is reported to the client
/// as a cancellation.
pub fn serve(
    reader: &mut dyn BufRead,
    writer: &mut dyn Write,
    widget: Option<String>,
    ask: &mut dyn FnMut(SecretRequest) -> Result<SecretReply, String>,
) -> io::Result<()> {
    writeln!(writer, "OK Pleased to meet you")?;
    writer.flush()?;
    let mut state = State::default();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let line = line.trim_end_matches(['\n', '\r']);
        let (command, argument) = line.split_once(' ').unwrap_or((line, ""));
        let argument = unescape(argument);
        match command.to_ascii_uppercase().as_str() {
            "SETTITLE" => state.title = argument,
            "SETDESC" => state.description = argument,
            "SETPROMPT" => state.prompt = argument,
            "SETERROR" => state.error = argument,
            "SETKEYINFO" => state.key = argument,
            "SETREPEAT" => {
                state.repeat = Some(if argument.is_empty() {
                    "Repeat:".into()
                } else {
                    argument
                })
            }
            "SETREPEATERROR" => state.repeat_error = argument,
            "SETTIMEOUT" => {
                state.timeout = argument
                    .trim()
                    .parse()
                    .ok()
                    .filter(|seconds| *seconds > 0)
                    .map(Duration::from_secs)
            }
            "GETINFO" => {
                let value = match argument.trim() {
                    "flavor" => Some("klemmbrett".to_owned()),
                    "version" => Some(env!("CARGO_PKG_VERSION").to_owned()),
                    "pid" => Some(std::process::id().to_string()),
                    _ => None,
                };
                if let Some(value) = value {
                    writeln!(writer, "D {}", escape(&value))?;
                }
            }
            "GETPIN" => {
                let result = getpin(&mut state, &widget, ask);
                state.error.clear();
                match result {
                    Some((pin, repeated)) => {
                        if repeated {
                            writeln!(writer, "S PIN_REPEATED")?;
                        }
                        let escaped = Zeroizing::new(escape(&pin));
                        for chunk in chunks(&escaped) {
                            let mut data = Zeroizing::new(Vec::with_capacity(chunk.len() + 3));
                            data.extend_from_slice(b"D ");
                            data.extend_from_slice(chunk.as_bytes());
                            data.push(b'\n');
                            writer.write_all(&data)?;
                        }
                    }
                    None => {
                        reply(writer, CANCELLED)?;
                        continue;
                    }
                }
            }
            "CONFIRM" | "MESSAGE" => {
                let mode = if command.eq_ignore_ascii_case("MESSAGE")
                    || argument.contains("--one-button")
                {
                    Mode::Message
                } else {
                    Mode::Confirm
                };
                let message = mode == Mode::Message;
                let answer = ask(state.request(mode, widget.clone()));
                state.error.clear();
                match answer {
                    Ok(SecretReply::Confirmed | SecretReply::Chose(_)) => {}
                    _ if message => {}
                    Ok(_) => {
                        reply(writer, NOT_CONFIRMED)?;
                        continue;
                    }
                    Err(_) => {
                        reply(writer, CANCELLED)?;
                        continue;
                    }
                }
            }
            "RESET" => state = State::default(),
            "BYE" => {
                reply(writer, "OK closing connection")?;
                return Ok(());
            }
            command if command.starts_with("SET") => {}
            "OPTION" | "NOP" => {}
            _ => {
                reply(writer, UNKNOWN_COMMAND)?;
                continue;
            }
        }
        reply(writer, "OK")?;
    }
}

/// Asks for the PIN, twice with `SETREPEAT`. `None` when cancelled.
fn getpin(
    state: &mut State,
    widget: &Option<String>,
    ask: &mut dyn FnMut(SecretRequest) -> Result<SecretReply, String>,
) -> Option<(Zeroizing<String>, bool)> {
    loop {
        let SecretReply::Secret(pin) = ask(state.request(Mode::Secret, widget.clone())).ok()?
        else {
            return None;
        };
        let Some(repeat) = &state.repeat else {
            return Some((pin, false));
        };
        let mut again = state.clone();
        again.prompt = repeat.clone();
        again.error.clear();
        let SecretReply::Secret(second) = ask(again.request(Mode::Secret, widget.clone())).ok()?
        else {
            return None;
        };
        if *second == *pin {
            return Some((pin, true));
        }
        state.error = if state.repeat_error.is_empty() {
            "The entries do not match".into()
        } else {
            state.repeat_error.clone()
        };
    }
}

fn reply(writer: &mut dyn Write, line: &str) -> io::Result<()> {
    writeln!(writer, "{line}")?;
    writer.flush()
}

/// `%XX` escapes in arguments, as gpg-agent sends newlines in descriptions.
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if let (b'%', Some(&[high, low])) = (bytes[index], bytes.get(index + 1..index + 3)) {
            if let (Some(high), Some(low)) =
                ((high as char).to_digit(16), (low as char).to_digit(16))
            {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            ch => out.push(ch),
        }
    }
    out
}

/// Splits at character boundaries without cutting a `%XX` escape.
fn chunks(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while rest.len() > DATA_CHUNK {
        let mut cut = DATA_CHUNK;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        if let Some(percent) = rest.as_bytes()[cut - 2..cut]
            .iter()
            .rposition(|byte| *byte == b'%')
        {
            cut = cut - 2 + percent;
        }
        out.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    out.push(rest);
    out
}

/// Runs `klemmbrett pinentry` on stdin and stdout; gpg-agent's options
/// (`--display`, `--ttyname`, ...) are ignored.
pub fn run(socket: &Path) -> i32 {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut writer = io::stdout().lock();
    let mut ask =
        |request: SecretRequest| ask_daemon(socket, &request).map_err(|err| err.to_string());
    match serve(&mut reader, &mut writer, widget_from_env(), &mut ask) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("klemmbrett pinentry: {err}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(input: &str, answers: Vec<SecretReply>) -> (String, Vec<SecretRequest>) {
        let mut answers = answers.into_iter();
        let mut asked = Vec::new();
        let mut output = Vec::new();
        let mut ask = |request: SecretRequest| {
            asked.push(request);
            answers.next().ok_or_else(|| "no daemon".to_owned())
        };
        serve(
            &mut input.as_bytes(),
            &mut output,
            Some("gpg".into()),
            &mut ask,
        )
        .unwrap();
        (String::from_utf8(output).unwrap(), asked)
    }

    fn secret(text: &str) -> SecretReply {
        SecretReply::Secret(Zeroizing::new(text.into()))
    }

    #[test]
    fn getpin_answers_like_gpg_agent_expects() {
        let (output, asked) = transcript(
            "OPTION ttyname=/dev/pts/1\nSETDESC Please enter the passphrase%0Afor key ABC\n\
             SETPROMPT Passphrase:\nSETTIMEOUT 30\nGETPIN\nBYE\n",
            vec![secret("100% sec\nret")],
        );
        assert_eq!(
            output,
            "OK Pleased to meet you\nOK\nOK\nOK\nOK\nD 100%25 sec%0Aret\nOK\nOK closing connection\n"
        );
        let request = &asked[0];
        assert_eq!(request.message, "Please enter the passphrase\nfor key ABC");
        assert_eq!(request.prompt.as_deref(), Some("Passphrase:"));
        assert_eq!(request.widget.as_deref(), Some("gpg"));
        assert_eq!(request.timeout, Some(Duration::from_secs(30)));
        assert_eq!(request.kind, AgentKind::Pinentry);
    }

    #[test]
    fn cancelling_and_confirming() {
        let (output, _) = transcript(
            "GETPIN\nCONFIRM\nCONFIRM\nMESSAGE\nFROB\n",
            vec![
                SecretReply::Cancelled,
                SecretReply::Confirmed,
                SecretReply::Cancelled,
                SecretReply::Cancelled,
            ],
        );
        let lines: Vec<_> = output.lines().collect();
        assert_eq!(lines[1], CANCELLED);
        assert_eq!(lines[2], "OK");
        assert_eq!(lines[3], NOT_CONFIRMED);
        assert_eq!(lines[4], "OK");
        assert_eq!(lines[5], UNKNOWN_COMMAND);
        let (output, _) = transcript("GETPIN\n", vec![]);
        assert!(output.ends_with(&format!("{CANCELLED}\n")));
    }

    #[test]
    fn repeat_asks_until_both_entries_match() {
        let (output, asked) = transcript(
            "SETREPEAT Repeat:\nSETERROR Bad passphrase\nGETPIN\n",
            vec![secret("a"), secret("b"), secret("c"), secret("c")],
        );
        assert!(output.ends_with("OK\nOK\nS PIN_REPEATED\nD c\nOK\n"));
        assert_eq!(asked.len(), 4);
        assert_eq!(asked[0].error.as_deref(), Some("Bad passphrase"));
        assert_eq!(asked[1].prompt.as_deref(), Some("Repeat:"));
        assert_eq!(asked[2].error.as_deref(), Some("The entries do not match"));
    }

    #[test]
    fn escapes_round_trip_and_long_data_is_split() {
        assert_eq!(unescape("a%0Ab%25c%"), "a\nb%c%");
        assert_eq!(unescape("%zz%4"), "%zz%4");
        let long = "%25".repeat(200);
        let parts = chunks(&long);
        assert!(parts.len() > 1);
        assert!(parts
            .iter()
            .all(|part| part.len() <= DATA_CHUNK && !part.ends_with('%')));
        assert_eq!(parts.concat(), long);
    }
}
