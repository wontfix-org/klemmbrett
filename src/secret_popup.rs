//! A secret question drawn in the popup shell.
//!
//! This is not a list. Choices are a fixed set of rows: nothing is filtered,
//! ranked, or marked, and a typed line is never a command. The only text that
//! leaves here is the answer.

use std::sync::Arc;

use crate::journal;
use crate::popup_shell::PopupShell;
use crate::secret::{self, Mode, SecretReply, SecretRequest};
use crate::ui::{
    paint_popup_card, typed_filter_char, PendingSecret, PopupChrome, PopupPlacement, RowData,
};
use crate::{accelerator_for, Layout, Location, Record, Theme};
use gpui::prelude::*;
use gpui::{
    uniform_list, Context, KeyDownEvent, Pixels, ScrollStrategy, UniformListScrollHandle,
    WeakEntity, Window,
};
use zeroize::Zeroizing;

/// A secret question on screen and what has been typed for it.
pub(crate) struct SecretSession {
    pub(crate) id: u64,
    pub(crate) request: SecretRequest,
    reply: Option<std::sync::mpsc::Sender<SecretReply>>,
    pub(crate) typed: Zeroizing<String>,
}

impl SecretSession {
    pub(crate) fn new(pending: PendingSecret) -> Self {
        Self {
            id: pending.id,
            request: pending.request,
            reply: Some(pending.reply),
            typed: Zeroizing::new(String::with_capacity(256)),
        }
    }

    pub(crate) fn answer(&mut self, reply: SecretReply) {
        if let Some(sender) = self.reply.take() {
            if sender.send(reply).is_err() {
                journal::info("dropped a secret answer; the asker is gone");
            }
        }
    }
}

impl Drop for SecretSession {
    fn drop(&mut self) {
        self.answer(SecretReply::Cancelled);
    }
}

pub(crate) struct SecretPopup {
    shell: Option<WeakEntity<PopupShell>>,
    session: Option<SecretSession>,
    /// Choice rows for `Mode::Choose`. Empty for every other mode.
    entries: Vec<Record>,
    selected: usize,
    label_max: usize,
    theme: Theme,
    layout: Arc<Layout>,
    widget_name: String,
    card: gpui::Bounds<Pixels>,
    location: Location,
    output_size: gpui::Size<Pixels>,
    scroll: UniformListScrollHandle,
}

impl SecretPopup {
    pub(crate) fn new(
        widget: String,
        session: SecretSession,
        entries: Vec<Record>,
        placement: &PopupPlacement,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            shell: None,
            session: Some(session),
            entries,
            selected: 0,
            label_max: placement.label_max,
            theme: placement.theme.clone(),
            layout: placement.layout.clone(),
            widget_name: widget,
            card: placement.card,
            location: placement.location,
            output_size: placement.output_size,
            scroll: UniformListScrollHandle::new(),
        }
    }

    pub(crate) fn attach(&mut self, shell: WeakEntity<PopupShell>) {
        self.shell = Some(shell);
    }

    pub(crate) fn theme(&self) -> &Theme {
        &self.theme
    }

    pub(crate) fn widget_name(&self) -> &str {
        &self.widget_name
    }

    pub(crate) fn secret_id(&self) -> Option<u64> {
        self.session.as_ref().map(|session| session.id)
    }

    /// Drops the session. That cancels the question unless it was answered.
    pub(crate) fn dismiss(&mut self) -> Option<u64> {
        self.session.take().map(|session| session.id)
    }

    pub(crate) fn restyle(&mut self, theme: Theme) {
        if theme == self.theme {
            return;
        }
        self.theme = theme;
        self.resize_card();
    }

    fn resize_card(&mut self) {
        let (width, height) =
            self.layout
                .card_size(&self.theme, self.entries.len(), self.label_max);
        let old = self.card;
        let (x, y) = crate::resize_at_location(
            self.location,
            (f32::from(old.origin.x), f32::from(old.origin.y)),
            (f32::from(old.size.width), f32::from(old.size.height)),
            (width, height),
            (
                f32::from(self.output_size.width),
                f32::from(self.output_size.height),
            ),
        );
        self.card = gpui::Bounds::new(
            gpui::point(gpui::px(x), gpui::px(y)),
            gpui::size(gpui::px(width), gpui::px(height)),
        );
    }

    /// Keeps the card on screen after the output it sits on changes size.
    pub(crate) fn fit_output(&mut self, output: gpui::Size<Pixels>, cx: &mut Context<Self>) {
        if self.output_size == output {
            return;
        }
        self.output_size = output;
        self.resize_card();
        cx.notify();
    }

    /// `true` asks the shell to close. The answer is stored before that, so
    /// dropping the session does not cancel a question that was just answered.
    pub(crate) fn on_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = event.keystroke.key.as_str();
        let mods = &event.keystroke.modifiers;
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        cx.stop_propagation();
        match key {
            "escape" => return true,
            "enter" => {
                let reply = match &session.request.mode {
                    Mode::Secret => {
                        SecretReply::Secret(Zeroizing::new(std::mem::take(&mut *session.typed)))
                    }
                    Mode::Confirm | Mode::Message => SecretReply::Confirmed,
                    // `selected` is the choice's place in the question. Nothing
                    // here reorders those rows. Example: choices ["yes", "no"],
                    // Down, Enter → Chose(1).
                    Mode::Choose(_) if self.entries.is_empty() => return false,
                    Mode::Choose(_) => SecretReply::Chose(self.selected),
                };
                session.answer(reply);
                return true;
            }
            "up" => self.move_selection(-1, cx),
            "down" => self.move_selection(1, cx),
            "backspace" => {
                session.typed.pop();
                cx.notify();
            }
            "u" if mods.control => {
                session.typed.clear();
                cx.notify();
            }
            _ if session.request.mode == Mode::Secret
                && !(mods.control || mods.alt || mods.platform) =>
            {
                if let Some(ch) = typed_filter_char(event) {
                    session.typed.push(ch);
                    cx.notify();
                }
            }
            _ => {}
        }
        false
    }

    fn move_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        let count = self.entries.len() as i32;
        if count == 0 {
            return;
        }
        self.selected = (self.selected as i32 + delta).rem_euclid(count) as usize;
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// A click on choice `index`. `true` when that choice was the answer.
    pub(crate) fn choose(&mut self, index: usize) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        if !matches!(session.request.mode, Mode::Choose(_)) {
            return false;
        }
        if index >= self.entries.len() {
            return false;
        }
        session.answer(SecretReply::Chose(index));
        true
    }

    fn rows(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let layout = self.layout.clone();
        let show_accelerators = !layout.has_inputbar();
        let rows: Vec<RowData> = self
            .entries
            .iter()
            .enumerate()
            .map(|(index, record)| RowData {
                record: record.clone(),
                label_max: self.label_max,
                accelerator: show_accelerators.then(|| accelerator_for(index)).flatten(),
                marked: false,
            })
            .collect();
        let selected = self.selected;
        let view = cx.weak_entity();
        uniform_list("secret-rows", rows.len(), move |range, _window, _cx| {
            range
                .map(|index| {
                    crate::ui::popup_row(
                        index,
                        &rows[index],
                        index == selected,
                        &layout,
                        &theme,
                        Some(choose_row(view.clone(), index)),
                    )
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .size_full()
        .flex_1()
        .into_any()
    }
}

fn choose_row(
    view: WeakEntity<SecretPopup>,
    index: usize,
) -> impl Fn(&gpui::MouseDownEvent, &mut Window, &mut gpui::App) + 'static {
    move |_, window, cx| {
        let mut parent = None;
        let close = view.update(cx, |this, _cx| {
            parent = this.shell.clone();
            this.choose(index)
        });
        let Ok(true) = close else {
            if let Err(err) = close {
                journal::error(&format!("failed to choose secret row {index}: {err}"));
            }
            return;
        };
        let Some(shell) = parent.and_then(|shell| shell.upgrade()) else {
            return;
        };
        shell.update(cx, |shell, cx| shell.close(window, cx));
    }
}

struct SecretChrome<'a> {
    widget: &'a str,
    session: &'a SecretSession,
}

impl PopupChrome for SecretChrome<'_> {
    fn prompt_text(&self, style_text: Option<&str>) -> String {
        self.session
            .request
            .prompt
            .clone()
            .or_else(|| style_text.map(str::to_owned))
            .unwrap_or_else(|| self.widget.to_owned())
    }

    fn entry_text(&self, style_text: Option<&str>) -> String {
        let shown = secret::entry_text(&self.session.request, &self.session.typed, style_text);
        if self.session.request.mode == Mode::Secret {
            format!("{shown}│")
        } else {
            shown
        }
    }

    fn hide_blank_text(&self) -> bool {
        true
    }
}

impl gpui::Render for SecretPopup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let theme = self.theme.clone();
        let layout = self.layout.clone();
        let rows = layout.has_listview().then(|| self.rows(cx));
        let Some(session) = self.session.as_ref() else {
            return gpui::div().into_any_element();
        };
        let mut owned = vec![
            ("widget".to_owned(), self.widget_name.clone()),
            ("query".to_owned(), String::new()),
            ("count".to_owned(), self.entries.len().to_string()),
        ];
        owned.extend(session.request.fields());
        let chrome = SecretChrome {
            widget: &self.widget_name,
            session,
        };
        let fields: Vec<(&str, &str)> = owned
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let preview = self.entries.get(self.selected);
        paint_popup_card(self.card, &theme, &layout, &fields, &chrome, rows, preview)
            .into_any_element()
    }
}
