//! Install a systemd user unit and a D-Bus session service file.

use klemmbrett::HISTORY_BUS_NAME;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

const UNIT_NAME: &str = "klemmbrett.service";
const IMPORT_VARS: &[&str] = &[
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_TYPE",
    "XDG_CURRENT_DESKTOP",
];

pub fn install(config: Option<&Path>) -> Result<(), String> {
    let exec = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|err| format!("failed to resolve this executable: {err}"))?;
    let config = config
        .map(|path| {
            path.canonicalize()
                .map_err(|err| format!("failed to resolve config {}: {err}", path.display()))
        })
        .transpose()?;
    let config = config.as_deref();

    write_file(&systemd_user_dir(), UNIT_NAME, &unit_file(&exec, config)?)?;
    write_file(
        &dbus_service_dir(),
        &format!("{HISTORY_BUS_NAME}.service"),
        &dbus_service_file(&exec, config)?,
    )?;

    import_session_environment();
    reload_user_units()?;
    enable_unit()?;
    if graphical_session_is_active() {
        start_unit()?;
        println!("Started {UNIT_NAME} for the current graphical session.");
    } else {
        println!(
            "{UNIT_NAME} is enabled for graphical-session.target and will start with the next graphical login."
        );
    }
    Ok(())
}

fn write_file(dir: &Path, name: &str, contents: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|err| format!("failed to create {}: {err}", dir.display()))?;
    let path = dir.join(name);
    std::fs::write(&path, contents)
        .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    println!("Installed {}", path.display());
    Ok(())
}

pub fn systemd_user_dir() -> PathBuf {
    systemd_user_dir_from_env(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

pub fn systemd_user_dir_from_env(xdg_config_home: Option<&OsStr>, home: Option<&OsStr>) -> PathBuf {
    xdg_dir(xdg_config_home, home, ".config")
        .join("systemd")
        .join("user")
}

pub fn dbus_service_dir() -> PathBuf {
    dbus_service_dir_from_env(
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

pub fn dbus_service_dir_from_env(xdg_data_home: Option<&OsStr>, home: Option<&OsStr>) -> PathBuf {
    xdg_dir(xdg_data_home, home, ".local/share")
        .join("dbus-1")
        .join("services")
}

fn xdg_dir(xdg: Option<&OsStr>, home: Option<&OsStr>, home_fallback: &str) -> PathBuf {
    xdg.map(PathBuf::from)
        .or_else(|| home.map(|home| PathBuf::from(home).join(home_fallback)))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

pub fn unit_file(exec: &Path, config: Option<&Path>) -> Result<String, String> {
    let exec_start = exec_start(exec, config)?;
    Ok(format!(
        "[Unit]\n\
         Description=Keep Wayland clipboard selections in sync and offer history\n\
         Documentation=https://github.com/wontfix-org/klemmbrett\n\
         PartOf=graphical-session.target\n\
         After=graphical-session-pre.target\n\
         \n\
         [Service]\n\
         Type=dbus\n\
         BusName={HISTORY_BUS_NAME}\n\
         ExecStart={exec_start}\n\
         PassEnvironment=WAYLAND_DISPLAY XDG_SESSION_TYPE\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n"
    ))
}

pub fn dbus_service_file(exec: &Path, config: Option<&Path>) -> Result<String, String> {
    let exec_start = exec_start(exec, config)?;
    Ok(format!(
        "[D-BUS Service]\n\
         Name={HISTORY_BUS_NAME}\n\
         Exec={exec_start}\n\
         SystemdService={UNIT_NAME}\n"
    ))
}

fn exec_start(exec: &Path, config: Option<&Path>) -> Result<String, String> {
    let mut command = format_exec_arg(exec)?;
    if let Some(config) = config {
        command.push_str(" --config ");
        command.push_str(&format_exec_arg(config)?);
    }
    Ok(command)
}

fn format_exec_arg(path: &Path) -> Result<String, String> {
    let text = path
        .to_str()
        .ok_or_else(|| format!("path {} is not valid UTF-8", path.display()))?;
    if text
        .chars()
        .any(|ch| ch.is_ascii_whitespace() || matches!(ch, '"' | '\'' | '\\' | '$' | '%'))
    {
        Ok(format!(
            "\"{}\"",
            text.replace('\\', "\\\\").replace('"', "\\\"")
        ))
    } else {
        Ok(text.to_owned())
    }
}

fn import_session_environment() {
    let vars: Vec<&str> = IMPORT_VARS
        .iter()
        .copied()
        .filter(|name| std::env::var_os(name).is_some())
        .collect();
    if vars.is_empty() {
        return;
    }
    let status = Command::new("systemctl")
        .arg("--user")
        .arg("import-environment")
        .args(&vars)
        .status();
    match status {
        Ok(status) if status.success() => {
            println!("Imported {} into the systemd user manager.", vars.join(" "));
        }
        Ok(status) => eprintln!(
            "Warning: systemctl --user import-environment {} exited with {status}",
            vars.join(" ")
        ),
        Err(err) => eprintln!("Warning: failed to import session environment: {err}"),
    }
}

fn reload_user_units() -> Result<(), String> {
    run_systemctl(&["daemon-reload"], "reload systemd user units")
}

fn enable_unit() -> Result<(), String> {
    run_systemctl(&["enable", UNIT_NAME], "enable the user unit")?;
    println!("Enabled {UNIT_NAME} for graphical-session.target.");
    Ok(())
}

fn start_unit() -> Result<(), String> {
    run_systemctl(&["restart", UNIT_NAME], "start the user unit")
}

fn graphical_session_is_active() -> bool {
    Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", "graphical-session.target"])
        .status()
        .is_ok_and(|status| status.success())
}

fn run_systemctl(args: &[&str], action: &str) -> Result<(), String> {
    let mut command = Command::new("systemctl");
    command.arg("--user").args(args);
    match command.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!(
            "failed to {action}: systemctl --user {} exited with {status}",
            args.join(" ")
        )),
        Err(err) => Err(format!("failed to {action}: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::Path;

    #[test]
    fn user_unit_dir_follows_xdg_then_home() {
        assert_eq!(
            systemd_user_dir_from_env(Some(OsStr::new("/xdg")), Some(OsStr::new("/home/me"))),
            PathBuf::from("/xdg/systemd/user")
        );
        assert_eq!(
            systemd_user_dir_from_env(None, Some(OsStr::new("/home/me"))),
            PathBuf::from("/home/me/.config/systemd/user")
        );
    }

    #[test]
    fn dbus_service_dir_follows_xdg_data_home() {
        assert_eq!(
            dbus_service_dir_from_env(Some(OsStr::new("/data")), Some(OsStr::new("/home/me"))),
            PathBuf::from("/data/dbus-1/services")
        );
        assert_eq!(
            dbus_service_dir_from_env(None, Some(OsStr::new("/home/me"))),
            PathBuf::from("/home/me/.local/share/dbus-1/services")
        );
    }

    #[test]
    fn unit_is_bound_to_the_graphical_session() {
        let unit = unit_file(Path::new("/usr/bin/klemmbrett"), None).unwrap();
        assert!(unit.contains("PartOf=graphical-session.target"));
        assert!(unit.contains("After=graphical-session-pre.target"));
        assert!(unit.contains("WantedBy=graphical-session.target"));
        assert!(unit.contains("Type=dbus\n"));
        assert!(unit.contains(&format!("BusName={HISTORY_BUS_NAME}\n")));
        assert!(unit.contains("PassEnvironment=WAYLAND_DISPLAY XDG_SESSION_TYPE\n"));
        assert!(unit.contains("ExecStart=/usr/bin/klemmbrett\n"));
        assert!(!unit.contains("--config"));
    }

    #[test]
    fn dbus_service_asks_systemd_to_own_the_bus_name() {
        let service = dbus_service_file(Path::new("/usr/bin/klemmbrett"), None).unwrap();
        assert!(service.contains(&format!("Name={HISTORY_BUS_NAME}\n")));
        assert!(service.contains("Exec=/usr/bin/klemmbrett\n"));
        assert!(service.contains(&format!("SystemdService={UNIT_NAME}\n")));
    }

    #[test]
    fn unit_can_pin_a_config_file() {
        let unit = unit_file(
            Path::new("/usr/bin/klemmbrett"),
            Some(Path::new("/etc/klemmbrett.hcl")),
        )
        .unwrap();
        assert!(unit.contains("ExecStart=/usr/bin/klemmbrett --config /etc/klemmbrett.hcl\n"));
        let service = dbus_service_file(
            Path::new("/usr/bin/klemmbrett"),
            Some(Path::new("/etc/klemmbrett.hcl")),
        )
        .unwrap();
        assert!(service.contains("Exec=/usr/bin/klemmbrett --config /etc/klemmbrett.hcl\n"));
    }

    #[test]
    fn exec_paths_with_spaces_are_quoted() {
        let unit = unit_file(Path::new("/opt/my apps/klemmbrett"), None).unwrap();
        assert!(unit.contains("ExecStart=\"/opt/my apps/klemmbrett\"\n"));
    }
}
