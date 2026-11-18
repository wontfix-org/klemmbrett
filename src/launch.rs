//! How commands started for the user are launched.
//!
//! Posix and `systemd-run --scope` are watched for about a second. An exec
//! failure is returned to the caller (`/no/such` is `No such file or
//! directory`). A process that then exits non-zero, or dies on a signal,
//! has its output logged (`/bin/false` printing `boom` and exiting 3).
//! Anything it prints after that window is discarded, and so is a clean
//! exit (`/bin/true`, or `notify-send` which exits 0). A wrapper is only
//! double-forked: `uwsm app -- firefox` exits as soon as Firefox has been
//! handed off, so its status is not Firefox's.

use std::env;
use std::ffi::CString;
use std::io;

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

    /// Starts `program_and_args` detached from the daemon.
    ///
    /// Posix and systemd-run log the program's output when it dies within
    /// about a second. A wrapper is detached without that watch.
    pub fn spawn(&self, program_and_args: &[&str]) -> Result<(), String> {
        let argv = self.argv(program_and_args);
        match self {
            Self::Wrapper(_) => spawn_orphaned(&argv),
            Self::Posix | Self::SystemdRun => spawn_detached(&argv),
        }
    }
}

pub fn running_as_systemd_unit() -> bool {
    env::var_os("INVOCATION_ID").is_some_and(|value| !value.is_empty())
}

/// How long a just-exec'd process is watched for a startup failure.
const STARTUP_GRACE_MS: i64 = 1000;

/// Bytes of startup output kept when that failure is logged.
const STARTUP_CAPTURE: usize = 16 * 1024;

/// Bytes of the command kept on the failure line.
const COMMAND_CAP: usize = 512;

// Status byte, then a native-endian errno when the byte is not 0.
// `0` — exec worked (`/bin/true` is running).
// `1` — exec failed (`/no/such` yields ENOENT).
// `2` — fork failed.
// `3` — setsid failed.
// `4` — pipe failed (`EMFILE`, "Too many open files").
const KIND_STARTED: u8 = 0;
const KIND_EXEC: u8 = 1;
const KIND_FORK: u8 = 2;
const KIND_SETSID: u8 = 3;
const KIND_PIPE: u8 = 4;

struct CloseFd(i32);

impl Drop for CloseFd {
    fn drop(&mut self) {
        if self.0 >= 0 {
            unsafe { libc::close(self.0) };
            self.0 = -1;
        }
    }
}

fn c_strings(argv: &[String]) -> Result<Vec<CString>, String> {
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err("launch command is empty".into());
    }
    argv.iter()
        .map(|part| {
            CString::new(part.as_str())
                .map_err(|_| format!("launch argument {part:?} contains an interior NUL"))
        })
        .collect()
}

fn pointers(args: &[CString]) -> Vec<*const libc::c_char> {
    let mut ptrs: Vec<*const libc::c_char> = args.iter().map(|arg| arg.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    ptrs
}

/// Double-fork + setsid, then exec `argv` with no shell.
///
/// Used for a wrapper. The wrapper's own exit is not the program's
/// (`uwsm app -- firefox` returns while Firefox is still starting), so
/// startup output is left on the inherited stderr.
fn spawn_orphaned(argv: &[String]) -> Result<(), String> {
    let c_args = c_strings(argv)?;
    let ptrs = pointers(&c_args);
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
        prepare_exec_signals();
        libc::execvp(program.as_ptr(), ptrs.as_ptr());
        eprintln!(
            "klemmbrett: failed to exec {}: {}",
            argv[0],
            io::Error::last_os_error()
        );
        libc::_exit(127);
    }
}

/// Exec `argv` in its own session and watch it for about a second.
///
/// Returns when exec has succeeded or failed. A non-zero exit inside the
/// grace window is logged afterwards, from the supervisor, without holding
/// the caller.
pub fn spawn_detached(argv: &[String]) -> Result<(), String> {
    let c_args = c_strings(argv)?;
    let ptrs = pointers(&c_args);
    let journal = env::var_os("JOURNAL_STREAM").is_some();
    let mut status_fds = [0i32; 2];
    if unsafe { libc::pipe2(status_fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(format!("pipe failed: {}", io::Error::last_os_error()));
    }
    let status_read = status_fds[0];
    let status_write = status_fds[1];

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        unsafe {
            libc::close(status_read);
            libc::close(status_write);
        }
        return Err(format!("fork failed: {}", io::Error::last_os_error()));
    }
    if pid == 0 {
        unsafe {
            libc::close(status_read);
            detach_and_supervise(status_write, &c_args, &ptrs, journal);
        }
    }

    unsafe { libc::close(status_write) };
    let _status_read = CloseFd(status_read);
    let mut status = 0;
    if unsafe { libc::waitpid(pid, &mut status, 0) } < 0 {
        return Err(format!("waitpid failed: {}", io::Error::last_os_error()));
    }
    read_launch_status(status_read, &argv[0])
}

/// Parses the supervisor status.
///
/// `0` means the program was exec'd (`/bin/true`). `1` plus an errno means
/// exec failed (`/no/such` → "No such file or directory"). `2` is a failed
/// fork, `3` a failed setsid, and `4` a failed pipe; each is followed by
/// the same errno.
fn read_launch_status(fd: i32, program: &str) -> Result<(), String> {
    wait_readable(fd)?;
    let mut kind = [0u8; 1];
    read_exact(fd, &mut kind)?;
    if kind[0] == KIND_STARTED {
        return Ok(());
    }
    let mut errno_buf = [0u8; 4];
    read_exact(fd, &mut errno_buf)?;
    let detail = io::Error::from_raw_os_error(i32::from_ne_bytes(errno_buf));
    Err(match kind[0] {
        KIND_EXEC => format!("failed to exec {program}: {detail}"),
        KIND_FORK => format!("fork failed: {detail}"),
        KIND_SETSID => format!("setsid failed: {detail}"),
        KIND_PIPE => format!("pipe failed: {detail}"),
        other => format!("launch failed with status {other}"),
    })
}

fn wait_readable(fd: i32) -> Result<(), String> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        let ready = unsafe { libc::poll(&mut pollfd, 1, 5_000) };
        if ready == 0 {
            return Err("timed out waiting for the program to start".into());
        }
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("waiting for the program to start failed: {err}"));
        }
        return Ok(());
    }
}

fn read_exact(fd: i32, buf: &mut [u8]) -> Result<(), String> {
    let mut got = 0;
    while got < buf.len() {
        let n = unsafe {
            libc::read(
                fd,
                buf[got..].as_mut_ptr().cast::<libc::c_void>(),
                buf.len() - got,
            )
        };
        if n == 0 {
            return Err("launch failed before the program started".into());
        }
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("reading launch status failed: {err}"));
        }
        got += n as usize;
    }
    Ok(())
}

/// Session leader exits immediately so the supervisor is not a child of
/// the daemon. The supervisor never returns: it is still the forked
/// multithreaded process, so it stays in libc and `_exit`.
unsafe fn detach_and_supervise(
    status_write: i32,
    args: &[CString],
    ptrs: &[*const libc::c_char],
    journal: bool,
) -> ! {
    if libc::setsid() < 0 {
        write_status(status_write, KIND_SETSID, errno());
        libc::_exit(1);
    }
    let supervisor = libc::fork();
    if supervisor < 0 {
        write_status(status_write, KIND_FORK, errno());
        libc::_exit(1);
    }
    if supervisor > 0 {
        libc::close(status_write);
        libc::_exit(0);
    }
    // The app replaces its own image. This process stays for the app's
    // lifetime to drain output, so name it apart from the daemon.
    libc::prctl(libc::PR_SET_NAME, c"kb-launch".as_ptr());
    supervise(status_write, args, ptrs, journal);
}

unsafe fn supervise(
    status_write: i32,
    args: &[CString],
    ptrs: &[*const libc::c_char],
    journal: bool,
) -> ! {
    let mut output = [0i32; 2];
    if libc::pipe2(output.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
        write_status(status_write, KIND_PIPE, errno());
        libc::_exit(1);
    }
    let mut exec_err = [0i32; 2];
    if libc::pipe2(exec_err.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
        write_status(status_write, KIND_PIPE, errno());
        libc::_exit(1);
    }

    let child = libc::fork();
    if child < 0 {
        write_status(status_write, KIND_FORK, errno());
        libc::_exit(1);
    }
    if child == 0 {
        libc::close(output[0]);
        libc::close(exec_err[0]);
        libc::close(status_write);
        exec_child(output[1], exec_err[1], ptrs);
    }
    libc::close(output[1]);
    libc::close(exec_err[1]);

    let mut errno_buf = [0u8; 4];
    let got = read_full(exec_err[0], &mut errno_buf);
    libc::close(exec_err[0]);
    if got == 4 {
        write_status(status_write, KIND_EXEC, i32::from_ne_bytes(errno_buf));
        libc::_exit(0);
    }
    write_status(status_write, KIND_STARTED, 0);
    libc::close(status_write);
    watch(child, output[0], args, journal);
}

unsafe fn exec_child(stdout_fd: i32, exec_err: i32, ptrs: &[*const libc::c_char]) -> ! {
    let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
    if devnull >= 0 {
        libc::dup2(devnull, libc::STDIN_FILENO);
        if devnull > libc::STDERR_FILENO {
            libc::close(devnull);
        }
    }
    libc::dup2(stdout_fd, libc::STDOUT_FILENO);
    libc::dup2(stdout_fd, libc::STDERR_FILENO);
    if stdout_fd > libc::STDERR_FILENO {
        libc::close(stdout_fd);
    }
    prepare_exec_signals();
    libc::execvp(ptrs[0], ptrs.as_ptr());
    let bytes = errno().to_ne_bytes();
    write_all(exec_err, &bytes);
    libc::_exit(127);
}

/// Rust ignores SIGPIPE so a closed pipe returns EPIPE. Restore the
/// default for the program, and unblock signals the daemon may have
/// blocked. Called only in the single-threaded child that is about to
/// exec.
unsafe fn prepare_exec_signals() {
    libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    let mut set = std::mem::zeroed::<libc::sigset_t>();
    libc::sigemptyset(&mut set);
    libc::pthread_sigmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
}

struct Captured {
    bytes: [u8; STARTUP_CAPTURE],
    len: usize,
    truncated: bool,
}

impl Captured {
    fn new() -> Self {
        Self {
            bytes: [0; STARTUP_CAPTURE],
            len: 0,
            truncated: false,
        }
    }

    fn output(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

unsafe fn watch(child: libc::pid_t, fd: i32, args: &[CString], journal: bool) -> ! {
    set_nonblock(fd);
    let mut captured = Captured::new();
    let started = mono_ms();
    loop {
        if let Some(status) = wait_hang(child) {
            finish(status, fd, &mut captured, args, journal);
        }
        let elapsed = mono_ms().saturating_sub(started);
        if elapsed >= STARTUP_GRACE_MS {
            break;
        }
        let left = (STARTUP_GRACE_MS - elapsed) as libc::c_int;
        match pump(fd, left, &mut captured) {
            Pump::Timeout => break,
            Pump::Eof => wait_rest_of_grace(child, fd, started, &mut captured, args, journal),
            Pump::Data => {}
        }
    }
    discard_until_dead(child, fd);
}

enum Pump {
    Data,
    Eof,
    Timeout,
}

unsafe fn wait_rest_of_grace(
    child: libc::pid_t,
    fd: i32,
    started: i64,
    captured: &mut Captured,
    args: &[CString],
    journal: bool,
) -> ! {
    loop {
        let _ = drain_into(fd, captured);
        if let Some(status) = wait_hang(child) {
            finish(status, fd, captured, args, journal);
        }
        let elapsed = mono_ms().saturating_sub(started);
        if elapsed >= STARTUP_GRACE_MS {
            discard_until_dead(child, fd);
        }
        let left = ((STARTUP_GRACE_MS - elapsed) as libc::c_int).min(50);
        libc::poll(std::ptr::null_mut(), 0, left);
    }
}

unsafe fn finish(
    status: i32,
    fd: i32,
    captured: &mut Captured,
    args: &[CString],
    journal: bool,
) -> ! {
    let _ = drain_into(fd, captured);
    if startup_failed(status) {
        log_failure(status, captured.output(), captured.truncated, args, journal);
    }
    libc::_exit(0);
}

fn startup_failed(status: i32) -> bool {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status) != 0
    } else {
        libc::WIFSIGNALED(status)
    }
}

/// The spawned pid has survived the grace window. Keep the pipe drained
/// so a later write does not block, and do not log any of it.
unsafe fn discard_until_dead(child: libc::pid_t, fd: i32) -> ! {
    let mut discard = [0u8; 4096];
    loop {
        if wait_hang(child).is_some() {
            libc::_exit(0);
        }
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // Wake on output or on the write end closing. A one-second cap
        // reaps a child that exited after forking a grandchild which still
        // holds the pipe (`daemon &`), instead of blocking in poll forever.
        let ready = libc::poll(&mut pollfd, 1, 1_000);
        if ready < 0 {
            if errno() == libc::EINTR {
                continue;
            }
            libc::_exit(0);
        }
        if ready == 0 {
            continue;
        }
        loop {
            let n = libc::read(
                fd,
                discard.as_mut_ptr().cast::<libc::c_void>(),
                discard.len(),
            );
            if n > 0 {
                continue;
            }
            if n == 0 || (pollfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)) != 0 {
                reap(child);
            }
            if errno() == libc::EINTR {
                continue;
            }
            break;
        }
    }
}

unsafe fn reap(child: libc::pid_t) -> ! {
    loop {
        let mut status = 0;
        let waited = libc::waitpid(child, &mut status, 0);
        if waited == child || (waited < 0 && errno() != libc::EINTR) {
            libc::_exit(0);
        }
    }
}

unsafe fn pump(fd: i32, timeout_ms: libc::c_int, captured: &mut Captured) -> Pump {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = libc::poll(&mut pollfd, 1, timeout_ms);
    if ready == 0 {
        return Pump::Timeout;
    }
    if ready < 0 {
        return if errno() == libc::EINTR {
            Pump::Data
        } else {
            Pump::Eof
        };
    }
    let eof = drain_into(fd, captured);
    let hung = pollfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL);
    if eof || hung != 0 {
        Pump::Eof
    } else {
        Pump::Data
    }
}

/// Reads what is already buffered. Returns whether the write end is gone.
unsafe fn drain_into(fd: i32, captured: &mut Captured) -> bool {
    let mut tmp = [0u8; 4096];
    loop {
        let n = libc::read(fd, tmp.as_mut_ptr().cast::<libc::c_void>(), tmp.len());
        if n == 0 {
            return true;
        }
        if n < 0 {
            let err = errno();
            if err == libc::EINTR {
                continue;
            }
            return err != libc::EAGAIN && err != libc::EWOULDBLOCK;
        }
        let mut rest = &tmp[..n as usize];
        if captured.len < captured.bytes.len() {
            let room = captured.bytes.len() - captured.len;
            let take = rest.len().min(room);
            captured.bytes[captured.len..captured.len + take].copy_from_slice(&rest[..take]);
            captured.len += take;
            rest = &rest[take..];
        }
        if !rest.is_empty() {
            captured.truncated = true;
        }
    }
}

unsafe fn wait_hang(pid: libc::pid_t) -> Option<i32> {
    loop {
        let mut status = 0;
        let waited = libc::waitpid(pid, &mut status, libc::WNOHANG);
        if waited == pid {
            return Some(status);
        }
        if waited == 0 {
            return None;
        }
        if errno() != libc::EINTR {
            return Some(0);
        }
    }
}

/// `klemmbrett: /bin/false failed to start (exit 1)`
/// `klemmbrett: /bin/false: boom`
///
/// With `JOURNAL_STREAM` set, each line is prefixed with `<4>`.
unsafe fn log_failure(
    status: i32,
    output: &[u8],
    truncated: bool,
    args: &[CString],
    journal: bool,
) {
    let fd = libc::STDERR_FILENO;
    write_prefix(fd, journal);
    write_command(fd, args);
    write_all(fd, b" failed to start (");
    if libc::WIFEXITED(status) {
        write_all(fd, b"exit ");
        write_u32(fd, libc::WEXITSTATUS(status) as u32);
    } else if libc::WIFSIGNALED(status) {
        write_all(fd, b"signal ");
        write_u32(fd, libc::WTERMSIG(status) as u32);
    } else {
        write_all(fd, b"status ");
        write_u32(fd, status as u32);
    }
    write_all(fd, b")\n");
    write_output(fd, output, args, journal);
    if truncated {
        write_prefix(fd, journal);
        if let Some(program) = args.first() {
            write_capped(fd, program.as_bytes(), 200);
        }
        write_all(fd, b": output truncated\n");
    }
}

unsafe fn write_output(fd: i32, output: &[u8], args: &[CString], journal: bool) {
    let mut start = 0usize;
    for (index, &byte) in output.iter().enumerate() {
        if byte != b'\n' {
            continue;
        }
        write_line(fd, &output[start..index], args, journal);
        start = index + 1;
    }
    if start < output.len() {
        write_line(fd, &output[start..], args, journal);
    }
}

unsafe fn write_line(fd: i32, mut line: &[u8], args: &[CString], journal: bool) {
    if line.last() == Some(&b'\r') {
        line = &line[..line.len() - 1];
    }
    if line.is_empty() {
        return;
    }
    write_prefix(fd, journal);
    if let Some(program) = args.first() {
        write_capped(fd, program.as_bytes(), 200);
    }
    write_all(fd, b": ");
    write_all(fd, line);
    write_all(fd, b"\n");
}

unsafe fn write_command(fd: i32, args: &[CString]) {
    let mut used = 0usize;
    for (index, arg) in args.iter().enumerate() {
        if used >= COMMAND_CAP {
            write_all(fd, b"...");
            return;
        }
        if index > 0 {
            write_all(fd, b" ");
            used += 1;
        }
        let bytes = arg.as_bytes();
        let room = COMMAND_CAP - used;
        if bytes.len() > room {
            write_all(fd, &bytes[..room]);
            write_all(fd, b"...");
            return;
        }
        write_all(fd, bytes);
        used += bytes.len();
    }
}

unsafe fn write_capped(fd: i32, bytes: &[u8], max: usize) {
    if bytes.len() > max {
        write_all(fd, &bytes[..max]);
        write_all(fd, b"...");
    } else {
        write_all(fd, bytes);
    }
}

unsafe fn write_prefix(fd: i32, journal: bool) {
    if journal {
        write_all(fd, b"<4>");
    }
    write_all(fd, b"klemmbrett: ");
}

unsafe fn write_u32(fd: i32, mut value: u32) {
    let mut buf = [0u8; 10];
    let mut index = buf.len();
    loop {
        index -= 1;
        buf[index] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    write_all(fd, &buf[index..]);
}

unsafe fn write_status(fd: i32, kind: u8, err: i32) {
    write_all(fd, &[kind]);
    if kind != KIND_STARTED {
        write_all(fd, &err.to_ne_bytes());
    }
}

unsafe fn write_all(fd: i32, mut buf: &[u8]) {
    while !buf.is_empty() {
        let n = libc::write(fd, buf.as_ptr().cast::<libc::c_void>(), buf.len());
        if n < 0 {
            if errno() == libc::EINTR {
                continue;
            }
            return;
        }
        if n == 0 {
            return;
        }
        buf = &buf[n as usize..];
    }
}

unsafe fn read_full(fd: i32, buf: &mut [u8]) -> usize {
    let mut got = 0;
    while got < buf.len() {
        let n = libc::read(
            fd,
            buf[got..].as_mut_ptr().cast::<libc::c_void>(),
            buf.len() - got,
        );
        if n == 0 {
            return got;
        }
        if n < 0 {
            if errno() == libc::EINTR {
                continue;
            }
            return got;
        }
        got += n as usize;
    }
    got
}

fn errno() -> i32 {
    unsafe { *libc::__errno_location() }
}

unsafe fn set_nonblock(fd: i32) {
    let flags = libc::fcntl(fd, libc::F_GETFL);
    if flags >= 0 {
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
}

unsafe fn mono_ms() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) != 0 {
        return 0;
    }
    ts.tv_sec * 1000 + ts.tv_nsec / 1_000_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Error;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard};
    use std::time::{Duration, Instant};

    static INVOCATION_LOCK: Mutex<()> = Mutex::new(());
    static STDERR_LOCK: Mutex<()> = Mutex::new(());

    /// Test output shares this process's stderr. Hold it across the whole
    /// capture so another test cannot dup over the pipe mid-read.
    struct Capture {
        saved: i32,
        read_fd: i32,
        write_fd: i32,
        buf: Vec<u8>,
        _lock: MutexGuard<'static, ()>,
    }

    impl Capture {
        fn install() -> Self {
            let lock = STDERR_LOCK.lock().unwrap_or_else(|err| err.into_inner());
            let mut fds = [0i32; 2];
            let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
            assert_eq!(rc, 0, "pipe: {}", Error::last_os_error());
            let saved = unsafe { libc::dup(libc::STDERR_FILENO) };
            assert!(saved >= 0, "dup stderr: {}", Error::last_os_error());
            let rc = unsafe { libc::dup2(fds[1], libc::STDERR_FILENO) };
            assert_eq!(
                rc,
                libc::STDERR_FILENO,
                "dup2 stderr: {}",
                Error::last_os_error()
            );
            let flags = unsafe { libc::fcntl(fds[0], libc::F_GETFL) };
            unsafe { libc::fcntl(fds[0], libc::F_SETFL, flags | libc::O_NONBLOCK) };
            Self {
                saved,
                read_fd: fds[0],
                write_fd: fds[1],
                buf: Vec::new(),
                _lock: lock,
            }
        }

        fn collect(&mut self) -> String {
            let mut tmp = [0u8; 4096];
            loop {
                let n = unsafe {
                    libc::read(
                        self.read_fd,
                        tmp.as_mut_ptr().cast::<libc::c_void>(),
                        tmp.len(),
                    )
                };
                if n > 0 {
                    self.buf.extend_from_slice(&tmp[..n as usize]);
                    continue;
                }
                break;
            }
            String::from_utf8_lossy(&self.buf).into_owned()
        }
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            unsafe {
                libc::dup2(self.saved, libc::STDERR_FILENO);
                libc::close(self.saved);
                libc::close(self.read_fd);
                libc::close(self.write_fd);
            }
        }
    }

    struct KillOnDrop(i32);

    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            unsafe { libc::kill(self.0, libc::SIGKILL) };
        }
    }

    fn write_script(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_str().unwrap().to_owned()
    }

    fn wait_text(cap: &mut Capture, timeout: Duration, ready: impl Fn(&str) -> bool) -> String {
        let started = Instant::now();
        loop {
            let text = cap.collect();
            if ready(&text) || started.elapsed() > timeout {
                return text;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

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

    #[test]
    fn a_missing_program_is_an_exec_error() {
        let err = spawn_detached(&["/no/such/klemmbrett-bin".into()]).unwrap_err();
        assert!(
            err.contains("failed to exec /no/such/klemmbrett-bin"),
            "{err}"
        );
        assert!(err.contains("No such file"), "{err}");
    }

    #[test]
    fn an_immediate_nonzero_exit_is_logged() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(
            dir.path(),
            "boom",
            "printf '%s\\n' kb-startup-boom-marker >&2\nexit 3",
        );
        let mut cap = Capture::install();
        spawn_detached(&[script.clone()]).unwrap();
        let text = wait_text(&mut cap, Duration::from_secs(2), |text| {
            text.contains("kb-startup-boom-marker")
        });
        drop(cap);
        assert!(text.contains("failed to start (exit 3)"), "{text}");
        assert!(text.contains("kb-startup-boom-marker"), "{text}");
        assert!(text.contains(&script), "{text}");
    }

    #[test]
    fn a_signal_during_startup_is_logged() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(
            dir.path(),
            "sig",
            "printf '%s\\n' kb-startup-signal-marker >&2\nkill -TERM $$",
        );
        let mut cap = Capture::install();
        spawn_detached(&[script]).unwrap();
        let text = wait_text(&mut cap, Duration::from_secs(2), |text| {
            text.contains("kb-startup-signal-marker")
        });
        drop(cap);
        assert!(
            text.contains("failed to start (signal 15)")
                || text.contains("failed to start (exit 143)"),
            "{text}"
        );
        assert!(text.contains("kb-startup-signal-marker"), "{text}");
    }

    #[test]
    fn a_clean_early_exit_is_not_logged() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(
            dir.path(),
            "ok",
            "printf '%s\\n' kb-startup-ok-marker >&2\nexit 0",
        );
        let mut cap = Capture::install();
        spawn_detached(&[script]).unwrap();
        std::thread::sleep(Duration::from_millis(1200));
        let text = cap.collect();
        drop(cap);
        assert!(!text.contains("kb-startup-ok-marker"), "{text}");
        assert!(!text.contains("failed to start"), "{text}");
    }

    #[test]
    fn a_program_that_stays_up_is_not_logged() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("pid");
        let script = write_script(
            dir.path(),
            "stay",
            "echo $$ > \"$1\"\nprintf '%s\\n' kb-startup-early-marker >&2\nexec sleep 30",
        );
        let mut cap = Capture::install();
        let started = Instant::now();
        spawn_detached(&[script, pid_path.to_str().unwrap().to_owned()]).unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(800),
            "launch waited for the program ({:?})",
            started.elapsed()
        );
        let pid = wait_pid(&pid_path);
        let _kill = KillOnDrop(pid);
        let parent = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
        let ppid: i32 = parent
            .lines()
            .find_map(|line| line.strip_prefix("PPid:"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let comm = std::fs::read_to_string(format!("/proc/{ppid}/comm")).unwrap();
        assert_eq!(comm.trim(), "kb-launch");
        std::thread::sleep(Duration::from_millis(1200));
        let text = cap.collect();
        drop(cap);
        assert!(!text.contains("kb-startup-early-marker"), "{text}");
        assert!(!text.contains("failed to start"), "{text}");
        assert!(
            Path::new(&format!("/proc/{pid}")).exists(),
            "program exited during the grace window"
        );
    }

    #[test]
    fn a_wrapper_is_not_watched() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(
            dir.path(),
            "wrap",
            "printf '%s\\n' kb-startup-wrap-marker >&2\nexit 3",
        );
        let mut cap = Capture::install();
        ResolvedLaunch::Wrapper(vec![]).spawn(&[&script]).unwrap();
        let text = wait_text(&mut cap, Duration::from_secs(2), |text| {
            text.contains("kb-startup-wrap-marker")
        });
        drop(cap);
        assert!(text.contains("kb-startup-wrap-marker"), "{text}");
        assert!(
            !text.contains("failed to start"),
            "wrapper output should stay on stderr, not a startup log: {text}"
        );
    }

    fn wait_pid(path: &Path) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(pid) = text.trim().parse::<i32>() {
                    if pid > 0 && Path::new(&format!("/proc/{pid}")).exists() {
                        return pid;
                    }
                }
            }
            if Instant::now() > deadline {
                panic!("pid file {} was not written", path.display());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
