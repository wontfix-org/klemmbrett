//! Starting the daemon from integration tests.
//!
//! The daemon is the Python package plus the `klemmbrett-py` extension. Set
//! `KLEMMBRETT_BIN` to an installed `klemmbrett` script to test that instead;
//! otherwise the extension is built and staged next to `python/klemmbrett`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// A command that runs `klemmbrett`; append the arguments.
pub fn daemon_command() -> Command {
    if let Some(bin) = std::env::var_os("KLEMMBRETT_BIN") {
        return Command::new(bin);
    }
    let staged = staged_package();
    let mut cmd = Command::new(python());
    cmd.env("PYTHONPATH", staged)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .args(["-m", "klemmbrett"]);
    cmd
}

fn python() -> String {
    std::env::var("PYO3_PYTHON").unwrap_or_else(|_| "python3".into())
}

fn staged_package() -> &'static Path {
    static STAGED: OnceLock<PathBuf> = OnceLock::new();
    STAGED.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"));
        let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .current_dir(root)
            .args(["build", "--quiet", "-p", "klemmbrett-py"])
            .status()
            .expect("run cargo build -p klemmbrett-py");
        assert!(status.success(), "cargo build -p klemmbrett-py failed");
        let library = target.join("debug").join("lib_core.so");
        let staged = target.join("test-python");
        let package = staged.join("klemmbrett");
        let _ = std::fs::remove_dir_all(&package);
        copy_tree(&root.join("python").join("klemmbrett"), &package);
        std::fs::copy(&library, package.join("_core.abi3.so"))
            .unwrap_or_else(|err| panic!("copy {}: {err}", library.display()));
        staged
    })
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create staged package dir");
    for entry in std::fs::read_dir(from).expect("read python package") {
        let entry = entry.expect("read python package entry");
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name != "__pycache__" {
                copy_tree(&path, &to.join(name));
            }
        } else if path.extension().is_some_and(|ext| ext == "py") {
            std::fs::copy(&path, to.join(name)).expect("copy python module");
        }
    }
}
