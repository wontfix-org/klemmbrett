//! Command line: the daemon, `show`, `prompt`, `watch`, `stash`, and
//! `--install-systemd`. Everything but the daemon talks Varlink to it.

use crate::varlink::{records_json, socket_path, Client, ClientError};
use crate::widget::{parse_records, validate_records, FileFormat, Record};
use clap::{Args as ClapArgs, Parser, Subcommand};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(
    name = "klemmbrett",
    about = "Keep Wayland selections in sync and copy values from configurable popup widgets."
)]
struct Args {
    /// Python configuration file (defaults to $XDG_CONFIG_HOME/klemmbrett/config.py)
    #[arg(long)]
    config: Option<PathBuf>,

    /// Install systemd user units bound to graphical-session.target and enable them
    #[arg(long)]
    install_systemd: bool,

    /// Varlink socket (defaults to $KLEMMBRETT_SOCKET, then $XDG_RUNTIME_DIR/klemmbrett/varlink.sock)
    #[arg(long, global = true, value_name = "PATH")]
    socket: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Sub>,
}

#[derive(Subcommand, Debug)]
enum Sub {
    /// Ask the running daemon to show a popup widget
    Show(PopupArgs),
    /// Ask the running daemon to prompt and print the selected values
    Prompt(PopupArgs),
    /// Print daemon events as JSON lines: changed widgets, selections, notifications
    Watch(WatchArgs),
    /// List the notifications in a stash as JSON lines, or mark them read
    Stash(StashArgs),
}

#[derive(ClapArgs, Debug, Clone)]
pub struct PopupArgs {
    /// Configured widget name
    widget: String,
    /// Record file, or "-" for stdin. Omit to use the widget provider.
    #[arg(value_name = "INPUT", allow_hyphen_values = true)]
    input: Option<String>,
    /// Pointer X in compositor global coordinates. Requires -y.
    #[arg(short = 'x', allow_hyphen_values = true)]
    x: Option<i32>,
    /// Pointer Y in compositor global coordinates. Requires -x.
    #[arg(short = 'y', allow_hyphen_values = true)]
    y: Option<i32>,
    /// Record format for INPUT: text, json, or yaml. Defaults to text.
    #[arg(long, value_name = "FORMAT")]
    format: Option<String>,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct WatchArgs {
    /// Only events about these widgets (notifications are not widget events)
    widgets: Vec<String>,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct StashArgs {
    /// Stash name, as given to notifications.Stash
    name: String,
    /// Only unread notifications
    #[arg(long)]
    unread: bool,
    /// Mark these keys read instead of listing
    #[arg(long = "mark-read", value_name = "KEY", num_args = 1.., conflicts_with = "all")]
    mark_read: Vec<String>,
    /// Mark every notification in the stash read
    #[arg(long)]
    all: bool,
}

/// What the command line asked for.
#[derive(Debug)]
pub enum Invocation {
    Daemon { config: Option<PathBuf> },
    InstallSystemd { config: Option<PathBuf> },
    Show(PopupArgs),
    Prompt(PopupArgs),
    Watch(WatchArgs),
    Stash(StashArgs),
}

/// A parsed command line: what to do and which socket to use.
#[derive(Debug)]
pub struct Parsed {
    pub invocation: Invocation,
    pub socket: Option<PathBuf>,
}

/// Parses `argv` (including the program name). Help and version requests
/// come back as an `Err` whose `exit_code` is 0.
pub fn parse(argv: &[String]) -> Result<Parsed, clap::Error> {
    let args = Args::try_parse_from(argv)?;
    let invocation = match args.command {
        Some(Sub::Show(popup)) => Invocation::Show(popup),
        Some(Sub::Prompt(popup)) => Invocation::Prompt(popup),
        Some(Sub::Watch(watch)) => Invocation::Watch(watch),
        Some(Sub::Stash(stash)) => Invocation::Stash(stash),
        None if args.install_systemd => Invocation::InstallSystemd {
            config: args.config,
        },
        None => Invocation::Daemon {
            config: args.config,
        },
    };
    Ok(Parsed {
        invocation,
        socket: args.socket,
    })
}

fn connect(socket: Option<&Path>) -> Result<Client, ClientError> {
    Client::connect(&socket_path(socket))
}

/// Runs `show` or `prompt` against the daemon and returns the exit code.
pub fn run_popup(args: &PopupArgs, prompt: bool, socket: Option<&Path>) -> i32 {
    let command = if prompt { "prompt" } else { "show" };
    let pointer = match (args.x, args.y) {
        (None, None) => Value::Null,
        (Some(x), Some(y)) => json!({ "x": x, "y": y }),
        _ => {
            eprintln!("{command}: -x and -y must both be given, or both omitted");
            return 1;
        }
    };
    let records = match load_records(&args.widget, args.format.as_deref(), args.input.as_deref()) {
        Ok(records) if records.is_empty() => Value::Null,
        Ok(records) => records_json(&records),
        Err(err) => {
            eprintln!("{command}: {err}");
            return 1;
        }
    };
    let parameters = json!({ "widget": args.widget, "pointer": pointer, "records": records });
    let method = if prompt { "Prompt" } else { "Show" };
    let reply = connect(socket).and_then(|mut client| client.call(method, parameters));
    match reply {
        Err(err) => {
            eprintln!("failed to {command} {:?}: {err}", args.widget);
            1
        }
        Ok(_) if !prompt => 0,
        Ok(reply) => {
            let values: Vec<&str> = reply["values"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            for value in &values {
                println!("{value}");
            }
            i32::from(values.is_empty())
        }
    }
}

/// Prints every event as one JSON line until the daemon goes away.
pub fn run_watch(args: &WatchArgs, socket: Option<&Path>) -> i32 {
    let widgets = if args.widgets.is_empty() {
        Value::Null
    } else {
        json!(args.widgets)
    };
    let stdout = std::io::stdout();
    let result = connect(socket).and_then(|mut client| {
        client.stream("Subscribe", json!({ "widgets": widgets }), |reply| {
            let event = &reply["event"];
            if event["kind"] == "ready" {
                return true;
            }
            let mut out = stdout.lock();
            writeln!(out, "{event}").and_then(|()| out.flush()).is_ok()
        })
    });
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("watch: {err}");
            1
        }
    }
}

/// Lists a stash as JSON lines, or marks notifications read.
pub fn run_stash(args: &StashArgs, socket: Option<&Path>) -> i32 {
    let result = connect(socket).and_then(|mut client| {
        if args.all || !args.mark_read.is_empty() {
            let keys = if args.all {
                Value::Null
            } else {
                json!(args.mark_read)
            };
            let reply =
                client.call("StashMarkRead", json!({ "stash": args.name, "keys": keys }))?;
            println!("{}", reply["marked"]);
            return Ok(());
        }
        let reply = client.call(
            "StashList",
            json!({ "stash": args.name, "unread_only": args.unread }),
        )?;
        for record in reply["records"].as_array().into_iter().flatten() {
            println!("{record}");
        }
        Ok(())
    });
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("stash: {err}");
            1
        }
    }
}

fn load_records(
    widget: &str,
    format: Option<&str>,
    input: Option<&str>,
) -> Result<Vec<Record>, String> {
    let Some(input) = input else {
        if format.is_some() {
            return Err("--format requires an input path or -".into());
        }
        return Ok(Vec::new());
    };
    let format = FileFormat::parse(format.unwrap_or("text"))?;
    let text = if input == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|err| format!("failed to read stdin: {err}"))?;
        text
    } else {
        std::fs::read_to_string(input).map_err(|err| format!("failed to read {input}: {err}"))?
    };
    let records = parse_records(&text, format)?;
    if records.is_empty() {
        return Err("input produced no records".into());
    }
    validate_records(widget, &records)?;
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    fn invocation(items: &[&str]) -> Invocation {
        parse(&argv(items)).unwrap().invocation
    }

    #[test]
    fn no_subcommand_starts_the_daemon() {
        match invocation(&["klemmbrett"]) {
            Invocation::Daemon { config } => assert!(config.is_none()),
            other => panic!("expected daemon, got {other:?}"),
        }
        match invocation(&["klemmbrett", "--config", "/c.py"]) {
            Invocation::Daemon { config } => {
                assert_eq!(config.as_deref(), Some(std::path::Path::new("/c.py")))
            }
            other => panic!("expected daemon, got {other:?}"),
        }
    }

    #[test]
    fn install_systemd_still_parses_without_a_subcommand() {
        assert!(matches!(
            invocation(&["klemmbrett", "--install-systemd"]),
            Invocation::InstallSystemd { config: None }
        ));
    }

    #[test]
    fn socket_is_global() {
        let parsed = parse(&argv(&["klemmbrett", "show", "run", "--socket", "/s"])).unwrap();
        assert_eq!(parsed.socket.as_deref(), Some(Path::new("/s")));
        let parsed = parse(&argv(&["klemmbrett", "--socket", "/d"])).unwrap();
        assert_eq!(parsed.socket.as_deref(), Some(Path::new("/d")));
    }

    #[test]
    fn show_accepts_pointer_coordinates_and_input() {
        match invocation(&["klemmbrett", "show", "run", "-x", "120", "-y", "-40"]) {
            Invocation::Show(popup) => {
                assert_eq!(
                    (popup.widget.as_str(), popup.x, popup.y, popup.input),
                    ("run", Some(120), Some(-40), None)
                );
            }
            other => panic!("expected show, got {other:?}"),
        }
        match invocation(&["klemmbrett", "prompt", "pick", "-", "--format", "json"]) {
            Invocation::Prompt(popup) => {
                assert_eq!(popup.input.as_deref(), Some("-"));
                assert_eq!(popup.format.as_deref(), Some("json"));
            }
            other => panic!("expected prompt, got {other:?}"),
        }
    }

    #[test]
    fn watch_and_stash_parse() {
        match invocation(&["klemmbrett", "watch", "inbox", "history"]) {
            Invocation::Watch(watch) => assert_eq!(watch.widgets, ["inbox", "history"]),
            other => panic!("expected watch, got {other:?}"),
        }
        match invocation(&["klemmbrett", "stash", "inbox", "--mark-read", "3", "4"]) {
            Invocation::Stash(stash) => {
                assert_eq!(stash.name, "inbox");
                assert_eq!(stash.mark_read, ["3", "4"]);
            }
            other => panic!("expected stash, got {other:?}"),
        }
        assert!(parse(&argv(&[
            "klemmbrett",
            "stash",
            "inbox",
            "--all",
            "--mark-read",
            "1"
        ]))
        .is_err());
    }

    #[test]
    fn positional_coordinates_and_long_flags_are_rejected() {
        assert!(parse(&argv(&["klemmbrett", "show", "run", "120", "-40"])).is_err());
        assert!(parse(&argv(&[
            "klemmbrett",
            "show",
            "run",
            "--x",
            "120",
            "--y",
            "80"
        ]))
        .is_err());
    }

    #[test]
    fn help_is_an_error_with_exit_code_zero() {
        let error = parse(&argv(&["klemmbrett", "--help"])).unwrap_err();
        assert_eq!(error.exit_code(), 0);
    }

    #[test]
    fn a_missing_daemon_is_reported_plainly() {
        let dir = tempfile::tempdir().unwrap();
        let error = connect(Some(&dir.path().join("none.sock"))).err().unwrap();
        assert!(
            error.to_string().contains("klemmbrett is not running"),
            "{error}"
        );
    }

    #[test]
    fn load_records_uses_the_file_provider_formats() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("items.json");
        std::fs::write(
            &path,
            r#"["plain", {"label":"Readable", "value":"copied", "n": 1}]"#,
        )
        .unwrap();
        let path = path.to_str().unwrap();
        assert_eq!(
            load_records("items", Some("json"), Some(path)).unwrap(),
            vec![
                Record::new("plain"),
                Record {
                    key: "copied".into(),
                    label: "Readable".into(),
                    value: "copied".into(),
                    data: json!({"n": 1}),
                },
            ]
        );
        let empty = dir.path().join("empty.txt");
        std::fs::write(&empty, "\n").unwrap();
        let error = load_records("items", None, Some(empty.to_str().unwrap())).unwrap_err();
        assert!(error.contains("no records"), "{error}");
        let error = load_records("items", Some("yaml"), None).unwrap_err();
        assert!(error.contains("--format requires"), "{error}");
        assert!(load_records("items", None, None).unwrap().is_empty());
    }
}
