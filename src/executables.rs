//! Executables found on `PATH`, and command lines typed into a run widget.

use crate::Record;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// First basename on `path` wins, then records are sorted by label.
pub fn executables_on_path(path: &str) -> Vec<Record> {
    let mut seen = HashSet::new();
    let mut records = Vec::new();
    for dir in path.split(':').filter(|dir| !dir.is_empty()) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.is_empty() || !seen.insert(name.to_owned()) {
                continue;
            }
            let file = entry.path();
            if !is_executable(&file) {
                continue;
            }
            records.push(Record {
                key: name.to_owned(),
                label: name.to_owned(),
                value: file.to_string_lossy().into_owned(),
                data: serde_json::Value::Null,
            });
        }
    }
    records.sort_by(|left, right| left.label.cmp(&right.label));
    records
}

/// `data.command` marks a typed command line, as opposed to a `PATH` basename.
pub fn is_command(record: &Record) -> bool {
    flag(&record.data, "command")
}

/// A command line offered for the current query, not yet remembered.
pub fn is_pending(record: &Record) -> bool {
    is_command(record) && flag(&record.data, "pending")
}

/// A command line loaded from history.
pub fn is_remembered(record: &Record) -> bool {
    is_command(record) && !is_pending(record)
}

fn flag(data: &Value, name: &str) -> bool {
    data.get(name).and_then(Value::as_bool) == Some(true)
}

fn command_record(line: &str, pending: bool) -> Record {
    let mut data = Map::new();
    data.insert("command".into(), Value::Bool(true));
    if pending {
        data.insert("pending".into(), Value::Bool(true));
    }
    Record {
        key: line.to_owned(),
        label: line.to_owned(),
        value: line.to_owned(),
        data: Value::Object(data),
    }
}

/// Adds history lines that are not already a row's label or key.
///
/// `firefox` is left out when a `PATH` row is labeled `firefox`.
/// `loginctl lock-session` is added beside `loginctl`.
pub fn merge_commands(entries: &mut Vec<Record>, commands: &[String]) {
    for command in commands {
        let command = command.trim();
        if command.is_empty()
            || entries
                .iter()
                .any(|record| record.key == command || record.label == command)
        {
            continue;
        }
        entries.push(command_record(command, false));
    }
}

/// Keeps one pending row for `query` when no label or key is exactly that text.
///
/// `firefox` is not added when a row is already labeled `firefox`.
/// `loginctl lock-session` is added. A blank query, or `enabled` false, adds nothing.
/// Leading and trailing whitespace is ignored: `  foot -e htop  ` is `foot -e htop`.
pub fn sync_query_command(entries: &mut Vec<Record>, query: &str, enabled: bool) {
    entries.retain(|record| !is_pending(record));
    if !enabled {
        return;
    }
    let line = query.trim();
    if line.is_empty()
        || entries
            .iter()
            .any(|record| record.label == line || record.key == line)
    {
        return;
    }
    entries.push(command_record(line, true));
}

/// Command lines among `picked` that are not exactly a `PATH` name.
///
/// `firefox` is skipped when a non-command row is labeled `firefox`.
/// `flatpak run org.foo` is kept.
pub fn commands_to_remember(entries: &[Record], picked: &[Record]) -> Vec<String> {
    let paths: HashSet<&str> = entries
        .iter()
        .filter(|record| !is_command(record))
        .flat_map(|record| [record.label.as_str(), record.key.as_str()])
        .collect();
    let mut kept = Vec::new();
    for record in picked {
        if !is_command(record) {
            continue;
        }
        let line = record.value.trim();
        if line.is_empty() || paths.contains(line) || kept.iter().any(|have| have == line) {
            continue;
        }
        kept.push(line.to_owned());
    }
    kept
}

/// Marks a `PATH` row or a command line to run in the configured terminal.
///
/// Other rows, such as a desktop application, are returned unchanged.
pub fn apply_terminal(record: Record) -> Record {
    if !(record.data.is_null() || is_command(&record)) {
        return record;
    }
    let mut data = match record.data {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    data.insert("terminal".into(), Value::Bool(true));
    Record {
        data: Value::Object(data),
        ..record
    }
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(c_path.as_ptr(), libc::X_OK) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn write_exec(dir: &Path, name: &str, mode: u32) {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn first_path_entry_wins_and_non_executables_are_skipped() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        write_exec(first.path(), "shared", 0o755);
        write_exec(second.path(), "shared", 0o755);
        write_exec(second.path(), "only-second", 0o755);
        write_exec(first.path(), "not-exec", 0o644);
        std::fs::create_dir(first.path().join("dirish")).unwrap();

        let path = format!("{}:{}", first.path().display(), second.path().display());
        let records = executables_on_path(&path);
        let names: Vec<_> = records.iter().map(|record| record.key.as_str()).collect();
        assert_eq!(names, vec!["only-second", "shared"]);
        assert_eq!(
            records[1].value,
            first.path().join("shared").to_string_lossy()
        );
    }

    #[test]
    fn a_typed_line_is_offered_unless_it_is_exactly_a_label() {
        let mut entries = vec![Record::new("loginctl"), Record::new("foot")];
        sync_query_command(&mut entries, "loginctl", true);
        assert_eq!(entries.len(), 2);

        sync_query_command(&mut entries, "  ", true);
        sync_query_command(&mut entries, "loginctl lock-session", false);
        assert_eq!(entries.len(), 2);

        sync_query_command(&mut entries, "  loginctl lock-session ", true);
        assert_eq!(entries.last().unwrap().label, "loginctl lock-session");
        assert!(is_pending(entries.last().unwrap()));

        sync_query_command(&mut entries, "foot -e htop", true);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries.last().unwrap().label, "foot -e htop");
        assert!(!entries.iter().any(|record| record.label.contains("lock-session")));
    }

    #[test]
    fn remembered_lines_join_path_names_without_duplicating_them() {
        let mut entries = vec![Record::new("firefox")];
        merge_commands(
            &mut entries,
            &[
                "firefox".into(),
                "  flatpak run org.foo ".into(),
                "flatpak run org.foo".into(),
                "   ".into(),
            ],
        );
        let labels: Vec<_> = entries.iter().map(|record| record.label.as_str()).collect();
        assert_eq!(labels, ["firefox", "flatpak run org.foo"]);
        assert!(is_remembered(&entries[1]));
        assert!(!is_pending(&entries[1]));
    }

    #[test]
    fn only_command_lines_that_are_not_path_names_are_stored() {
        let path = Record::new("firefox");
        let typed = command_record("firefox --private", true);
        let again = command_record("firefox", true);
        let flatpak = command_record("flatpak run org.foo", false);
        let picked = [path.clone(), typed.clone(), again, flatpak.clone(), typed];
        assert_eq!(
            commands_to_remember(&[path, flatpak.clone()], &picked),
            vec!["firefox --private".to_string(), "flatpak run org.foo".to_string()]
        );
    }

    #[test]
    fn terminal_marks_a_program_or_a_command_and_leaves_other_rows() {
        let program = apply_terminal(Record::new("/usr/bin/firefox"));
        assert_eq!(program.data["terminal"], serde_json::json!(true));
        assert!(program.data.get("command").is_none());

        let command = apply_terminal(command_record("htop", false));
        assert_eq!(command.data["terminal"], serde_json::json!(true));
        assert_eq!(command.data["command"], serde_json::json!(true));

        let app = Record::new("firefox.desktop").with_data(serde_json::json!({"argv": ["firefox"]}));
        assert_eq!(apply_terminal(app.clone()).data, app.data);
    }

    #[test]
    fn the_typed_line_sorts_with_the_other_matches() {
        let mut entries = vec![Record::new("loginctl")];
        sync_query_command(&mut entries, "loginctl lock-session", true);
        let ranking = crate::rank::Ranking::from_json(&serde_json::json!({
            "match": { "kind": "fuzzy" }
        }))
        .unwrap();
        let labels = |query: &str| {
            let order = crate::rank::order(
                &entries,
                query,
                &ranking,
                &[],
                &crate::rank::Priors::default(),
            );
            order
                .iter()
                .map(|index| entries[*index].label.as_str())
                .collect::<Vec<_>>()
        };
        // The second word does not occur in the program name, so only the typed line matches.
        assert_eq!(labels("loginctl lock-session"), ["loginctl lock-session"]);
        let partial = labels("login");
        assert!(partial.contains(&"loginctl"));
        assert!(partial.contains(&"loginctl lock-session"));
    }
}
