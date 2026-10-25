//! How commands started for the user are launched.

use std::env;
use std::ffi::CString;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    Auto,
    Posix,
    SystemdRun,
    Wrapper(Vec<String>),
}

impl Default for Launch {
    fn default() -> Self {
        Self::Auto
    }
}

impl Launch {
    pub fn resolve(&self) -> ResolvedLaunch {
        match self {
            Self::Auto => {
                if running_as_systemd_unit() {
                    ResolvedLaunch::SystemdRun
                } else {
                    ResolvedLaunch::Posix
                }
            }
            Self::Posix => ResolvedLaunch::Posix,
            Self::SystemdRun => ResolvedLaunch::SystemdRun,
            Self::Wrapper(prefix) => ResolvedLaunch::Wrapper(prefix.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedLaunch {
    Posix,
    SystemdRun,
    Wrapper(Vec<String>),
}

impl ResolvedLaunch {
    pub fn argv(&self, program_and_args: &[&str]) -> Vec<String> {
        match self {
            Self::Posix => program_and_args
                .iter()
                .map(|part| (*part).to_owned())
                .collect(),
            Self::SystemdRun => {
                let mut argv = vec![
                    "systemd-run".into(),
                    "--user".into(),
                    "--scope".into(),
                    "--quiet".into(),
                    "--collect".into(),
                    "--".into(),
                ];
                argv.extend(program_and_args.iter().map(|part| (*part).to_owned()));
                argv
            }
            Self::Wrapper(prefix) => {
                let mut argv = prefix.clone();
                argv.extend(program_and_args.iter().map(|part| (*part).to_owned()));
                argv
            }
        }
    }
}

pub fn running_as_systemd_unit() -> bool {
    env::var_os("INVOCATION_ID").is_some_and(|value| !value.is_empty())
}

/// Double-fork + setsid, then exec `argv` with no shell.
pub fn spawn_detached(argv: &[String]) -> Result<(), String> {
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err("launch command is empty".into());
    }
    let c_args: Vec<CString> = argv
        .iter()
        .map(|part| {
            CString::new(part.as_str())
                .map_err(|_| format!("launch argument {part:?} contains an interior NUL"))
        })
        .collect::<Result<_, _>>()?;
    let program = c_args[0].clone();

    unsafe {
        let pid = libc::fork();
        if pid < 0 {
            return Err(format!("fork failed: {}", io::Error::last_os_error()));
        }
        if pid > 0 {
            let mut status = 0;
            if libc::waitpid(pid, &mut status, 0) < 0 {
                return Err(format!("waitpid failed: {}", io::Error::last_os_error()));
            }
            if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 {
                return Ok(());
            }
            return Err(format!(
                "failed to detach {}; check stderr for the exec error",
                argv[0]
            ));
        }

        if libc::setsid() < 0 {
            libc::_exit(1);
        }
        let child = libc::fork();
        if child < 0 {
            libc::_exit(1);
        }
        if child > 0 {
            libc::_exit(0);
        }

        let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        if devnull >= 0 {
            libc::dup2(devnull, libc::STDIN_FILENO);
            if devnull > libc::STDERR_FILENO {
                libc::close(devnull);
            }
        }

        let mut ptrs: Vec<*const libc::c_char> = c_args.iter().map(|arg| arg.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        libc::execvp(program.as_ptr(), ptrs.as_ptr());
        eprintln!(
            "klemmbrett: failed to exec {}: {}",
            Path::new(&argv[0]).display(),
            io::Error::last_os_error()
        );
        libc::_exit(127);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static INVOCATION_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn auto_is_systemd_run_when_invocation_id_is_set() {
        let _guard = INVOCATION_LOCK.lock().unwrap();
        let previous = env::var_os("INVOCATION_ID");
        env::set_var("INVOCATION_ID", "unit-test");
        assert_eq!(Launch::Auto.resolve(), ResolvedLaunch::SystemdRun);
        match previous {
            Some(value) => env::set_var("INVOCATION_ID", value),
            None => env::remove_var("INVOCATION_ID"),
        }
    }

    #[test]
    fn auto_is_posix_without_invocation_id() {
        let _guard = INVOCATION_LOCK.lock().unwrap();
        let previous = env::var_os("INVOCATION_ID");
        env::remove_var("INVOCATION_ID");
        assert_eq!(Launch::Auto.resolve(), ResolvedLaunch::Posix);
        if let Some(value) = previous {
            env::set_var("INVOCATION_ID", value);
        }
    }

    #[test]
    fn systemd_run_prefixes_the_scope_flags() {
        assert_eq!(
            ResolvedLaunch::SystemdRun.argv(&["/usr/bin/thunderbird"]),
            vec![
                "systemd-run",
                "--user",
                "--scope",
                "--quiet",
                "--collect",
                "--",
                "/usr/bin/thunderbird"
            ]
        );
    }

    #[test]
    fn wrapper_is_a_prefix() {
        assert_eq!(
            ResolvedLaunch::Wrapper(vec!["uwsm".into(), "app".into(), "--".into()])
                .argv(&["xdg-open", "https://example.com"]),
            vec!["uwsm", "app", "--", "xdg-open", "https://example.com"]
        );
    }

    #[test]
    fn posix_argv_is_the_program_only() {
        assert_eq!(
            ResolvedLaunch::Posix.argv(&["/bin/true"]),
            vec!["/bin/true"]
        );
    }

    #[test]
    fn posix_detach_leaves_the_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("ids");
        let script = dir.path().join("write-ids");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s %s %s' \"$$\" \"$PPID\" \"$(cat /proc/self/sessionid)\" >\"{}\"\n",
                out.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        spawn_detached(&[script.to_string_lossy().into_owned()]).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut text = String::new();
        while std::time::Instant::now() < deadline {
            if let Ok(contents) = std::fs::read_to_string(&out) {
                text = contents;
                if !text.is_empty() {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let parts: Vec<_> = text.split_whitespace().collect();
        assert_eq!(parts.len(), 3, "{text:?}");
        let ppid: i32 = parts[1].parse().unwrap();
        let sid: i32 = parts[2].trim().parse().unwrap();
        let us = std::process::id() as i32;
        let our_sid = unsafe { libc::getsid(0) };
        assert_ne!(ppid, us, "child should not keep klemmbrett as PPID");
        assert_ne!(sid, our_sid, "child should be in a new session");
    }
}
