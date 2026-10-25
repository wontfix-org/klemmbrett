mod dbus;
mod systemd;
mod ui;

use clap::{Parser, Subcommand};
use klemmbrett::{
    load_config, request_show_menu, ArboardBackend, ClipboardSync, Sources, HISTORY_BUS_NAME,
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
    Activate {
        /// Configured source name
        source: String,
        /// Pointer X in compositor global coordinates
        #[arg(allow_hyphen_values = true, default_value_t = 0)]
        x: i32,
        /// Pointer Y in compositor global coordinates
        #[arg(allow_hyphen_values = true, default_value_t = 0)]
        y: i32,
    },
}

fn main() -> ExitCode {
    let args = Args::parse();
    match args.command {
        Some(Command::Activate { source, x, y }) => {
            if let Err(err) = request_show_menu(HISTORY_BUS_NAME, &source, x, y) {
                eprintln!("failed to activate {source:?}: {err}");
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
    fn activate_defaults_coordinates_to_the_origin() {
        let args = Args::try_parse_from(["klemmbrett", "activate", "history"]).unwrap();
        match args.command {
            Some(Command::Activate { source, x, y }) => {
                assert_eq!((source.as_str(), x, y), ("history", 0, 0));
            }
            other => panic!("expected activate, got {other:?}"),
        }
    }

    #[test]
    fn activate_accepts_pointer_coordinates() {
        let args = Args::try_parse_from(["klemmbrett", "activate", "run", "120", "-40"]).unwrap();
        match args.command {
            Some(Command::Activate { source, x, y }) => {
                assert_eq!((source.as_str(), x, y), ("run", 120, -40));
            }
            other => panic!("expected activate, got {other:?}"),
        }
    }

    #[test]
    fn install_systemd_still_parses_without_a_subcommand() {
        let args = Args::try_parse_from(["klemmbrett", "--install-systemd"]).unwrap();
        assert!(args.install_systemd);
        assert!(args.command.is_none());
    }
}
