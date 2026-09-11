//! End-to-end test against a private headless sway compositor.
//!
//! The deterministic loop-prevention cases live in `loop_prevention.rs`; this
//! test covers the part a fake cannot: that `ArboardBackend` talks to the real
//! `wlr-data-control` protocol and addresses the two selections correctly.
//!
//! The compositor is started by the test itself in its own `XDG_RUNTIME_DIR`,
//! so it never reads from or writes to a logged-in Wayland session.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use klemmbrett::{Action, ArboardBackend, Backend, ClipboardSync, Selection};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);

/// A headless sway instance with a runtime directory of its own.
struct Compositor {
    child: Child,
    runtime_dir: PathBuf,
    log_path: PathBuf,
    socket: String,
}

impl Compositor {
    fn start() -> Option<Self> {
        if Command::new("sway")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            return None;
        }

        let unique = format!("klemmbrett-test-{}", std::process::id());
        let runtime_dir = std::env::temp_dir().join(&unique);
        let _ = fs::remove_dir_all(&runtime_dir);
        fs::create_dir_all(&runtime_dir).expect("create runtime dir");
        // A Wayland runtime directory must not be world accessible.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&runtime_dir, fs::Permissions::from_mode(0o700))
                .expect("lock down runtime dir");
        }

        // Deliberately kept after the run so a failure can be investigated.
        let log_path = std::env::temp_dir().join(format!("{unique}.log"));
        let log = fs::File::create(&log_path).expect("create compositor log");
        let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sway_config");

        let child = Command::new("sway")
            .arg("-d")
            .arg("-c")
            .arg(&config)
            .env("XDG_RUNTIME_DIR", &runtime_dir)
            .env("WLR_BACKENDS", "headless")
            .env("WLR_RENDERER", "pixman")
            .env("WLR_LIBINPUT_NO_DEVICES", "1")
            // Never let the compositor attach to a real session.
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .stdout(log.try_clone().expect("clone log handle"))
            .stderr(log)
            .spawn()
            .expect("spawn sway");

        let mut compositor = Compositor {
            child,
            runtime_dir,
            log_path,
            socket: String::new(),
        };
        compositor.socket = compositor.wait_for_socket();
        Some(compositor)
    }

    fn wait_for_socket(&mut self) -> String {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        while Instant::now() < deadline {
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!("sway exited early with {status}\n{}", self.log());
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
        panic!(
            "sway did not create a Wayland socket within {STARTUP_TIMEOUT:?}\n{}",
            self.log()
        );
    }

    /// Points this process at the private compositor.
    ///
    /// Environment variables are process wide, which is why this file holds a
    /// single `#[test]`: two tests would race over these.
    fn attach(&self) {
        std::env::set_var("XDG_RUNTIME_DIR", &self.runtime_dir);
        std::env::set_var("WAYLAND_DISPLAY", &self.socket);
        std::env::remove_var("DISPLAY");
    }

    fn log(&self) -> String {
        fs::read_to_string(&self.log_path).unwrap_or_default()
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.runtime_dir);
        eprintln!("compositor log left at {}", self.log_path.display());
    }
}

/// Wraps the real backend so the test can count how many writes actually
/// reached the compositor, which is what a sync loop would inflate.
struct CountingBackend {
    inner: ArboardBackend,
    writes: usize,
}

impl Backend for CountingBackend {
    fn read(&mut self, selection: Selection) -> Option<String> {
        self.inner.read(selection)
    }

    fn write(&mut self, selection: Selection, text: &str) -> Result<(), String> {
        self.writes += 1;
        self.inner.write(selection, text)
    }

    fn sleep(&mut self, duration: Duration) {
        self.inner.sleep(duration);
    }
}

/// Acts as the user: an independent clipboard connection, like `wl-copy`.
fn user_sets(selection: Selection, text: &str) {
    let mut user = ArboardBackend::new().expect("user clipboard connection");
    user.write(selection, text).expect("user write");

    // Wait for the compositor to actually hand out the new value, so the test
    // is not racing the write it just made.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if user.read(selection).as_deref() == Some(text) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the {} never took {text:?}", selection.name());
}

fn read_now(selection: Selection) -> String {
    let mut reader = ArboardBackend::new().expect("reader clipboard connection");
    reader.read(selection).unwrap_or_default()
}

fn expect_copied(action: Action, target: Selection, text: &str) {
    match action {
        Action::Copied {
            target: got_target,
            text: got_text,
            confirmed,
        } => {
            assert_eq!(got_target, target, "copied to the wrong selection");
            assert_eq!(got_text, text, "copied the wrong text");
            assert!(confirmed, "the compositor never confirmed the write");
        }
        other => panic!("expected a copy to the {}, got {other:?}", target.name()),
    }
}

#[test]
fn syncs_both_ways_against_a_real_compositor_without_looping() {
    let Some(compositor) = Compositor::start() else {
        eprintln!("skipping: sway is not installed");
        return;
    };
    compositor.attach();

    // A freshly started compositor has empty selections. If this test ever
    // attached to a logged-in session instead, it would find that session's
    // clipboard here and stop before disturbing it.
    assert_eq!(
        (
            read_now(Selection::Clipboard),
            read_now(Selection::Primary)
        ),
        (String::new(), String::new()),
        "expected the private compositor's selections to start empty"
    );

    let backend = CountingBackend {
        inner: ArboardBackend::new().expect("sync clipboard connection"),
        writes: 0,
    };
    let mut sync = ClipboardSync::new(backend);

    // A copy reaches the primary selection.
    user_sets(Selection::Clipboard, "from-clipboard");
    expect_copied(sync.tick(), Selection::Primary, "from-clipboard");
    assert_eq!(read_now(Selection::Primary), "from-clipboard");

    // A mouse selection reaches the clipboard.
    user_sets(Selection::Primary, "from-primary");
    expect_copied(sync.tick(), Selection::Clipboard, "from-primary");
    assert_eq!(read_now(Selection::Clipboard), "from-primary");

    // The real point of the exercise: once both sides agree, the engine's own
    // writes must not keep re-triggering each other.
    let writes_before = sync.backend_mut().writes;
    for _ in 0..20 {
        assert_eq!(
            sync.tick(),
            Action::InSync,
            "the sync loop kept writing after both sides agreed"
        );
    }
    assert_eq!(
        sync.backend_mut().writes,
        writes_before,
        "the sync loop wrote to the compositor while idle"
    );
    assert_eq!(read_now(Selection::Clipboard), "from-primary");
    assert_eq!(read_now(Selection::Primary), "from-primary");

    // Re-copying an earlier value is a real edit, not an echo of our own write.
    user_sets(Selection::Clipboard, "from-clipboard");
    expect_copied(sync.tick(), Selection::Primary, "from-clipboard");
    assert_eq!(read_now(Selection::Primary), "from-clipboard");

    for _ in 0..10 {
        assert_eq!(sync.tick(), Action::InSync);
    }
}
