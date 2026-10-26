//! Run a widget transform and replace the selected value.

use crate::config::{interpolate_command_with, interpolate_text, Miss, Transform};
use regex::Regex;
use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

pub fn run_transform(
    transform: &Transform,
    key: &str,
    label: &str,
    value: &str,
) -> Result<String, String> {
    match transform {
        Transform::Exec {
            command,
            stdin,
            args,
        } => run_exec(command, stdin, args, key, label, value),
        Transform::Regex {
            pattern,
            replace,
            on_miss,
            args,
        } => run_regex(pattern, replace, *on_miss, args, key, label, value),
    }
}

fn run_exec(
    command: &[String],
    stdin: &str,
    extras: &BTreeMap<String, String>,
    key: &str,
    label: &str,
    value: &str,
) -> Result<String, String> {
    let argv = interpolate_command_with(command, key, label, value, extras);
    let stdin = interpolate_text(stdin, key, label, value, extras);
    run_waited_command(&argv, &stdin)
}

pub fn run_waited_command(argv: &[String], stdin: &str) -> Result<String, String> {
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err("command is empty".into());
    }
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to start {}: {err}", argv[0]))?;
    {
        let mut pipe = child
            .stdin
            .take()
            .ok_or_else(|| format!("{} stdin was not piped", argv[0]))?;
        pipe.write_all(stdin.as_bytes())
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

fn run_regex(
    pattern: &str,
    replace: &str,
    on_miss: Miss,
    extras: &BTreeMap<String, String>,
    key: &str,
    label: &str,
    value: &str,
) -> Result<String, String> {
    let pattern = interpolate_text(pattern, key, label, value, extras);
    let replace = interpolate_text(replace, key, label, value, extras);
    let re = Regex::new(&pattern).map_err(|err| format!("invalid regex: {err}"))?;
    if on_miss == Miss::Fail && !re.is_match(value) {
        return Err("regex did not match".into());
    }
    Ok(re.replace_all(value, replace.as_str()).into_owned())
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

    fn exec(command: &[&str]) -> Transform {
        Transform::Exec {
            command: command.iter().map(|part| (*part).to_owned()).collect(),
            stdin: "${value}".into(),
            args: BTreeMap::new(),
        }
    }

    #[test]
    fn stdout_replaces_the_value_and_drops_one_newline() {
        let extras = BTreeMap::from([("expr".into(), ".data".into())]);
        let out = run_transform(
            &Transform::Exec {
                command: vec![
                    "sh".into(),
                    "-c".into(),
                    "printf %s \"$1\"; cat >/dev/null".into(),
                    "sh".into(),
                    "${expr}".into(),
                ],
                stdin: "${value}".into(),
                args: extras,
            },
            "k",
            "Readable",
            "{\"data\":\"ignored\"}",
        )
        .unwrap();
        assert_eq!(out, ".data");
    }

    #[test]
    fn stdin_is_the_current_value() {
        let out = run_transform(&exec(&["cat"]), "k", "Readable", "from-stdin").unwrap();
        assert_eq!(out, "from-stdin");
    }

    #[test]
    fn stdin_template_is_interpolated() {
        let out = run_transform(
            &Transform::Exec {
                command: vec!["cat".into()],
                stdin: "${key}: ${value}".into(),
                args: BTreeMap::new(),
            },
            "k",
            "Readable",
            "from-stdin",
        )
        .unwrap();
        assert_eq!(out, "k: from-stdin");
    }

    #[test]
    fn a_failing_command_is_an_error() {
        let error = run_transform(
            &exec(&["sh", "-c", "cat >/dev/null; exit 1"]),
            "k",
            "Readable",
            "value",
        )
        .unwrap_err();
        assert!(error.contains("exited unsuccessfully"), "{error}");
    }

    #[test]
    fn regex_substitutes_a_named_capture() {
        let out = run_transform(
            &Transform::Regex {
                pattern: "https://[^/]+/browse/(?P<ticket>[A-Z]+-\\d+)".into(),
                replace: "${ticket}".into(),
                on_miss: Miss::Fail,
                args: BTreeMap::new(),
            },
            "k",
            "Readable",
            "see https://example.com/browse/ABC-12",
        )
        .unwrap();
        assert_eq!(out, "see ABC-12");
    }

    #[test]
    fn regex_miss_can_keep_the_value() {
        let out = run_transform(
            &Transform::Regex {
                pattern: "nope".into(),
                replace: "x".into(),
                on_miss: Miss::Keep,
                args: BTreeMap::new(),
            },
            "k",
            "Readable",
            "unchanged",
        )
        .unwrap();
        assert_eq!(out, "unchanged");
    }

    #[test]
    fn regex_miss_can_fail() {
        let error = run_transform(
            &Transform::Regex {
                pattern: "nope".into(),
                replace: "x".into(),
                on_miss: Miss::Fail,
                args: BTreeMap::new(),
            },
            "k",
            "Readable",
            "value",
        )
        .unwrap_err();
        assert!(error.contains("did not match"), "{error}");
    }

    #[test]
    fn regex_replace_interpolates_value() {
        let out = run_transform(
            &Transform::Regex {
                pattern: "id".into(),
                replace: "${value}-ok".into(),
                on_miss: Miss::Fail,
                args: BTreeMap::new(),
            },
            "k",
            "Readable",
            "id",
        )
        .unwrap();
        assert_eq!(out, "id-ok");
    }

    #[test]
    fn regex_pattern_interpolates_args() {
        let out = run_transform(
            &Transform::Regex {
                pattern: "${prefix}\\d+".into(),
                replace: "N".into(),
                on_miss: Miss::Fail,
                args: BTreeMap::from([("prefix".into(), "id-".into())]),
            },
            "k",
            "Readable",
            "id-12",
        )
        .unwrap();
        assert_eq!(out, "N");
    }
}
