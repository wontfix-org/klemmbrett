mod dbus;
mod systemd;
mod ui;

use clap::{Parser, Subcommand};
use klemmbrett::{
    load_config, request_show, ArboardBackend, ClipboardSync, Sources, HISTORY_BUS_NAME,
};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(
    about = "Keep Wayland selections in sync and copy values from configurable popup sources."
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
    /// Ask the running daemon to show a popup source
    Show {
        /// Configured source name
        source: String,
        /// Pointer X in compositor global coordinates. Requires y.
        #[arg(allow_hyphen_values = true)]
        x: Option<i32>,
        /// Pointer Y in compositor global coordinates. Requires x.
        #[arg(allow_hyphen_values = true)]
        y: Option<i32>,
    },
}

fn main() -> ExitCode {
    let args = Args::parse();
    match args.command {
        Some(Command::Show { source, x, y }) => {
            let pointer = match (x, y) {
                (None, None) => None,
                (Some(x), Some(y)) => Some((x, y)),
                _ => {
                    eprintln!("show: x and y must both be given, or both omitted");
                    return ExitCode::FAILURE;
                }
            };
            if let Err(err) = request_show(HISTORY_BUS_NAME, &source, pointer) {
                eprintln!("failed to show {source:?}: {err}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
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
            let sources = Sources::from_config(&config).unwrap_or_else(|err| {
                panic!("Invalid source configuration: {err}");
            });

            let backend =
                ArboardBackend::new().expect("Failed to initialize the Wayland clipboard.");
            let sync = ClipboardSync::new(backend);

            ui::run(
                sync,
                sources,
                config.popup.label_max,
                config.popup.theme,
                config.launch,
            );
            ExitCode::SUCCESS
        }
    }
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
            Some(Command::Show { source, x, y }) => {
                assert_eq!((source.as_str(), x, y), ("history", None, None));
            }
            other => panic!("expected show, got {other:?}"),
        }
    }

    #[test]
    fn show_accepts_pointer_coordinates() {
        let args = Args::try_parse_from(["klemmbrett", "show", "run", "120", "-40"]).unwrap();
        match args.command {
            Some(Command::Show { source, x, y }) => {
                assert_eq!((source.as_str(), x, y), ("run", Some(120), Some(-40)));
            }
            other => panic!("expected show, got {other:?}"),
        }
    }

    #[test]
    fn install_systemd_still_parses_without_a_subcommand() {
        let args = Args::try_parse_from(["klemmbrett", "--install-systemd"]).unwrap();
        assert!(args.install_systemd);
        assert!(args.command.is_none());
    }
}
