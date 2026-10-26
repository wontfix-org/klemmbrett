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
    div, point, prelude::*, px, rgba, size, uniform_list, App, Bounds, Context, DisplayId, Entity,
    FocusHandle, Focusable, KeyDownEvent, LinuxWindowBackground, MouseButton, Pixels, QuitMode,
    ScrollStrategy, Styled, UniformListScrollHandle, WeakEntity, Window, WindowBounds,
    WindowDecorations, WindowHandle, WindowKind, WindowOptions,
};
use klemmbrett::{
    filter_records, history_candidate, index_for_accelerator, menu_item_spec, place_at_location,
    run_transform, run_waited_command, spawn_detached, ArboardBackend, Backend, ClipboardSync,
    Edges, Launch, Location, Orientation, OutputGeom, Record, ResolvedLaunch, SelectAction,
    Selection, Style, Theme, Widget, Widgets, POLL_INTERVAL,
};
use ksni::blocking::TrayMethods;
use std::collections::HashSet;
use std::sync::mpsc::{self, RecvTimeoutError};

const TRAY_ICON_NAME: &str = "edit-paste";
const TRAY_ICON_SIZES: [i32; 4] = [16, 22, 24, 32];

enum ClipCmd {
    Restore(String),
}

pub fn run(
    sync: ClipboardSync<ArboardBackend>,
    widgets: Widgets,
    label_max: usize,
    theme: Theme,
    launch: Launch,
) {
    let (tx, rx) = async_channel::unbounded::<UiCmd>();
    let widget_names: HashSet<_> = widgets.names().into_iter().collect();
    dbus::spawn(widget_names, tx.clone());
    install_tray(widgets.names(), tx.clone());
    let clip_tx = spawn_clipboard(sync, tx);

    gpui_platform::application()
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx: &mut App| {
            let app = cx.new(|_cx| Klemmbrett::new(widgets, label_max, theme, clip_tx, launch));
            app.update(cx, |this, cx| this.prewarm_popup(cx));

            let dispatcher = app.clone();
            cx.spawn(async move |cx| {
                while let Ok(cmd) = rx.recv().await {
                    dispatcher.update(cx, |this, cx| match cmd {
                        UiCmd::Show { widget, pointer } => {
                            this.show_widget_menu(&widget, pointer, cx)
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

fn run_user_command(launch: &ResolvedLaunch, program_and_args: &[&str]) -> Result<(), String> {
    let argv = launch.argv(program_and_args);
    spawn_detached(&argv)
}

struct Klemmbrett {
    widgets: Widgets,
    label_max: usize,
    theme: Theme,
    popup: Option<WindowHandle<PopupView>>,
    clip_tx: mpsc::Sender<ClipCmd>,
    launch: ResolvedLaunch,
}

impl Klemmbrett {
    fn new(
        widgets: Widgets,
        label_max: usize,
        theme: Theme,
        clip_tx: mpsc::Sender<ClipCmd>,
        launch: Launch,
    ) -> Self {
        Self {
            widgets,
            label_max: label_max.max(1),
            theme,
            popup: None,
            clip_tx,
            launch: launch.resolve(),
        }
    }

    fn record_history(&mut self, text: &str) {
        if let Err(err) = self.widgets.record_history(text) {
            eprintln!("Failed to record clipboard history: {err}");
        }
    }

    fn restore(&mut self, text: &str) {
        if let Err(err) = self.clip_tx.send(ClipCmd::Restore(text.to_owned())) {
            eprintln!("Failed to restore the clipboard: {err}");
        }
    }

    fn select_record(&mut self, widget: &str, key: &str, label: &str, value: &str) {
        let mut value = value.to_owned();
        if let Some(transforms) = self.widgets.transforms(widget) {
            for transform in transforms {
                match run_transform(transform, key, label, &value) {
                    Ok(next) => value = next,
                    Err(err) => {
                        eprintln!("Failed to transform {widget:?}: {err}");
                        return;
                    }
                }
            }
        }
        let actions = self.widgets.actions(widget).unwrap_or_default().to_vec();
        for action in &actions {
            match action {
                SelectAction::CopyToClipboard => self.restore(&value),
                launch @ SelectAction::Launch { .. } => {
                    let argv = launch
                        .argv(key, label, &value)
                        .expect("launch action has argv");
                    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
                    if let Err(err) = run_user_command(&self.launch, &argv) {
                        eprintln!("Failed to launch {argv:?}: {err}");
                    }
                }
                exec @ SelectAction::Exec { .. } => {
                    let argv = exec.argv(key, label, &value).expect("exec action has argv");
                    let stdin = exec
                        .stdin(key, label, &value)
                        .expect("exec action has stdin");
                    if let Err(err) = run_waited_command(&argv, &stdin) {
                        eprintln!("Failed to exec {argv:?}: {err}");
                    }
                }
                SelectAction::ActivateWindow => {
                    if let Err(err) = self.widgets.activate_window(&value) {
                        eprintln!("Failed to activate window {value:?}: {err}");
                    }
                }
            }
        }
    }

    fn drop_popup_handle(&mut self) {
        self.popup = None;
    }

    fn prewarm_popup(&mut self, cx: &mut Context<Self>) {
        let Some(placement) =
            popup_placement(cx, None, 1, self.label_max, &self.theme, Location::Pointer)
        else {
            return;
        };
        self.open_popup(Vec::new(), placement, false, String::new(), cx);
    }

    fn show_widget_menu(
        &mut self,
        widget: &str,
        pointer: Option<(i32, i32)>,
        cx: &mut Context<Self>,
    ) {
        let entries = match self.widgets.records(widget) {
            Ok(entries) => entries,
            Err(err) => {
                eprintln!("Failed to read widget {widget:?}: {err}");
                return;
            }
        };
        let presentation = self
            .widgets
            .presentation(widget)
            .cloned()
            .unwrap_or_else(|| klemmbrett::Presentation {
                theme: self.theme.clone(),
                location: Location::Pointer,
            });

        let Some(placement) = popup_placement(
            cx,
            pointer,
            entries.len(),
            self.label_max,
            &presentation.theme,
            presentation.location,
        ) else {
            eprintln!("Failed to place the popup: no displays.");
            return;
        };

        if self.reuse_popup(&entries, &placement, widget, cx) {
            return;
        }
        self.open_popup(entries, placement, true, widget.to_owned(), cx);
    }

    fn reuse_popup(
        &mut self,
        entries: &[Record],
        placement: &PopupPlacement,
        widget: &str,
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
            view.reload(entries.to_vec(), placement, widget, cx);
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
        widget: String,
        cx: &mut Context<Self>,
    ) {
        let app = cx.entity();
        let options = overlay_options(&placement, visible);
        match cx.open_window(options, move |window, cx| {
            let view = if visible {
                cx.new(|cx| PopupView::shown(app, entries, placement, widget, cx))
            } else {
                cx.new(|cx| PopupView::new(app, entries, placement, widget, cx))
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
    theme: Theme,
}

fn popup_placement(
    cx: &App,
    pointer: Option<(i32, i32)>,
    rows: usize,
    label_max: usize,
    theme: &Theme,
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

    let (width, height) = theme.card_size(rows, label_max);
    let (index, margin_x, margin_y) = place_at_location(
        location,
        pointer,
        width as i32,
        height as i32,
        &outputs,
        primary,
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
        theme: theme.clone(),
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
    widget_name: String,
    query: String,
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
        widget: String,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            app,
            entries,
            selected: 0,
            label_max: placement.label_max,
            theme: placement.theme,
            widget_name: widget,
            query: String::new(),
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
        widget: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new(app, entries, placement, widget, cx);
        view.visible = true;
        view
    }

    fn reload(
        &mut self,
        entries: Vec<Record>,
        placement: &PopupPlacement,
        widget: &str,
        cx: &mut Context<Self>,
    ) {
        self.entries = entries;
        self.card = placement.card;
        self.theme = placement.theme.clone();
        self.label_max = placement.label_max;
        self.widget_name = widget.to_owned();
        self.query.clear();
        self.selected = 0;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn visible_entries(&self) -> Vec<&Record> {
        if self.theme.has_inputbar() {
            filter_records(&self.entries, &self.query)
        } else {
            self.entries.iter().collect()
        }
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
        let filter = self.theme.has_inputbar();
        match key {
            "escape" => {
                self.hide(window, cx);
                cx.stop_propagation();
                return;
            }
            "enter" => {
                self.activate(self.selected, window, cx);
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
                self.clamp_selection();
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
                self.clamp_selection();
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
        cx.notify();
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, label, value)) = self.visible_entries().get(index).map(|record| {
            (
                record.key.clone(),
                record.label.clone(),
                record.value.clone(),
            )
        }) else {
            return;
        };
        let widget = self.widget_name.clone();
        self.app.update(cx, |app, _cx| {
            app.select_record(&widget, &key, &label, &value)
        });
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

        let theme = self.theme.clone();
        let visible = self.visible_entries();
        let show_list = theme
            .window()
            .packed_children(&theme)
            .contains(&Widget::ListView)
            && theme.listview().enabled;
        let rows = if !show_list {
            div().flex_1().into_any()
        } else if visible.is_empty() {
            popup_row(0, None, "No entries", false, &theme, |_, _, _| {}).into_any()
        } else {
            let selected = self.selected;
            let label_max = self.label_max;
            let show_accelerators = !theme.has_inputbar();
            let specs: Vec<_> = visible
                .iter()
                .enumerate()
                .map(|(index, record)| {
                    let mut spec = menu_item_spec(index, &record.label, label_max);
                    if !show_accelerators {
                        spec.accelerator = None;
                    }
                    spec
                })
                .collect();
            let view = cx.weak_entity();
            let list_theme = theme.clone();
            uniform_list("history-rows", specs.len(), move |range, _window, _cx| {
                range
                    .map(|index| {
                        let spec = &specs[index];
                        popup_row(
                            index,
                            spec.accelerator,
                            &spec.text,
                            index == selected,
                            &list_theme,
                            activate_row(view.clone(), index),
                        )
                    })
                    .collect()
            })
            .track_scroll(&self.scroll)
            .size_full()
            .flex_1()
            .into_any()
        };

        let card = self.card;
        let window_style = theme.window();
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
                .line_height(px(theme.row_extent()))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
            window_style,
        );
        let mut rows = Some(rows);
        for child in window_style.packed_children(&theme) {
            card_el = match child {
                Widget::InputBar => card_el.child(inputbar(&theme, &self.widget_name, &self.query)),
                Widget::ListView => match rows.take() {
                    Some(rows) => card_el.child(rows),
                    None => card_el,
                },
                _ => card_el,
            };
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

fn inputbar(theme: &Theme, prompt: &str, query: &str) -> gpui::Div {
    let style = theme.inputbar();
    let mut bar = apply_box(
        div()
            .w_full()
            .h(px(style.height.unwrap_or(36.0)))
            .flex_none()
            .overflow_hidden(),
        style,
    );
    for child in style.packed_children(theme) {
        bar = match child {
            Widget::Prompt => bar.child(apply_box(
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .child(prompt.to_owned()),
                theme.prompt(),
            )),
            Widget::Entry => bar.child(apply_box(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(format!("{query}│")),
                theme.entry(),
            )),
            _ => bar,
        };
    }
    bar
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
    theme: &Theme,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let style = theme.row_style(selected);
    let hover = theme.element_hover();
    let height = theme.row_extent();
    let mut row = apply_box(
        div()
            .id(index)
            .w_full()
            .h(px(height))
            .flex_none()
            .overflow_hidden(),
        style,
    )
    .hover(move |row| row.bg(paint(hover.background)))
    .on_mouse_down(MouseButton::Left, on_click);

    for child in style.packed_children(theme) {
        row = match child {
            Widget::ElementIndex => row.child(element_index(accelerator, theme.element_index())),
            Widget::ElementText => row.child(element_text(text, theme.element_text())),
            _ => row,
        };
    }
    row
}

fn element_index(accelerator: Option<char>, style: &Style) -> gpui::Div {
    apply_box(
        div()
            .w(px(style.width.unwrap_or(16.0)))
            .flex_none()
            .whitespace_nowrap()
            .child(accelerator.map(|c| c.to_string()).unwrap_or_default()),
        style,
    )
}

fn element_text(text: &str, style: &Style) -> gpui::Div {
    apply_box(
        div().flex_1().min_w_0().truncate().child(text.to_owned()),
        style,
    )
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

fn paint(color: klemmbrett::Rgba) -> gpui::Rgba {
    rgba(color.to_u32())
}

fn install_tray(widgets: Vec<String>, tx: async_channel::Sender<UiCmd>) {
    let icons = tray_icon_pixmaps();
    match (TrayIcon { widgets, tx, icons }).spawn() {
        Ok(handle) => {
            // The tray lives for the whole session; dropping it would remove the icon.
            std::mem::forget(handle);
        }
        Err(err) => eprintln!("Failed to install the StatusNotifierItem tray: {err}"),
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
                                    let _ = this.tx.send_blocking(UiCmd::Show {
                                        widget: name.clone(),
                                        pointer: None,
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
