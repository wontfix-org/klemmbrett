//! The history popup and the tray icon.
//!
//! The popup is a layer-shell surface that asks for exclusive keyboard
//! interactivity, with the history rows packed *inside* it. It deliberately
//! does not use an `xdg_popup`: taking the keyboard grab an `xdg_popup` needs
//! requires the serial of a real input event. The popup is opened from D-Bus,
//! so no such serial exists and the grab silently does nothing until the
//! pointer happens to enter the surface. Layer-shell's keyboard interactivity
//! is the protocol's supported way to focus a surface that was not opened from
//! user input, so the rows and their key handling live here instead.

use crate::dbus::{self, UiCmd};
use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui::{
    div, point, prelude::*, px, rgb, size, transparent_black, uniform_list, App, Bounds, Context,
    DisplayId, Entity, FocusHandle, Focusable, KeyDownEvent, LinuxWindowBackground, MouseButton,
    Pixels, QuitMode, ScrollStrategy, UniformListScrollHandle, WeakEntity, Window, WindowBounds,
    WindowDecorations, WindowHandle, WindowKind, WindowOptions,
};
use klemmbrett::{
    history_candidate, index_for_accelerator, menu_item_spec, place_on_outputs, ArboardBackend,
    Backend, ClipboardSync, OutputGeom, Record, Selection, Sources, POLL_INTERVAL,
};
use ksni::blocking::TrayMethods;
use std::collections::HashSet;
use std::sync::mpsc::{self, RecvTimeoutError};

const TRAY_ICON_NAME: &str = "edit-paste";
const TRAY_ICON_SIZES: [i32; 4] = [16, 22, 24, 32];
const ROW_HEIGHT: f32 = 28.0;
const CHAR_WIDTH: f32 = 9.0;
const BORDER: f32 = 1.0;
const MAX_POPUP_HEIGHT: f32 = 600.0;

enum ClipCmd {
    Restore(String),
}

pub fn run(sync: ClipboardSync<ArboardBackend>, sources: Sources, label_max: usize) {
    let (tx, rx) = async_channel::unbounded::<UiCmd>();
    let source_names: HashSet<_> = sources.names().into_iter().collect();
    dbus::spawn(source_names, tx.clone());
    install_tray(sources.names(), tx.clone());
    let clip_tx = spawn_clipboard(sync, tx);

    gpui_platform::application()
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx: &mut App| {
            let app = cx.new(|_cx| Klemmbrett::new(sources, label_max, clip_tx));
            app.update(cx, |this, cx| this.prewarm_popup(cx));

            let dispatcher = app.clone();
            cx.spawn(async move |cx| {
                while let Ok(cmd) = rx.recv().await {
                    dispatcher.update(cx, |this, cx| match cmd {
                        UiCmd::ShowMenu { source, x, y } => {
                            this.show_source_menu(&source, x, y, cx)
                        }
                        UiCmd::RecordHistory(text) => this.record_history(&text),
                        UiCmd::Quit => cx.quit(),
                    });
                }
            })
            .detach();
        });
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
            let _ = ui.send_blocking(UiCmd::RecordHistory(seed));
            loop {
                match rx.recv_timeout(POLL_INTERVAL) {
                    Ok(ClipCmd::Restore(text)) => {
                        if let Err(err) = sync.backend_mut().write(Selection::Clipboard, &text) {
                            eprintln!("Failed to restore the clipboard: {err}");
                            continue;
                        }
                        let _ = ui.send_blocking(UiCmd::RecordHistory(text));
                        let action = sync.tick();
                        let last = sync.last_text().to_owned();
                        if let Some(text) = history_candidate(&action, &last) {
                            let _ = ui.send_blocking(UiCmd::RecordHistory(text.to_owned()));
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        let action = sync.tick();
                        let last = sync.last_text().to_owned();
                        if let Some(text) = history_candidate(&action, &last) {
                            let _ = ui.send_blocking(UiCmd::RecordHistory(text.to_owned()));
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
    sources: Sources,
    label_max: usize,
    popup: Option<WindowHandle<PopupView>>,
    clip_tx: mpsc::Sender<ClipCmd>,
}

impl Klemmbrett {
    fn new(sources: Sources, label_max: usize, clip_tx: mpsc::Sender<ClipCmd>) -> Self {
        Self {
            sources,
            label_max: label_max.max(1),
            popup: None,
            clip_tx,
        }
    }

    fn record_history(&mut self, text: &str) {
        if let Err(err) = self.sources.record_history(text) {
            eprintln!("Failed to record clipboard history: {err}");
        }
    }

    fn restore(&mut self, text: &str) {
        if let Err(err) = self.clip_tx.send(ClipCmd::Restore(text.to_owned())) {
            eprintln!("Failed to restore the clipboard: {err}");
        }
    }

    fn drop_popup_handle(&mut self) {
        self.popup = None;
    }

    fn prewarm_popup(&mut self, cx: &mut Context<Self>) {
        let Some(placement) = popup_placement(cx, 0, 0, 1, self.label_max) else {
            return;
        };
        self.open_popup(Vec::new(), placement, false, cx);
    }

    fn show_source_menu(&mut self, source: &str, x: i32, y: i32, cx: &mut Context<Self>) {
        let entries = match self.sources.records(source) {
            Ok(entries) => entries,
            Err(err) => {
                eprintln!("Failed to read source {source:?}: {err}");
                return;
            }
        };

        let Some(placement) = popup_placement(cx, x, y, entries.len(), self.label_max) else {
            eprintln!("Failed to place the popup: no displays.");
            return;
        };

        if self.reuse_popup(&entries, &placement, cx) {
            return;
        }
        self.open_popup(entries, placement, true, cx);
    }

    fn reuse_popup(
        &mut self,
        entries: &[Record],
        placement: &PopupPlacement,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(handle) = self.popup else {
            return false;
        };
        match handle.update(cx, |view, window, cx| {
            if view.display_id != placement.display_id {
                return false;
            }
            window.resize(placement.output_size);
            view.reload(entries.to_vec(), placement.card, cx);
            view.reveal(window, cx);
            true
        }) {
            Ok(true) => true,
            Ok(false) => {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
                self.popup = None;
                false
            }
            Err(_) => {
                self.popup = None;
                false
            }
        }
    }

    fn open_popup(
        &mut self,
        entries: Vec<Record>,
        placement: PopupPlacement,
        visible: bool,
        cx: &mut Context<Self>,
    ) {
        let app = cx.entity();
        let options = overlay_options(&placement, visible);
        match cx.open_window(options, move |window, cx| {
            let view = if visible {
                cx.new(|cx| PopupView::shown(app, entries, placement, cx))
            } else {
                cx.new(|cx| PopupView::new(app, entries, placement, cx))
            };
            if visible {
                let focus = view.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
                window.activate();
            }
            view
        }) {
            Ok(handle) => self.popup = Some(handle),
            Err(err) => eprintln!("Failed to open the history popup: {err}"),
        }
    }
}

struct PopupPlacement {
    display_id: DisplayId,
    output_size: gpui::Size<Pixels>,
    card: Bounds<Pixels>,
    label_max: usize,
}

fn popup_placement(
    cx: &App,
    x: i32,
    y: i32,
    rows: usize,
    label_max: usize,
) -> Option<PopupPlacement> {
    let displays = cx.displays();
    if displays.is_empty() {
        return None;
    }
    let primary = cx
        .primary_display()
        .and_then(|primary| displays.iter().position(|d| d.id() == primary.id()))
        .unwrap_or(0);

    let mut outputs = Vec::with_capacity(displays.len());
    for display in &displays {
        let bounds = display.bounds();
        outputs.push(OutputGeom::new(
            pixels_i32(bounds.origin.x),
            pixels_i32(bounds.origin.y),
            pixels_i32(bounds.size.width).max(1),
            pixels_i32(bounds.size.height).max(1),
        ));
    }

    let width = ((label_max as f32 * CHAR_WIDTH) + 56.0).clamp(200.0, 520.0);
    let height = (rows.max(1) as f32 * ROW_HEIGHT + BORDER * 2.0).min(MAX_POPUP_HEIGHT);
    let (index, margin_x, margin_y) =
        place_on_outputs(x, y, width as i32, height as i32, &outputs, primary)?;
    let output_size = displays[index].bounds().size;

    Some(PopupPlacement {
        display_id: displays[index].id(),
        output_size,
        card: Bounds::new(
            point(px(margin_x as f32), px(margin_y as f32)),
            size(px(width), px(height)),
        ),
        label_max,
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
    display_id: DisplayId,
    card: Bounds<Pixels>,
    visible: bool,
    seen_active: bool,
    scroll: UniformListScrollHandle,
}

impl PopupView {
    fn new(
        app: Entity<Klemmbrett>,
        entries: Vec<Record>,
        placement: PopupPlacement,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            app,
            entries,
            selected: 0,
            label_max: placement.label_max,
            display_id: placement.display_id,
            card: placement.card,
            visible: false,
            seen_active: false,
            scroll: UniformListScrollHandle::new(),
        }
    }

    fn shown(
        app: Entity<Klemmbrett>,
        entries: Vec<Record>,
        placement: PopupPlacement,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new(app, entries, placement, cx);
        view.visible = true;
        view
    }

    fn reload(&mut self, entries: Vec<Record>, card: Bounds<Pixels>, cx: &mut Context<Self>) {
        self.entries = entries;
        self.card = card;
        self.selected = 0;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
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

    fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
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
        match key {
            "escape" => {
                self.hide(window, cx);
                cx.stop_propagation();
                return;
            }
            "enter" | "space" => {
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
            _ => {}
        }

        if mods.control || mods.alt || mods.platform {
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

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(value) = self.entries.get(index).map(|record| record.value.clone()) else {
            return;
        };
        self.app.update(cx, |app, _cx| app.restore(&value));
        self.hide(window, cx);
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

        let rows = if self.entries.is_empty() {
            popup_row(0, None, "No entries", false, |_, _, _| {}).into_any()
        } else {
            let selected = self.selected;
            let label_max = self.label_max;
            let specs: Vec<_> = self
                .entries
                .iter()
                .enumerate()
                .map(|(index, record)| menu_item_spec(index, &record.label, label_max))
                .collect();
            let view = cx.weak_entity();
            uniform_list("history-rows", specs.len(), move |range, _window, _cx| {
                range
                    .map(|index| {
                        let spec = &specs[index];
                        popup_row(
                            index,
                            spec.accelerator,
                            &spec.text,
                            index == selected,
                            activate_row(view.clone(), index),
                        )
                    })
                    .collect()
            })
            .track_scroll(&self.scroll)
            .size_full()
            .into_any()
        };

        let card = self.card;
        div()
            .id("klemmbrett-overlay")
            .track_focus(&self.focus_handle)
            .size_full()
            .relative()
            .bg(transparent_black())
            .on_key_down(cx.listener(|this, event, window, cx| this.on_key(event, window, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, window, cx| this.hide(window, cx)),
            )
            .child(
                div()
                    .id("klemmbrett-popup")
                    .absolute()
                    .left(card.origin.x)
                    .top(card.origin.y)
                    .w(card.size.width)
                    .h(card.size.height)
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .bg(rgb(0x2b2b2b))
                    .border_1()
                    .border_color(rgb(0x555555))
                    .text_color(rgb(0xe8e8e8))
                    .text_sm()
                    .line_height(px(ROW_HEIGHT))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(rows),
            )
    }
}

fn activate_row(
    view: WeakEntity<PopupView>,
    index: usize,
) -> impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static {
    move |_, window, cx| {
        let _ = view.update(cx, |this, cx| this.activate(index, window, cx));
    }
}

fn popup_row(
    index: usize,
    accelerator: Option<char>,
    text: &str,
    selected: bool,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(index)
        .w_full()
        .h(px(ROW_HEIGHT))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .px_2()
        .gap_2()
        .overflow_hidden()
        .when(selected, |row| row.bg(rgb(0x3d5a80)))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(
            div()
                .w(px(16.))
                .flex_none()
                .whitespace_nowrap()
                .text_color(rgb(0x888888))
                .child(accelerator.map(|c| c.to_string()).unwrap_or_default()),
        )
        .child(div().flex_1().min_w_0().truncate().child(text.to_owned()))
}

fn install_tray(sources: Vec<String>, tx: async_channel::Sender<UiCmd>) {
    let icons = tray_icon_pixmaps();
    match (TrayIcon { sources, tx, icons }).spawn() {
        Ok(handle) => {
            // The tray lives for the whole session; dropping it would remove the icon.
            std::mem::forget(handle);
        }
        Err(err) => eprintln!("Failed to install the StatusNotifierItem tray: {err}"),
    }
}

struct TrayIcon {
    sources: Vec<String>,
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
        if !self.sources.is_empty() {
            items.push(
                SubMenu {
                    label: "Sources".into(),
                    submenu: self
                        .sources
                        .iter()
                        .map(|name| {
                            let name = name.clone();
                            StandardItem {
                                label: name.clone(),
                                activate: Box::new(move |this: &mut Self| {
                                    let _ = this.tx.send_blocking(UiCmd::ShowMenu {
                                        source: name.clone(),
                                        x: 0,
                                        y: 0,
                                    });
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
                    let _ = this.tx.send_blocking(UiCmd::Quit);
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
