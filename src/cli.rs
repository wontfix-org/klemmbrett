//! Command line: the picker, `run`, `install-systemd`, `show`, `prompt`,
//! `watch`, and `stash`. Everything but `run` talks Varlink to the daemon.

use crate::varlink::{records_json, socket_path, Client, ClientError};
use crate::widget::{parse_records, validate_records, FileFormat, Record};
use clap::{error::ErrorKind, Args as ClapArgs, CommandFactory, Parser, Subcommand};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(
    name = "kb",
    about = "Pick from a list, and keep Wayland selections in sync.",
    after_help = "With no subcommand, kb is a picker. Choices come from stdin when it is\n\
not a terminal, or from a file. Each picked value is printed. Cancelling exits 1;\n\
a problem reading the choices or reaching the daemon exits 2.\n\n\
  printf '%s\\n' one two | kb\n\
  kb -m --query tw choices.txt\n\n\
The popup is kb.picker() in the configuration (layout, matching, multi-select).\n\
It does not use a named widget. Left unconfigured, it is a centered filter with\n\
fuzzy matching.",
    subcommand_precedence_over_arg = true
)]
struct Args {
    /// Varlink socket (defaults to $KLEMMBRETT_SOCKET, then $XDG_RUNTIME_DIR/klemmbrett/varlink.sock)
    #[arg(long, global = true, value_name = "PATH")]
    socket: Option<PathBuf>,

    /// Mark several rows (Tab / Shift-Tab). Overrides the configured picker.
    #[arg(short = 'm', long)]
    multi: bool,

    /// Pick one row even when the configured picker allows several.
    #[arg(long, conflicts_with = "multi")]
    no_multi: bool,

    /// Text the filter starts with.
    #[arg(short = 'q', long, value_name = "TEXT")]
    query: Option<String>,

    /// Label in front of the filter. Overrides the configured picker.
    #[arg(long, value_name = "TEXT")]
    prompt: Option<String>,

    /// Choices are separated by NUL instead of newlines. Text input only.
    #[arg(short = '0', long = "read0")]
    read0: bool,

    /// Pointer X in compositor global coordinates. Requires -y.
    #[arg(short = 'x', allow_hyphen_values = true)]
    x: Option<i32>,

    /// Pointer Y in compositor global coordinates. Requires -x.
    #[arg(short = 'y', allow_hyphen_values = true)]
    y: Option<i32>,

    /// Record format: text, json, or yaml. Defaults to text.
    #[arg(long, value_name = "FORMAT")]
    format: Option<String>,

    /// Choices file, or "-" for stdin. Omit to read stdin when it is not a terminal.
    #[arg(value_name = "INPUT")]
    input: Option<String>,

    #[command(subcommand)]
    command: Option<Sub>,
}

#[derive(Subcommand, Debug)]
enum Sub {
    /// Run the daemon
    Run {
        /// Python configuration file (defaults to $XDG_CONFIG_HOME/klemmbrett/config.py)
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Ask the running daemon to show a popup widget
    Show(PopupArgs),
    /// Ask the running daemon to prompt and print the selected values
    Prompt(PopupArgs),
    /// Print daemon events as JSON lines: changed widgets, selections, notifications
    Watch(WatchArgs),
    /// List the notifications in a stash as JSON lines, or mark them read
    Stash(StashArgs),
    /// Ask for a password in the popup, for SSH_ASKPASS, SUDO_ASKPASS, and GIT_ASKPASS
    Askpass(AskpassArgs),
    /// A pinentry for gpg-agent (pinentry-program) that asks in the popup
    Pinentry(PinentryArgs),
    /// Install systemd user units bound to graphical-session.target and enable them
    InstallSystemd {
        /// Configuration file to pin in ExecStart
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(ClapArgs, Debug, Clone)]
pub struct AskpassArgs {
    /// The question, as ssh, sudo, or git pass it
    #[arg(allow_hyphen_values = true)]
    pub prompt: Option<String>,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct PinentryArgs {
    /// gpg-agent's options (--display, --ttyname, ...), which are ignored
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    pub ignored: Vec<String>,
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
    /// The ad-hoc picker: no widget name, choices from stdin or a file.
    Pick(PickArgs),
    Daemon {
        config: Option<PathBuf>,
    },
    InstallSystemd {
        config: Option<PathBuf>,
    },
    Show(PopupArgs),
    Prompt(PopupArgs),
    Watch(WatchArgs),
    Stash(StashArgs),
    Askpass(AskpassArgs),
    Pinentry(PinentryArgs),
}

/// One ad-hoc pick. `multi` left out keeps the configured picker.
#[derive(Debug, Clone)]
pub struct PickArgs {
    pub multi: Option<bool>,
    pub query: Option<String>,
    pub prompt: Option<String>,
    pub read0: bool,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub format: Option<String>,
    pub input: Option<String>,
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
    if args.command.is_some() && picker_options_set(&args) {
        let mut command = Args::command();
        return Err(command.error(
            ErrorKind::ArgumentConflict,
            "picker options are only for `kb` with no subcommand",
        ));
    }
    let invocation = match args.command {
        Some(Sub::Run { config }) => Invocation::Daemon { config },
        Some(Sub::Show(popup)) => Invocation::Show(popup),
        Some(Sub::Prompt(popup)) => Invocation::Prompt(popup),
        Some(Sub::Watch(watch)) => Invocation::Watch(watch),
        Some(Sub::Stash(stash)) => Invocation::Stash(stash),
        Some(Sub::Askpass(askpass)) => Invocation::Askpass(askpass),
        Some(Sub::Pinentry(pinentry)) => Invocation::Pinentry(pinentry),
        Some(Sub::InstallSystemd { config }) => Invocation::InstallSystemd { config },
        None => Invocation::Pick(PickArgs {
            multi: match (args.multi, args.no_multi) {
                (true, _) => Some(true),
                (false, true) => Some(false),
                (false, false) => None,
            },
            query: args.query,
            prompt: args.prompt,
            read0: args.read0,
            x: args.x,
            y: args.y,
            format: args.format,
            input: args.input,
        }),
    };
    Ok(Parsed {
        invocation,
        socket: args.socket,
    })
}

fn picker_options_set(args: &Args) -> bool {
    args.multi
        || args.no_multi
        || args.query.is_some()
        || args.prompt.is_some()
        || args.read0
        || args.x.is_some()
        || args.y.is_some()
        || args.format.is_some()
        || args.input.is_some()
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
    let records = match load_records(
        &args.widget,
        args.format.as_deref(),
        args.input.as_deref(),
        false,
        false,
        false,
    ) {
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

/// Opens the configured picker and prints each picked value.
///
/// Exit 0 when something was picked, 1 when the popup was cancelled or
/// nothing was picked, 2 when the choices or the daemon could not be used.
pub fn run_pick(args: &PickArgs, socket: Option<&Path>) -> i32 {
    let pointer = match (args.x, args.y) {
        (None, None) => Value::Null,
        (Some(x), Some(y)) => json!({ "x": x, "y": y }),
        _ => {
            eprintln!("kb: -x and -y must both be given, or both omitted");
            return 2;
        }
    };
    let input = match pick_input(args.input.as_deref(), std::io::stdin().is_terminal()) {
        Ok(input) => input,
        Err(err) => {
            eprintln!("kb: {err}");
            return 2;
        }
    };
    let records = match load_records(
        "picker",
        args.format.as_deref(),
        Some(input),
        args.read0,
        true,
        true,
    ) {
        Ok(records) => records,
        Err(err) => {
            eprintln!("kb: {err}");
            return 2;
        }
    };
    let parameters = json!({
        "records": records_json(&records),
        "pointer": pointer,
        "query": args.query,
        "multi": args.multi,
        "prompt": args.prompt,
    });
    match connect(socket).and_then(|mut client| client.call("Pick", parameters)) {
        Err(err) => {
            eprintln!("kb: {err}");
            2
        }
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

/// The path to read, or `"-"` for stdin. A missing path reads stdin only
/// when it is not a terminal, so a bare `kb` in a shell does not wait.
pub fn pick_input(input: Option<&str>, stdin_is_terminal: bool) -> Result<&str, String> {
    match input {
        Some(input) => Ok(input),
        None if stdin_is_terminal => {
            Err("no choices on stdin; pipe a list, or pass a file: kb choices.txt".into())
        }
        None => Ok("-"),
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

/// Runs `askpass`: prints the answer and exits 0, or exits 1.
pub fn run_askpass(args: &AskpassArgs, socket: Option<&Path>) -> i32 {
    crate::askpass::run(&socket_path(socket), args.prompt.as_deref())
}

/// Runs `pinentry` on stdin and stdout.
pub fn run_pinentry(socket: Option<&Path>) -> i32 {
    crate::pinentry::run(&socket_path(socket))
}

fn load_records(
    widget: &str,
    format: Option<&str>,
    input: Option<&str>,
    nul: bool,
    allow_empty: bool,
    disambiguate: bool,
) -> Result<Vec<Record>, String> {
    let Some(input) = input else {
        if format.is_some() || nul {
            return Err("--format requires an input path or -".into());
        }
        return Ok(Vec::new());
    };
    let format = FileFormat::parse(format.unwrap_or("text"))?;
    if nul && format != FileFormat::Text {
        return Err("--read0 applies to text input only".into());
    }
    let mut text = if input == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|err| format!("failed to read stdin: {err}"))?;
        text
    } else {
        std::fs::read_to_string(input).map_err(|err| format!("failed to read {input}: {err}"))?
    };
    if nul {
        text = text.replace('\0', "\n");
    }
    let mut records = parse_records(&text, format)?;
    if records.is_empty() && !allow_empty {
        return Err("input produced no records".into());
    }
    if disambiguate {
        uniquify_keys(&mut records);
    }
    validate_records(widget, &records)?;
    Ok(records)
}

/// Later copies of a key become `key#2`, `key#3`, and so on. The value,
/// which is what the picker prints, stays as it was read.
fn uniquify_keys(records: &mut [Record]) {
    let mut seen = HashSet::new();
    for record in records {
        if seen.insert(record.key.clone()) {
            continue;
        }
        let mut n = 2;
        loop {
            let key = format!("{}#{n}", record.key);
            if seen.insert(key.clone()) {
                record.key = key;
                break;
            }
            n += 1;
        }
    }
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
    fn run_starts_the_daemon() {
        match invocation(&["kb", "run"]) {
            Invocation::Daemon { config } => assert!(config.is_none()),
            other => panic!("expected daemon, got {other:?}"),
        }
        match invocation(&["kb", "run", "--config", "/c.py"]) {
            Invocation::Daemon { config } => {
                assert_eq!(config.as_deref(), Some(std::path::Path::new("/c.py")))
            }
            other => panic!("expected daemon, got {other:?}"),
        }
        assert!(parse(&argv(&["kb", "--config", "/c.py"])).is_err());
    }

    #[test]
    fn no_subcommand_is_the_picker() {
        match invocation(&["kb"]) {
            Invocation::Pick(pick) => {
                assert_eq!(
                    (
                        pick.input.as_deref(),
                        pick.multi,
                        pick.query.as_deref(),
                        pick.read0
                    ),
                    (None, None, None, false)
                );
            }
            other => panic!("expected the picker, got {other:?}"),
        }
        match invocation(&[
            "kb",
            "-m",
            "-q",
            "ab",
            "--prompt",
            "> ",
            "-0",
            "--format",
            "text",
            "-x",
            "1",
            "-y",
            "-2",
            "choices.txt",
        ]) {
            Invocation::Pick(pick) => {
                assert_eq!(pick.multi, Some(true));
                assert_eq!(pick.query.as_deref(), Some("ab"));
                assert_eq!(pick.prompt.as_deref(), Some("> "));
                assert!(pick.read0);
                assert_eq!(pick.format.as_deref(), Some("text"));
                assert_eq!((pick.x, pick.y), (Some(1), Some(-2)));
                assert_eq!(pick.input.as_deref(), Some("choices.txt"));
            }
            other => panic!("expected the picker, got {other:?}"),
        }
        match invocation(&["kb", "--no-multi", "-"]) {
            Invocation::Pick(pick) => {
                assert_eq!(pick.multi, Some(false));
                assert_eq!(pick.input.as_deref(), Some("-"));
            }
            other => panic!("expected the picker, got {other:?}"),
        }
        assert!(parse(&argv(&["kb", "-m", "run"])).is_err());
        assert!(parse(&argv(&["kb", "--query", "hi", "show", "history"])).is_err());
        assert!(matches!(
            invocation(&["kb", "show", "history"]),
            Invocation::Show(_)
        ));
        assert_eq!(
            pick_input(None, true).unwrap_err(),
            "no choices on stdin; pipe a list, or pass a file: kb choices.txt"
        );
        assert_eq!(pick_input(None, false).unwrap(), "-");
        assert_eq!(pick_input(Some("items.txt"), true).unwrap(), "items.txt");
    }

    #[test]
    fn install_systemd_is_a_subcommand() {
        assert!(matches!(
            invocation(&["kb", "install-systemd"]),
            Invocation::InstallSystemd { config: None }
        ));
        match invocation(&["kb", "install-systemd", "--config", "/c.py"]) {
            Invocation::InstallSystemd { config } => {
                assert_eq!(config.as_deref(), Some(Path::new("/c.py")))
            }
            other => panic!("expected install-systemd, got {other:?}"),
        }
        assert!(parse(&argv(&["kb", "--install-systemd"])).is_err());
        assert!(parse(&argv(&["kb", "--config", "/c.py", "install-systemd"])).is_err());
    }

    #[test]
    fn socket_is_global() {
        let parsed = parse(&argv(&["kb", "show", "run", "--socket", "/s"])).unwrap();
        assert_eq!(parsed.socket.as_deref(), Some(Path::new("/s")));
        let parsed = parse(&argv(&["kb", "run", "--socket", "/d"])).unwrap();
        assert_eq!(parsed.socket.as_deref(), Some(Path::new("/d")));
        let parsed = parse(&argv(&["kb", "--socket", "/d", "run"])).unwrap();
        assert_eq!(parsed.socket.as_deref(), Some(Path::new("/d")));
    }

    #[test]
    fn show_accepts_pointer_coordinates_and_input() {
        match invocation(&["kb", "show", "run", "-x", "120", "-y", "-40"]) {
            Invocation::Show(popup) => {
                assert_eq!(
                    (popup.widget.as_str(), popup.x, popup.y, popup.input),
                    ("run", Some(120), Some(-40), None)
                );
            }
            other => panic!("expected show, got {other:?}"),
        }
        match invocation(&["kb", "prompt", "pick", "-", "--format", "json"]) {
            Invocation::Prompt(popup) => {
                assert_eq!(popup.input.as_deref(), Some("-"));
                assert_eq!(popup.format.as_deref(), Some("json"));
            }
            other => panic!("expected prompt, got {other:?}"),
        }
    }

    #[test]
    fn watch_and_stash_parse() {
        match invocation(&["kb", "watch", "inbox", "history"]) {
            Invocation::Watch(watch) => assert_eq!(watch.widgets, ["inbox", "history"]),
            other => panic!("expected watch, got {other:?}"),
        }
        match invocation(&["kb", "stash", "inbox", "--mark-read", "3", "4"]) {
            Invocation::Stash(stash) => {
                assert_eq!(stash.name, "inbox");
                assert_eq!(stash.mark_read, ["3", "4"]);
            }
            other => panic!("expected stash, got {other:?}"),
        }
        assert!(parse(&argv(&[
            "kb",
            "stash",
            "inbox",
            "--all",
            "--mark-read",
            "1"
        ]))
        .is_err());
    }

    #[test]
    fn askpass_and_pinentry_take_what_their_callers_pass() {
        match invocation(&["kb", "askpass", "[sudo] password for me: "]) {
            Invocation::Askpass(args) => {
                assert_eq!(args.prompt.as_deref(), Some("[sudo] password for me: "))
            }
            other => panic!("expected askpass, got {other:?}"),
        }
        assert!(matches!(
            invocation(&["kb", "askpass", "-not-a-flag"]),
            Invocation::Askpass(AskpassArgs { prompt: Some(_) })
        ));
        match invocation(&[
            "kb",
            "pinentry",
            "--display",
            ":0",
            "--ttyname",
            "/dev/pts/1",
        ]) {
            Invocation::Pinentry(args) => assert_eq!(args.ignored.len(), 4),
            other => panic!("expected pinentry, got {other:?}"),
        }
    }

    #[test]
    fn positional_coordinates_and_long_flags_are_rejected() {
        assert!(parse(&argv(&["kb", "show", "run", "120", "-40"])).is_err());
        assert!(parse(&argv(&["kb", "show", "run", "--x", "120", "--y", "80"])).is_err());
    }

    #[test]
    fn help_is_an_error_with_exit_code_zero() {
        let error = parse(&argv(&["kb", "--help"])).unwrap_err();
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
            load_records("items", Some("json"), Some(path), false, false, false).unwrap(),
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
        let error = load_records(
            "items",
            None,
            Some(empty.to_str().unwrap()),
            false,
            false,
            false,
        )
        .unwrap_err();
        assert!(error.contains("no records"), "{error}");
        let error = load_records("items", Some("yaml"), None, false, false, false).unwrap_err();
        assert!(error.contains("--format requires"), "{error}");
        assert!(load_records("items", None, None, false, false, false)
            .unwrap()
            .is_empty());
        let nul_path = dir.path().join("items.txt");
        std::fs::write(&nul_path, "one\0two\0").unwrap();
        assert_eq!(
            load_records(
                "items",
                None,
                Some(nul_path.to_str().unwrap()),
                true,
                true,
                false
            )
            .unwrap(),
            vec![Record::new("one"), Record::new("two")]
        );
        let error =
            load_records("items", Some("json"), Some(path), true, false, false).unwrap_err();
        assert!(error.contains("--read0"), "{error}");
        assert!(load_records(
            "items",
            None,
            Some(empty.to_str().unwrap()),
            false,
            true,
            false
        )
        .unwrap()
        .is_empty());
        let dupes = dir.path().join("dupes.txt");
        std::fs::write(&dupes, "a\na\na#2\n").unwrap();
        let dupes = load_records(
            "items",
            None,
            Some(dupes.to_str().unwrap()),
            false,
            false,
            true,
        )
        .unwrap();
        assert_eq!(
            dupes
                .iter()
                .map(|record| record.key.as_str())
                .collect::<Vec<_>>(),
            ["a", "a#2", "a#2#2"]
        );
        assert!(dupes
            .iter()
            .all(|record| record.value == "a" || record.value == "a#2"));
    }
}
