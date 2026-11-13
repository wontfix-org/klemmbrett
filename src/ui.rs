//! The popup, notification toasts, the tray icon, and the handle Python uses
//! to drive the daemon.
//!
//! The popup is a layer-shell surface that asks for exclusive keyboard
//! interactivity, with the rows packed *inside* it. It deliberately does not
//! use an `xdg_popup`: taking the keyboard grab an `xdg_popup` needs requires
//! the serial of a real input event. The popup is opened over Varlink, so no
//! such serial exists and the grab silently does nothing until the pointer
//! happens to enter the surface. Layer-shell's keyboard interactivity is the
//! protocol's supported way to focus a surface that was not opened from user
//! input, so the rows and their key handling live here instead.

use crate::config::{AppConfig, MultiSelect};
use crate::data::DataPath;
use crate::events;
use crate::history::shorten_label;
use crate::journal;
use crate::launch::{spawn_detached, ResolvedLaunch};
use crate::layout::{expand, expand_record, Layout, LayoutContext, Node, NodeKind};
use crate::notifications::{self, CloseReason, Notification, NotificationCenter, ToastCmd};
use crate::rank::{self, Priors, Ranking};
use crate::usage::{self, UsageStore};
use crate::varlink::{self, ShowRequest};
use crate::widget::{fetch_scored, Handlers, WidgetDef, Widgets};
use crate::windows::WindowList;
use crate::{
    accelerator_for, activation_records, focused_display_index, history_candidate,
    index_for_accelerator, output_layout, place_at_location, resize_at_location, toggle_record,
    ArboardBackend, Backend, ClipboardSync, Edges, Location, Orientation, OutputGeom, Record,
    Selection, Style, Theme, ThemeSpec, POLL_INTERVAL,
};
use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui::{
    canvas, div, point, prelude::*, px, rgba, size, uniform_list, AnyElement, App, Bounds, Context,
    Corners, DisplayId, Entity, FocusHandle, Focusable, KeyDownEvent, LinuxWindowBackground,
    MouseButton, Pixels, QuitMode, RenderImage, ScrollStrategy, Styled, UniformListScrollHandle,
    WeakEntity, Window, WindowBounds, WindowDecorations, WindowHandle, WindowKind, WindowOptions,
};
use ksni::blocking::TrayMethods;
use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};

const TRAY_ICON_NAME: &str = "edit-paste";
const TRAY_ICON_SIZES: [i32; 4] = [16, 22, 24, 32];

type Reply = mpsc::Sender<Result<Vec<String>, String>>;

/// A request that must run on the GPUI thread.
#[derive(Debug)]
pub(crate) enum UiCmd {
    Show {
        widget: String,
        pointer: Option<(i32, i32)>,
        records: Option<Vec<Record>>,
        reply: Option<Reply>,
    },
    /// These widgets' records changed; an open popup showing one reloads.
    Refresh(Vec<String>),
    Toast(ToastCmd),
    RecordHistory(String),
    SetTheme(ThemeSpec),
    Quit,
}

enum ClipCmd {
    Restore(String),
}

/// Lets code outside the UI thread act on the running daemon. Every method
/// returns immediately.
#[derive(Clone)]
pub struct Runtime {
    ui: async_channel::Sender<UiCmd>,
    clip: mpsc::Sender<ClipCmd>,
    launch: ResolvedLaunch,
    notifications: Option<Arc<NotificationCenter>>,
}

impl Runtime {
    /// Puts `text` on the clipboard. The sync loop copies it to the primary
    /// selection and history records it, as for any other copy.
    pub fn copy(&self, text: &str) -> Result<(), String> {
        self.clip
            .send(ClipCmd::Restore(text.to_owned()))
            .map_err(|_| "the clipboard thread is not running".to_string())
    }

    /// Starts a detached process the way the `launch` setting says.
    pub fn launch(&self, argv: &[String]) -> Result<(), String> {
        let parts: Vec<&str> = argv.iter().map(String::as_str).collect();
        spawn_detached(&self.launch.argv(&parts))
    }

    /// Focuses a toplevel by the key a `WaylandWindows` provider reported.
    pub fn activate_window(&self, key: &str) -> Result<(), String> {
        WindowList::shared().activate(key)
    }

    /// Replaces the file-wide palette until the daemon exits.
    pub fn set_theme(&self, palette: ThemeSpec) -> Result<(), String> {
        self.send(UiCmd::SetTheme(palette))
    }

    /// Opens a widget. `records` replaces its provider.
    pub fn show(
        &self,
        widget: &str,
        records: Option<Vec<Record>>,
        pointer: Option<(i32, i32)>,
    ) -> Result<(), String> {
        self.send(UiCmd::Show {
            widget: widget.to_owned(),
            pointer,
            records,
            reply: None,
        })
    }

    /// Tells open popups and `Subscribe` callers that these widgets changed.
    pub fn refresh(&self, widgets: Vec<String>) -> Result<(), String> {
        for widget in &widgets {
            events::publish(
                serde_json::json!({ "kind": "changed", "widget": widget }),
                Some(widget),
            );
        }
        self.send(UiCmd::Refresh(widgets))
    }

    /// Shows an open notification as a toast.
    pub fn show_notification(&self, id: u32) -> Result<(), String> {
        self.center()?.show(id)
    }

    /// Ends a notification that is not shown (dropped or stashed).
    pub fn close_notification(&self, id: u32, reason: CloseReason) -> Result<(), String> {
        self.center()?.closed(id, reason);
        Ok(())
    }

    fn center(&self) -> Result<&NotificationCenter, String> {
        self.notifications
            .as_deref()
            .ok_or_else(|| "notifications are not enabled".to_string())
    }

    pub fn quit(&self) -> Result<(), String> {
        self.send(UiCmd::Quit)
    }

    fn send(&self, cmd: UiCmd) -> Result<(), String> {
        self.ui
            .send_blocking(cmd)
            .map_err(|_| "the popup thread is not running".to_string())
    }
}

/// Runs the daemon until the tray or [`Runtime::quit`] asks it to stop.
///
/// `on_ready` receives the [`Runtime`] before the first event is handled.
pub fn run(config: AppConfig, on_ready: impl FnOnce(Runtime)) -> Result<(), String> {
    let AppConfig {
        label_max,
        launch,
        widgets,
        handlers,
        notifications: notification_config,
        socket,
    } = config;
    let listener = varlink::bind(&varlink::socket_path(socket.as_deref()))?;
    let backend = ArboardBackend::new()
        .map_err(|err| format!("failed to initialize the Wayland clipboard: {err}"))?;
    let sync = ClipboardSync::new(backend);
    widgets.start();

    let (tx, rx) = async_channel::unbounded::<UiCmd>();
    let center = notification_config.map(|config| {
        let toasts = tx.clone();
        let center = NotificationCenter::new(config, handlers.clone(), move |cmd| {
            send_ui(&toasts, UiCmd::Toast(cmd), "deliver a notification");
        });
        notifications::serve(center.clone(), None);
        center
    });
    let show = tx.clone();
    varlink::Server::new(
        widgets.defs().to_vec(),
        handlers.clone(),
        center.clone(),
        move |request: ShowRequest| {
            show.send_blocking(UiCmd::Show {
                widget: request.widget,
                pointer: request.pointer,
                records: request.records,
                reply: request.reply,
            })
            .map_err(|err| format!("failed to request popup: {err}"))
        },
    )
    .serve(listener);
    install_tray(widgets.names_with_provider(), tx.clone());
    let clip = spawn_clipboard(sync, tx.clone());
    on_ready(Runtime {
        ui: tx,
        clip,
        launch: launch.resolve(),
        notifications: center.clone(),
    });
    crate::systemd::notify_ready();
    let usage = open_usage(&widgets);

    gpui_platform::application()
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx: &mut App| {
            let app = cx.new(|_cx| Klemmbrett::new(widgets, handlers, label_max, center, usage));
            app.update(cx, |this, cx| this.prewarm_popup(cx));

            let dispatcher = app.clone();
            cx.spawn(async move |cx| {
                while let Ok(cmd) = rx.recv().await {
                    dispatcher.update(cx, |this, cx| match cmd {
                        UiCmd::Show {
                            widget,
                            pointer,
                            records,
                            reply,
                        } => this.show_widget_menu(widget, pointer, records, reply, cx),
                        UiCmd::Refresh(widgets) => this.refresh(widgets, cx),
                        UiCmd::Toast(cmd) => this.toast(cmd, cx),
                        UiCmd::RecordHistory(text) => this.record_history(&text),
                        UiCmd::SetTheme(palette) => this.set_theme(palette, cx),
                        UiCmd::Quit => cx.quit(),
                    });
                }
            })
            .detach();
        });
    Ok(())
}

fn send_ui(tx: &async_channel::Sender<UiCmd>, cmd: UiCmd, what: &str) {
    if let Err(err) = tx.send_blocking(cmd) {
        journal::error(&format!("failed to {what}: {err}"));
    }
}

fn spawn_clipboard(
    mut sync: ClipboardSync<ArboardBackend>,
    ui: async_channel::Sender<UiCmd>,
) -> mpsc::Sender<ClipCmd> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("klemmbrett-clipboard".into())
        .spawn(move || {
            let seed = sync.last_text().to_owned();
            send_ui(&ui, UiCmd::RecordHistory(seed), "send clipboard history");
            loop {
                match rx.recv_timeout(POLL_INTERVAL) {
                    Ok(ClipCmd::Restore(text)) => {
                        if let Err(err) = sync.backend_mut().write(Selection::Clipboard, &text) {
                            journal::error(&format!("Failed to restore the clipboard: {err}"));
                            continue;
                        }
                        send_ui(&ui, UiCmd::RecordHistory(text), "send clipboard history");
                        let action = sync.tick();
                        let last = sync.last_text().to_owned();
                        if let Some(text) = history_candidate(&action, &last) {
                            send_ui(
                                &ui,
                                UiCmd::RecordHistory(text.to_owned()),
                                "send clipboard history",
                            );
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        let action = sync.tick();
                        let last = sync.last_text().to_owned();
                        if let Some(text) = history_candidate(&action, &last) {
                            send_ui(
                                &ui,
                                UiCmd::RecordHistory(text.to_owned()),
                                "send clipboard history",
                            );
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .expect("spawn clipboard thread");
    tx
}

struct Klemmbrett {
    widgets: Widgets,
    handlers: Arc<dyn Handlers>,
    label_max: usize,
    popup: Option<WindowHandle<PopupView>>,
    /// Bumped by every open request so a slow provider cannot replace a newer popup.
    show_seq: u64,
    center: Option<Arc<NotificationCenter>>,
    toasts: Option<WindowHandle<ToastView>>,
    /// Bumped whenever a toast is shown or updated, so stale timers do nothing.
    toast_seq: u64,
    /// Past picks, when a widget ranks by them.
    usage: Option<Arc<Mutex<UsageStore>>>,
}

impl Klemmbrett {
    fn new(
        widgets: Widgets,
        handlers: Arc<dyn Handlers>,
        label_max: usize,
        center: Option<Arc<NotificationCenter>>,
        usage: Option<Arc<Mutex<UsageStore>>>,
    ) -> Self {
        Self {
            widgets,
            handlers,
            label_max: label_max.max(1),
            popup: None,
            show_seq: 0,
            center,
            toasts: None,
            toast_seq: 0,
            usage,
        }
    }

    /// Notes the picked `keys` of `widget` for its rankers, off the UI thread.
    fn record_usage(&self, widget: &str, keys: Vec<String>, query: String, cx: &mut Context<Self>) {
        let (Some(store), Some(def)) = (self.usage.clone(), self.widgets.get(widget)) else {
            return;
        };
        let namespaces = def.ranking.usage_namespaces(widget);
        if namespaces.is_empty() || keys.is_empty() {
            return;
        }
        cx.background_executor()
            .spawn(async move {
                let mut store = store
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let now = usage::now_secs();
                for namespace in &namespaces {
                    for key in &keys {
                        if let Err(err) = store.record(namespace, key, &query, now) {
                            journal::error(&format!("failed to record usage: {err}"));
                            return;
                        }
                    }
                }
            })
            .detach();
    }

    fn record_history(&mut self, text: &str) {
        if let Err(err) = self.widgets.record_history(text) {
            journal::error(&format!("Failed to record clipboard history: {err}"));
        }
        self.handlers.clipboard(text);
    }

    fn select(&self, widget: &str, records: Vec<Record>) {
        events::publish(
            serde_json::json!({
                "kind": "selected",
                "widget": widget,
                "records": varlink::records_json(&records),
            }),
            Some(widget),
        );
        self.handlers.select(widget, records);
    }

    /// Reloads the open popup when it shows one of `widgets` from its provider.
    /// The query, the highlighted row, and the marks survive.
    fn refresh(&mut self, widgets: Vec<String>, cx: &mut Context<Self>) {
        let Some(handle) = self.popup else {
            return;
        };
        let name = match handle.update(cx, |view, _, _| {
            (view.visible && view.live).then(|| view.widget_name.clone())
        }) {
            Ok(Some(name)) => name,
            Ok(None) => return,
            Err(err) => {
                journal::error(&format!("failed to refresh the popup: {err}"));
                return;
            }
        };
        if !widgets.contains(&name) {
            return;
        }
        let Some(def) = self.widgets.get(&name).cloned() else {
            return;
        };
        let seq = self.show_seq;
        let usage = self.usage.clone();
        let load = cx
            .background_executor()
            .spawn(async move { load_entries(&def, None, usage.as_deref()) });
        let label = name.clone();
        cx.spawn(async move |this, cx| {
            let loaded = load.await;
            if let Err(err) = this.update(cx, |this, cx| {
                if seq != this.show_seq {
                    return;
                }
                let (entries, priors) = match loaded {
                    Ok(loaded) => loaded,
                    Err(err) => {
                        journal::error(&format!("Failed to refresh widget {name:?}: {err}"));
                        return;
                    }
                };
                if let Some(handle) = this.popup {
                    if let Err(err) = handle.update(cx, |view, _window, cx| {
                        if view.visible && view.widget_name == name {
                            view.replace_entries(entries, priors, cx);
                        }
                    }) {
                        journal::error(&format!("failed to replace popup entries: {err}"));
                    }
                }
            }) {
                journal::error(&format!("failed to refresh {label:?}: {err}"));
            }
        })
        .detach();
    }

    fn toast(&mut self, cmd: ToastCmd, cx: &mut Context<Self>) {
        match cmd {
            ToastCmd::Show(notification) => self.show_toast(*notification, cx),
            ToastCmd::Close(id) => self.close_toast(id, None, cx),
        }
    }

    fn show_toast(&mut self, notification: Notification, cx: &mut Context<Self>) {
        let Some(center) = self.center.clone() else {
            return;
        };
        self.toast_seq += 1;
        let seq = self.toast_seq;
        let id = notification.id;
        let lifetime = notification.lifetime_ms(center.config().timeout_ms);
        let toast = Toast { notification, seq };
        let theme = self.widgets.default_theme().clone();
        let config = center.config().clone();
        let updated = if let Some(handle) = self.toasts {
            match handle.update(cx, |view, window, cx| {
                view.theme = theme.clone();
                view.upsert(toast.clone());
                window.resize(view.size());
                cx.notify();
            }) {
                Ok(()) => Some(()),
                Err(err) => {
                    journal::error(&format!("failed to update the toast window: {err}"));
                    None
                }
            }
        } else {
            None
        };
        if updated.is_none() {
            let app = cx.entity();
            let view = ToastView {
                app,
                toasts: vec![toast],
                theme,
                max_visible: config.max_visible.max(1),
                plates: Vec::new(),
            };
            let options = toast_options(view.size(), config.location, view.theme.toast().margin);
            match cx.open_window(options, move |_window, cx| cx.new(|_cx| view)) {
                Ok(handle) => self.toasts = Some(handle),
                Err(err) => {
                    journal::error(&format!("Failed to show a notification: {err}"));
                    self.toasts = None;
                }
            }
        }
        if let Some(ms) = lifetime {
            let timer = cx
                .background_executor()
                .timer(std::time::Duration::from_millis(u64::from(ms)));
            cx.spawn(async move |this, cx| {
                timer.await;
                if let Err(err) = this.update(cx, |this, cx| this.expire_toast(id, seq, cx)) {
                    journal::error(&format!("failed to expire toast {id}: {err}"));
                }
            })
            .detach();
        }
    }

    fn expire_toast(&mut self, id: u32, seq: u64, cx: &mut Context<Self>) {
        let Some(handle) = self.toasts else {
            return;
        };
        let current = match handle.update(cx, |view, _, _| {
            view.toasts
                .iter()
                .any(|toast| toast.notification.id == id && toast.seq == seq)
        }) {
            Ok(current) => current,
            Err(err) => {
                journal::error(&format!("failed to expire toast {id}: {err}"));
                false
            }
        };
        if current {
            self.close_toast(id, Some(CloseReason::Expired), cx);
        }
    }

    /// Removes a toast. `reason` is what the sender hears; `None` when it
    /// asked for the close itself and already heard.
    fn close_toast(&mut self, id: u32, reason: Option<CloseReason>, cx: &mut Context<Self>) {
        if let Some(handle) = self.toasts {
            let emptied = handle.update(cx, |view, window, cx| {
                view.toasts.retain(|toast| toast.notification.id != id);
                if view.toasts.is_empty() {
                    window.remove_window();
                    true
                } else {
                    window.resize(view.size());
                    cx.notify();
                    false
                }
            });
            if let Err(err) = &emptied {
                journal::error(&format!("failed to close toast {id}: {err}"));
            }
            if !matches!(emptied, Ok(false)) {
                self.toasts = None;
            }
        }
        if let (Some(reason), Some(center)) = (reason, &self.center) {
            center.closed(id, reason);
        }
    }

    /// A click on a toast: run `action` if given, then close it.
    fn toast_clicked(&mut self, id: u32, action: Option<String>, cx: &mut Context<Self>) {
        if let (Some(action), Some(center)) = (action, &self.center) {
            center.invoke(id, &action);
        }
        // The click is dispatched while this toast window is borrowed, so
        // closing it here cannot re-enter that window. Run once the update ends.
        let this = cx.entity();
        cx.defer(move |cx| {
            this.update(cx, |this, cx| {
                this.close_toast(id, Some(CloseReason::Dismissed), cx);
            });
        });
    }

    fn highlight(&self, widget: &str, record: Record) {
        if self.widgets.get(widget).is_some_and(|def| def.highlight) {
            self.handlers.highlight(widget, record);
        }
    }

    fn theme_for(&self, widget: &str) -> Theme {
        self.widgets
            .theme(widget)
            .unwrap_or_else(|| self.widgets.default_theme())
            .clone()
    }

    fn set_theme(&mut self, palette: ThemeSpec, cx: &mut Context<Self>) {
        self.widgets.set_palette(palette);
        let Some(handle) = self.popup else {
            return;
        };
        let name = match handle.update(cx, |view, _, _| view.widget_name.clone()) {
            Ok(name) => name,
            Err(err) => {
                journal::error(&format!("failed to read the open popup: {err}"));
                return;
            }
        };
        let theme = self.theme_for(&name);
        if let Err(err) = handle.update(cx, |view, _, cx| {
            view.restyle(theme);
            cx.notify();
        }) {
            journal::error(&format!("failed to restyle the popup: {err}"));
        }
    }

    fn drop_popup_handle(&mut self) {
        self.popup = None;
    }

    fn prewarm_popup(&mut self, cx: &mut Context<Self>) {
        let theme = self.widgets.default_theme().clone();
        let layout = Arc::new(Layout::menu(false));
        let Some(placement) = popup_placement(
            cx,
            None,
            1,
            self.label_max,
            theme,
            layout,
            Location::Pointer,
        ) else {
            return;
        };
        self.open_popup(Vec::new(), placement, None, cx);
    }

    fn show_widget_menu(
        &mut self,
        widget: String,
        pointer: Option<(i32, i32)>,
        records: Option<Vec<Record>>,
        reply: Option<Reply>,
        cx: &mut Context<Self>,
    ) {
        let Some(def) = self.widgets.get(&widget).cloned() else {
            let err = format!("unknown widget {widget:?}");
            journal::error(&err);
            finish_prompt(reply, Err(err));
            return;
        };
        if records.is_none() && def.source.is_none() {
            let err = format!("widget {widget:?} has no provider; pass records");
            journal::error(&format!("Failed to read widget {widget:?}: {err}"));
            finish_prompt(reply, Err(err));
            return;
        }
        self.show_seq += 1;
        let seq = self.show_seq;
        let live = records.is_none();
        let ctx = LayoutContext {
            widget: widget.clone(),
            multi: def.multi.is_some(),
            prompt: reply.is_some(),
        };
        let usage = self.usage.clone();
        let load = cx.background_executor().spawn(async move {
            let layout = def
                .layout
                .build(&ctx)
                .map_err(|err| format!("widget {:?}: invalid layout: {err}", def.name))?;
            let (entries, priors) = if layout.has_listview() {
                load_entries(&def, records, usage.as_deref())?
            } else {
                (Vec::new(), Priors::default())
            };
            Ok::<_, String>((layout, entries, priors))
        });
        cx.spawn(async move |this, cx| {
            let loaded = load.await;
            let name = widget.clone();
            if let Err(err) = this.update(cx, |this, cx| {
                this.finish_show(seq, widget, pointer, loaded, live, reply, cx)
            }) {
                journal::error(&format!("failed to show {name:?}: {err}"));
            }
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_show(
        &mut self,
        seq: u64,
        widget: String,
        pointer: Option<(i32, i32)>,
        loaded: Result<(Arc<Layout>, Vec<Record>, Priors), String>,
        live: bool,
        reply: Option<Reply>,
        cx: &mut Context<Self>,
    ) {
        if seq != self.show_seq {
            finish_prompt(reply, Ok(Vec::new()));
            return;
        }
        let (layout, entries, priors) = match loaded {
            Ok(loaded) => loaded,
            Err(err) => {
                journal::error(&format!("Failed to read widget {widget:?}: {err}"));
                finish_prompt(reply, Err(err));
                return;
            }
        };
        let theme = self.theme_for(&widget);
        let def = self.widgets.get(&widget);
        let location = def
            .and_then(|def| def.location)
            .or(layout.location())
            .or(layout.window_style(&theme).location)
            .unwrap_or(Location::Pointer);
        let Some(placement) = popup_placement(
            cx,
            pointer,
            entries.len(),
            self.label_max,
            theme,
            layout,
            location,
        ) else {
            let err = "Failed to place the popup: no displays.".to_string();
            journal::error(&err);
            finish_prompt(reply, Err(err));
            return;
        };
        let shown = Shown {
            widget,
            multi: def.and_then(|def| def.multi.clone()),
            search: def.map(|def| def.search.clone()).unwrap_or_default(),
            ranking: def.map(|def| def.ranking.clone()).unwrap_or_default(),
            priors,
            live,
            reply,
        };
        if let Some(shown) = self.reuse_popup(&entries, &placement, shown, cx) {
            self.open_popup(entries, placement, Some(shown), cx);
        }
    }

    /// `None` when the existing popup was reused. `Some(shown)` when the
    /// caller still has to open a window.
    fn reuse_popup(
        &mut self,
        entries: &[Record],
        placement: &PopupPlacement,
        shown: Shown,
        cx: &mut Context<Self>,
    ) -> Option<Shown> {
        let Some(handle) = self.popup else {
            return Some(shown);
        };
        let mut shown = Some(shown);
        match handle.update(cx, |view, window, cx| {
            if view.display_id != placement.display_id {
                view.finish_prompt(Ok(Vec::new()));
                return false;
            }
            window.resize(placement.output_size);
            view.reload(
                entries.to_vec(),
                placement,
                shown.take().expect("shown"),
                cx,
            );
            view.reveal(window, cx);
            true
        }) {
            Ok(true) => None,
            Ok(false) => {
                if let Err(err) = handle.update(cx, |_, window, _| window.remove_window()) {
                    journal::error(&format!("failed to remove the popup window: {err}"));
                }
                self.popup = None;
                shown
            }
            Err(err) => {
                journal::error(&format!("failed to reuse the popup: {err}"));
                self.popup = None;
                shown
            }
        }
    }

    fn open_popup(
        &mut self,
        entries: Vec<Record>,
        placement: PopupPlacement,
        shown: Option<Shown>,
        cx: &mut Context<Self>,
    ) {
        let app = cx.entity();
        let visible = shown.is_some();
        let options = overlay_options(&placement, visible);
        match cx.open_window(options, move |window, cx| {
            let view = cx.new(|cx| PopupView::new(app, entries, placement, shown, cx));
            if visible {
                let focus = view.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
                window.activate();
            }
            view
        }) {
            Ok(handle) => self.popup = Some(handle),
            Err(err) => {
                journal::error(&format!("Failed to open the popup: {err}"));
                // The closure, and its reply sender, are dropped with the
                // failed open. `Prompt` then fails instead of waiting.
            }
        }
    }
}

/// What a visible popup was opened for.
struct Shown {
    widget: String,
    multi: Option<MultiSelect>,
    search: Vec<DataPath>,
    ranking: Ranking,
    priors: Priors,
    /// The rows come from the provider, so a refresh may reload them.
    live: bool,
    reply: Option<Reply>,
}

/// Opens the usage store when a widget ranks by past picks.
fn open_usage(widgets: &Widgets) -> Option<Arc<Mutex<UsageStore>>> {
    if !widgets.uses_usage() {
        return None;
    }
    let path = usage::default_usage_path();
    match UsageStore::open(&path) {
        Ok(store) => Some(Arc::new(Mutex::new(store))),
        Err(err) => {
            journal::error(&format!(
                "failed to open {}: {err}; rows are not ranked by past picks",
                path.display()
            ));
            None
        }
    }
}

/// The rows to show (`records`, or the provider's) and their rank scores.
fn load_entries(
    def: &WidgetDef,
    records: Option<Vec<Record>>,
    usage: Option<&Mutex<UsageStore>>,
) -> Result<(Vec<Record>, Priors), String> {
    let (entries, python) = match records {
        Some(records) => (records, HashMap::new()),
        None => fetch_scored(def)?,
    };
    let priors = Priors::load(&def.ranking, &def.name, &python, usage, usage::now_secs());
    Ok((entries, priors))
}

fn finish_prompt(reply: Option<Reply>, result: Result<Vec<String>, String>) {
    if let Some(reply) = reply {
        if reply.send(result).is_err() {
            journal::info("dropped a prompt reply; the waiter is gone");
        }
    }
}

struct PopupPlacement {
    display_id: DisplayId,
    output_size: gpui::Size<Pixels>,
    card: Bounds<Pixels>,
    label_max: usize,
    theme: Theme,
    layout: Arc<Layout>,
    location: Location,
}

fn popup_placement(
    cx: &App,
    pointer: Option<(i32, i32)>,
    rows: usize,
    label_max: usize,
    theme: Theme,
    layout: Arc<Layout>,
    location: Location,
) -> Option<PopupPlacement> {
    let displays = cx.displays();
    if displays.is_empty() {
        return None;
    }
    let primary = cx
        .primary_display()
        .and_then(|primary| displays.iter().position(|d| d.id() == primary.id()))
        .unwrap_or(0);

    let output_positions = output_layout();
    let mut display_ids = Vec::with_capacity(displays.len());
    let mut outputs = Vec::with_capacity(displays.len());
    for display in &displays {
        let bounds = display.bounds();
        let mut x = pixels_i32(bounds.origin.x);
        let mut y = pixels_i32(bounds.origin.y);
        let id = display.uuid().ok();
        if let Some(id) = id {
            if let Some(&(global_x, global_y)) = output_positions.positions.get(&id) {
                x = global_x;
                y = global_y;
            }
        }
        display_ids.push(id);
        outputs.push(OutputGeom::new(
            x,
            y,
            pixels_i32(bounds.size.width).max(1),
            pixels_i32(bounds.size.height).max(1),
        ));
    }

    let (width, height) = layout.card_size(&theme, rows, label_max);
    let output_index = if pointer.is_none() {
        focused_display_index(&display_ids, output_positions.focused, primary)
    } else {
        primary
    };
    let (index, margin_x, margin_y) = place_at_location(
        location,
        pointer,
        width as i32,
        height as i32,
        &outputs,
        output_index,
    )?;
    let output_size = displays[index].bounds().size;

    Some(PopupPlacement {
        display_id: displays[index].id(),
        output_size,
        card: Bounds::new(
            point(px(margin_x as f32), px(margin_y as f32)),
            size(px(width), px(height)),
        ),
        label_max,
        theme,
        layout,
        location,
    })
}

fn overlay_options(placement: &PopupPlacement, show: bool) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            placement.output_size,
        ))),
        titlebar: None,
        focus: show,
        show,
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: "klemmbrett".into(),
            layer: Layer::Overlay,
            anchor: Anchor::LEFT.union(Anchor::TOP),
            exclusive_zone: Some(px(0.)),
            exclusive_edge: None,
            margin: Some((px(0.), px(0.), px(0.), px(0.))),
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
        }),
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: Some(placement.display_id),
        app_id: Some("klemmbrett".into()),
        window_decorations: Some(WindowDecorations::Client),
        linux_window_background: LinuxWindowBackground::Transparent,
        ..Default::default()
    }
}

fn pixels_i32(value: Pixels) -> i32 {
    f32::from(value).round() as i32
}

struct PopupView {
    focus_handle: FocusHandle,
    app: Entity<Klemmbrett>,
    entries: Vec<Record>,
    selected: usize,
    label_max: usize,
    theme: Theme,
    layout: Arc<Layout>,
    widget_name: String,
    multi: Option<MultiSelect>,
    search: Vec<DataPath>,
    ranking: Ranking,
    priors: Priors,
    /// Indices into `entries` that match `query`, in the order shown.
    order: Vec<usize>,
    live: bool,
    query: String,
    display_id: DisplayId,
    card: Bounds<Pixels>,
    location: Location,
    output_size: gpui::Size<Pixels>,
    visible: bool,
    seen_active: bool,
    scroll: UniformListScrollHandle,
    checked: Vec<Record>,
    /// Key of the row highlight handlers last ran for (or the row the popup opened on).
    highlighted: Option<String>,
    reply: Option<Reply>,
}

impl PopupView {
    fn new(
        app: Entity<Klemmbrett>,
        entries: Vec<Record>,
        placement: PopupPlacement,
        shown: Option<Shown>,
        cx: &mut Context<Self>,
    ) -> Self {
        let visible = shown.is_some();
        let Shown {
            widget,
            multi,
            search,
            ranking,
            priors,
            live,
            reply,
        } = shown.unwrap_or(Shown {
            widget: String::new(),
            multi: None,
            search: Vec::new(),
            ranking: Ranking::default(),
            priors: Priors::default(),
            live: false,
            reply: None,
        });
        let mut view = Self {
            focus_handle: cx.focus_handle(),
            app,
            entries,
            selected: 0,
            label_max: placement.label_max,
            theme: placement.theme,
            layout: placement.layout,
            widget_name: widget,
            multi,
            search,
            ranking,
            priors,
            order: Vec::new(),
            live,
            query: String::new(),
            display_id: placement.display_id,
            card: placement.card,
            location: placement.location,
            output_size: placement.output_size,
            visible,
            seen_active: false,
            scroll: UniformListScrollHandle::new(),
            checked: Vec::new(),
            highlighted: None,
            reply,
        };
        view.refilter();
        view.highlighted = view.first_key();
        view
    }

    fn reload(
        &mut self,
        entries: Vec<Record>,
        placement: &PopupPlacement,
        shown: Shown,
        cx: &mut Context<Self>,
    ) {
        self.finish_prompt(Ok(Vec::new()));
        self.reply = shown.reply;
        self.entries = entries;
        self.card = placement.card;
        self.location = placement.location;
        self.output_size = placement.output_size;
        self.theme = placement.theme.clone();
        self.layout = placement.layout.clone();
        self.label_max = placement.label_max;
        self.widget_name = shown.widget;
        self.multi = shown.multi;
        self.search = shown.search;
        self.ranking = shown.ranking;
        self.priors = shown.priors;
        self.live = shown.live;
        self.query.clear();
        self.refilter();
        self.highlighted = self.first_key();
        self.selected = 0;
        self.checked.clear();
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// The rows matching the query, in the order they are shown.
    fn visible_entries(&self) -> Vec<&Record> {
        self.order
            .iter()
            .map(|index| &self.entries[*index])
            .collect()
    }

    /// Matches and orders the rows again; call after the query, the rows,
    /// or the priors change. Without an input bar the query stays empty.
    fn refilter(&mut self) {
        self.order = rank::order(
            &self.entries,
            &self.query,
            &self.ranking,
            &self.search,
            &self.priors,
        );
    }

    fn first_key(&self) -> Option<String> {
        self.order
            .first()
            .map(|index| self.entries[*index].key.clone())
    }

    /// New rows from the provider while the popup is open. The row under the
    /// cursor stays selected when it still exists, and marks on rows that
    /// still exist are kept (with their new contents).
    fn replace_entries(&mut self, entries: Vec<Record>, priors: Priors, cx: &mut Context<Self>) {
        let current = self
            .visible_entries()
            .get(self.selected)
            .map(|record| record.key.clone());
        self.checked = self
            .checked
            .iter()
            .filter_map(|marked| entries.iter().find(|record| record.key == marked.key))
            .cloned()
            .collect();
        self.entries = entries;
        self.priors = priors;
        self.refilter();
        if let Some(index) = current.and_then(|key| {
            self.visible_entries()
                .iter()
                .position(|record| record.key == key)
        }) {
            self.selected = index;
        }
        self.clamp_selection();
        self.resize_card();
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        self.sync_highlight(cx);
        cx.notify();
    }

    fn reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = true;
        self.seen_active = false;
        window.set_visible(true);
        window.focus(&self.focus_handle, cx);
        window.activate();
        cx.notify();
    }

    fn finish_prompt(&mut self, result: Result<Vec<String>, String>) {
        if let Some(reply) = self.reply.take() {
            if reply.send(result).is_err() {
                journal::info("dropped a prompt reply; the waiter is gone");
            }
        }
    }

    fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        self.finish_prompt(Ok(Vec::new()));
        self.visible = false;
        self.seen_active = false;
        // Layer-shell remap after a null-buffer unmap is broken in GPUI-CE:
        // prepare_to_map is xdg-only, so set_visible(true) never attaches a
        // buffer again. Destroy this surface and build a fresh hidden one.
        window.remove_window();
        let app = self.app.clone();
        app.update(cx, |klemmbrett, _cx| klemmbrett.drop_popup_handle());
        cx.defer(move |cx| {
            app.update(cx, |klemmbrett, cx| {
                if klemmbrett.popup.is_none() {
                    klemmbrett.prewarm_popup(cx);
                }
            });
        });
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let mods = &event.keystroke.modifiers;
        let filter = self.layout.has_inputbar();
        if self.layout.has_listview() && !mods.platform {
            if let Some(multi) = &self.multi {
                if multi
                    .select
                    .matches(key, mods.shift, mods.control, mods.alt)
                {
                    self.toggle_and_move(1, cx);
                    cx.stop_propagation();
                    return;
                }
                if multi
                    .deselect
                    .matches(key, mods.shift, mods.control, mods.alt)
                {
                    self.toggle_and_move(-1, cx);
                    cx.stop_propagation();
                    return;
                }
            }
        }
        match key {
            "escape" => {
                self.hide(window, cx);
                cx.stop_propagation();
                return;
            }
            "enter" => {
                self.confirm(window, cx);
                cx.stop_propagation();
                return;
            }
            "space" if !filter => {
                self.activate(self.selected, window, cx);
                cx.stop_propagation();
                return;
            }
            "up" => {
                self.move_selection(-1, cx);
                cx.stop_propagation();
                return;
            }
            "down" => {
                self.move_selection(1, cx);
                cx.stop_propagation();
                return;
            }
            "backspace" if filter => {
                self.query.pop();
                self.refilter();
                self.clamp_selection();
                self.sync_highlight(cx);
                cx.notify();
                cx.stop_propagation();
                return;
            }
            _ => {}
        }

        if mods.control || mods.alt || mods.platform {
            return;
        }
        if filter {
            if let Some(ch) = typed_filter_char(event) {
                self.query.push(ch);
                self.refilter();
                self.clamp_selection();
                self.sync_highlight(cx);
                cx.notify();
                cx.stop_propagation();
            }
            return;
        }
        let typed = event
            .keystroke
            .key_char
            .as_deref()
            .and_then(|s| s.chars().next())
            .or_else(|| key.chars().next());
        if let Some(ch) = typed {
            if let Some(index) = index_for_accelerator(ch, self.entries.len()) {
                self.activate(index, window, cx);
                cx.stop_propagation();
            }
        }
    }

    fn clamp_selection(&mut self) {
        let count = self.visible_entries().len();
        if count == 0 {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(count - 1);
        }
    }

    fn move_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        let count = self.visible_entries().len() as i32;
        if count == 0 {
            return;
        }
        self.selected = (self.selected as i32 + delta).rem_euclid(count) as usize;
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        self.sync_highlight(cx);
        cx.notify();
    }

    /// Tells the widget's highlight handler when the selected row changes.
    /// `Prompt` popups only return values, so they skip it like other handlers.
    fn sync_highlight(&mut self, cx: &mut Context<Self>) {
        let Some(record) = self
            .visible_entries()
            .get(self.selected)
            .map(|record| (*record).clone())
        else {
            return;
        };
        if self.highlighted.as_deref() == Some(record.key.as_str()) {
            return;
        }
        self.highlighted = Some(record.key.clone());
        if self.reply.is_some() {
            return;
        }
        let widget = self.widget_name.clone();
        self.app
            .update(cx, |app, _cx| app.highlight(&widget, record));
    }

    fn restyle(&mut self, theme: Theme) {
        if theme == self.theme {
            return;
        }
        self.theme = theme;
        self.resize_card();
    }

    /// Fits the card to the theme and row count, anchored where it was opened.
    fn resize_card(&mut self) {
        let theme = &self.theme;
        let (width, height) = self
            .layout
            .card_size(theme, self.entries.len(), self.label_max);
        let old = self.card;
        let (x, y) = resize_at_location(
            self.location,
            (f32::from(old.origin.x), f32::from(old.origin.y)),
            (f32::from(old.size.width), f32::from(old.size.height)),
            (width, height),
            (
                f32::from(self.output_size.width),
                f32::from(self.output_size.height),
            ),
        );
        self.card = Bounds::new(point(px(x), px(y)), size(px(width), px(height)));
    }

    fn toggle_and_move(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some(record) = self
            .visible_entries()
            .get(self.selected)
            .map(|record| (*record).clone())
        else {
            return;
        };
        toggle_record(&mut self.checked, &record);
        self.move_selection(delta, cx);
        cx.notify();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.layout.has_listview() {
            let text = self.query.clone();
            self.submit_records(vec![Record::new(text)], window, cx);
            return;
        }
        if self.multi.is_some() {
            let visible = self.visible_entries();
            let current = visible.get(self.selected).copied();
            let records: Vec<Record> = activation_records(&self.checked, current)
                .into_iter()
                .cloned()
                .collect();
            self.submit_records(records, window, cx);
            return;
        }
        self.activate(self.selected, window, cx);
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(record) = self
            .visible_entries()
            .get(index)
            .map(|record| (*record).clone())
        else {
            return;
        };
        self.submit_records(vec![record], window, cx);
    }

    fn submit_records(
        &mut self,
        records: Vec<Record>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if records.is_empty() {
            return;
        }
        if self.layout.has_listview() {
            let widget = self.widget_name.clone();
            let keys = records.iter().map(|record| record.key.clone()).collect();
            let query = self.query.clone();
            self.app
                .update(cx, |app, cx| app.record_usage(&widget, keys, query, cx));
        }
        if self.reply.is_some() {
            let values = records.iter().map(|record| record.value.clone()).collect();
            self.finish_prompt(Ok(values));
        } else {
            let widget = self.widget_name.clone();
            self.app.update(cx, |app, _cx| app.select(&widget, records));
        }
        self.hide(window, cx);
    }

    fn rows(&self, visible: &[&Record], cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let layout = self.layout.clone();
        let empty;
        let (records, clickable): (Vec<&Record>, bool) = if visible.is_empty() {
            empty = Record {
                key: String::new(),
                label: "No entries".into(),
                value: String::new(),
                data: serde_json::Value::Null,
            };
            (vec![&empty], false)
        } else {
            (visible.to_vec(), true)
        };
        let show_accelerators = !layout.has_inputbar() && clickable;
        let rows: Vec<RowData> = records
            .iter()
            .enumerate()
            .map(|(index, record)| RowData {
                record: (*record).clone(),
                label_max: self.label_max,
                accelerator: show_accelerators.then(|| accelerator_for(index)).flatten(),
                marked: self.checked.iter().any(|item| item.key == record.key),
            })
            .collect();
        let selected = self.selected;
        let view = cx.weak_entity();
        uniform_list("popup-rows", rows.len(), move |range, _window, _cx| {
            range
                .map(|index| {
                    popup_row(
                        index,
                        &rows[index],
                        clickable && index == selected,
                        &layout,
                        &theme,
                        clickable.then(|| activate_row(view.clone(), index)),
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

impl Drop for PopupView {
    fn drop(&mut self) {
        self.finish_prompt(Ok(Vec::new()));
    }
}

impl Focusable for PopupView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PopupView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.visible {
            if window.is_window_active() {
                self.seen_active = true;
            } else if self.seen_active {
                self.hide(window, cx);
            }
        }

        let theme = self.theme.clone();
        let layout = self.layout.clone();
        let visible: Vec<Record> = self.visible_entries().into_iter().cloned().collect();
        let mut rows = layout
            .has_listview()
            .then(|| self.rows(&visible.iter().collect::<Vec<_>>(), cx));

        let card = self.card;
        let window_style = layout.window_style(&theme);
        let font_size = window_style.font_size.unwrap_or(14.0);
        let mut card_el = apply_box(
            div()
                .id("klemmbrett-popup")
                .absolute()
                .left(card.origin.x)
                .top(card.origin.y)
                .w(card.size.width)
                .h(card.size.height)
                .overflow_hidden()
                .text_size(px(font_size))
                .line_height(px(layout.row_extent(&theme)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
            &window_style,
        );
        let count = visible.len().to_string();
        let chrome = Chrome {
            theme: &theme,
            fields: [
                ("widget", self.widget_name.as_str()),
                ("query", self.query.as_str()),
                ("count", count.as_str()),
            ],
            widget: &self.widget_name,
            query: &self.query,
        };
        for child in &layout.root().children {
            if let Some(element) = chrome.render(child, &window_style, &mut rows) {
                card_el = card_el.child(element);
            }
        }
        apply_box(
            div()
                .id("klemmbrett-overlay")
                .track_focus(&self.focus_handle)
                .size_full()
                .relative(),
            theme.overlay(),
        )
        .on_key_down(cx.listener(|this, event, window, cx| this.on_key(event, window, cx)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, window, cx| this.hide(window, cx)),
        )
        .child(card_el)
    }
}

fn typed_filter_char(event: &KeyDownEvent) -> Option<char> {
    let ch = event.keystroke.key_char.as_deref()?.chars().next()?;
    (!ch.is_control()).then_some(ch)
}

/// Draws the nodes outside the rows: the input bar and window-level text.
struct Chrome<'a> {
    theme: &'a Theme,
    fields: [(&'static str, &'a str); 3],
    widget: &'a str,
    query: &'a str,
}

impl Chrome<'_> {
    fn render(
        &self,
        node: &Node,
        parent: &Style,
        rows: &mut Option<AnyElement>,
    ) -> Option<AnyElement> {
        Some(match &node.kind {
            NodeKind::ListView => return rows.take(),
            NodeKind::InputBar => {
                let style = self.theme.inputbar().with(&node.style);
                let mut bar = apply_box(
                    div()
                        .w_full()
                        .h(px(style.height.unwrap_or(36.0)))
                        .flex_none()
                        .overflow_hidden(),
                    &style,
                );
                for child in &node.children {
                    if let Some(element) = self.render(child, &style, rows) {
                        bar = bar.child(element);
                    }
                }
                bar.into_any()
            }
            NodeKind::Prompt => {
                let style = self.theme.prompt().with(&node.style);
                let text = style.text.clone().unwrap_or_else(|| self.widget.to_owned());
                apply_box(div().flex_none().whitespace_nowrap().child(text), &style).into_any()
            }
            NodeKind::Entry => apply_box(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(format!("{}│", self.query)),
                &self.theme.entry().with(&node.style),
            )
            .into_any(),
            NodeKind::Text(template) => {
                let style = Style::plain(parent, Orientation::Vertical).with(&node.style);
                text_box(&style)
                    .child(expand(template, &self.fields))
                    .into_any()
            }
            NodeKind::Spacer => spacer(&node.style.width, &node.style.height),
            NodeKind::Row | NodeKind::Column => {
                let style = container_style(node, parent);
                let mut el = container(&style, parent);
                for child in &node.children {
                    if let Some(element) = self.render(child, &style, rows) {
                        el = el.child(element);
                    }
                }
                el.into_any()
            }
            _ => return None,
        })
    }
}

struct RowData {
    record: Record,
    label_max: usize,
    accelerator: Option<char>,
    marked: bool,
}

fn activate_row(
    view: WeakEntity<PopupView>,
    index: usize,
) -> impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static {
    move |_, window, cx| {
        if let Err(err) = view.update(cx, |this, cx| this.activate(index, window, cx)) {
            journal::error(&format!("failed to activate popup row {index}: {err}"));
        }
    }
}

fn popup_row(
    index: usize,
    data: &RowData,
    selected: bool,
    layout: &Layout,
    theme: &Theme,
    on_click: Option<impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static>,
) -> gpui::Stateful<gpui::Div> {
    let style = layout.row_style(theme, selected);
    let hover = theme.element_hover().background;
    let mut row = apply_box(
        div()
            .id(index)
            .w_full()
            .h(px(layout.row_extent(theme)))
            .flex_none()
            .overflow_hidden(),
        &style,
    );
    if let Some(on_click) = on_click {
        row = row
            .hover(move |row| row.bg(paint(hover)))
            .on_mouse_down(MouseButton::Left, on_click);
    }
    if let Some(element) = layout.element() {
        for child in &element.children {
            row = row.child(row_node(child, &style, data, theme));
        }
    }
    row
}

fn row_node(node: &Node, parent: &Style, data: &RowData, theme: &Theme) -> AnyElement {
    match &node.kind {
        NodeKind::Marker => {
            let style = theme.element_marker().with(&node.style);
            let mark = if data.marked {
                style.text.clone().unwrap_or_else(|| ">".into())
            } else {
                String::new()
            };
            column_box(&style, 16.0).child(mark).into_any()
        }
        NodeKind::Index => {
            let style = theme.element_index().with(&node.style);
            column_box(&style, 16.0)
                .child(data.accelerator.map(|c| c.to_string()).unwrap_or_default())
                .into_any()
        }
        NodeKind::Text(template) => {
            let style = theme.element_text().with(&node.style);
            text_box(&style)
                .child(expand_record(template, &data.record, data.label_max))
                .into_any()
        }
        NodeKind::Field { path, fallback } => {
            let style = theme.element_text().with(&node.style);
            let text = data
                .record
                .text(path)
                .map(|text| shorten_label(&text, data.label_max))
                .unwrap_or_else(|| fallback.clone());
            text_box(&style).child(text).into_any()
        }
        NodeKind::Spacer => spacer(&node.style.width, &node.style.height),
        NodeKind::Row | NodeKind::Column => {
            let style = container_style(node, parent);
            let mut el = container(&style, parent);
            for child in &node.children {
                el = el.child(row_node(child, &style, data, theme));
            }
            el.into_any()
        }
        _ => div().into_any(),
    }
}

/// A fixed-width column such as the marker or accelerator.
fn column_box(style: &Style, default_width: f32) -> gpui::Div {
    apply_box(
        div()
            .w(px(style.width.unwrap_or(default_width)))
            .flex_none()
            .whitespace_nowrap(),
        style,
    )
}

/// Text takes its natural width and shrinks with an ellipsis, unless it has a width.
fn text_box(style: &Style) -> gpui::Div {
    let el = apply_box(div(), style);
    let el = match style.width {
        Some(width) => el.w(px(width)).flex_none(),
        None => el.flex_shrink(1.0).min_w_0(),
    };
    el.truncate()
}

fn spacer(width: &Option<f32>, height: &Option<f32>) -> AnyElement {
    let mut el = div();
    el = match width {
        Some(width) => el.w(px(*width)).flex_none(),
        None => el.flex_1(),
    };
    if let Some(height) = height {
        el = el.h(px(*height));
    }
    el.into_any()
}

fn container_style(node: &Node, parent: &Style) -> Style {
    let orientation = if node.kind == NodeKind::Row {
        Orientation::Horizontal
    } else {
        Orientation::Vertical
    };
    Style::plain(parent, orientation).with(&node.style)
}

fn container(style: &Style, parent: &Style) -> gpui::Div {
    let el = apply_box(div(), style);
    let el = match parent.orientation {
        Orientation::Horizontal => el.flex_1().min_w_0(),
        Orientation::Vertical => el.w_full(),
    };
    let el = match style.width {
        Some(width) => el.w(px(width)).flex_none(),
        None => el,
    };
    match style.height {
        Some(height) => el.h(px(height)),
        None => el,
    }
}

fn apply_box<E: Styled>(element: E, style: &Style) -> E {
    let mut element = apply_padding(element, style.padding)
        .bg(paint(style.background))
        .border_color(paint(style.border_color))
        .rounded(px(style.border_radius))
        .text_color(paint(style.text_color));
    element = apply_border(element, style.border);
    match style.orientation {
        Orientation::Horizontal => element
            .flex()
            .flex_row()
            .items_center()
            .gap(px(style.spacing)),
        Orientation::Vertical => element.flex().flex_col().gap(px(style.spacing)),
    }
}

fn apply_padding<E: Styled>(element: E, padding: Edges) -> E {
    element
        .pt(px(padding.top))
        .pr(px(padding.right))
        .pb(px(padding.bottom))
        .pl(px(padding.left))
}

fn apply_border<E: Styled>(element: E, border: Edges) -> E {
    if border.top == border.right && border.right == border.bottom && border.bottom == border.left {
        element.border(px(border.top))
    } else {
        element
            .border_l(px(border.left))
            .border_r(px(border.right))
            .border_t(px(border.top))
            .border_b(px(border.bottom))
    }
}

fn paint(color: crate::Rgba) -> gpui::Rgba {
    rgba(color.to_u32())
}

const TOAST_GAP: f32 = 8.0;
const TOAST_ACTION_HEIGHT: f32 = 24.0;

#[derive(Clone)]
struct Toast {
    notification: Notification,
    seq: u64,
}

impl Toast {
    fn actions(&self) -> impl Iterator<Item = &(String, String)> {
        self.notification
            .actions
            .iter()
            .filter(|(key, _)| key != "default")
    }
}

/// The stacked toasts, newest nearest the anchor. Never takes the keyboard.
struct ToastView {
    app: Entity<Klemmbrett>,
    toasts: Vec<Toast>,
    theme: Theme,
    max_visible: usize,
    plates: Vec<(crate::toast_plate::Key, std::sync::Arc<RenderImage>)>,
}

impl ToastView {
    /// An update keeps the toast's place; a new one goes first.
    fn upsert(&mut self, toast: Toast) {
        match self
            .toasts
            .iter_mut()
            .find(|shown| shown.notification.id == toast.notification.id)
        {
            Some(shown) => *shown = toast,
            None => self.toasts.insert(0, toast),
        }
    }

    fn visible(&self) -> &[Toast] {
        &self.toasts[..self.toasts.len().min(self.max_visible)]
    }

    fn card_height(&self, toast: &Toast) -> f32 {
        let card = self.theme.toast();
        let mut height = card.padding.vertical()
            + card.border.vertical()
            + self.theme.toast_summary().height.unwrap_or(20.0);
        if !toast.notification.body.is_empty() {
            height += card.spacing + self.theme.toast_body().height.unwrap_or(36.0);
        }
        if toast.actions().next().is_some() {
            height += card.spacing + TOAST_ACTION_HEIGHT;
        }
        height
    }

    fn size(&self) -> gpui::Size<Pixels> {
        let visible = self.visible();
        let cards: f32 = visible.iter().map(|toast| self.card_height(toast)).sum();
        let gaps = TOAST_GAP * visible.len().saturating_sub(1) as f32;
        let width = self.theme.toast().width.unwrap_or(360.0);
        size(px(width), px((cards + gaps).max(1.0)))
    }

    fn plate(
        &mut self,
        width: f32,
        height: f32,
        radius: f32,
        border: f32,
        fill: crate::Rgba,
        stroke: crate::Rgba,
        scale: f32,
    ) -> std::sync::Arc<RenderImage> {
        let key = crate::toast_plate::Key::new(width, height, radius, border, fill, stroke, scale);
        if let Some((_, image)) = self.plates.iter().find(|(cached, _)| *cached == key) {
            return image.clone();
        }
        let image = crate::toast_plate::rasterize(key);
        let image = std::sync::Arc::new(image);
        self.plates.push((key, image.clone()));
        if self.plates.len() > 8 {
            self.plates.remove(0);
        }
        image
    }
}

fn toast_options(size: gpui::Size<Pixels>, location: Location, margin: Edges) -> WindowOptions {
    let anchor = match location {
        Location::NorthWest => Anchor::TOP | Anchor::LEFT,
        Location::North => Anchor::TOP,
        Location::SouthEast => Anchor::BOTTOM | Anchor::RIGHT,
        Location::South => Anchor::BOTTOM,
        Location::SouthWest => Anchor::BOTTOM | Anchor::LEFT,
        Location::West => Anchor::LEFT,
        Location::East => Anchor::RIGHT,
        Location::NorthEast | Location::Center | Location::Pointer => Anchor::TOP | Anchor::RIGHT,
    };
    let margin = (
        px(margin.top),
        px(margin.right),
        px(margin.bottom),
        px(margin.left),
    );
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            size,
        ))),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: "klemmbrett-notifications".into(),
            layer: Layer::Overlay,
            anchor,
            exclusive_zone: Some(px(0.)),
            exclusive_edge: None,
            margin: Some(margin),
            keyboard_interactivity: KeyboardInteractivity::None,
        }),
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        app_id: Some("klemmbrett".into()),
        window_decorations: Some(WindowDecorations::Client),
        linux_window_background: LinuxWindowBackground::Transparent,
        ..Default::default()
    }
}

impl Render for ToastView {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let card_style = self.theme.toast().clone();
        let summary_style = self.theme.toast_summary().clone();
        let body_style = self.theme.toast_body().clone();
        let selected_style = self.theme.element_selected().clone();
        let mut column = div().size_full().flex().flex_col().gap(px(TOAST_GAP));
        let visible = self.visible().to_vec();
        for toast in visible {
            let id = toast.notification.id;
            let default = toast
                .notification
                .has_default_action()
                .then(|| "default".to_string());
            let app = self.app.clone();
            let dismiss = self.app.clone();
            let summary_height = summary_style.height.unwrap_or(20.0);
            let body_height = body_style.height.unwrap_or(36.0);
            let height = self.card_height(&toast);
            let scale = window.scale_factor();
            let card_width = card_style.width.unwrap_or(360.0);
            let radius = card_style.border_radius;
            let border = card_style.border.top;
            let fill = card_style.background;
            let stroke = card_style.border_color;
            let plate = self.plate(card_width, height, radius, border, fill, stroke, scale);
            let painted = plate.clone();
            let mut card = apply_padding(
                div()
                    .size_full()
                    .overflow_hidden()
                    .text_size(px(card_style.font_size.unwrap_or(14.0)))
                    .text_color(paint(card_style.text_color))
                    .flex()
                    .flex_col()
                    .gap(px(card_style.spacing)),
                Edges {
                    top: card_style.padding.top + card_style.border.top,
                    right: card_style.padding.right + card_style.border.right,
                    bottom: card_style.padding.bottom + card_style.border.bottom,
                    left: card_style.padding.left + card_style.border.left,
                },
            )
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                let action = default.clone();
                app.update(cx, |app, cx| app.toast_clicked(id, action, cx));
            })
            .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                dismiss.update(cx, |app, cx| app.toast_clicked(id, None, cx));
            })
            .child(
                apply_box(
                    div()
                        .w_full()
                        .h(px(summary_height))
                        .line_height(px(summary_height))
                        .font_weight(gpui::FontWeight::BOLD)
                        .truncate(),
                    &summary_style,
                )
                .child(toast.notification.summary.clone()),
            );
            if !toast.notification.body.is_empty() {
                card = card.child(
                    apply_box(
                        div()
                            .w_full()
                            .h(px(body_height))
                            .line_height(px(body_height / 2.0))
                            .overflow_hidden()
                            .line_clamp(2)
                            .text_ellipsis(),
                        &body_style,
                    )
                    .child(toast.notification.body.clone()),
                );
            }
            if toast.actions().next().is_some() {
                let mut row = div()
                    .w_full()
                    .h(px(TOAST_ACTION_HEIGHT))
                    .flex()
                    .flex_row()
                    .gap(px(8.0));
                for (key, label) in toast.actions() {
                    let app = self.app.clone();
                    let key = key.clone();
                    let selected = &selected_style;
                    row = row.child(
                        apply_box(
                            div()
                                .id(gpui::SharedString::from(format!("toast-{id}-{key}")))
                                .h_full()
                                .px(px(8.0))
                                .flex_none()
                                .whitespace_nowrap(),
                            selected,
                        )
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            cx.stop_propagation();
                            let action = Some(key.clone());
                            app.update(cx, |app, cx| app.toast_clicked(id, action, cx));
                        })
                        .child(label.clone()),
                    );
                }
                card = card.child(row);
            }
            column = column.child(
                div()
                    .id(("toast", id as usize))
                    .relative()
                    .w_full()
                    .h(px(height))
                    .flex_none()
                    .child(
                        canvas(
                            move |_bounds, _window, _app| painted,
                            move |bounds, plate, window, _app| {
                                let _ = window.paint_image(
                                    bounds,
                                    bounds,
                                    Corners::default(),
                                    0.0,
                                    plate,
                                    0,
                                    false,
                                );
                            },
                        )
                        .absolute()
                        .top(px(0.))
                        .left(px(0.))
                        .right(px(0.))
                        .bottom(px(0.)),
                    )
                    .child(card),
            );
        }
        column
    }
}

fn install_tray(widgets: Vec<String>, tx: async_channel::Sender<UiCmd>) {
    let icons = tray_icon_pixmaps();
    match (TrayIcon { widgets, tx, icons }).spawn() {
        Ok(handle) => {
            // The tray lives for the whole session; dropping it would remove the icon.
            std::mem::forget(handle);
        }
        Err(err) => journal::error(&format!(
            "Failed to install the StatusNotifierItem tray: {err}"
        )),
    }
}

struct TrayIcon {
    widgets: Vec<String>,
    tx: async_channel::Sender<UiCmd>,
    icons: Vec<ksni::Icon>,
}

impl ksni::Tray for TrayIcon {
    // Left-click should open the menu, not only the host's context-click.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "klemmbrett".into()
    }

    fn title(&self) -> String {
        "klemmbrett".into()
    }

    fn icon_name(&self) -> String {
        TRAY_ICON_NAME.into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icons.clone()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::ApplicationStatus
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            icon_name: TRAY_ICON_NAME.into(),
            title: "Clipboard history".into(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        let mut items = Vec::new();
        if !self.widgets.is_empty() {
            items.push(
                SubMenu {
                    label: "Widgets".into(),
                    submenu: self
                        .widgets
                        .iter()
                        .map(|name| {
                            let name = name.clone();
                            StandardItem {
                                label: name.clone(),
                                activate: Box::new(move |this: &mut Self| {
                                    send_ui(
                                        &this.tx,
                                        UiCmd::Show {
                                            widget: name.clone(),
                                            pointer: None,
                                            records: None,
                                            reply: None,
                                        },
                                        "open a widget from the tray",
                                    );
                                }),
                                ..Default::default()
                            }
                            .into()
                        })
                        .collect(),
                    ..Default::default()
                }
                .into(),
            );
            items.push(MenuItem::Separator);
        }
        items.push(
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|this: &mut Self| {
                    send_ui(&this.tx, UiCmd::Quit, "quit from the tray");
                }),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

fn tray_icon_pixmaps() -> Vec<ksni::Icon> {
    TRAY_ICON_SIZES
        .into_iter()
        .filter_map(drawn_clipboard_icon)
        .collect()
}

fn drawn_clipboard_icon(size: i32) -> Option<ksni::Icon> {
    if size < 8 {
        return None;
    }
    let size_u = size as usize;
    let mut rgba = vec![0_u8; size_u * size_u * 4];
    let paint = |pixels: &mut [u8], x: i32, y: i32, r: u8, g: u8, b: u8, a: u8| {
        if x < 0 || y < 0 || x >= size || y >= size {
            return;
        }
        let i = ((y as usize) * size_u + x as usize) * 4;
        pixels[i] = r;
        pixels[i + 1] = g;
        pixels[i + 2] = b;
        pixels[i + 3] = a;
    };

    let inset = (size / 5).max(1);
    let clip_h = (size / 5).max(2);
    let clip_inset = inset + 1;
    for y in inset..size - inset {
        for x in inset..size - inset {
            paint(&mut rgba, x, y, 0xee, 0xee, 0xee, 0xff);
        }
    }
    for y in inset / 2..inset / 2 + clip_h {
        for x in clip_inset..size - clip_inset {
            paint(&mut rgba, x, y, 0xbb, 0xbb, 0xbb, 0xff);
        }
    }

    let data = rgba_to_argb(&rgba, size, size, 4, size * 4, true)?;
    Some(ksni::Icon {
        width: size,
        height: size,
        data,
    })
}

/// StatusNotifierItem pixmaps are ARGB32 in network byte order.
fn rgba_to_argb(
    pixels: &[u8],
    width: i32,
    height: i32,
    n_channels: i32,
    rowstride: i32,
    has_alpha: bool,
) -> Option<Vec<u8>> {
    if width <= 0 || height <= 0 || n_channels < 3 {
        return None;
    }
    let width = width as usize;
    let height = height as usize;
    let n_channels = n_channels as usize;
    let rowstride = rowstride as usize;
    let mut data = Vec::with_capacity(width.checked_mul(height)?.checked_mul(4)?);
    for y in 0..height {
        let row = y.checked_mul(rowstride)?;
        for x in 0..width {
            let i = row.checked_add(x.checked_mul(n_channels)?)?;
            let needed = if has_alpha { 4 } else { 3 };
            if i.checked_add(needed)? > pixels.len() {
                return None;
            }
            let r = pixels[i];
            let g = pixels[i + 1];
            let b = pixels[i + 2];
            let a = if has_alpha { pixels[i + 3] } else { 255 };
            data.extend_from_slice(&[a, r, g, b]);
        }
    }
    Some(data)
}

#[cfg(test)]
mod tests {
    use super::rgba_to_argb;

    #[test]
    fn rgba_pixels_become_network_order_argb() {
        // One opaque red pixel, tightly packed.
        let data = rgba_to_argb(&[255, 0, 0, 255], 1, 1, 4, 4, true);
        assert_eq!(data, Some(vec![255, 255, 0, 0]));
    }

    #[test]
    fn rgb_rows_pad_alpha_and_skip_stride() {
        // One green pixel, then 2 bytes of row padding.
        let pixels = [0, 255, 0, 9, 9];
        let data = rgba_to_argb(&pixels, 1, 1, 3, 5, false);
        assert_eq!(data, Some(vec![255, 0, 255, 0]));
    }

    #[test]
    fn a_truncated_row_is_rejected() {
        assert_eq!(rgba_to_argb(&[255, 0], 1, 1, 4, 4, true), None);
    }
}
