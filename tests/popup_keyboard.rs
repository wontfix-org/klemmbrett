//! Proves the history popup reacts to the keyboard without any pointer input.
//!
//! This is the case a unit test cannot cover: the popup is opened from D-Bus,
//! so there is no input event serial, and a Wayland `xdg_popup` grab needs one.
//! The surface therefore has to take keyboard focus through layer-shell
//! interactivity instead. Only a real compositor can show whether that works,
//! so this drives a private headless sway and types into it.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use klemmbrett::{
    ArboardBackend, Backend, HistoryStore, Selection, HISTORY_INTERFACE, HISTORY_OBJECT_PATH,
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);

/// Looks `program` up on `PATH` without running it.
///
/// Probing with `--version` would be enough for the command line tools, but
/// the decoy below is a graphical client that ignores unknown flags and would
/// open a window on whatever session the test itself inherited.
fn have(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(program);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            candidate
                .metadata()
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        }
        #[cfg(not(unix))]
        {
            candidate.is_file()
        }
    })
}

/// A headless sway plus a session bus, both private to this test.
struct Session {
    sway: Child,
    dbus: Child,
    app: Option<Child>,
    decoy: Option<Child>,
    runtime_dir: PathBuf,
    socket: String,
    bus_address: String,
}

impl Session {
    fn start() -> Option<Self> {
        if !have("sway") || !have("wtype") || !have("dbus-daemon") {
            return None;
        }

        let unique = format!("klemmbrett-popup-{}", std::process::id());
        let runtime_dir = std::env::temp_dir().join(&unique);
        let _ = fs::remove_dir_all(&runtime_dir);
        fs::create_dir_all(&runtime_dir).expect("create runtime dir");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&runtime_dir, fs::Permissions::from_mode(0o700))
                .expect("lock down runtime dir");
        }

        let mut dbus = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn dbus-daemon");
        let mut address = String::new();
        BufReader::new(dbus.stdout.take().expect("dbus stdout"))
            .read_line(&mut address)
            .expect("read bus address");
        let bus_address = address.trim().to_owned();
        assert!(!bus_address.is_empty(), "dbus-daemon printed no address");

        let config = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sway_config");
        let log = fs::File::create(runtime_dir.join("sway.log")).expect("create sway log");
        let sway = Command::new("sway")
            .arg("-c")
            .arg(&config)
            .env("XDG_RUNTIME_DIR", &runtime_dir)
            .env("WLR_BACKENDS", "headless")
            .env("WLR_RENDERER", "pixman")
            .env("WLR_LIBINPUT_NO_DEVICES", "1")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .stdout(log.try_clone().expect("clone log handle"))
            .stderr(log)
            .spawn()
            .expect("spawn sway");

        let mut session = Session {
            sway,
            dbus,
            app: None,
            decoy: None,
            runtime_dir,
            socket: String::new(),
            bus_address,
        };
        session.socket = session.wait_for_socket();
        Some(session)
    }

    /// Opens an ordinary window that holds the keyboard focus.
    ///
    /// Without one the popup is the only surface around and every compositor
    /// hands it the keyboard regardless of what it asked for, which would let
    /// a popup that never requested keyboard interactivity pass this test.
    fn open_focused_window(&mut self) {
        if !have("weston-simple-shm") {
            return;
        }
        self.decoy = self
            .command("weston-simple-shm")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok();
        std::thread::sleep(Duration::from_millis(750));
    }

    fn wait_for_socket(&mut self) -> String {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        while Instant::now() < deadline {
            if let Ok(Some(status)) = self.sway.try_wait() {
                panic!("sway exited early with {status}");
            }
            if let Ok(entries) = fs::read_dir(&self.runtime_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.starts_with("wayland-") && !name.ends_with(".lock") {
                        return name;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("sway did not create a Wayland socket in time");
    }

    fn command(&self, program: &str) -> Command {
        let mut cmd = Command::new(program);
        cmd.env("XDG_RUNTIME_DIR", &self.runtime_dir)
            .env("WAYLAND_DISPLAY", &self.socket)
            .env("DBUS_SESSION_BUS_ADDRESS", &self.bus_address)
            .env_remove("DISPLAY");
        cmd
    }

    fn start_app(&mut self, db: &std::path::Path) {
        let config = self.runtime_dir.join("config.toml");
        fs::write(
            &config,
            format!(
                "[[sources]]\nname = \"history\"\ntype = \"clipboard-history\"\ndb = {:?}\n",
                db
            ),
        )
        .expect("write app config");
        let log = fs::File::create(self.runtime_dir.join("app.log")).expect("create app log");
        let app = self
            .command(env!("CARGO_BIN_EXE_klemmbrett"))
            .arg("--config")
            .arg(config)
            .stdout(log.try_clone().expect("clone log handle"))
            .stderr(log)
            .spawn()
            .expect("spawn klemmbrett");
        self.app = Some(app);
    }

    /// Points this process at the private compositor so `arboard` can read the
    /// clipboard. Safe here because this file holds a single test.
    fn attach(&self) {
        std::env::set_var("XDG_RUNTIME_DIR", &self.runtime_dir);
        std::env::set_var("WAYLAND_DISPLAY", &self.socket);
        std::env::remove_var("DISPLAY");
    }

    fn app_log(&self) -> String {
        fs::read_to_string(self.runtime_dir.join("app.log")).unwrap_or_default()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        for child in [self.app.as_mut(), self.decoy.as_mut()]
            .into_iter()
            .flatten()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = self.sway.kill();
        let _ = self.sway.wait();
        let _ = self.dbus.kill();
        let _ = self.dbus.wait();
        let _ = fs::remove_dir_all(&self.runtime_dir);
    }
}

/// Waits for the daemon to claim the bus name, then asks for the popup.
fn show_menu(session: &Session, source: &str, x: i32, y: i32) {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut last_error = None;
    while Instant::now() < deadline {
        let attempt = zbus::blocking::connection::Builder::address(session.bus_address.as_str())
            .and_then(|builder| builder.build())
            .and_then(|conn| {
                conn.call_method(
                    Some("org.klemmbrett"),
                    HISTORY_OBJECT_PATH,
                    Some(HISTORY_INTERFACE),
                    "ShowMenu",
                    &(source, x, y),
                )
            });
        match attempt {
            Ok(_) => return,
            Err(err) => last_error = Some(err),
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!(
        "the daemon never answered ShowMenu: {last_error:?}\n{}",
        session.app_log()
    );
}

#[test]
fn a_history_entry_can_be_picked_by_key_without_moving_the_pointer() {
    let Some(mut session) = Session::start() else {
        eprintln!("skipping: sway, wtype or dbus-daemon is not installed");
        return;
    };

    // Seed the history before the daemon opens the database, so the popup has
    // known rows: "beta" is row 1 and "alpha" is row 2.
    let db = session.runtime_dir.join("history.sqlite");
    {
        let mut store = HistoryStore::open(&db, 10).expect("open history");
        store.record("alpha").expect("record alpha");
        store.record("beta").expect("record beta");
    }

    session.open_focused_window();
    session.start_app(&db);
    show_menu(&session, "history", 100, 100);
    std::thread::sleep(Duration::from_millis(750));

    // A headless seat has no keyboard until wtype creates a virtual one, and
    // the client only binds a keyboard once the capability appears. The first
    // key makes the seat grow a keyboard; the delay lets the popup bind it
    // before the key that matters. On a real seat the keyboard is always present.
    //
    // "a" would be row 10, which does not exist here, so it changes nothing.
    let typed = session
        .command("wtype")
        .args(["-k", "a", "-s", "1200", "-k", "2"])
        .status()
        .expect("run wtype");
    assert!(typed.success(), "wtype failed");

    session.attach();
    let mut clipboard = ArboardBackend::new().expect("clipboard connection");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = String::new();
    while Instant::now() < deadline {
        seen = clipboard.read(Selection::Clipboard).unwrap_or_default();
        if seen == "alpha" {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    panic!(
        "pressing the accelerator did not put the history entry on the clipboard \
         (clipboard is {seen:?})\n{}",
        session.app_log()
    );
}
