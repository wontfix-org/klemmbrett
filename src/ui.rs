//! The history popup and the tray icon.
//!
//! The popup is a layer-shell surface that asks for exclusive keyboard
//! interactivity, with the history rows packed *inside* it. It deliberately
//! does not use `gtk::Menu::popup_at_*`: a Wayland menu is an `xdg_popup`, and
//! taking the keyboard grab an `xdg_popup` needs requires the serial of a real
//! input event. The popup is opened from D-Bus, so no such serial exists and
//! the grab silently does nothing until the pointer happens to enter the
//! surface. Layer-shell's keyboard interactivity is the protocol's supported
//! way to focus a surface that was not opened from user input, so the rows and
//! their key handling live here instead.

use gdk::prelude::*;
use gtk::prelude::*;
use gtk_layer_shell::LayerShell;
use klemmbrett::{
    history_candidate, index_for_accelerator, menu_item_spec, place_on_outputs, ArboardBackend,
    Backend, ClipboardSync, OutputGeom, Record, Selection, Sources,
};
use ksni::blocking::TrayMethods;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const CSS: &str = "
.klemmbrett-popup { padding: 2px; }
.klemmbrett-accel { opacity: 0.55; margin-right: 12px; }
";

pub struct App {
    inner: RefCell<AppState>,
    label_max: usize,
    popup: RefCell<Option<Popup>>,
}

struct AppState {
    sync: ClipboardSync<ArboardBackend>,
    sources: Sources,
}

/// The live popup: its surface, the rows it offers, and the list showing them.
struct Popup {
    window: gtk::Window,
    list: gtk::ListBox,
    entries: Vec<Record>,
}

impl App {
    pub fn new(sync: ClipboardSync<ArboardBackend>, sources: Sources, label_max: usize) -> Self {
        Self {
            inner: RefCell::new(AppState { sync, sources }),
            label_max: label_max.max(1),
            popup: RefCell::new(None),
        }
    }

    pub fn record_current(&self) {
        let mut inner = self.inner.borrow_mut();
        let text = inner.sync.last_text().to_owned();
        if let Err(err) = inner.sources.record_history(&text) {
            eprintln!("Failed to seed clipboard history: {err}");
        }
    }

    pub fn poll(&self) {
        let mut inner = self.inner.borrow_mut();
        let action = inner.sync.tick();
        let last = inner.sync.last_text().to_owned();
        if let Some(text) = history_candidate(&action, &last) {
            if let Err(err) = inner.sources.record_history(text) {
                eprintln!("Failed to record clipboard history: {err}");
            }
        }
    }

    fn restore(&self, text: &str) {
        let mut inner = self.inner.borrow_mut();
        if let Err(err) = inner.sync.backend_mut().write(Selection::Clipboard, text) {
            eprintln!("Failed to restore the clipboard: {err}");
            return;
        }
        if let Err(err) = inner.sources.record_history(text) {
            eprintln!("Failed to record restored clipboard text: {err}");
        }
    }

    pub fn source_names(&self) -> Vec<String> {
        self.inner.borrow().sources.names()
    }

    pub fn show_source_menu(self: &Rc<Self>, source: &str, x: i32, y: i32) {
        self.dismiss_popup();

        let entries = match self.inner.borrow().sources.records(source) {
            Ok(entries) => entries,
            Err(err) => {
                eprintln!("Failed to read source {source:?}: {err}");
                return;
            }
        };

        let window = layer_window();
        let list = gtk::ListBox::new();
        list.style_context().add_class("klemmbrett-popup");
        list.set_selection_mode(gtk::SelectionMode::Browse);

        if entries.is_empty() {
            let row = gtk::ListBoxRow::new();
            let label = gtk::Label::new(Some("No entries"));
            label.set_xalign(0.0);
            label.set_margin_start(8);
            label.set_margin_end(8);
            row.add(&label);
            row.set_selectable(false);
            row.set_activatable(false);
            list.add(&row);
        } else {
            for (index, record) in entries.iter().enumerate() {
                list.add(&build_row(&menu_item_spec(
                    index,
                    &record.label,
                    self.label_max,
                )));
            }
        }

        let frame = gtk::Frame::new(None);
        frame.set_shadow_type(gtk::ShadowType::Out);
        frame.add(&list);
        window.add(&frame);
        place_layer_window(&window, x, y);

        let app = Rc::clone(self);
        list.connect_row_activated(move |_, row| {
            app.activate(row.index());
        });

        let app = Rc::clone(self);
        window.connect_key_press_event(move |_, event| app.on_key(event));

        // If the compositor moves the keyboard elsewhere the popup is stale,
        // and leaving it up would keep holding exclusive focus.
        let app = Rc::clone(self);
        window.connect_focus_out_event(move |_, _| {
            app.dismiss_popup();
            glib::Propagation::Proceed
        });

        window.show_all();
        if let Some(first) = list.row_at_index(0) {
            if first.is_selectable() {
                list.select_row(Some(&first));
            }
        }

        *self.popup.borrow_mut() = Some(Popup {
            window,
            list,
            entries,
        });
    }

    fn on_key(self: &Rc<Self>, event: &gdk::EventKey) -> glib::Propagation {
        let keyval = event.keyval();
        match keyval {
            gdk::keys::constants::Escape => {
                self.dismiss_popup();
                return glib::Propagation::Stop;
            }
            gdk::keys::constants::Return
            | gdk::keys::constants::KP_Enter
            | gdk::keys::constants::space => {
                let selected = self
                    .popup
                    .borrow()
                    .as_ref()
                    .and_then(|popup| popup.list.selected_row())
                    .map(|row| row.index());
                if let Some(index) = selected {
                    self.activate(index);
                }
                return glib::Propagation::Stop;
            }
            gdk::keys::constants::Up => {
                self.move_selection(-1);
                return glib::Propagation::Stop;
            }
            gdk::keys::constants::Down => {
                self.move_selection(1);
                return glib::Propagation::Stop;
            }
            _ => {}
        }

        // Modifiers would mean a compositor binding rather than an accelerator.
        let plain = !event
            .state()
            .intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::MOD1_MASK);
        let rows = self.popup.borrow().as_ref().map_or(0, |p| p.entries.len());
        if let (true, Some(key)) = (plain, keyval.to_unicode()) {
            if let Some(index) = index_for_accelerator(key, rows) {
                self.activate(index as i32);
                return glib::Propagation::Stop;
            }
        }

        glib::Propagation::Proceed
    }

    fn move_selection(&self, delta: i32) {
        let popup = self.popup.borrow();
        let Some(popup) = popup.as_ref() else {
            return;
        };
        let count = popup.entries.len() as i32;
        if count == 0 {
            return;
        }
        let current = popup.list.selected_row().map_or(0, |row| row.index());
        let next = (current + delta).rem_euclid(count);
        if let Some(row) = popup.list.row_at_index(next) {
            popup.list.select_row(Some(&row));
        }
    }

    fn activate(self: &Rc<Self>, index: i32) {
        let value = self
            .popup
            .borrow()
            .as_ref()
            .and_then(|popup| popup.entries.get(index as usize))
            .map(|record| record.value.clone());
        let Some(value) = value else {
            return;
        };
        self.dismiss_popup();
        self.restore(&value);
    }

    fn dismiss_popup(&self) {
        if let Some(popup) = self.popup.borrow_mut().take() {
            popup.window.close();
        }
    }
}

fn build_row(spec: &klemmbrett::MenuItemSpec) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    hbox.set_margin_start(8);
    hbox.set_margin_end(8);

    let accel = gtk::Label::new(spec.accelerator.map(|c| c.to_string()).as_deref());
    accel.style_context().add_class("klemmbrett-accel");
    accel.set_width_chars(1);
    accel.set_xalign(0.0);
    hbox.pack_start(&accel, false, false, 0);

    let label = gtk::Label::new(Some(&spec.text));
    label.set_xalign(0.0);
    label.set_ellipsize(gdk::pango::EllipsizeMode::End);
    hbox.pack_start(&label, true, true, 0);

    row.add(&hbox);
    row
}

fn layer_window() -> gtk::Window {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_decorated(false);
    window.set_resizable(false);

    window.init_layer_shell();
    window.set_namespace("klemmbrett");
    window.set_layer(gtk_layer_shell::Layer::Overlay);
    window.set_anchor(gtk_layer_shell::Edge::Left, true);
    window.set_anchor(gtk_layer_shell::Edge::Top, true);
    // Without this the compositor never gives the surface the keyboard, and
    // the popup only reacts once the pointer wanders into it.
    window.set_keyboard_mode(gtk_layer_shell::KeyboardMode::Exclusive);
    window.set_exclusive_zone(-1);

    window
}

/// Places the popup from the pointer and the window's size, then corrects if
/// the allocated size differs. The last applied size is remembered so a
/// margin change does not loop.
fn place_layer_window(window: &gtk::Window, x: i32, y: i32) {
    let Some((outputs, primary)) = output_geometry() else {
        return;
    };
    let applied = Rc::new(Cell::new((0, 0)));
    let (_, nat) = window.preferred_size();
    apply_layer_placement(
        window,
        x,
        y,
        (nat.width, nat.height),
        &outputs,
        primary,
        &applied,
    );

    window.connect_size_allocate(move |window, alloc| {
        apply_layer_placement(
            window,
            x,
            y,
            (alloc.width(), alloc.height()),
            &outputs,
            primary,
            &applied,
        );
    });
}

fn apply_layer_placement(
    window: &gtk::Window,
    x: i32,
    y: i32,
    size: (i32, i32),
    outputs: &[OutputGeom],
    primary: usize,
    applied: &Cell<(i32, i32)>,
) {
    if applied.get() == size {
        return;
    }
    applied.set(size);
    let Some((index, margin_x, margin_y)) =
        place_on_outputs(x, y, size.0, size.1, outputs, primary)
    else {
        return;
    };
    if let Some(monitor) = gdk_monitor(index) {
        window.set_monitor(&monitor);
    }
    window.set_layer_shell_margin(gtk_layer_shell::Edge::Left, margin_x);
    window.set_layer_shell_margin(gtk_layer_shell::Edge::Top, margin_y);
}

pub fn install_styles() {
    let provider = gtk::CssProvider::new();
    if let Err(err) = provider.load_from_data(CSS.as_bytes()) {
        eprintln!("Failed to load the popup stylesheet: {err}");
        return;
    }
    if let Some(screen) = gdk::Screen::default() {
        gtk::StyleContext::add_provider_for_screen(
            &screen,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

const TRAY_ICON_NAME: &str = "edit-paste";
const TRAY_ICON_SIZES: [i32; 4] = [16, 22, 24, 32];

enum TrayCmd {
    ShowSource(String),
    Quit,
}

struct TrayIcon {
    sources: Vec<String>,
    tx: glib::Sender<TrayCmd>,
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
            icon_name: "edit-paste".into(),
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
                                    let _ = this.tx.send(TrayCmd::ShowSource(name.clone()));
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
                    let _ = this.tx.send(TrayCmd::Quit);
                }),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

pub fn install_tray(app: Rc<App>) {
    let sources = app.source_names();
    #[allow(deprecated)]
    let (tx, rx) = glib::MainContext::channel(glib::Priority::DEFAULT);
    rx.attach(None, move |cmd| {
        match cmd {
            TrayCmd::ShowSource(source) => {
                let (x, y) = pointer_position();
                app.show_source_menu(&source, x, y);
            }
            TrayCmd::Quit => gtk::main_quit(),
        }
        glib::ControlFlow::Continue
    });

    // Qtile (and some other hosts) resolve IconName with pyxdg, which does not
    // find action icons such as edit-paste. Ayatana used to send pixels;
    // without a pixmap those hosts register the item and then hide it.
    let icons = tray_icon_pixmaps();
    if icons.is_empty() {
        eprintln!("Failed to load the tray icon {TRAY_ICON_NAME} from the GTK theme.");
    }

    match (TrayIcon { sources, tx, icons }).spawn() {
        Ok(handle) => {
            // The tray lives for the whole session; dropping it would remove the icon.
            std::mem::forget(handle);
        }
        Err(err) => eprintln!("Failed to install the StatusNotifierItem tray: {err}"),
    }
}

fn tray_icon_pixmaps() -> Vec<ksni::Icon> {
    let Some(theme) = gtk::IconTheme::default() else {
        return Vec::new();
    };
    let flags = gtk::IconLookupFlags::FORCE_SIZE | gtk::IconLookupFlags::GENERIC_FALLBACK;
    TRAY_ICON_SIZES
        .into_iter()
        .filter_map(|size| {
            theme
                .load_icon(TRAY_ICON_NAME, size, flags)
                .ok()
                .flatten()
                .and_then(|pixbuf| pixbuf_to_sni_icon(&pixbuf))
        })
        .collect()
}

fn pixbuf_to_sni_icon(pixbuf: &gtk::gdk_pixbuf::Pixbuf) -> Option<ksni::Icon> {
    let width = pixbuf.width();
    let height = pixbuf.height();
    let data = rgba_to_argb(
        &pixbuf.read_pixel_bytes(),
        width,
        height,
        pixbuf.n_channels(),
        pixbuf.rowstride(),
        pixbuf.has_alpha(),
    )?;
    Some(ksni::Icon {
        width,
        height,
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

fn pointer_position() -> (i32, i32) {
    if let Some(display) = gdk::Display::default() {
        if let Some(seat) = display.default_seat() {
            if let Some(pointer) = seat.pointer() {
                let (_, x, y) = pointer.position();
                return (x, y);
            }
        }
    }
    (0, 0)
}

fn output_geometry() -> Option<(Vec<OutputGeom>, usize)> {
    let display = gdk::Display::default()?;
    let mut outputs = Vec::new();
    let mut primary = 0;
    for i in 0..display.n_monitors() {
        let monitor = display.monitor(i)?;
        if monitor.is_primary() {
            primary = outputs.len();
        }
        let geo = monitor.geometry();
        outputs.push(OutputGeom::new(geo.x(), geo.y(), geo.width(), geo.height()));
    }
    if outputs.is_empty() {
        None
    } else {
        Some((outputs, primary))
    }
}

fn gdk_monitor(index: usize) -> Option<gdk::Monitor> {
    gdk::Display::default()?.monitor(index as i32)
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
