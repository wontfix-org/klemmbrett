//! Behaviors a list popup runs around its own keys.
//!
//! A behavior sees the open list, not the shared widget definition. Marks and
//! anything else that exists only while the popup is open live in [`ListState`]
//! slots, one per behavior, indexed in the same order as the behavior list.

use std::any::Any;
use std::sync::Arc;

use crate::config::{KeySpec, MultiSelect};
use crate::{activation_records, toggle_record, Record};

/// What a behavior did with a key. `Pass` lets the next behavior, then the
/// list's own keys, see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Pass,
    Consume,
}

/// A key the popup received. `tab` with no modifiers is select for the
/// default multi-select binding; `shift-tab` is deselect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPress {
    pub key: String,
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub platform: bool,
}

impl KeyPress {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            shift: false,
            control: false,
            alt: false,
            platform: false,
        }
    }

    pub fn shift(mut self) -> Self {
        self.shift = true;
        self
    }
}

/// Work the list applies after a behavior returns: change the rows, submit,
/// or ask the daemon to forget a command. Behaviors mutate [`ListState`]
/// themselves for selection and marks.
#[derive(Debug, Default)]
pub struct Effects {
    pub close: bool,
    pub submit: Option<Vec<Record>>,
    /// Submit the highlighted row, or the marked rows, the way Enter does.
    pub submit_selection: bool,
    pub remove_keys: Vec<String>,
    pub set_query: Option<String>,
    pub refresh: bool,
    pub calls: Vec<AppCall>,
}

/// Something only the daemon can do, queued by a behavior or the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCall {
    ForgetCommand {
        widget: String,
        command: String,
    },
    RememberCommands {
        widget: String,
        commands: Vec<String>,
    },
}

/// What [`Effects::apply`] changed on the list. The popup still has to
/// redraw, scroll, and run `calls`.
#[derive(Debug)]
pub struct Applied {
    pub refilter: bool,
    pub close: bool,
    pub submit: Option<Vec<Record>>,
    pub submit_selection: bool,
    pub refresh: bool,
    pub calls: Vec<AppCall>,
}

impl Effects {
    pub fn apply(&mut self, list: &mut ListState) -> Applied {
        let mut refilter = false;
        if let Some(query) = self.set_query.take() {
            list.query = query;
            refilter = true;
        }
        if !self.remove_keys.is_empty() {
            list.remove_keys(&self.remove_keys);
            self.remove_keys.clear();
            refilter = true;
        }
        Applied {
            refilter,
            close: self.close,
            submit: self.submit.take(),
            submit_selection: self.submit_selection,
            refresh: self.refresh,
            calls: std::mem::take(&mut self.calls),
        }
    }
}

/// The rows, the query, and per-behavior slots for one open list.
#[derive(Debug)]
pub struct ListState {
    /// The widget this list was opened for. Command history is stored under it.
    pub widget: String,
    pub entries: Vec<Record>,
    pub query: String,
    /// Indices into `entries` that match `query`, in the order shown.
    pub order: Vec<usize>,
    pub selected: usize,
    slots: Vec<Option<Box<dyn Any + Send>>>,
}

impl ListState {
    pub fn new(entries: Vec<Record>, query: String) -> Self {
        Self {
            widget: String::new(),
            entries,
            query,
            order: Vec::new(),
            selected: 0,
            slots: Vec::new(),
        }
    }

    pub fn init_slot<T: Default + Send + 'static>(&mut self, index: usize) {
        if index >= self.slots.len() {
            self.slots.resize_with(index + 1, || None);
        }
        if self.slots[index].is_none() {
            self.slots[index] = Some(Box::new(T::default()));
        }
    }

    pub fn slot<T: Send + 'static>(&self, index: usize) -> Option<&T> {
        self.slots.get(index)?.as_ref()?.downcast_ref()
    }

    pub fn slot_mut<T: Send + 'static>(&mut self, index: usize) -> Option<&mut T> {
        self.slots.get_mut(index)?.as_mut()?.downcast_mut()
    }

    pub fn visible(&self) -> Vec<&Record> {
        self.order
            .iter()
            .map(|index| &self.entries[*index])
            .collect()
    }

    pub fn move_selection(&mut self, delta: i32) {
        let count = self.order.len() as i32;
        if count == 0 {
            return;
        }
        self.selected = (self.selected as i32 + delta).rem_euclid(count) as usize;
    }

    /// Keeps marks whose keys are still in `entries`, using the new record.
    pub fn retain_marks(&mut self, slot: usize) {
        let Some(marks) = self.slot::<Vec<Record>>(slot) else {
            return;
        };
        let kept: Vec<Record> = marks
            .iter()
            .filter_map(|marked| self.entries.iter().find(|record| record.key == marked.key))
            .cloned()
            .collect();
        if let Some(marks) = self.slot_mut::<Vec<Record>>(slot) {
            *marks = kept;
        }
    }

    /// Drops rows, and any `Vec<Record>` slot entries, whose key is in `keys`.
    pub fn remove_keys(&mut self, keys: &[String]) {
        self.entries
            .retain(|entry| !keys.iter().any(|key| key == &entry.key));
        for slot in &mut self.slots {
            if let Some(marks) = slot
                .as_mut()
                .and_then(|value| value.downcast_mut::<Vec<Record>>())
            {
                marks.retain(|entry| !keys.iter().any(|key| key == &entry.key));
            }
        }
    }
}

pub trait Behavior: Send + Sync {
    /// Background thread, before the popup opens.
    fn on_load(&self, _widget: &str, _entries: &mut Vec<Record>) {}

    /// After the query or the rows change, before they are ranked.
    fn on_query(&self, _list: &mut ListState) {}

    fn on_key(&self, _key: &KeyPress, _list: &mut ListState, _fx: &mut Effects) -> Flow {
        Flow::Pass
    }

    /// The rows about to be submitted. May replace them or record them.
    fn on_submit(&self, _list: &ListState, _picked: &mut Vec<Record>, _fx: &mut Effects) {}

    /// Layouts still receive `multi` in `LayoutContext`.
    fn marks_rows(&self) -> bool {
        false
    }
}

/// How a widget asks for a behavior. The `multi` wire key becomes
/// [`BehaviorSpec::Multi`]; `tab` selects and `shift-tab` deselects unless
/// the widget named other keys. The `run_query` wire key becomes
/// [`BehaviorSpec::RunCommands`], bound to Shift-Delete and Shift-Enter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BehaviorSpec {
    Multi { select: KeySpec, deselect: KeySpec },
    RunCommands { forget: KeySpec, terminal: KeySpec },
}

impl BehaviorSpec {
    pub fn marks_rows(&self) -> bool {
        matches!(self, Self::Multi { .. })
    }

    pub fn runs_commands(&self) -> bool {
        matches!(self, Self::RunCommands { .. })
    }

    pub fn multi_default() -> Self {
        let keys = MultiSelect::fzf();
        Self::Multi {
            select: keys.select,
            deselect: keys.deselect,
        }
    }

    /// Shift-Delete forgets a remembered line. Shift-Enter runs it in a terminal.
    pub fn run_default() -> Self {
        Self::RunCommands {
            forget: KeySpec::parse("shift-delete").expect("shift-delete"),
            terminal: KeySpec::parse("shift-enter").expect("shift-enter"),
        }
    }

    /// `Some(true)` adds the default multi-select keys when none are set.
    /// `Some(false)` removes multi-select. `None` leaves the list alone.
    pub fn apply_picker_choice(behaviors: &mut Vec<Self>, choice: Option<bool>) {
        match choice {
            Some(true) if !behaviors.iter().any(Self::marks_rows) => {
                behaviors.push(Self::multi_default());
            }
            Some(false) => behaviors.retain(|spec| !spec.marks_rows()),
            _ => {}
        }
    }

    pub fn build(self, slot: usize) -> Arc<dyn Behavior> {
        match self {
            Self::Multi { select, deselect } => Arc::new(Multi {
                select,
                deselect,
                slot,
            }),
            Self::RunCommands { forget, terminal } => Arc::new(RunCommands {
                forget,
                terminal,
                slot,
            }),
        }
    }
}

/// Tab marks the highlighted row and moves down. Shift-Tab marks it and moves up.
pub struct Multi {
    pub select: KeySpec,
    pub deselect: KeySpec,
    slot: usize,
}

impl Multi {
    fn toggle(&self, list: &mut ListState, delta: i32) -> Flow {
        let record = {
            let visible = list.visible();
            let Some(record) = visible.get(list.selected) else {
                return Flow::Consume;
            };
            (*record).clone()
        };
        if let Some(marks) = list.slot_mut::<Vec<Record>>(self.slot) {
            toggle_record(marks, &record);
        }
        list.move_selection(delta);
        Flow::Consume
    }
}

impl Behavior for Multi {
    fn marks_rows(&self) -> bool {
        true
    }

    fn on_key(&self, key: &KeyPress, list: &mut ListState, _fx: &mut Effects) -> Flow {
        if key.platform {
            return Flow::Pass;
        }
        if self
            .select
            .matches(&key.key, key.shift, key.control, key.alt)
        {
            return self.toggle(list, 1);
        }
        if self
            .deselect
            .matches(&key.key, key.shift, key.control, key.alt)
        {
            return self.toggle(list, -1);
        }
        Flow::Pass
    }

    fn on_submit(&self, list: &ListState, picked: &mut Vec<Record>, _fx: &mut Effects) {
        let marks = list
            .slot::<Vec<Record>>(self.slot)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let current = list.visible().get(list.selected).copied();
        *picked = activation_records(marks, current)
            .into_iter()
            .cloned()
            .collect();
    }
}

/// Shift-Delete drops a remembered command. Shift-Enter marks the submitted
/// rows to run in a terminal. A typed query that is not already a row is
/// offered as a command, and accepting one stores it for the next open.
pub struct RunCommands {
    pub forget: KeySpec,
    pub terminal: KeySpec,
    slot: usize,
}

#[derive(Debug, Default)]
struct RunMark {
    terminal: bool,
}

impl RunCommands {
    fn matches(&self, spec: &KeySpec, key: &KeyPress) -> bool {
        !key.platform && spec.matches(&key.key, key.shift, key.control, key.alt)
    }
}

impl Behavior for RunCommands {
    fn on_load(&self, widget: &str, entries: &mut Vec<Record>) {
        // `firefox` is left out when a PATH row is already labeled `firefox`.
        // `flatpak run org.foo` is added beside `flatpak`.
        let commands = crate::usage::remembered_commands(widget);
        crate::executables::merge_commands(entries, &commands);
    }

    fn on_query(&self, list: &mut ListState) {
        // `loginctl` is not added when a row is labeled `loginctl`.
        // `  loginctl lock-session ` is offered as `loginctl lock-session`.
        crate::executables::sync_query_command(&mut list.entries, &list.query, true);
    }

    fn on_key(&self, key: &KeyPress, list: &mut ListState, fx: &mut Effects) -> Flow {
        if self.matches(&self.forget, key) {
            if let Some(record) = list.visible().get(list.selected).copied() {
                if crate::executables::is_remembered(record) {
                    fx.remove_keys.push(record.key.clone());
                    fx.calls.push(AppCall::ForgetCommand {
                        widget: list.widget.clone(),
                        command: record.value.clone(),
                    });
                }
            }
            return Flow::Consume;
        }
        if self.matches(&self.terminal, key) {
            if let Some(mark) = list.slot_mut::<RunMark>(self.slot) {
                mark.terminal = true;
            }
            fx.submit_selection = true;
            return Flow::Consume;
        }
        Flow::Pass
    }

    fn on_submit(&self, list: &ListState, picked: &mut Vec<Record>, fx: &mut Effects) {
        if list
            .slot::<RunMark>(self.slot)
            .is_some_and(|mark| mark.terminal)
        {
            *picked = std::mem::take(picked)
                .into_iter()
                .map(crate::executables::apply_terminal)
                .collect();
        }
        let commands = crate::executables::commands_to_remember(&list.entries, picked);
        if !commands.is_empty() {
            fx.calls.push(AppCall::RememberCommands {
                widget: list.widget.clone(),
                commands,
            });
        }
    }
}

/// Clears the terminal mark after a submit so the next one is a normal run.
pub fn clear_terminal(list: &mut ListState) {
    for slot in &mut list.slots {
        if let Some(mark) = slot
            .as_mut()
            .and_then(|value| value.downcast_mut::<RunMark>())
        {
            mark.terminal = false;
        }
    }
}

/// Builds the behaviors for one open and the slots they store marks in.
pub fn install(
    specs: &[BehaviorSpec],
    entries: Vec<Record>,
    query: String,
) -> (Vec<Arc<dyn Behavior>>, ListState) {
    let mut state = ListState::new(entries, query);
    let behaviors = specs
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, spec)| {
            if spec.marks_rows() {
                state.init_slot::<Vec<Record>>(index);
            }
            if spec.runs_commands() {
                state.init_slot::<RunMark>(index);
            }
            spec.build(index)
        })
        .collect();
    (behaviors, state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> ListState {
        let mut list = ListState::new(
            vec![Record::new("one"), Record::new("two"), Record::new("three")],
            String::new(),
        );
        list.order = vec![0, 1, 2];
        list.init_slot::<Vec<Record>>(0);
        list
    }

    fn multi() -> Multi {
        let spec = BehaviorSpec::multi_default();
        let BehaviorSpec::Multi { select, deselect } = spec else {
            panic!("multi_default");
        };
        Multi {
            select,
            deselect,
            slot: 0,
        }
    }

    #[test]
    fn tab_marks_the_row_and_moves_down() {
        let behavior = multi();
        let mut list = rows();
        let mut fx = Effects::default();
        assert_eq!(
            behavior.on_key(&KeyPress::new("tab"), &mut list, &mut fx),
            Flow::Consume
        );
        assert_eq!(list.selected, 1);
        let marks = list.slot::<Vec<Record>>(0).unwrap();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].key, "one");
    }

    #[test]
    fn shift_tab_marks_the_row_and_moves_up() {
        let behavior = multi();
        let mut list = rows();
        let mut fx = Effects::default();
        assert_eq!(
            behavior.on_key(&KeyPress::new("tab").shift(), &mut list, &mut fx),
            Flow::Consume
        );
        assert_eq!(list.selected, 2);
        assert_eq!(list.slot::<Vec<Record>>(0).unwrap()[0].key, "one");
    }

    #[test]
    fn an_unbound_key_is_passed_on() {
        let behavior = multi();
        let mut list = rows();
        let mut fx = Effects::default();
        assert_eq!(
            behavior.on_key(&KeyPress::new("enter"), &mut list, &mut fx),
            Flow::Pass
        );
        assert!(list.slot::<Vec<Record>>(0).unwrap().is_empty());
    }

    #[test]
    fn submit_uses_marks_in_the_order_they_were_made() {
        let behavior = multi();
        let mut list = rows();
        let mut fx = Effects::default();
        behavior.on_key(&KeyPress::new("tab"), &mut list, &mut fx);
        behavior.on_key(&KeyPress::new("tab"), &mut list, &mut fx);
        let mut picked = vec![Record::new("ignored")];
        behavior.on_submit(&list, &mut picked, &mut fx);
        assert_eq!(
            picked
                .iter()
                .map(|record| record.key.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
    }

    #[test]
    fn submit_without_marks_keeps_the_highlighted_row() {
        let behavior = multi();
        let list = rows();
        let mut picked = Vec::new();
        behavior.on_submit(&list, &mut picked, &mut Effects::default());
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].key, "one");
    }

    #[test]
    fn picker_choice_adds_or_removes_multi_select() {
        let mut behaviors = Vec::new();
        BehaviorSpec::apply_picker_choice(&mut behaviors, None);
        assert!(behaviors.is_empty());
        BehaviorSpec::apply_picker_choice(&mut behaviors, Some(true));
        assert_eq!(behaviors, vec![BehaviorSpec::multi_default()]);
        BehaviorSpec::apply_picker_choice(&mut behaviors, Some(true));
        assert_eq!(behaviors.len(), 1);
        BehaviorSpec::apply_picker_choice(&mut behaviors, Some(false));
        assert!(behaviors.is_empty());
    }

    #[test]
    fn effects_change_the_query_and_drop_rows() {
        let mut list = rows();
        list.init_slot::<Vec<Record>>(0);
        list.slot_mut::<Vec<Record>>(0)
            .unwrap()
            .push(Record::new("two"));
        let mut fx = Effects::default();
        fx.set_query = Some("x".into());
        fx.remove_keys.push("two".into());
        fx.calls.push(AppCall::ForgetCommand {
            widget: "run".into(),
            command: "two".into(),
        });
        let applied = fx.apply(&mut list);
        assert!(applied.refilter);
        assert_eq!(list.query, "x");
        assert_eq!(list.entries.len(), 2);
        assert!(list.slot::<Vec<Record>>(0).unwrap().is_empty());
        assert_eq!(
            applied.calls,
            vec![AppCall::ForgetCommand {
                widget: "run".into(),
                command: "two".into(),
            }]
        );
    }

    fn run_list(entries: Vec<Record>, query: &str) -> (Arc<dyn Behavior>, ListState) {
        let (behaviors, mut list) = install(&[BehaviorSpec::run_default()], entries, query.into());
        list.widget = "run".into();
        list.order = (0..list.entries.len()).collect();
        (behaviors.into_iter().next().unwrap(), list)
    }

    #[test]
    fn a_typed_line_is_offered_unless_it_is_exactly_a_label() {
        let (behavior, mut list) = run_list(
            vec![Record::new("loginctl"), Record::new("foot")],
            "loginctl",
        );
        behavior.on_query(&mut list);
        assert_eq!(list.entries.len(), 2);

        list.query = "  ".into();
        behavior.on_query(&mut list);
        assert_eq!(list.entries.len(), 2);

        list.query = "  loginctl lock-session ".into();
        behavior.on_query(&mut list);
        assert_eq!(list.entries.last().unwrap().label, "loginctl lock-session");
        assert!(crate::executables::is_pending(list.entries.last().unwrap()));

        list.query = "foot -e htop".into();
        behavior.on_query(&mut list);
        assert_eq!(list.entries.len(), 3);
        assert_eq!(list.entries.last().unwrap().label, "foot -e htop");
        assert!(!list
            .entries
            .iter()
            .any(|record| record.label.contains("lock-session")));
    }

    #[test]
    fn shift_delete_forgets_a_remembered_line_and_leaves_a_path_row() {
        let remembered = crate::executables::merge_commands;
        let mut entries = vec![Record::new("firefox")];
        remembered(&mut entries, &["flatpak run org.foo".into()]);
        let (behavior, mut list) = run_list(entries, "");
        list.selected = 1;
        let mut fx = Effects::default();
        assert_eq!(
            behavior.on_key(&KeyPress::new("delete").shift(), &mut list, &mut fx),
            Flow::Consume
        );
        assert_eq!(fx.remove_keys, ["flatpak run org.foo"]);
        assert_eq!(
            fx.calls,
            vec![AppCall::ForgetCommand {
                widget: "run".into(),
                command: "flatpak run org.foo".into(),
            }]
        );

        list.selected = 0;
        let mut fx = Effects::default();
        assert_eq!(
            behavior.on_key(&KeyPress::new("delete").shift(), &mut list, &mut fx),
            Flow::Consume
        );
        assert!(fx.remove_keys.is_empty());
        assert!(fx.calls.is_empty());
    }

    #[test]
    fn shift_enter_marks_the_submit_to_run_in_a_terminal() {
        let (behavior, mut list) = run_list(vec![Record::new("/usr/bin/firefox")], "");
        let mut fx = Effects::default();
        assert_eq!(
            behavior.on_key(&KeyPress::new("enter").shift(), &mut list, &mut fx),
            Flow::Consume
        );
        assert!(fx.submit_selection);
        let mut picked = vec![Record::new("/usr/bin/firefox")];
        behavior.on_submit(&list, &mut picked, &mut fx);
        assert_eq!(picked[0].data["terminal"], serde_json::json!(true));

        let desktop =
            Record::new("firefox.desktop").with_data(serde_json::json!({"argv": ["firefox"]}));
        let mut picked = vec![desktop.clone()];
        behavior.on_submit(&list, &mut picked, &mut Effects::default());
        // The terminal mark is still set, so a desktop row stays unmarked.
        assert_eq!(picked[0].data, desktop.data);
    }

    #[test]
    fn submit_remembers_a_typed_command_and_not_a_path_name() {
        let typed = {
            let mut entries = vec![Record::new("firefox")];
            crate::executables::sync_query_command(&mut entries, "firefox --private", true);
            entries
        };
        let (behavior, list) = run_list(typed, "firefox --private");
        let mut picked = vec![list.entries[1].clone(), list.entries[0].clone()];
        let mut fx = Effects::default();
        behavior.on_submit(&list, &mut picked, &mut fx);
        assert_eq!(
            fx.calls,
            vec![AppCall::RememberCommands {
                widget: "run".into(),
                commands: vec!["firefox --private".into()],
            }]
        );
    }

    #[test]
    fn an_unbound_key_is_left_for_the_list() {
        let (behavior, mut list) = run_list(vec![Record::new("firefox")], "");
        let mut fx = Effects::default();
        assert_eq!(
            behavior.on_key(&KeyPress::new("enter"), &mut list, &mut fx),
            Flow::Pass
        );
        assert!(!fx.submit_selection);
    }
}
