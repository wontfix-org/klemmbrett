//! Install systemd user units, and tell systemd when the daemon is ready.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

const UNIT_NAME: &str = "klemmbrett.service";
const SOCKET_NAME: &str = "klemmbrett.socket";
/// What earlier versions installed for D-Bus activation.
const LEGACY_DBUS_SERVICE: &str = "org.wontfix.klemmbrett.service";
const IMPORT_VARS: &[&str] = &[
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_TYPE",
    "XDG_CURRENT_DESKTOP",
];

/// `exec` is the command that starts klemmbrett, such as the console
/// script path or `python -m klemmbrett`.
pub fn install(exec: &[String], config: Option<&Path>) -> Result<(), String> {
    if exec.is_empty() {
        return Err("no command to start klemmbrett with".into());
    }
    let config = config
        .map(|path| {
            path.canonicalize()
                .map_err(|err| format!("failed to resolve config {}: {err}", path.display()))
        })
        .transpose()?;
    let config = config.as_deref();

    let units = systemd_user_dir();
    write_file(&units, UNIT_NAME, &unit_file(exec, config)?)?;
    write_file(&units, SOCKET_NAME, &socket_file())?;
    remove_legacy_dbus_service(&dbus_service_dir());

    import_session_environment();
    reload_user_units()?;
    enable_units()?;
    if graphical_session_is_active() {
        start_units()?;
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

/// A leftover D-Bus service file would let old `Show` calls start the unit.
fn remove_legacy_dbus_service(dir: &Path) {
    let path = dir.join(LEGACY_DBUS_SERVICE);
    match std::fs::remove_file(&path) {
        Ok(()) => println!("Removed {}", path.display()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => eprintln!("Warning: failed to remove {}: {err}", path.display()),
    }
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

pub fn unit_file(exec: &[String], config: Option<&Path>) -> Result<String, String> {
    let exec_start = exec_start(exec, config)?;
    Ok(format!(
        "[Unit]\n\
         Description=Keep Wayland clipboard selections in sync and offer history\n\
         Documentation=https://github.com/wontfix-org/klemmbrett\n\
         PartOf=graphical-session.target\n\
         After=graphical-session-pre.target {SOCKET_NAME}\n\
         Requires={SOCKET_NAME}\n\
         \n\
         [Service]\n\
         Type=notify\n\
         ExecStart={exec_start}\n\
         PassEnvironment=WAYLAND_DISPLAY XDG_SESSION_TYPE\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n\
         Also={SOCKET_NAME}\n"
    ))
}

/// The Varlink socket. systemd holds it, so a call made while the daemon
/// restarts waits instead of failing, and the first call starts the unit.
pub fn socket_file() -> String {
    String::from(
        "[Unit]\n\
         Description=klemmbrett Varlink socket\n\
         PartOf=graphical-session.target\n\
         \n\
         [Socket]\n\
         ListenStream=%t/klemmbrett/varlink.sock\n\
         SocketMode=0600\n\
         DirectoryMode=0700\n\
         RemoveOnStop=yes\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n",
    )
}

fn exec_start(exec: &[String], config: Option<&Path>) -> Result<String, String> {
    let mut parts = exec
        .iter()
        .map(|part| format_exec_arg(part))
        .collect::<Vec<_>>();
    if let Some(config) = config {
        let text = config
            .to_str()
            .ok_or_else(|| format!("path {} is not valid UTF-8", config.display()))?;
        parts.push("--config".into());
        parts.push(format_exec_arg(text));
    }
    Ok(parts.join(" "))
}

fn format_exec_arg(text: &str) -> String {
    if text
        .chars()
        .any(|ch| ch.is_ascii_whitespace() || matches!(ch, '"' | '\'' | '\\' | '$' | '%'))
    {
        format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        text.to_owned()
    }
}

/// Sends `READY=1` to `$NOTIFY_SOCKET` when systemd started this process
/// as a `Type=notify` unit. Does nothing otherwise.
pub fn notify_ready() {
    let Some(socket) = std::env::var_os("NOTIFY_SOCKET") else {
        return;
    };
    std::env::remove_var("NOTIFY_SOCKET");
    if let Err(err) = send_notify(&socket, b"READY=1") {
        eprintln!("klemmbrett: failed to notify systemd: {err}");
    }
}

fn send_notify(socket: &OsStr, message: &[u8]) -> std::io::Result<()> {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};

    let bytes = socket.as_bytes();
    let addr = match bytes.strip_prefix(b"@") {
        Some(name) => SocketAddr::from_abstract_name(name)?,
        None => SocketAddr::from_pathname(Path::new(socket))?,
    };
    UnixDatagram::unbound()?.send_to_addr(message, &addr)?;
    Ok(())
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

fn enable_units() -> Result<(), String> {
    run_systemctl(&["enable", SOCKET_NAME, UNIT_NAME], "enable the user units")?;
    println!("Enabled {SOCKET_NAME} and {UNIT_NAME} for graphical-session.target.");
    Ok(())
}

fn start_units() -> Result<(), String> {
    // A daemon from an older install still owns the socket path; stop it first.
    run_systemctl(&["stop", UNIT_NAME], "stop the user unit")?;
    run_systemctl(&["restart", SOCKET_NAME], "start the socket unit")?;
    run_systemctl(&["start", UNIT_NAME], "start the user unit")
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

    fn exec(path: &str) -> Vec<String> {
        vec![path.to_owned()]
    }

    #[test]
    fn module_invocations_keep_every_argument() {
        let unit = unit_file(
            &["/usr/bin/python3".into(), "-m".into(), "klemmbrett".into()],
            None,
        )
        .unwrap();
        assert!(unit.contains("ExecStart=/usr/bin/python3 -m klemmbrett\n"));
    }

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
    fn unit_is_a_notify_service_bound_to_the_session_and_its_socket() {
        let unit = unit_file(&exec("/usr/bin/klemmbrett"), None).unwrap();
        assert!(unit.contains("PartOf=graphical-session.target"));
        assert!(unit.contains("After=graphical-session-pre.target klemmbrett.socket\n"));
        assert!(unit.contains("Requires=klemmbrett.socket\n"));
        assert!(unit.contains("WantedBy=graphical-session.target"));
        assert!(unit.contains("Also=klemmbrett.socket\n"));
        assert!(unit.contains("Type=notify\n"));
        assert!(!unit.contains("dbus"), "{unit}");
        assert!(unit.contains("PassEnvironment=WAYLAND_DISPLAY XDG_SESSION_TYPE\n"));
        assert!(unit.contains("ExecStart=/usr/bin/klemmbrett\n"));
        assert!(!unit.contains("--config"));
    }

    #[test]
    fn socket_unit_listens_where_the_daemon_looks() {
        let socket = socket_file();
        assert!(socket.contains("ListenStream=%t/klemmbrett/varlink.sock\n"));
        assert!(socket.contains("SocketMode=0600\n"));
        assert!(socket.contains("WantedBy=graphical-session.target\n"));
    }

    #[test]
    fn legacy_dbus_service_files_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join(LEGACY_DBUS_SERVICE);
        std::fs::write(&legacy, "[D-BUS Service]\n").unwrap();
        remove_legacy_dbus_service(dir.path());
        assert!(!legacy.exists());
        remove_legacy_dbus_service(dir.path());
    }

    #[test]
    fn unit_can_pin_a_config_file() {
        let unit = unit_file(
            &exec("/usr/bin/klemmbrett"),
            Some(Path::new("/etc/klemmbrett.py")),
        )
        .unwrap();
        assert!(unit.contains("ExecStart=/usr/bin/klemmbrett --config /etc/klemmbrett.py\n"));
    }

    #[test]
    fn exec_paths_with_spaces_are_quoted() {
        let unit = unit_file(&exec("/opt/my apps/klemmbrett"), None).unwrap();
        assert!(unit.contains("ExecStart=\"/opt/my apps/klemmbrett\"\n"));
    }

    #[test]
    fn ready_reaches_the_notify_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notify");
        let receiver = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
        send_notify(path.as_os_str(), b"READY=1").unwrap();
        let mut buffer = [0u8; 16];
        let n = receiver.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..n], b"READY=1");
    }
}
