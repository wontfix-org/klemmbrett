//! Command line: the daemon, `show`, `prompt`, and `--install-systemd`.

use crate::widget::{parse_records, validate_records, FileFormat, Record};
use crate::{request_prompt, request_show, HISTORY_BUS_NAME};
use clap::{Args as ClapArgs, Parser, Subcommand};
use std::io::Read;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "klemmbrett",
    about = "Keep Wayland selections in sync and copy values from configurable popup widgets."
)]
struct Args {
    /// Python configuration file (defaults to $XDG_CONFIG_HOME/klemmbrett/config.py)
    #[arg(long)]
    config: Option<PathBuf>,

    /// Install a systemd user unit bound to graphical-session.target and enable it
    #[arg(long)]
    install_systemd: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Ask the running daemon to show a popup widget
    Show(PopupArgs),
    /// Ask the running daemon to prompt and print the selected values
    Prompt(PopupArgs),
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

/// What the command line asked for.
#[derive(Debug)]
pub enum Invocation {
    Daemon { config: Option<PathBuf> },
    InstallSystemd { config: Option<PathBuf> },
    Show(PopupArgs),
    Prompt(PopupArgs),
}

/// Parses `argv` (including the program name). Help and version requests
/// come back as an `Err` whose `exit_code` is 0.
pub fn parse(argv: &[String]) -> Result<Invocation, clap::Error> {
    let args = Args::try_parse_from(argv)?;
    Ok(match args.command {
        Some(Command::Show(popup)) => Invocation::Show(popup),
        Some(Command::Prompt(popup)) => Invocation::Prompt(popup),
        None if args.install_systemd => Invocation::InstallSystemd {
            config: args.config,
        },
        None => Invocation::Daemon {
            config: args.config,
        },
    })
}

/// Runs `show` or `prompt` against the daemon and returns the exit code.
pub fn run_popup(args: &PopupArgs, prompt: bool) -> i32 {
    let command = if prompt { "prompt" } else { "show" };
    let pointer = match (args.x, args.y) {
        (None, None) => None,
        (Some(x), Some(y)) => Some((x, y)),
        _ => {
            eprintln!("{command}: -x and -y must both be given, or both omitted");
            return 1;
        }
    };
    let records = match load_records(&args.widget, args.format.as_deref(), args.input.as_deref())
    {
        Ok(records) => records,
        Err(err) => {
            eprintln!("{command}: {err}");
            return 1;
        }
    };
    if prompt {
        match request_prompt(HISTORY_BUS_NAME, &args.widget, pointer, &records) {
            Ok(values) if values.is_empty() => 1,
            Ok(values) => {
                for value in values {
                    println!("{value}");
                }
                0
            }
            Err(err) => {
                eprintln!("failed to prompt {:?}: {err}", args.widget);
                1
            }
        }
    } else if let Err(err) = request_show(HISTORY_BUS_NAME, &args.widget, pointer, &records) {
        eprintln!("failed to show {:?}: {err}", args.widget);
        1
    } else {
        0
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

    #[test]
    fn no_subcommand_starts_the_daemon() {
        match parse(&argv(&["klemmbrett"])).unwrap() {
            Invocation::Daemon { config } => assert!(config.is_none()),
            other => panic!("expected daemon, got {other:?}"),
        }
        match parse(&argv(&["klemmbrett", "--config", "/c.py"])).unwrap() {
            Invocation::Daemon { config } => {
                assert_eq!(config.as_deref(), Some(std::path::Path::new("/c.py")))
            }
            other => panic!("expected daemon, got {other:?}"),
        }
    }

    #[test]
    fn install_systemd_still_parses_without_a_subcommand() {
        assert!(matches!(
            parse(&argv(&["klemmbrett", "--install-systemd"])).unwrap(),
            Invocation::InstallSystemd { config: None }
        ));
    }

    #[test]
    fn show_accepts_pointer_coordinates_and_input() {
        match parse(&argv(&["klemmbrett", "show", "run", "-x", "120", "-y", "-40"])).unwrap() {
            Invocation::Show(popup) => {
                assert_eq!(
                    (popup.widget.as_str(), popup.x, popup.y, popup.input),
                    ("run", Some(120), Some(-40), None)
                );
            }
            other => panic!("expected show, got {other:?}"),
        }
        match parse(&argv(&[
            "klemmbrett",
            "prompt",
            "pick",
            "-",
            "--format",
            "json",
        ]))
        .unwrap()
        {
            Invocation::Prompt(popup) => {
                assert_eq!(popup.input.as_deref(), Some("-"));
                assert_eq!(popup.format.as_deref(), Some("json"));
            }
            other => panic!("expected prompt, got {other:?}"),
        }
    }

    #[test]
    fn positional_coordinates_and_long_flags_are_rejected() {
        assert!(parse(&argv(&["klemmbrett", "show", "run", "120", "-40"])).is_err());
        assert!(parse(&argv(&["klemmbrett", "show", "run", "--x", "120", "--y", "80"])).is_err());
    }

    #[test]
    fn help_is_an_error_with_exit_code_zero() {
        let error = parse(&argv(&["klemmbrett", "--help"])).unwrap_err();
        assert_eq!(error.exit_code(), 0);
    }

    #[test]
    fn load_records_uses_the_file_provider_formats() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("items.json");
        std::fs::write(
            &path,
            r#"["plain", {"label":"Readable", "value":"copied"}]"#,
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
