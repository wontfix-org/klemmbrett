//! `kb provision`: write the config each desktop integration needs.
//!
//! `systemd` installs the user units. `askpass` points `SSH_ASKPASS`,
//! `SUDO_ASKPASS`, and `GIT_ASKPASS` at `klemmbrett-askpass`. `pinentry`
//! points gpg-agent's `pinentry-program` at `klemmbrett-pinentry`.

use crate::systemd::with_home_token;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

const ASKPASS_FILE: &str = "askpass.conf";
const ASKPASS_PROGRAMS: &[&str] = &["SSH_ASKPASS", "SUDO_ASKPASS", "GIT_ASKPASS"];
const ASKPASS_REQUIRE: &str = "SSH_ASKPASS_REQUIRE";
const PINENTRY_FILE: &str = "gpg-agent.conf";
const PINENTRY_KEY: &str = "pinentry-program";

/// Writes `$XDG_CONFIG_HOME/environment.d/askpass.conf`.
pub fn askpass(exe: &[String]) -> Result<(), String> {
    let program = find_program(exe, "klemmbrett-askpass")?;
    let path = write_askpass(
        &environment_dir()?,
        &program,
        std::env::var_os("HOME").as_deref(),
    )?;
    println!("Updated {}", path.display());
    println!("SSH_ASKPASS, SUDO_ASKPASS, and GIT_ASKPASS apply at the next login.");
    Ok(())
}

/// Sets `pinentry-program` in `$GNUPGHOME/gpg-agent.conf` and restarts the agent.
pub fn pinentry(exe: &[String]) -> Result<(), String> {
    let program = find_program(exe, "klemmbrett-pinentry")?;
    let path = write_pinentry(&gnupg_dir()?, &program, std::env::var_os("HOME").as_deref())?;
    println!("Updated {}", path.display());
    reload_gpg_agent();
    Ok(())
}

fn write_askpass(dir: &Path, program: &Path, home: Option<&OsStr>) -> Result<PathBuf, String> {
    let program = with_home_token(path_text(program)?, home, "${HOME}");
    ensure_dir(dir)?;
    let path = dir.join(ASKPASS_FILE);
    let existing = read_optional(&path)?;
    write_text(&path, &render_askpass(&existing, &program), false)?;
    Ok(path)
}

fn write_pinentry(dir: &Path, program: &Path, home: Option<&OsStr>) -> Result<PathBuf, String> {
    let program = with_home_token(path_text(program)?, home, "~");
    ensure_private_dir(dir)?;
    let path = dir.join(PINENTRY_FILE);
    let existing = read_optional(&path)?;
    write_text(&path, &render_pinentry(&existing, &program), true)?;
    Ok(path)
}

/// `SSH_ASKPASS`, `SUDO_ASKPASS`, and `GIT_ASKPASS` are set to `program`.
/// `SSH_ASKPASS_REQUIRE=prefer` is added only when that line is absent, so a
/// value of `force` stays. Comments and other assignments stay put.
fn render_askpass(existing: &str, program: &str) -> String {
    let program = quote_value(program);
    let mut seen = HashSet::new();
    let mut require = false;
    let mut out = Vec::new();
    for line in split_lines(existing) {
        if let Some(key) = assignment_key(&line) {
            if ASKPASS_PROGRAMS.contains(&key) {
                if seen.insert(key.to_owned()) {
                    out.push(format!("{key}={program}"));
                }
                continue;
            }
            if key == ASKPASS_REQUIRE {
                require = true;
            }
        }
        out.push(line);
    }
    for key in [
        "SSH_ASKPASS",
        ASKPASS_REQUIRE,
        "SUDO_ASKPASS",
        "GIT_ASKPASS",
    ] {
        if key == ASKPASS_REQUIRE {
            if !require {
                out.push(format!("{key}=prefer"));
            }
            continue;
        }
        if !seen.contains(key) {
            out.push(format!("{key}={program}"));
        }
    }
    join_lines(&out)
}

/// Replaces every `pinentry-program` line with one pointing at `program`.
/// Other lines, including comments, stay in order.
fn render_pinentry(existing: &str, program: &str) -> String {
    let setting = format!("{PINENTRY_KEY} {}", quote_value(program));
    let mut written = false;
    let mut out = Vec::new();
    for line in split_lines(existing) {
        if is_pinentry_setting(&line) {
            if !written {
                out.push(setting.clone());
                written = true;
            }
            continue;
        }
        out.push(line);
    }
    if !written {
        out.push(setting);
    }
    join_lines(&out)
}

fn assignment_key(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let (key, _) = trimmed.split_once('=')?;
    let key = key.trim();
    if key.is_empty()
        || !key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }
    Some(key)
}

fn is_pinentry_setting(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return false;
    }
    trimmed.split_whitespace().next() == Some(PINENTRY_KEY)
}

/// Quotes a value the way systemd's `environment.d` and gpg's config parser
/// both accept: bare when it is a single token, double quotes otherwise.
fn quote_value(value: &str) -> String {
    let safe = value.chars().all(|ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':' | '+' | '@' | '~')
    });
    if safe {
        value.to_owned()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

fn split_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| line.trim_end_matches('\r').to_owned())
        .collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

fn join_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn find_program(exe: &[String], name: &str) -> Result<PathBuf, String> {
    find_program_in(exe, name, std::env::var_os("PATH").as_deref())
}

/// The helper installed next to `kb` wins, so the integration matches the
/// command that was just run. `python -m klemmbrett` has no sibling script
/// and falls through to `PATH`.
fn find_program_in(exe: &[String], name: &str, path: Option<&OsStr>) -> Result<PathBuf, String> {
    if let Some(found) = sibling_program(exe, name) {
        return absolute_path(found);
    }
    if let Some(path) = path {
        if let Some(found) = search_path(name, path) {
            return absolute_path(found);
        }
    }
    Err(format!("could not find {name} next to kb or on PATH"))
}

fn sibling_program(exe: &[String], name: &str) -> Option<PathBuf> {
    if exe.len() != 1 {
        return None;
    }
    let path = Path::new(&exe[0]);
    if path.file_name().and_then(|item| item.to_str()) != Some("kb") {
        return None;
    }
    let candidate = path.parent()?.join(name);
    executable(&candidate).then_some(candidate)
}

fn search_path(name: &str, path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path).find_map(|dir| {
        let candidate = dir.join(name);
        executable(&candidate).then_some(candidate)
    })
}

fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

fn absolute_path(path: PathBuf) -> Result<PathBuf, String> {
    std::path::absolute(&path).map_err(|err| format!("failed to resolve {}: {err}", path.display()))
}

fn path_text(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("path {} is not valid UTF-8", path.display()))
}

fn environment_dir() -> Result<PathBuf, String> {
    Ok(config_home(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )?
    .join("environment.d"))
}

fn config_home(xdg_config_home: Option<&OsStr>, home: Option<&OsStr>) -> Result<PathBuf, String> {
    if let Some(xdg) = xdg_config_home.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(xdg));
    }
    if let Some(home) = home.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home).join(".config"));
    }
    Err("HOME is not set; cannot locate the config directory".into())
}

fn gnupg_dir() -> Result<PathBuf, String> {
    gnupg_dir_from(
        std::env::var_os("GNUPGHOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

fn gnupg_dir_from(gnupghome: Option<&OsStr>, home: Option<&OsStr>) -> Result<PathBuf, String> {
    if let Some(dir) = gnupghome.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    match home.filter(|value| !value.is_empty()) {
        Some(home) => Ok(PathBuf::from(home).join(".gnupg")),
        None => Err("HOME is not set; cannot locate the GnuPG directory".into()),
    }
}

fn ensure_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|err| format!("failed to create {}: {err}", dir.display()))
}

/// GnuPG ignores a home directory that other users can read. A directory
/// we create is private; an existing one is left as the user set it.
fn ensure_private_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::fs::PermissionsExt;
    if dir.is_dir() {
        return Ok(());
    }
    if dir.exists() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    if let Some(parent) = dir.parent() {
        if !parent.as_os_str().is_empty() {
            ensure_dir(parent)?;
        }
    }
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(dir)
        .map_err(|err| format!("failed to create {}: {err}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|err| format!("failed to set permissions on {}: {err}", dir.display()))
}

fn read_optional(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(format!("failed to read {}: {err}", path.display())),
    }
}

fn write_text(path: &Path, contents: &str, private: bool) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let created = !path.exists();
    std::fs::write(path, contents)
        .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    if private && created {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|err| format!("failed to set permissions on {}: {err}", path.display()))?;
    }
    Ok(())
}

fn reload_gpg_agent() {
    match Command::new("gpgconf")
        .args(["--kill", "gpg-agent"])
        .status()
    {
        Ok(status) if status.success() => {
            println!("Stopped gpg-agent so it reloads pinentry-program.");
        }
        Ok(status) => eprintln!("Warning: gpgconf --kill gpg-agent exited with {status}"),
        Err(err) => eprintln!("Warning: failed to restart gpg-agent: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn exec_file(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, "").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn askpass_file_sets_the_three_programs_and_prefer() {
        let text = render_askpass("", "/usr/bin/klemmbrett-askpass");
        assert_eq!(
            text,
            "SSH_ASKPASS=/usr/bin/klemmbrett-askpass\n\
             SSH_ASKPASS_REQUIRE=prefer\n\
             SUDO_ASKPASS=/usr/bin/klemmbrett-askpass\n\
             GIT_ASKPASS=/usr/bin/klemmbrett-askpass\n"
        );
    }

    #[test]
    fn askpass_update_keeps_other_lines_and_an_existing_require() {
        let existing = "\
# kept\n\
SSH_ASKPASS=/old\n\
OTHER=1\n\
SSH_ASKPASS_REQUIRE=force\n\
SSH_ASKPASS=/older\n\
SUDO_ASKPASS = /also-old\n";
        let text = render_askpass(existing, "/usr/bin/klemmbrett-askpass");
        assert_eq!(
            text,
            "\
# kept\n\
SSH_ASKPASS=/usr/bin/klemmbrett-askpass\n\
OTHER=1\n\
SSH_ASKPASS_REQUIRE=force\n\
SUDO_ASKPASS=/usr/bin/klemmbrett-askpass\n\
GIT_ASKPASS=/usr/bin/klemmbrett-askpass\n"
        );
    }

    #[test]
    fn askpass_quotes_a_path_with_spaces() {
        let text = render_askpass("", "/home/me/my apps/klemmbrett-askpass");
        assert!(text.contains("SSH_ASKPASS=\"/home/me/my apps/klemmbrett-askpass\"\n"));
    }

    #[test]
    fn pinentry_appends_when_the_option_is_missing() {
        let text = render_pinentry("default-cache-ttl 600\n", "/usr/bin/klemmbrett-pinentry");
        assert_eq!(
            text,
            "default-cache-ttl 600\n\
             pinentry-program /usr/bin/klemmbrett-pinentry\n"
        );
    }

    #[test]
    fn pinentry_replaces_the_option_and_drops_a_duplicate() {
        let existing = "\
# pinentry-program /comment\n\
pinentry-program /usr/bin/pinentry\n\
default-cache-ttl 600\n\
pinentry-program /usr/bin/pinentry-gtk\n";
        let text = render_pinentry(existing, "/opt/klemmbrett-pinentry");
        assert_eq!(
            text,
            "\
# pinentry-program /comment\n\
pinentry-program /opt/klemmbrett-pinentry\n\
default-cache-ttl 600\n"
        );
    }

    #[test]
    fn pinentry_quotes_a_path_with_spaces() {
        let text = render_pinentry("", "/home/me/my apps/klemmbrett-pinentry");
        assert_eq!(
            text,
            "pinentry-program \"/home/me/my apps/klemmbrett-pinentry\"\n"
        );
    }

    #[test]
    fn program_next_to_kb_wins_over_path() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let other = dir.path().join("other");
        let kb = bin.join("kb");
        let sibling = bin.join("klemmbrett-askpass");
        let decoy = other.join("klemmbrett-askpass");
        exec_file(&kb);
        exec_file(&sibling);
        exec_file(&decoy);
        let found = find_program_in(
            &[kb.to_str().unwrap().to_owned()],
            "klemmbrett-askpass",
            Some(other.as_os_str()),
        )
        .unwrap();
        assert_eq!(found, std::path::absolute(&sibling).unwrap());
    }

    #[test]
    fn module_invocation_uses_path() {
        let dir = tempfile::tempdir().unwrap();
        let other = dir.path().join("other");
        let decoy = other.join("klemmbrett-pinentry");
        exec_file(&decoy);
        let found = find_program_in(
            &["/usr/bin/python3".into(), "-m".into(), "klemmbrett".into()],
            "klemmbrett-pinentry",
            Some(other.as_os_str()),
        )
        .unwrap();
        assert_eq!(found, std::path::absolute(&decoy).unwrap());
    }

    #[test]
    fn a_non_executable_sibling_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let other = dir.path().join("other");
        let kb = bin.join("kb");
        let sibling = bin.join("klemmbrett-askpass");
        let decoy = other.join("klemmbrett-askpass");
        exec_file(&kb);
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(&sibling, "").unwrap();
        std::fs::set_permissions(&sibling, std::fs::Permissions::from_mode(0o644)).unwrap();
        exec_file(&decoy);
        let found = find_program_in(
            &[kb.to_str().unwrap().to_owned()],
            "klemmbrett-askpass",
            Some(other.as_os_str()),
        )
        .unwrap();
        assert_eq!(found, std::path::absolute(&decoy).unwrap());
    }

    #[test]
    fn a_missing_program_is_an_error() {
        let err = find_program_in(
            &["/usr/bin/kb".into()],
            "klemmbrett-askpass",
            Some(OsStr::new("/nowhere")),
        )
        .unwrap_err();
        assert!(err.contains("klemmbrett-askpass"), "{err}");
    }

    #[test]
    fn config_home_follows_xdg_then_home() {
        assert_eq!(
            config_home(Some(OsStr::new("/xdg")), Some(OsStr::new("/home/me"))).unwrap(),
            PathBuf::from("/xdg")
        );
        assert_eq!(
            config_home(Some(OsStr::new("")), Some(OsStr::new("/home/me"))).unwrap(),
            PathBuf::from("/home/me/.config")
        );
        assert!(config_home(None, None).is_err());
    }

    #[test]
    fn gnupg_home_follows_the_environment_then_home() {
        assert_eq!(
            gnupg_dir_from(Some(OsStr::new("/keys")), Some(OsStr::new("/home/me"))).unwrap(),
            PathBuf::from("/keys")
        );
        assert_eq!(
            gnupg_dir_from(None, Some(OsStr::new("/home/me"))).unwrap(),
            PathBuf::from("/home/me/.gnupg")
        );
        assert!(gnupg_dir_from(None, None).is_err());
    }

    #[test]
    fn a_program_under_home_is_written_with_interpolation() {
        let home = OsStr::new("/home/me");
        let dir = tempfile::tempdir().unwrap();
        let askpass = write_askpass(
            dir.path(),
            Path::new("/home/me/.local/bin/klemmbrett-askpass"),
            Some(home),
        )
        .unwrap();
        let text = std::fs::read_to_string(askpass).unwrap();
        assert!(
            text.contains("SSH_ASKPASS=\"${HOME}/.local/bin/klemmbrett-askpass\"\n"),
            "{text}"
        );
        assert!(!text.contains("/home/me"), "{text}");

        let gnupg = dir.path().join("gnupg");
        let pinentry = write_pinentry(
            &gnupg,
            Path::new("/home/me/.local/bin/klemmbrett-pinentry"),
            Some(home),
        )
        .unwrap();
        let text = std::fs::read_to_string(pinentry).unwrap();
        assert_eq!(text, "pinentry-program ~/.local/bin/klemmbrett-pinentry\n");
    }

    #[test]
    fn a_longer_home_prefix_stays_absolute() {
        let home = OsStr::new("/home/me");
        let dir = tempfile::tempdir().unwrap();
        let gnupg = dir.path().join("gnupg");
        std::fs::create_dir(&gnupg).unwrap();
        let path = write_pinentry(
            &gnupg,
            Path::new("/home/me2/.local/bin/klemmbrett-pinentry"),
            Some(home),
        )
        .unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("pinentry-program /home/me2/.local/bin/klemmbrett-pinentry\n"));
    }

    #[test]
    fn askpass_file_is_written_under_environment_d() {
        let dir = tempfile::tempdir().unwrap();
        let path =
            write_askpass(dir.path(), Path::new("/usr/bin/klemmbrett-askpass"), None).unwrap();
        assert_eq!(path, dir.path().join("askpass.conf"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("SSH_ASKPASS=/usr/bin/klemmbrett-askpass\n"));
    }

    #[test]
    fn new_gnupg_dir_and_conf_are_private() {
        let dir = tempfile::tempdir().unwrap();
        let gnupg = dir.path().join("gnupg");
        let path = write_pinentry(&gnupg, Path::new("/usr/bin/klemmbrett-pinentry"), None).unwrap();
        let dir_mode = std::fs::metadata(&gnupg).unwrap().permissions().mode() & 0o777;
        let file_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("pinentry-program /usr/bin/klemmbrett-pinentry\n"));
    }

    #[test]
    fn pinentry_update_keeps_an_existing_file_mode() {
        let dir = tempfile::tempdir().unwrap();
        let gnupg = dir.path().join("gnupg");
        std::fs::create_dir(&gnupg).unwrap();
        let path = gnupg.join("gpg-agent.conf");
        std::fs::write(&path, "default-cache-ttl 600\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        write_pinentry(&gnupg, Path::new("/bin/klemmbrett-pinentry"), None).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("default-cache-ttl 600\n"));
        assert!(text.contains("pinentry-program /bin/klemmbrett-pinentry\n"));
    }
}
