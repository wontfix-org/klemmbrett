//! The logind session this daemon belongs to.
//!
//! Subscribes to `org.freedesktop.login1.Session` and keeps the latest
//! snapshot: whether the session is active, idle, or locked, and the
//! descriptive properties around it. `XDG_SESSION_ID` selects the session
//! when it is set; otherwise logind's graphical session for this user is used.

use std::collections::HashMap;
use std::sync::Mutex;

use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Message;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::MatchRule;

use crate::journal;
use crate::track;

static CURRENT: Mutex<Option<LoginSession>> = Mutex::new(None);

/// The session properties klemmbrett last read. `None` when tracking is off
/// or logind could not be probed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginSession {
    pub id: String,
    pub path: String,
    pub name: String,
    pub kind: String,
    pub class: String,
    pub desktop: String,
    pub display: String,
    pub remote: bool,
    pub remote_host: String,
    pub vtnr: u32,
    pub seat: String,
    pub active: bool,
    pub state: String,
    pub idle: bool,
    pub locked: bool,
}

impl LoginSession {
    fn new(id: String, path: String) -> Self {
        Self {
            id,
            path,
            name: String::new(),
            kind: String::new(),
            class: String::new(),
            desktop: String::new(),
            display: String::new(),
            remote: false,
            remote_host: String::new(),
            vtnr: 0,
            seat: String::new(),
            active: false,
            state: String::new(),
            idle: false,
            locked: false,
        }
    }

    fn apply(&mut self, properties: &HashMap<String, OwnedValue>) {
        for (name, value) in properties {
            match name.as_str() {
                "Id" => assign_string(&mut self.id, value),
                "Name" => assign_string(&mut self.name, value),
                "Type" => assign_string(&mut self.kind, value),
                "Class" => assign_string(&mut self.class, value),
                "Desktop" => assign_string(&mut self.desktop, value),
                "Display" => assign_string(&mut self.display, value),
                "RemoteHost" => assign_string(&mut self.remote_host, value),
                "State" => assign_string(&mut self.state, value),
                "Remote" => assign_bool(&mut self.remote, value),
                "Active" => assign_bool(&mut self.active, value),
                "IdleHint" => assign_bool(&mut self.idle, value),
                "LockedHint" => assign_bool(&mut self.locked, value),
                "VTNr" => assign_u32(&mut self.vtnr, value),
                "Seat" => {
                    if let Some(seat) = seat_id(value) {
                        self.seat = seat;
                    }
                }
                _ => {}
            }
        }
    }

    fn transitions(&self, next: &LoginSession) -> Vec<String> {
        let mut lines = Vec::new();
        if self.active != next.active {
            lines.push(format!(
                "login session {}: {}",
                self.id,
                if next.active { "active" } else { "inactive" }
            ));
        }
        if self.state != next.state {
            lines.push(format!("login session {}: state {}", self.id, next.state));
        }
        if self.idle != next.idle {
            lines.push(format!(
                "login session {}: {}",
                self.id,
                if next.idle { "idle" } else { "in use" }
            ));
        }
        if self.locked != next.locked {
            lines.push(format!(
                "login session {}: {}",
                self.id,
                if next.locked { "locked" } else { "unlocked" }
            ));
        }
        lines
    }
}

/// The latest snapshot, if tracking is running.
pub fn current() -> Option<LoginSession> {
    CURRENT.lock().ok().and_then(|slot| slot.clone())
}

/// Starts the subscription unless `requested` is `Some(false)`.
///
/// A missing session logs a warning when tracking was left unset, and aborts
/// startup when it was set to `true`.
pub fn start(requested: Option<bool>) -> Result<(), String> {
    if requested == Some(false) {
        return Ok(());
    }
    match setup() {
        Ok(ready) => ready.spawn(),
        Err(err) => track::finish(requested, "login session tracking", Err(err)),
    }
}

struct Ready {
    session: LoginSession,
    connection: Connection,
    signals: MessageIterator,
}

impl Ready {
    fn spawn(self) -> Result<(), String> {
        let Ready {
            session,
            connection,
            signals,
        } = self;
        let id = session.id.clone();
        store(&session);
        journal::info(&summary(&session));
        std::thread::Builder::new()
            .name("klemmbrett-session".into())
            .spawn(move || run(session, connection, signals))
            .map_err(|err| format!("failed to watch login session {id}: {err}"))?;
        Ok(())
    }
}

fn setup() -> Result<Ready, String> {
    let connection = Connection::system().map_err(|err| format!("no system bus: {err}"))?;
    let (id, path) = resolve(&connection)?;
    let properties = get_all(&connection, &path)?;
    let mut session = LoginSession::new(id, path.to_string());
    session.apply(&properties);
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.login1")
        .map_err(|err| format!("failed to subscribe to login session {}: {err}", session.id))?
        .path(path)
        .map_err(|err| format!("failed to subscribe to login session {}: {err}", session.id))?
        .build();
    let signals = MessageIterator::for_match_rule(rule, &connection, None)
        .map_err(|err| format!("failed to subscribe to login session {}: {err}", session.id))?;
    Ok(Ready {
        session,
        connection,
        signals,
    })
}

fn run(mut session: LoginSession, connection: Connection, signals: MessageIterator) {
    for message in signals {
        match message {
            Ok(message) => {
                if let Err(err) = on_signal(&mut session, &connection, &message) {
                    journal::warn(&format!("login session {}: {err}", session.id));
                }
            }
            Err(err) => {
                journal::warn(&format!(
                    "login session {} subscription ended: {err}",
                    session.id
                ));
                break;
            }
        }
    }
}

fn resolve(connection: &Connection) -> Result<(String, OwnedObjectPath), String> {
    if let Some(id) = std::env::var("XDG_SESSION_ID")
        .ok()
        .filter(|id| !id.is_empty())
    {
        let path = call::<OwnedObjectPath>(
            connection,
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
            "GetSession",
            &(id.as_str(),),
        )
        .map_err(|err| format!("logind has no session {id}: {err}"))?;
        return Ok((id, path));
    }
    let uid = unsafe { libc::getuid() };
    let user = call::<OwnedObjectPath>(
        connection,
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
        "GetUser",
        &(uid,),
    )
    .map_err(|err| format!("logind does not know uid {uid}: {err}"))?;
    let display = call::<OwnedValue>(
        connection,
        user.as_str(),
        "org.freedesktop.DBus.Properties",
        "Get",
        &("org.freedesktop.login1.User", "Display"),
    )
    .map_err(|err| format!("logind has no graphical session for uid {uid}: {err}"))?;
    let Some((id, path)) = user_display(&display) else {
        return Err(format!(
            "logind returned an unreadable Display for uid {uid}"
        ));
    };
    if id.is_empty() {
        return Err(format!(
            "uid {uid} has no graphical session; set XDG_SESSION_ID"
        ));
    }
    Ok((id, path))
}

/// `User.Display` is `(session id, session object path)`.
fn user_display(value: &OwnedValue) -> Option<(String, OwnedObjectPath)> {
    let Value::Structure(structure) = view(value) else {
        return None;
    };
    let fields = structure.fields();
    let Value::Str(id) = fields.first()? else {
        return None;
    };
    let Value::ObjectPath(path) = fields.get(1)? else {
        return None;
    };
    Some((
        id.as_str().to_owned(),
        OwnedObjectPath::try_from(path.as_str()).ok()?,
    ))
}

fn get_all(
    connection: &Connection,
    path: &OwnedObjectPath,
) -> Result<HashMap<String, OwnedValue>, String> {
    call(
        connection,
        path.as_str(),
        "org.freedesktop.DBus.Properties",
        "GetAll",
        &("org.freedesktop.login1.Session",),
    )
    .map_err(|err| format!("failed to read login session {path}: {err}"))
}

fn call<T>(
    connection: &Connection,
    path: &str,
    interface: &str,
    method: &str,
    body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
) -> Result<T, String>
where
    T: for<'de> zbus::zvariant::DynamicDeserialize<'de>,
{
    connection
        .call_method(
            Some("org.freedesktop.login1"),
            path,
            Some(interface),
            method,
            body,
        )
        .and_then(|reply| reply.body().deserialize())
        .map_err(|err| err.to_string())
}

fn on_signal(
    session: &mut LoginSession,
    connection: &Connection,
    message: &Message,
) -> Result<(), String> {
    let header = message.header();
    let interface = header.interface().map(|name| name.as_str()).unwrap_or("");
    let member = header.member().map(|name| name.as_str()).unwrap_or("");
    match (interface, member) {
        ("org.freedesktop.login1.Session", "Lock") => {
            apply_member(session, true);
            Ok(())
        }
        ("org.freedesktop.login1.Session", "Unlock") => {
            apply_member(session, false);
            Ok(())
        }
        ("org.freedesktop.DBus.Properties", "PropertiesChanged") => {
            let (interface, changed, invalidated): (
                String,
                HashMap<String, OwnedValue>,
                Vec<String>,
            ) = message
                .body()
                .deserialize()
                .map_err(|err| format!("PropertiesChanged: {err}"))?;
            if interface != "org.freedesktop.login1.Session" {
                return Ok(());
            }
            if !changed.is_empty() {
                note(session, &changed);
            }
            if !invalidated.is_empty() {
                refresh(session, connection, &invalidated)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn apply_member(session: &mut LoginSession, locked: bool) {
    let mut next = session.clone();
    next.locked = locked;
    commit(session, next);
}

fn note(session: &mut LoginSession, changed: &HashMap<String, OwnedValue>) {
    let mut next = session.clone();
    next.apply(changed);
    commit(session, next);
}

fn commit(session: &mut LoginSession, next: LoginSession) {
    for line in session.transitions(&next) {
        journal::info(&line);
    }
    *session = next;
    store(session);
}

fn refresh(
    session: &mut LoginSession,
    connection: &Connection,
    names: &[String],
) -> Result<(), String> {
    let mut changed = HashMap::new();
    for name in names {
        let value = call::<OwnedValue>(
            connection,
            &session.path,
            "org.freedesktop.DBus.Properties",
            "Get",
            &("org.freedesktop.login1.Session", name.as_str()),
        )
        .map_err(|err| format!("failed to read {name}: {err}"))?;
        changed.insert(name.clone(), value);
    }
    note(session, &changed);
    Ok(())
}

fn store(session: &LoginSession) {
    if let Ok(mut slot) = CURRENT.lock() {
        *slot = Some(session.clone());
    }
}

fn summary(session: &LoginSession) -> String {
    format!(
        "watching login session {id}: type={kind} class={class} desktop={desktop} display={display} seat={seat} state={state} active={active} idle={idle} locked={locked}",
        id = session.id,
        kind = session.kind,
        class = session.class,
        desktop = session.desktop,
        display = session.display,
        seat = session.seat,
        state = session.state,
        active = session.active,
        idle = session.idle,
        locked = session.locked,
    )
}

fn assign_string(slot: &mut String, value: &OwnedValue) {
    if let Value::Str(text) = view(value) {
        *slot = text.as_str().to_owned();
    }
}

fn assign_bool(slot: &mut bool, value: &OwnedValue) {
    if let Value::Bool(value) = view(value) {
        *slot = *value;
    }
}

fn assign_u32(slot: &mut u32, value: &OwnedValue) {
    if let Value::U32(value) = view(value) {
        *slot = *value;
    }
}

fn seat_id(value: &OwnedValue) -> Option<String> {
    let Value::Structure(structure) = view(value) else {
        return None;
    };
    match structure.fields().first()? {
        Value::Str(text) => Some(text.as_str().to_owned()),
        _ => None,
    }
}

/// `Properties.Get` returns one variant. A dict from `GetAll` is already the
/// inner value. Accept either shape.
fn view(value: &OwnedValue) -> &Value<'_> {
    match &**value {
        Value::Value(inner) => inner,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::{ObjectPath, Structure, Value};

    fn string(text: &str) -> OwnedValue {
        OwnedValue::try_from(Value::from(text)).unwrap()
    }

    fn seat(name: &str) -> OwnedValue {
        let path = ObjectPath::try_from("/org/freedesktop/login1/seat/seat0").unwrap();
        OwnedValue::try_from(Value::Structure(Structure::from((name, path)))).unwrap()
    }

    #[test]
    fn properties_fill_the_session_and_changes_are_named() {
        let mut session =
            LoginSession::new("2".into(), "/org/freedesktop/login1/session/_32".into());
        let properties = HashMap::from([
            ("Type".into(), string("wayland")),
            ("Class".into(), string("user")),
            ("Desktop".into(), string("sway")),
            ("Display".into(), string("wayland-1")),
            ("State".into(), string("active")),
            ("Active".into(), OwnedValue::from(true)),
            ("IdleHint".into(), OwnedValue::from(false)),
            ("LockedHint".into(), OwnedValue::from(false)),
            ("VTNr".into(), OwnedValue::from(1_u32)),
            ("Seat".into(), seat("seat0")),
            ("Name".into(), string("michael")),
        ]);
        session.apply(&properties);
        assert_eq!(session.kind, "wayland");
        assert_eq!(session.desktop, "sway");
        assert_eq!(session.display, "wayland-1");
        assert_eq!(session.seat, "seat0");
        assert_eq!(session.vtnr, 1);
        assert!(session.active);
        assert!(!session.idle);
        assert!(!session.locked);

        let mut next = session.clone();
        next.apply(&HashMap::from([
            ("IdleHint".into(), OwnedValue::from(true)),
            ("LockedHint".into(), OwnedValue::from(true)),
            ("State".into(), string("closing")),
        ]));
        assert_eq!(
            session.transitions(&next),
            [
                "login session 2: state closing".to_string(),
                "login session 2: idle".to_string(),
                "login session 2: locked".to_string(),
            ]
        );

        let active = session.active;
        session.apply(&HashMap::from([("Active".into(), string("not-a-bool"))]));
        assert_eq!(session.active, active);
    }

    #[test]
    fn lock_and_unlock_signals_update_the_hint() {
        let mut session = LoginSession::new("2".into(), "/session".into());
        apply_member(&mut session, true);
        assert!(session.locked);
        assert_eq!(current().unwrap().locked, true);
        apply_member(&mut session, true);
        apply_member(&mut session, false);
        assert!(!current().unwrap().locked);
    }

    #[test]
    fn a_missing_session_is_a_warning_until_tracking_is_requested() {
        assert!(track::finish(None, "login session tracking", Ok(())).is_ok());
        assert!(track::finish(None, "login session tracking", Err("no system bus".into())).is_ok());
        let err = track::finish(
            Some(true),
            "login session tracking",
            Err("no system bus".into()),
        )
        .unwrap_err();
        assert_eq!(
            err,
            "login session tracking was requested but is not available: no system bus"
        );
    }
}
