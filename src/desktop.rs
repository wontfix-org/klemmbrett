//! Activates a desktop application that sets `DBusActivatable=true`.
//!
//! The session bus name is the desktop-file id with `.desktop` removed.
//! `Activate` is sent to `org.freedesktop.Application` at the object path
//! formed by replacing `.` with `/`.

use std::collections::HashMap;

use zbus::zvariant::Value;

/// Asks the session bus to activate `app_id`. The caller launches `Exec`
/// when this fails.
pub fn activate(app_id: &str) -> Result<(), String> {
    let path = object_path(app_id)?;
    let connection =
        zbus::blocking::Connection::session().map_err(|err| format!("no session bus: {err}"))?;
    let platform_data: HashMap<&str, Value> = HashMap::new();
    connection
        .call_method(
            Some(app_id),
            path.as_str(),
            Some("org.freedesktop.Application"),
            "Activate",
            &(platform_data,),
        )
        .map_err(|err| format!("could not activate {app_id}: {err}"))?;
    Ok(())
}

/// `/org/gnome/Nautilus` for `org.gnome.Nautilus`.
pub fn object_path(app_id: &str) -> Result<String, String> {
    if app_id.len() > 255 || !app_id.contains('.') {
        return Err(format!("not a D-Bus application id: {app_id}"));
    }
    for element in app_id.split('.') {
        let mut chars = element.chars();
        let Some(first) = chars.next() else {
            return Err(format!("not a D-Bus application id: {app_id}"));
        };
        if !first.is_ascii_alphabetic() && first != '_' {
            return Err(format!("not a D-Bus application id: {app_id}"));
        }
        if chars.any(|ch| !ch.is_ascii_alphanumeric() && ch != '_') {
            return Err(format!("not a D-Bus application id: {app_id}"));
        }
    }
    Ok(format!("/{}", app_id.replace('.', "/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_id_becomes_an_object_path() {
        assert_eq!(
            object_path("org.gnome.Nautilus").unwrap(),
            "/org/gnome/Nautilus"
        );
    }

    #[test]
    fn a_hyphenated_id_is_not_a_bus_name() {
        assert!(object_path("foo-bar").is_err());
        assert!(object_path("htop").is_err());
    }
}
