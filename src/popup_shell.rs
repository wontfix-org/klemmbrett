//! The popup window: focus, and which content it is showing.
//!
//! The window is reused and prewarmed. A list and a secret question are
//! different content; this shell only owns the surface they are drawn on.
//! The card's size stays with the content, which is what computes it.

use crate::rank::Priors;
use crate::secret_popup::SecretPopup;
use crate::ui::{apply_box, Klemmbrett, ListPopup};
use crate::{Record, Theme};
use gpui::prelude::*;
use gpui::{
    div, App, Context, DisplayId, Entity, FocusHandle, Focusable, IntoElement, KeyDownEvent,
    MouseButton, Pixels, Window,
};

pub(crate) enum PopupContent {
    List(Entity<ListPopup>),
    Secret(Entity<SecretPopup>),
}

impl PopupContent {
    fn attach(&self, shell: gpui::WeakEntity<PopupShell>, cx: &mut App) {
        match self {
            Self::List(list) => list.update(cx, |list, _cx| list.attach(shell)),
            Self::Secret(secret) => secret.update(cx, |secret, _cx| secret.attach(shell)),
        }
    }

    fn theme<'a>(&'a self, cx: &'a App) -> &'a Theme {
        match self {
            Self::List(list) => list.read(cx).theme(),
            Self::Secret(secret) => secret.read(cx).theme(),
        }
    }

    fn on_key(&self, event: &KeyDownEvent, window: &mut Window, cx: &mut App) -> bool {
        match self {
            Self::List(list) => list.update(cx, |list, cx| list.on_key(event, window, cx)),
            Self::Secret(secret) => {
                secret.update(cx, |secret, cx| secret.on_key(event, window, cx))
            }
        }
    }

    fn dismiss(&self, cx: &mut App) -> Option<u64> {
        match self {
            Self::List(list) => list.update(cx, |list, _cx| list.dismiss()),
            Self::Secret(secret) => secret.update(cx, |secret, _cx| secret.dismiss()),
        }
    }

    fn secret_id(&self, cx: &App) -> Option<u64> {
        match self {
            Self::List(_) => None,
            Self::Secret(secret) => secret.read(cx).secret_id(),
        }
    }

    fn widget_name(&self, cx: &App) -> String {
        match self {
            Self::List(list) => list.read(cx).widget_name().to_owned(),
            Self::Secret(secret) => secret.read(cx).widget_name().to_owned(),
        }
    }

    fn element(&self) -> gpui::AnyElement {
        match self {
            Self::List(list) => list.clone().into_any_element(),
            Self::Secret(secret) => secret.clone().into_any_element(),
        }
    }
}

pub(crate) struct PopupShell {
    focus_handle: FocusHandle,
    app: Entity<Klemmbrett>,
    content: PopupContent,
    display_id: DisplayId,
    visible: bool,
    /// Set once the window has been active, so the unmapped prewarm does not
    /// count as a focus loss.
    seen_active: bool,
}

impl PopupShell {
    pub(crate) fn new(
        app: Entity<Klemmbrett>,
        content: PopupContent,
        display_id: DisplayId,
        visible: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let shell = cx.weak_entity();
        content.attach(shell, cx);
        Self {
            focus_handle: cx.focus_handle(),
            app,
            content,
            display_id,
            visible,
            seen_active: false,
        }
    }

    pub(crate) fn app(&self) -> Entity<Klemmbrett> {
        self.app.clone()
    }

    pub(crate) fn display_id(&self) -> DisplayId {
        self.display_id
    }

    pub(crate) fn list(&self) -> Option<Entity<ListPopup>> {
        match &self.content {
            PopupContent::List(list) => Some(list.clone()),
            PopupContent::Secret(_) => None,
        }
    }

    /// Drops whatever is showing and draws `content` in this same window.
    pub(crate) fn replace(&mut self, content: PopupContent, cx: &mut Context<Self>) {
        let shell = cx.weak_entity();
        content.attach(shell, cx);
        self.content = content;
        cx.notify();
    }

    pub(crate) fn reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = true;
        self.seen_active = false;
        window.set_visible(true);
        window.focus(&self.focus_handle, cx);
        window.activate();
        cx.notify();
    }

    /// The open provider popup, when this window is actually on screen.
    pub(crate) fn live_widget(&self, cx: &App) -> Option<String> {
        if !self.visible {
            return None;
        }
        let PopupContent::List(list) = &self.content else {
            return None;
        };
        let list = list.read(cx);
        list.is_live().then(|| list.widget_name().to_owned())
    }

    pub(crate) fn widget_name(&self, cx: &App) -> String {
        self.content.widget_name(cx)
    }

    pub(crate) fn secret_id(&self, cx: &App) -> Option<u64> {
        self.content.secret_id(cx)
    }

    pub(crate) fn replace_entries(
        &mut self,
        name: &str,
        entries: Vec<Record>,
        priors: Priors,
        cx: &mut Context<Self>,
    ) {
        if !self.visible {
            return;
        }
        let PopupContent::List(list) = &self.content else {
            return;
        };
        list.update(cx, |list, cx| {
            if list.widget_name() == name {
                list.replace_entries(entries, priors, cx);
            }
        });
    }

    pub(crate) fn restyle(&mut self, theme: Theme, cx: &mut Context<Self>) {
        match &self.content {
            PopupContent::List(list) => list.update(cx, |list, cx| {
                list.restyle(theme);
                cx.notify();
            }),
            PopupContent::Secret(secret) => secret.update(cx, |secret, cx| {
                secret.restyle(theme);
                cx.notify();
            }),
        }
    }

    pub(crate) fn dismiss_content(&mut self, cx: &mut Context<Self>) -> Option<u64> {
        self.content.dismiss(cx)
    }

    /// Resizes the surface to `output` and keeps the card inside it.
    pub(crate) fn fit_output(
        &mut self,
        output: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.resize(output);
        match &self.content {
            PopupContent::List(list) => {
                list.update(cx, |list, cx| list.fit_output(output, cx));
            }
            PopupContent::Secret(secret) => {
                secret.update(cx, |secret, cx| secret.fit_output(output, cx));
            }
        }
    }

    /// Dismisses the content, destroys the surface, and asks for a fresh hidden one.
    pub(crate) fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        let secret = self.dismiss_content(cx);
        self.visible = false;
        self.seen_active = false;
        // Layer-shell remap after a null-buffer unmap is broken in GPUI-CE:
        // prepare_to_map is xdg-only, so set_visible(true) never attaches a
        // buffer again. Destroy this surface and build a fresh hidden one.
        let app = self.app.clone();
        app.update(cx, |klemmbrett, _cx| klemmbrett.expect_popup_close());
        window.remove_window();
        app.update(cx, |klemmbrett, _cx| klemmbrett.drop_popup_handle());
        cx.defer(move |cx| {
            app.update(cx, |klemmbrett, cx| {
                if !klemmbrett.popup_open() {
                    klemmbrett.prewarm_popup(cx);
                }
                if let Some(id) = secret {
                    klemmbrett.secret_closed(id, cx);
                }
            });
        });
    }
}

impl Focusable for PopupShell {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PopupShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        if self.visible {
            if window.is_window_active() {
                self.seen_active = true;
            } else if self.seen_active {
                self.close(window, cx);
            }
        }
        let overlay = self.content.theme(cx).overlay().clone();
        apply_box(
            div()
                .id("klemmbrett-overlay")
                .track_focus(&self.focus_handle)
                .size_full()
                .relative(),
            &overlay,
        )
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            let close = this.content.on_key(event, window, cx);
            if close {
                this.close(window, cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, window, cx| this.close(window, cx)),
        )
        .child(self.content.element())
    }
}
