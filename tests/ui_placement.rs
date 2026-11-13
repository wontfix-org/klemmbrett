//! Checks where the popup card actually appears on screen, per compositor.
//!
//! Unit tests cover `place_at_location`; this test covers what they cannot:
//! that the compositor honours the layer-shell surface, the output geometry
//! klemmbrett sees matches the real one, and the card is painted at the
//! computed margins. The card is themed a solid marker colour, the output is
//! captured over `wlr-screencopy`, and the marker's bounding box is compared
//! with `place_at_location` for the measured card size.
//!
//! `KLEMMBRETT_COMPOSITOR` picks one of `sway`, `river`, `qtile`, `cosmic`
//! (comma-separated for several); unset runs every one that is installed.
//! Each compositor runs headless in a private `XDG_RUNTIME_DIR`. COSMIC has
//! no headless backend, so it runs as a fullscreen window inside headless
//! sway and the outer output is captured.
//!
//! Failed cases leave their screenshot and logs under
//! `<target>/ui-artifacts/<compositor>/<case>/`.

mod common;

use klemmbrett::varlink::records_json;
use klemmbrett::{place_at_location, Client, Location, OutputGeom, Record};
use serde_json::json;
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
const CARD_TIMEOUT: Duration = Duration::from_secs(20);
const OUTPUT_MODE: &str = "1280x800";
const MARKER: [u8; 3] = [0xff, 0x00, 0xff];

const CONFIG: &str = r##"
import klemmbrett as kb
from klemmbrett.style import Style, Theme

kb.configure(
    theme=Theme(
        defaults=Style(background_color="#ff00ff", text_color="#ff00ff", border_color="#ff00ff"),
        window=Style(border=0, border_radius=0),
        element_selected=Style(background_color="#ff00ff"),
        element_hover=Style(background_color="#ff00ff"),
    )
)
kb.widget("center", location="center")
kb.widget("pointer", location="pointer")
kb.widget("south-east", location="south east")

from klemmbrett import notifications, ui
inbox = notifications.serve(
    lambda n: notifications.STASH if n.summary.startswith("stash") else notifications.SHOW,
    location="south-west",
)
kb.widget("inbox", inbox.unread, search=["summary"])
"##;

/// The toast layer surface keeps this far from the output edges.
const TOAST_MARGIN: i32 = 12;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Sway,
    River,
    Qtile,
    Cosmic,
}

impl Kind {
    const ALL: [Kind; 4] = [Kind::Sway, Kind::River, Kind::Qtile, Kind::Cosmic];

    fn name(self) -> &'static str {
        match self {
            Kind::Sway => "sway",
            Kind::River => "river",
            Kind::Qtile => "qtile",
            Kind::Cosmic => "cosmic",
        }
    }

    fn programs(self) -> &'static [&'static str] {
        match self {
            Kind::Sway => &["sway"],
            Kind::River => &["river", "riverctl"],
            Kind::Qtile => &["qtile"],
            Kind::Cosmic => &["sway", "cosmic-comp"],
        }
    }

    fn selected() -> Vec<Kind> {
        let Ok(list) = std::env::var("KLEMMBRETT_COMPOSITOR") else {
            return Kind::ALL.to_vec();
        };
        list.split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| {
                Kind::ALL
                    .into_iter()
                    .find(|kind| kind.name() == name)
                    .unwrap_or_else(|| panic!("unknown KLEMMBRETT_COMPOSITOR entry {name:?}"))
            })
            .collect()
    }
}

/// One placement to check: which widget to open and where the pointer is.
struct Case {
    name: &'static str,
    widget: &'static str,
    location: Location,
    pointer: fn(i32, i32) -> Option<(i32, i32)>,
}

const CASES: [Case; 4] = [
    Case {
        name: "center-without-pointer",
        widget: "center",
        location: Location::Center,
        pointer: |_, _| None,
    },
    Case {
        name: "pointer",
        widget: "pointer",
        location: Location::Pointer,
        pointer: |_, _| Some((200, 150)),
    },
    Case {
        name: "pointer-flipped",
        widget: "pointer",
        location: Location::Pointer,
        pointer: |width, height| Some((width - 30, height - 20)),
    },
    Case {
        name: "south-east",
        widget: "south-east",
        location: Location::SouthEast,
        pointer: |_, _| Some((200, 150)),
    },
];

/// Looks `program` up on `PATH` without running it.
fn have(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        dir.join(program)
            .metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    })
}

fn sockets(dir: &Path) -> HashSet<String> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("wayland-") && !name.ends_with(".lock"))
        .collect()
}

/// A compositor (or a nested pair), its bus, and the daemon under test.
struct Session {
    kind: Kind,
    runtime_dir: PathBuf,
    dbus: Child,
    bus_address: String,
    /// Outermost first, so teardown can go in reverse.
    compositors: Vec<Child>,
    /// The socket klemmbrett connects to.
    client_socket: String,
    /// The socket whose output is captured; the outer sway for COSMIC.
    capture_socket: String,
    app: Option<Child>,
    output: (i32, i32),
}

impl Session {
    fn start(kind: Kind) -> Result<Self, String> {
        for program in kind.programs().iter().chain(&["dbus-daemon"]) {
            if !have(program) {
                return Err(format!("{program} is not installed"));
            }
        }

        let runtime_dir = std::env::temp_dir().join(format!(
            "klemmbrett-ui-{}-{}",
            kind.name(),
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&runtime_dir);
        fs::create_dir_all(&runtime_dir).expect("create runtime dir");
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

        let mut session = Session {
            kind,
            runtime_dir,
            dbus,
            bus_address,
            compositors: Vec::new(),
            client_socket: String::new(),
            capture_socket: String::new(),
            app: None,
            output: (0, 0),
        };

        let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ui");
        let outer = match kind {
            Kind::Sway | Kind::Cosmic => {
                let mut sway = Command::new("sway");
                sway.arg("-c").arg(ui.join("sway.conf"));
                session.spawn_compositor(sway, "sway", None)?
            }
            Kind::River => {
                let mut river = Command::new("river");
                river.arg("-c").arg(ui.join("river-init"));
                session.spawn_compositor(river, "river", None)?
            }
            Kind::Qtile => {
                let mut qtile = Command::new("qtile");
                qtile
                    .args(["start", "-b", "wayland", "-c"])
                    .arg(ui.join("qtile_config.py"));
                session.spawn_compositor(qtile, "qtile", None)?
            }
        };
        session.capture_socket = outer.clone();
        session.set_output_mode(&outer);

        session.client_socket = if kind == Kind::Cosmic {
            let cosmic = Command::new("cosmic-comp");
            session.spawn_compositor(cosmic, "cosmic-comp", Some(&outer))?
        } else {
            outer
        };

        let baseline = session.wait_for_capture()?;
        session.output = (baseline.width, baseline.height);
        Ok(session)
    }

    /// Starts a compositor and waits for the Wayland socket it adds.
    ///
    /// `parent` is the socket of the compositor to nest inside; without one
    /// the compositor runs on the wlroots headless backend.
    fn spawn_compositor(
        &mut self,
        mut cmd: Command,
        label: &str,
        parent: Option<&str>,
    ) -> Result<String, String> {
        let before = sockets(&self.runtime_dir);
        let log = fs::File::create(self.runtime_dir.join(format!("{label}.log")))
            .expect("create compositor log");
        self.isolate(&mut cmd);
        cmd.env("WLR_RENDERER", "pixman")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("WLR_LIBINPUT_NO_DEVICES", "1")
            .stdout(log.try_clone().expect("clone log handle"))
            .stderr(log);
        match parent {
            Some(socket) => cmd.env("WAYLAND_DISPLAY", socket),
            None => cmd
                .env("WLR_BACKENDS", "headless")
                .env_remove("WAYLAND_DISPLAY"),
        };
        let child = cmd.spawn().map_err(|err| format!("spawn {label}: {err}"))?;
        self.compositors.push(child);

        let deadline = Instant::now() + STARTUP_TIMEOUT;
        while Instant::now() < deadline {
            if let Ok(Some(status)) = self.compositors.last_mut().expect("just pushed").try_wait() {
                return Err(format!(
                    "{label} exited early with {status}\n{}",
                    self.log(label)
                ));
            }
            if let Some(socket) = sockets(&self.runtime_dir).difference(&before).next() {
                return Ok(socket.clone());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(format!(
            "{label} created no Wayland socket in time\n{}",
            self.log(label)
        ))
    }

    /// Keeps a child away from the logged-in session and the user's files.
    fn isolate(&self, cmd: &mut Command) {
        cmd.env("XDG_RUNTIME_DIR", &self.runtime_dir)
            .env("XDG_CONFIG_HOME", self.runtime_dir.join("config"))
            .env("XDG_DATA_HOME", self.runtime_dir.join("data"))
            .env("XDG_CACHE_HOME", self.runtime_dir.join("cache"))
            .env("XDG_STATE_HOME", self.runtime_dir.join("state"))
            .env("DBUS_SESSION_BUS_ADDRESS", &self.bus_address)
            .env_remove("DISPLAY")
            .env_remove("SWAYSOCK");
    }

    /// Sizes the first output through wlr-output-management where it can.
    ///
    /// A compositor without it keeps its default mode; the test reads the
    /// real size from the capture either way.
    fn set_output_mode(&self, socket: &str) {
        if !have("wlr-randr") {
            return;
        }
        let run = |args: &[&str]| {
            let mut cmd = Command::new("wlr-randr");
            self.isolate(&mut cmd);
            cmd.env("WAYLAND_DISPLAY", socket)
                .args(args)
                .stderr(Stdio::null())
                .output()
                .ok()
                .filter(|out| out.status.success())
        };
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let name = loop {
            let name = run(&["--json"]).and_then(|out| {
                let outputs: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
                outputs.get(0)?.get("name")?.as_str().map(str::to_owned)
            });
            if name.is_some() || Instant::now() > deadline {
                break name;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        if let Some(name) = name {
            let _ = run(&["--output", &name, "--custom-mode", OUTPUT_MODE]);
        }
    }

    fn capture(&self) -> Result<capture::Image, String> {
        capture::output(
            &self.runtime_dir.join(&self.capture_socket),
            &self.runtime_dir,
        )
    }

    /// Captures until the compositor has an output to capture.
    fn wait_for_capture(&self) -> Result<capture::Image, String> {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            match self.capture() {
                Ok(image) if image.width > 0 && image.height > 0 => return Ok(image),
                Ok(_) if Instant::now() < deadline => {}
                Err(_) if Instant::now() < deadline => {}
                Ok(_) => return Err("the output has no size".into()),
                Err(err) => return Err(format!("cannot capture the output: {err}")),
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn start_app(&mut self) {
        let config = self.runtime_dir.join("config.py");
        fs::write(&config, CONFIG).expect("write app config");
        let log = fs::File::create(self.runtime_dir.join("app.log")).expect("create app log");
        let mut cmd = common::daemon_command();
        self.isolate(&mut cmd);
        cmd.env("WAYLAND_DISPLAY", &self.client_socket)
            .args(["run", "--config"])
            .arg(config)
            .stdout(log.try_clone().expect("clone log handle"))
            .stderr(log);
        self.app = Some(cmd.spawn().expect("spawn klemmbrett"));
    }

    fn stop_app(&mut self) {
        if let Some(mut app) = self.app.take() {
            let _ = app.kill();
            let _ = app.wait();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match self.capture() {
                Ok(image) if image.bounding_box(MARKER).is_some() => {}
                _ => return,
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Waits for the daemon's Varlink socket, then asks for the popup.
    fn show(&self, widget: &str, pointer: Option<(i32, i32)>) -> Result<(), String> {
        let records = records_json(&["alpha", "beta", "gamma"].map(Record::new));
        let pointer = pointer.map(|(x, y)| json!({ "x": x, "y": y }));
        let socket = self.runtime_dir.join("klemmbrett").join("varlink.sock");
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut last_error = None;
        while Instant::now() < deadline {
            let attempt = Client::connect(&socket).and_then(|mut client| {
                client.call(
                    "Show",
                    json!({ "widget": widget, "pointer": pointer, "records": records }),
                )
            });
            match attempt {
                Ok(_) => return Ok(()),
                Err(err) => last_error = Some(err.to_string()),
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(format!("the daemon never answered Show: {last_error:?}"))
    }

    /// Captures until the card is on screen and has stopped changing.
    fn wait_for_card(
        &self,
    ) -> Result<(capture::Image, capture::Box), (String, Option<capture::Image>)> {
        let deadline = Instant::now() + CARD_TIMEOUT;
        let mut previous = None;
        let mut last = None;
        while Instant::now() < deadline {
            let image = self.capture().map_err(|err| (err, None))?;
            let found = image.bounding_box(MARKER);
            if found.is_some() && found == previous {
                return Ok((image, found.expect("checked above")));
            }
            previous = found;
            last = Some(image);
            std::thread::sleep(Duration::from_millis(150));
        }
        Err(("the card never appeared".into(), last))
    }

    fn check(&mut self, case: &Case) -> Result<capture::Box, String> {
        let (width, height) = self.output;
        let pointer = (case.pointer)(width, height);
        self.start_app();
        let result = self.show(case.widget, pointer).map_err(|err| (err, None)).and_then(|()| {
            let (image, card) = self.wait_for_card()?;
            let outputs = [OutputGeom::new(0, 0, width, height)];
            let expected = place_at_location(case.location, pointer, card.width, card.height, &outputs, 0)
                .map(|(_, x, y)| (x, y));
            let close = expected.is_some_and(|(x, y)| (card.x - x).abs() <= 1 && (card.y - y).abs() <= 1);
            if close {
                Ok(card)
            } else {
                Err((
                    format!(
                        "card at ({}, {}) size {}x{}, expected origin {expected:?} on a {width}x{height} output with pointer {pointer:?}",
                        card.x, card.y, card.width, card.height
                    ),
                    Some(image),
                ))
            }
        });
        self.stop_app();
        result.map_err(|(message, image)| {
            let dir = self.save_artifacts(case, image.as_ref());
            format!("{message}\nartifacts: {}", dir.display())
        })
    }

    /// A critical notification sent over Varlink stays up as a toast in the
    /// configured corner.
    fn check_toast(&mut self) -> Result<capture::Box, String> {
        let (width, height) = self.output;
        self.start_app();
        let socket = self.runtime_dir.join("klemmbrett").join("varlink.sock");
        let deadline = Instant::now() + Duration::from_secs(15);
        let sent = loop {
            let attempt = Client::connect(&socket).and_then(|mut client| {
                client.call(
                    "Notify",
                    json!({ "app_name": "ui", "summary": "Toast", "body": "body", "urgency": 2 }),
                )
            });
            match attempt {
                Ok(_) => break Ok(()),
                Err(err) if Instant::now() >= deadline => {
                    break Err((format!("the daemon never answered Notify: {err}"), None))
                }
                Err(_) => std::thread::sleep(Duration::from_millis(250)),
            }
        };
        let result = sent.and_then(|()| {
            let (image, toast) = self.wait_for_card()?;
            self.check_stash(&socket).map_err(|err| (err, None))?;
            let bottom = toast.y + toast.height;
            if (toast.x - TOAST_MARGIN).abs() <= 1 && (bottom - (height - TOAST_MARGIN)).abs() <= 1 {
                Ok(toast)
            } else {
                Err((
                    format!(
                        "toast at ({}, {}) size {}x{}, expected its bottom-left corner at ({TOAST_MARGIN}, {}) on a {width}x{height} output",
                        toast.x, toast.y, toast.width, toast.height, height - TOAST_MARGIN
                    ),
                    Some(image),
                ))
            }
        });
        self.stop_app();
        result.map_err(|(message, image)| {
            let dir = self.artifact_dir("toast", image.as_ref());
            format!("{message}\nartifacts: {}", dir.display())
        })
    }

    /// The Python route stashes a notification, and the stash and the widget
    /// on it answer over Varlink.
    fn check_stash(&self, socket: &Path) -> Result<(), String> {
        let mut client = Client::connect(socket).map_err(|err| err.to_string())?;
        let reply = client
            .call("Notify", json!({ "app_name": "ui", "summary": "stash me" }))
            .map_err(|err| err.to_string())?;
        let key = reply["id"].to_string();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let listed = client
                .call(
                    "StashList",
                    json!({ "stash": "notifications", "unread_only": true }),
                )
                .map_err(|err| err.to_string())?;
            if listed["records"]
                .as_array()
                .is_some_and(|records| records.iter().any(|record| record["key"] == key.as_str()))
            {
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "the stash never listed notification {key}: {listed}"
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let records = client
            .call("Records", json!({ "widget": "inbox" }))
            .map_err(|err| err.to_string())?;
        let record = &records["records"][0];
        if record["data"]["summary"] != "stash me" || record["data"]["read"] != false {
            return Err(format!("unexpected inbox record {record}"));
        }
        let marked = client
            .call("StashMarkRead", json!({ "stash": "notifications" }))
            .map_err(|err| err.to_string())?;
        if marked["marked"] != 1 {
            return Err(format!(
                "expected one notification marked read, got {marked}"
            ));
        }
        Ok(())
    }

    fn save_artifacts(&self, case: &Case, image: Option<&capture::Image>) -> PathBuf {
        self.artifact_dir(case.name, image)
    }

    fn artifact_dir(&self, name: &str, image: Option<&capture::Image>) -> PathBuf {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .parent()
            .expect("target tmp dir has a parent")
            .join("ui-artifacts")
            .join(self.kind.name())
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create artifact dir");
        if let Some(image) = image {
            let _ = image.write_ppm(&dir.join("screenshot.ppm"));
        }
        for entry in fs::read_dir(&self.runtime_dir)
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "log") {
                let _ = fs::copy(&path, dir.join(entry.file_name()));
            }
        }
        dir
    }

    fn log(&self, label: &str) -> String {
        fs::read_to_string(self.runtime_dir.join(format!("{label}.log"))).unwrap_or_default()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(mut app) = self.app.take() {
            let _ = app.kill();
            let _ = app.wait();
        }
        for child in self.compositors.iter_mut().rev() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = self.dbus.kill();
        let _ = self.dbus.wait();
        let _ = fs::remove_dir_all(&self.runtime_dir);
    }
}

#[test]
fn the_card_lands_where_placement_puts_it() {
    let mut failures = Vec::new();
    for kind in Kind::selected() {
        let mut session = match Session::start(kind) {
            Ok(session) => session,
            Err(reason) if reason.ends_with("is not installed") => {
                eprintln!("ui_placement: skip {}: {reason}", kind.name());
                continue;
            }
            Err(reason) => {
                failures.push(format!("{}: {reason}", kind.name()));
                continue;
            }
        };
        let (width, height) = session.output;
        eprintln!(
            "ui_placement: run {} on a {width}x{height} output",
            kind.name()
        );
        for case in &CASES {
            match session.check(case) {
                Ok(card) => eprintln!(
                    "ui_placement: pass {}/{}: card at ({}, {}) size {}x{}",
                    kind.name(),
                    case.name,
                    card.x,
                    card.y,
                    card.width,
                    card.height
                ),
                Err(message) => {
                    eprintln!("ui_placement: fail {}/{}", kind.name(), case.name);
                    failures.push(format!("{}/{}: {message}", kind.name(), case.name));
                }
            }
        }
        match session.check_toast() {
            Ok(toast) => eprintln!(
                "ui_placement: pass {}/toast: at ({}, {}) size {}x{}",
                kind.name(),
                toast.x,
                toast.y,
                toast.width,
                toast.height
            ),
            Err(message) => {
                eprintln!("ui_placement: fail {}/toast", kind.name());
                failures.push(format!("{}/toast: {message}", kind.name()));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

/// A minimal `wlr-screencopy` client, so the test needs no screenshot tool.
mod capture {
    use std::fs;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::AsFd;
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use wayland_client::globals::{registry_queue_init, GlobalListContents};
    use wayland_client::protocol::wl_buffer::WlBuffer;
    use wayland_client::protocol::wl_output::WlOutput;
    use wayland_client::protocol::wl_registry::{self, WlRegistry};
    use wayland_client::protocol::wl_shm::{Format, WlShm};
    use wayland_client::protocol::wl_shm_pool::WlShmPool;
    use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle, WEnum};
    use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_frame_v1::{
        self, ZwlrScreencopyFrameV1,
    };
    use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1;

    pub struct Image {
        pub width: i32,
        pub height: i32,
        rgb: Vec<[u8; 3]>,
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Box {
        pub x: i32,
        pub y: i32,
        pub width: i32,
        pub height: i32,
    }

    impl Image {
        /// The smallest rectangle holding every pixel of exactly `color`.
        pub fn bounding_box(&self, color: [u8; 3]) -> Option<Box> {
            let (mut min_x, mut min_y, mut max_x, mut max_y) = (i32::MAX, i32::MAX, -1, -1);
            for (index, pixel) in self.rgb.iter().enumerate() {
                if *pixel == color {
                    let x = index as i32 % self.width;
                    let y = index as i32 / self.width;
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
            (max_x >= 0).then(|| Box {
                x: min_x,
                y: min_y,
                width: max_x - min_x + 1,
                height: max_y - min_y + 1,
            })
        }

        pub fn write_ppm(&self, path: &Path) -> std::io::Result<()> {
            let mut file = fs::File::create(path)?;
            write!(file, "P6\n{} {}\n255\n", self.width, self.height)?;
            file.write_all(&self.rgb.concat())
        }
    }

    #[derive(Default)]
    struct State {
        offers: Vec<(Format, u32, u32, u32)>,
        y_invert: bool,
        ready: bool,
        failed: bool,
    }

    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<ZwlrScreencopyFrameV1, ()> for State {
        fn event(
            state: &mut Self,
            _: &ZwlrScreencopyFrameV1,
            event: zwlr_screencopy_frame_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            use zwlr_screencopy_frame_v1::{Event, Flags};
            match event {
                Event::Buffer {
                    format: WEnum::Value(format),
                    width,
                    height,
                    stride,
                } => state.offers.push((format, width, height, stride)),
                Event::Flags { flags } => {
                    state.y_invert = matches!(flags, WEnum::Value(f) if f.contains(Flags::YInvert));
                }
                Event::Ready { .. } => state.ready = true,
                Event::Failed => state.failed = true,
                _ => {}
            }
        }
    }

    delegate_noop!(State: ignore WlShm);
    delegate_noop!(State: ignore WlBuffer);
    delegate_noop!(State: ignore WlOutput);
    delegate_noop!(State: WlShmPool);
    delegate_noop!(State: ZwlrScreencopyManagerV1);

    /// Captures the first output of the compositor listening on `socket`.
    /// The shared-memory buffer is a temporary file in `scratch`.
    pub fn output(socket: &Path, scratch: &Path) -> Result<Image, String> {
        let stream = UnixStream::connect(socket).map_err(|err| format!("connect: {err}"))?;
        let conn = Connection::from_socket(stream).map_err(|err| format!("connection: {err}"))?;
        let (globals, mut queue) =
            registry_queue_init::<State>(&conn).map_err(|err| format!("registry: {err}"))?;
        let qh = queue.handle();
        let shm: WlShm = globals
            .bind(&qh, 1..=1, ())
            .map_err(|err| format!("wl_shm: {err}"))?;
        let output: WlOutput = globals
            .bind(&qh, 1..=4, ())
            .map_err(|err| format!("wl_output: {err}"))?;
        let manager: ZwlrScreencopyManagerV1 = globals
            .bind(&qh, 1..=3, ())
            .map_err(|err| format!("no wlr-screencopy: {err}"))?;

        let mut state = State::default();
        let frame = manager.capture_output(0, &output, &qh, ());
        queue
            .roundtrip(&mut state)
            .map_err(|err| format!("roundtrip: {err}"))?;
        let (format, width, height, stride) = state
            .offers
            .iter()
            .copied()
            .find(|(format, ..)| {
                matches!(
                    format,
                    Format::Argb8888 | Format::Xrgb8888 | Format::Abgr8888 | Format::Xbgr8888
                )
            })
            .ok_or_else(|| format!("no 32-bit shm format offered: {:?}", state.offers))?;

        let size = stride as usize * height as usize;
        let mut file = tempfile::tempfile_in(scratch).map_err(|err| format!("shm file: {err}"))?;
        file.set_len(size as u64)
            .map_err(|err| format!("shm size: {err}"))?;
        let pool = shm.create_pool(file.as_fd(), size as i32, &qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            format,
            &qh,
            (),
        );
        frame.copy(&buffer);
        while !state.ready && !state.failed {
            queue
                .blocking_dispatch(&mut state)
                .map_err(|err| format!("dispatch: {err}"))?;
        }
        frame.destroy();
        buffer.destroy();
        pool.destroy();
        if state.failed {
            return Err("the compositor failed the copy".into());
        }

        let mut bytes = Vec::with_capacity(size);
        file.seek(SeekFrom::Start(0))
            .and_then(|_| file.read_to_end(&mut bytes))
            .map_err(|err| format!("read shm: {err}"))?;
        let red_first = matches!(format, Format::Abgr8888 | Format::Xbgr8888);
        let mut rgb = Vec::with_capacity(width as usize * height as usize);
        for row in 0..height as usize {
            let source = if state.y_invert {
                height as usize - 1 - row
            } else {
                row
            };
            let line = &bytes[source * stride as usize..][..width as usize * 4];
            for pixel in line.chunks_exact(4) {
                rgb.push(if red_first {
                    [pixel[0], pixel[1], pixel[2]]
                } else {
                    [pixel[2], pixel[1], pixel[0]]
                });
            }
        }
        Ok(Image {
            width: width as i32,
            height: height as i32,
            rgb,
        })
    }
}
