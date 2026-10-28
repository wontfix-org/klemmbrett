mod dbus;
mod systemd;
mod ui;

use clap::{Args as ClapArgs, Parser, Subcommand};
use klemmbrett::{
    load_config, parse_records, request_prompt, request_show, validate_records, ArboardBackend,
    ClipboardSync, FileFormat, Record, Widgets, HISTORY_BUS_NAME,
};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(
    about = "Keep Wayland selections in sync and copy values from configurable popup widgets."
)]
struct Args {
    /// HCL configuration file (defaults to $XDG_CONFIG_HOME/klemmbrett/config.hcl)
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

#[derive(ClapArgs, Debug)]
struct PopupArgs {
    /// Configured widget name
    widget: String,
    /// Pointer X in compositor global coordinates. Requires y.
    #[arg(allow_hyphen_values = true)]
    x: Option<i32>,
    /// Pointer Y in compositor global coordinates. Requires x.
    #[arg(allow_hyphen_values = true)]
    y: Option<i32>,
    /// Record format (text, json, or yaml). Reads --input, or stdin when --input is omitted.
    #[arg(long)]
    format: Option<String>,
    /// Record file. Requires --format. Omit to read stdin.
    #[arg(long)]
    input: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match args.command {
        Some(Command::Show(popup)) => show_popup("show", popup, false),
        Some(Command::Prompt(popup)) => show_popup("prompt", popup, true),
        None if args.install_systemd => {
            systemd::install(args.config.as_deref()).unwrap_or_else(|err| {
                panic!("{err}");
            });
            ExitCode::SUCCESS
        }
        None => {
            let config = load_config(args.config.as_deref()).unwrap_or_else(|err| {
                panic!("{err}");
            });
            let widgets = Widgets::from_config(&config).unwrap_or_else(|err| {
                panic!("Invalid widget configuration: {err}");
            });

            let backend =
                ArboardBackend::new().expect("Failed to initialize the Wayland clipboard.");
            let sync = ClipboardSync::new(backend);

            ui::run(
                sync,
                widgets,
                config.popup.label_max,
                config.popup.theme,
                config.launch,
            );
            ExitCode::SUCCESS
        }
    }
}

fn show_popup(command: &str, args: PopupArgs, prompt: bool) -> ExitCode {
    let pointer = match (args.x, args.y) {
        (None, None) => None,
        (Some(x), Some(y)) => Some((x, y)),
        _ => {
            eprintln!("{command}: x and y must both be given, or both omitted");
            return ExitCode::FAILURE;
        }
    };
    let records = match load_records(&args.widget, args.format.as_deref(), args.input.as_deref()) {
        Ok(records) => records,
        Err(err) => {
            eprintln!("{command}: {err}");
            return ExitCode::FAILURE;
        }
    };
    if prompt {
        match request_prompt(HISTORY_BUS_NAME, &args.widget, pointer, &records) {
            Ok(values) if values.is_empty() => ExitCode::FAILURE,
            Ok(values) => {
                for value in values {
                    println!("{value}");
                }
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("failed to prompt {:?}: {err}", args.widget);
                ExitCode::FAILURE
            }
        }
    } else if let Err(err) = request_show(HISTORY_BUS_NAME, &args.widget, pointer, &records) {
        eprintln!("failed to show {:?}: {err}", args.widget);
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn load_records(
    widget: &str,
    format: Option<&str>,
    input: Option<&Path>,
) -> Result<Vec<Record>, String> {
    let Some(format) = format else {
        if input.is_some() {
            return Err("--input requires --format".into());
        }
        return Ok(Vec::new());
    };
    let format = FileFormat::parse(format)?;
    let text = match input {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|err| format!("failed to read {}: {err}", path.display()))?,
        None => {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|err| format!("failed to read stdin: {err}"))?;
            text
        }
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
    use clap::Parser;

    #[test]
    fn no_subcommand_starts_the_daemon() {
        let args = Args::try_parse_from(["klemmbrett"]).unwrap();
        assert!(args.command.is_none());
        assert!(!args.install_systemd);
        assert!(args.config.is_none());
    }

    #[test]
    fn show_omits_pointer_coordinates() {
        let args = Args::try_parse_from(["klemmbrett", "show", "history"]).unwrap();
        match args.command {
            Some(Command::Show(popup)) => {
                assert_eq!(
                    (popup.widget.as_str(), popup.x, popup.y),
                    ("history", None, None)
                );
                assert!(popup.format.is_none());
                assert!(popup.input.is_none());
            }
            other => panic!("expected show, got {other:?}"),
        }
    }

    #[test]
    fn show_accepts_pointer_coordinates() {
        let args = Args::try_parse_from(["klemmbrett", "show", "run", "120", "-40"]).unwrap();
        match args.command {
            Some(Command::Show(popup)) => {
                assert_eq!(
                    (popup.widget.as_str(), popup.x, popup.y),
                    ("run", Some(120), Some(-40))
                );
            }
            other => panic!("expected show, got {other:?}"),
        }
    }

    #[test]
    fn show_and_prompt_accept_format_and_input() {
        let args = Args::try_parse_from([
            "klemmbrett",
            "show",
            "history",
            "--format",
            "yaml",
            "--input",
            "items.yaml",
        ])
        .unwrap();
        match args.command {
            Some(Command::Show(popup)) => {
                assert_eq!(popup.format.as_deref(), Some("yaml"));
                assert_eq!(popup.input.unwrap(), PathBuf::from("items.yaml"));
            }
            other => panic!("expected show, got {other:?}"),
        }
        let args = Args::try_parse_from([
            "klemmbrett",
            "prompt",
            "run",
            "120",
            "-40",
            "--format",
            "json",
        ])
        .unwrap();
        match args.command {
            Some(Command::Prompt(popup)) => {
                assert_eq!(
                    (
                        popup.widget.as_str(),
                        popup.x,
                        popup.y,
                        popup.format.as_deref()
                    ),
                    ("run", Some(120), Some(-40), Some("json"))
                );
                assert!(popup.input.is_none());
            }
            other => panic!("expected prompt, got {other:?}"),
        }
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
        assert_eq!(
            load_records("items", Some("json"), Some(&path)).unwrap(),
            vec![
                Record::new("plain"),
                Record {
                    key: "copied".into(),
                    label: "Readable".into(),
                    value: "copied".into(),
                },
            ]
        );
        std::fs::write(dir.path().join("empty.txt"), "\n").unwrap();
        let error =
            load_records("items", Some("text"), Some(&dir.path().join("empty.txt"))).unwrap_err();
        assert!(error.contains("no records"), "{error}");
        let error = load_records("items", None, Some(&path)).unwrap_err();
        assert!(error.contains("--input requires --format"), "{error}");
        assert!(load_records("items", None, None).unwrap().is_empty());
    }

    #[test]
    fn install_systemd_still_parses_without_a_subcommand() {
        let args = Args::try_parse_from(["klemmbrett", "--install-systemd"]).unwrap();
        assert!(args.install_systemd);
        assert!(args.command.is_none());
    }
}
