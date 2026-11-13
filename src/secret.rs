//! Secret prompts: passwords, confirmations, and choices that agents ask for.
//!
//! An agent (the systemd ask-password watcher, the polkit agent, or a
//! `kb askpass` / `kb pinentry` client over Varlink) builds a
//! [`SecretRequest`]. [`SecretRoutes`] decides which widget shows it, and the
//! popup answers with a [`SecretReply`]. Answers go back to the agent only:
//! they never reach events, handlers, history, or usage.

use crate::journal;
use crate::layout::{Layout, Node, NodeKind};
use crate::theme::Location;
use crate::widget::{Handlers, WidgetDef};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use zeroize::Zeroizing;

/// Which protocol a question came in by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentKind {
    Systemd,
    Askpass,
    Pinentry,
    Polkit,
}

impl AgentKind {
    pub const ALL: [Self; 4] = [Self::Systemd, Self::Askpass, Self::Pinentry, Self::Polkit];

    pub fn name(self) -> &'static str {
        match self {
            Self::Systemd => "systemd",
            Self::Askpass => "askpass",
            Self::Pinentry => "pinentry",
            Self::Polkit => "polkit",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

/// How typed text is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Echo {
    /// Nothing at all, not even the length.
    No,
    /// One mask character per typed character.
    #[default]
    Masked,
    /// The text itself, for answers that are not secret.
    Yes,
}

impl Echo {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "no" => Some(Self::No),
            "masked" => Some(Self::Masked),
            "yes" => Some(Self::Yes),
            _ => None,
        }
    }
}

/// What the popup asks for.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Mode {
    /// Typed text, answered with [`SecretReply::Secret`].
    #[default]
    Secret,
    /// Enter or Escape, answered with [`SecretReply::Confirmed`].
    Confirm,
    /// Only a message; Enter dismisses it with [`SecretReply::Confirmed`].
    Message,
    /// One of these, answered with [`SecretReply::Chose`].
    Choose(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SecretRequest {
    pub kind: AgentKind,
    /// The widget the asker named, overriding the agent's own.
    pub widget: Option<String>,
    /// Identifies the question to its asker, such as `cryptsetup:/dev/sda2`.
    pub id: String,
    pub title: String,
    pub message: String,
    /// Replaces the prompt's text.
    pub prompt: Option<String>,
    /// Why the last answer was not accepted.
    pub error: Option<String>,
    /// Shown as `{details.<name>}`, and given to `route` functions.
    pub details: Vec<(String, String)>,
    pub echo: Echo,
    pub mode: Mode,
    /// Cancelled when no answer comes in time.
    pub timeout: Option<Duration>,
    /// The process asking, when known.
    pub pid: Option<u32>,
}

impl SecretRequest {
    pub fn new(kind: AgentKind) -> Self {
        Self {
            kind,
            widget: None,
            id: String::new(),
            title: String::new(),
            message: String::new(),
            prompt: None,
            error: None,
            details: Vec::new(),
            echo: Echo::Masked,
            mode: Mode::Secret,
            timeout: None,
            pid: None,
        }
    }

    pub fn detail(&self, name: &str) -> Option<&str> {
        self.details
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// What `route` functions see: everything but the answer.
    pub fn question(&self) -> Value {
        let details: Map<String, Value> = self
            .details
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect();
        json!({
            "kind": self.kind.name(),
            "id": self.id,
            "title": self.title,
            "message": self.message,
            "details": details,
            "pid": self.pid,
        })
    }

    /// Template fields for the popup: `title`, `message`, `error`, `prompt`,
    /// `id`, and `details.<name>`.
    pub fn fields(&self) -> Vec<(String, String)> {
        let mut fields = vec![
            ("title".to_owned(), self.title.clone()),
            ("message".to_owned(), self.message.clone()),
            ("error".to_owned(), self.error.clone().unwrap_or_default()),
            ("prompt".to_owned(), self.prompt.clone().unwrap_or_default()),
            ("id".to_owned(), self.id.clone()),
        ];
        fields.extend(
            self.details
                .iter()
                .map(|(key, value)| (format!("details.{key}"), value.clone())),
        );
        fields
    }
}

#[derive(Debug, PartialEq)]
pub enum SecretReply {
    Secret(Zeroizing<String>),
    Confirmed,
    Chose(usize),
    Cancelled,
}

/// A question handed to the popup; [`SecretTicket::wait`] blocks for the answer.
pub struct SecretTicket {
    pub id: u64,
    answer: mpsc::Receiver<SecretReply>,
}

impl SecretTicket {
    pub fn new(id: u64, answer: mpsc::Receiver<SecretReply>) -> Self {
        Self { id, answer }
    }

    /// The answer; a vanished popup counts as cancelled.
    pub fn wait(self) -> SecretReply {
        self.answer.recv().unwrap_or(SecretReply::Cancelled)
    }
}

/// What agents ask through: the daemon's runtime, or a test double.
pub trait SecretAsker: Send + Sync {
    /// Queues `request` and returns at once.
    fn begin(&self, request: SecretRequest) -> Result<SecretTicket, AskError>;
    /// Cancels a question that is no longer needed.
    fn withdraw(&self, id: u64);

    fn ask(&self, request: SecretRequest) -> Result<SecretReply, AskError> {
        Ok(self.begin(request)?.wait())
    }
}

/// Why a question could not be asked.
#[derive(Debug, Clone, PartialEq)]
pub enum AskError {
    /// No widget serves this kind of question.
    NoAgent(AgentKind),
    UnknownWidget(String),
    Failed(String),
}

impl std::fmt::Display for AskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAgent(kind) => write!(f, "no widget answers {} questions", kind.name()),
            Self::UnknownWidget(name) => write!(f, "unknown widget {name:?}"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

/// The widgets that answer each kind of question, and the config's `route`.
pub struct SecretRoutes {
    agents: Vec<(String, AgentKind, bool)>,
    widgets: HashSet<String>,
    handlers: Arc<dyn Handlers>,
}

impl SecretRoutes {
    pub fn new(defs: &[WidgetDef], handlers: Arc<dyn Handlers>) -> Self {
        Self {
            agents: defs
                .iter()
                .filter_map(|def| {
                    def.agent
                        .map(|agent| (def.name.clone(), agent.kind, agent.route))
                })
                .collect(),
            widgets: defs.iter().map(|def| def.name.clone()).collect(),
            handlers,
        }
    }

    pub fn serves(&self, kind: AgentKind) -> bool {
        self.agents.iter().any(|(_, agent, _)| *agent == kind)
    }

    /// The widget to show `request` in: the one the asker named, else the
    /// agent's widget, or where its `route` sends it.
    pub fn target(&self, request: &SecretRequest) -> Result<String, AskError> {
        if let Some(widget) = &request.widget {
            return if self.widgets.contains(widget) {
                Ok(widget.clone())
            } else {
                Err(AskError::UnknownWidget(widget.clone()))
            };
        }
        let (name, _, route) = self
            .agents
            .iter()
            .find(|(_, kind, _)| *kind == request.kind)
            .ok_or(AskError::NoAgent(request.kind))?;
        if *route {
            match self.handlers.route_secret(name, request.question()) {
                Some(target) if self.widgets.contains(&target) => return Ok(target),
                Some(target) => journal::error(&format!(
                    "widget {name:?}: route returned unknown widget {target:?}"
                )),
                None => {}
            }
        }
        Ok(name.clone())
    }
}

/// An agent widget's wire form: which questions it answers, and whether its
/// `route` function picks the widget per question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentSpec {
    pub kind: AgentKind,
    pub route: bool,
}

/// The built-in card for `request`: the title, message, and error that are
/// set, then the input bar, or the choices as rows.
pub fn default_layout(request: &SecretRequest) -> Layout {
    let mut children: Vec<Node> = [("{title}", &request.title), ("{message}", &request.message)]
        .into_iter()
        .chain(request.error.as_ref().map(|error| ("{error}", error)))
        .filter(|(_, text)| !text.is_empty())
        .map(|(template, _)| Node::text(template))
        .collect();
    match &request.mode {
        Mode::Choose(_) => children.push(Node::new(
            NodeKind::ListView,
            vec![Node::new(NodeKind::Element, vec![Node::text("{label}")])],
        )),
        Mode::Message => {}
        Mode::Secret | Mode::Confirm => children.push(Node::new(
            NodeKind::InputBar,
            vec![Node::leaf(NodeKind::Prompt), Node::leaf(NodeKind::Entry)],
        )),
    }
    if children.is_empty() {
        children.push(Node::text("{id}"));
    }
    Layout::new(
        Node::new(NodeKind::Window, children),
        Some(Location::Center),
    )
    .expect("secret layout")
}

/// What the entry shows for `typed`.
pub fn entry_text(request: &SecretRequest, typed: &str, mask: Option<&str>) -> String {
    match request.mode {
        Mode::Confirm => "Enter to confirm, Escape to cancel".to_owned(),
        Mode::Message | Mode::Choose(_) => String::new(),
        Mode::Secret => match request.echo {
            Echo::No => String::new(),
            Echo::Masked => mask.unwrap_or("•").repeat(typed.chars().count()),
            Echo::Yes => typed.to_owned(),
        },
    }
}

/// Undoes GLib's `g_strescape`, which polkit's helper applies to every line.
pub fn unescape_c(text: &str) -> String {
    let mut out = Vec::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        index += 1;
        if byte != b'\\' || index == bytes.len() {
            out.push(byte);
            continue;
        }
        let escaped = bytes[index];
        index += 1;
        match escaped {
            b'0'..=b'7' => {
                let mut value = u32::from(escaped - b'0');
                for _ in 0..2 {
                    match bytes.get(index) {
                        Some(digit @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(digit - b'0');
                            index += 1;
                        }
                        _ => break,
                    }
                }
                out.push(value as u8);
            }
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'v' => out.push(0x0b),
            other => out.push(other),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::{NoHandlers, Record};
    use std::sync::Mutex;

    struct Router(Mutex<Vec<Value>>, Option<&'static str>);

    impl Handlers for Router {
        fn select(&self, _widget: &str, _records: Vec<Record>) {}
        fn highlight(&self, _widget: &str, _record: Record) {}
        fn route_secret(&self, _widget: &str, question: Value) -> Option<String> {
            self.0.lock().unwrap().push(question);
            self.1.map(str::to_owned)
        }
    }

    fn agent(name: &str, kind: AgentKind, route: bool) -> WidgetDef {
        let mut def = WidgetDef::new(name, None);
        def.agent = Some(AgentSpec { kind, route });
        def
    }

    #[test]
    fn questions_go_to_the_agent_widget_or_where_route_sends_them() {
        let defs = vec![
            agent("admin", AgentKind::Polkit, true),
            agent("ssh", AgentKind::Askpass, false),
            WidgetDef::new("mount", None),
        ];
        let mut request = SecretRequest::new(AgentKind::Polkit);
        request.details.push((
            "action_id".into(),
            "org.freedesktop.udisks2.filesystem-mount".into(),
        ));

        let router = Arc::new(Router(Mutex::new(Vec::new()), Some("mount")));
        let routes = SecretRoutes::new(&defs, router.clone());
        assert_eq!(routes.target(&request).unwrap(), "mount");
        let question = &router.0.lock().unwrap()[0];
        assert_eq!(question["kind"], "polkit");
        assert_eq!(
            question["details"]["action_id"],
            "org.freedesktop.udisks2.filesystem-mount"
        );

        let routes = SecretRoutes::new(
            &defs,
            Arc::new(Router(Mutex::new(Vec::new()), Some("nope"))),
        );
        assert_eq!(routes.target(&request).unwrap(), "admin");

        let routes = SecretRoutes::new(&defs, Arc::new(NoHandlers));
        assert_eq!(
            routes
                .target(&SecretRequest::new(AgentKind::Askpass))
                .unwrap(),
            "ssh"
        );
        assert_eq!(
            routes.target(&SecretRequest::new(AgentKind::Pinentry)),
            Err(AskError::NoAgent(AgentKind::Pinentry))
        );
        let mut named = SecretRequest::new(AgentKind::Pinentry);
        named.widget = Some("mount".into());
        assert_eq!(routes.target(&named).unwrap(), "mount");
        named.widget = Some("gone".into());
        assert_eq!(
            routes.target(&named),
            Err(AskError::UnknownWidget("gone".into()))
        );
    }

    #[test]
    fn fields_expose_details_for_templates() {
        let mut request = SecretRequest::new(AgentKind::Polkit);
        request.title = "Authenticate".into();
        request
            .details
            .push(("command_line".into(), "/usr/bin/true".into()));
        let fields = request.fields();
        assert!(fields.contains(&("title".into(), "Authenticate".into())));
        assert!(fields.contains(&("details.command_line".into(), "/usr/bin/true".into())));
    }

    #[test]
    fn the_default_card_leaves_out_empty_lines() {
        let mut request = SecretRequest::new(AgentKind::Systemd);
        request.message = "Passphrase for backup".into();
        let layout = default_layout(&request);
        assert!(layout.has_inputbar() && !layout.has_listview());
        assert_eq!(layout.root().children.len(), 2);
        request.mode = Mode::Choose(vec!["root".into(), "admin".into()]);
        request.error = Some("Try again".into());
        let layout = default_layout(&request);
        assert!(layout.has_listview() && !layout.has_inputbar());
        assert_eq!(layout.root().children.len(), 3);
    }

    #[test]
    fn the_entry_masks_by_echo_mode() {
        let mut request = SecretRequest::new(AgentKind::Askpass);
        assert_eq!(entry_text(&request, "hunter2", None), "•••••••");
        assert_eq!(entry_text(&request, "ab", Some("*")), "**");
        request.echo = Echo::No;
        assert_eq!(entry_text(&request, "hunter2", None), "");
        request.echo = Echo::Yes;
        assert_eq!(entry_text(&request, "yes", None), "yes");
    }

    #[test]
    fn glib_escapes_are_undone() {
        assert_eq!(unescape_c(r"Password: "), "Password: ");
        assert_eq!(unescape_c(r#"a\nb\t\\c\"d"#), "a\nb\t\\c\"d");
        assert_eq!(unescape_c(r"\303\244"), "ä");
        assert_eq!(unescape_c("trailing\\"), "trailing\\");
    }
}
