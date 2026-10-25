//! Executables found on `PATH`.

use crate::Record;
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
            });
        }
    }
    records.sort_by(|left, right| left.label.cmp(&right.label));
    records
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
}
