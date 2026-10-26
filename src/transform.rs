//! Run a widget transform and replace the selected value with stdout.

use crate::config::interpolate_command_with;
use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

pub fn run_transform(
    command: &[String],
    extras: &BTreeMap<String, String>,
    key: &str,
    label: &str,
    value: &str,
) -> Result<String, String> {
    let argv = interpolate_command_with(command, key, label, value, extras);
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err("transform command is empty".into());
    }
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to start {}: {err}", argv[0]))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("{} stdin was not piped", argv[0]))?;
        stdin
            .write_all(value.as_bytes())
            .map_err(|err| format!("failed to write to {}: {err}", argv[0]))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|err| format!("failed to wait for {}: {err}", argv[0]))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        return Err(if stderr.is_empty() {
            format!("{} exited unsuccessfully", argv[0])
        } else {
            format!("{} exited unsuccessfully: {stderr}", argv[0])
        });
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|err| format!("{} stdout is not UTF-8: {err}", argv[0]))?;
    Ok(strip_one_trailing_newline(&stdout))
}

fn strip_one_trailing_newline(text: &str) -> String {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdout_replaces_the_value_and_drops_one_newline() {
        let extras = BTreeMap::from([("expr".into(), ".data".into())]);
        let out = run_transform(
            &[
                "sh".into(),
                "-c".into(),
                "printf %s \"$1\"; cat >/dev/null".into(),
                "sh".into(),
                "${expr}".into(),
            ],
            &extras,
            "k",
            "Readable",
            "{\"data\":\"ignored\"}",
        )
        .unwrap();
        assert_eq!(out, ".data");
    }

    #[test]
    fn stdin_is_the_current_value() {
        let out = run_transform(
            &["cat".into()],
            &BTreeMap::new(),
            "k",
            "Readable",
            "from-stdin",
        )
        .unwrap();
        assert_eq!(out, "from-stdin");
    }

    #[test]
    fn a_failing_command_is_an_error() {
        let error = run_transform(
            &["sh".into(), "-c".into(), "cat >/dev/null; exit 1".into()],
            &BTreeMap::new(),
            "k",
            "Readable",
            "value",
        )
        .unwrap_err();
        assert!(error.contains("exited unsuccessfully"), "{error}");
    }
}
