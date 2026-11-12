//! Popup styles for the catalog elements.
//!
//! A [`ThemeSpec`] is a sparse set of properties per element and state. Specs
//! are merged over the built-in default, so omitted keys keep today's look.
//! Which elements appear, and where, is decided by a [`crate::Layout`].

mod color;

pub use color::{cmyk, hsl, hwb, named_color, parse_color};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Widget {
    Overlay,
    Window,
    ListView,
    Element,
    ElementIndex,
    ElementText,
    ElementMarker,
    InputBar,
    Prompt,
    Entry,
    Toast,
    ToastSummary,
    ToastBody,
}

impl Widget {
    pub const ALL: [Widget; 13] = [
        Self::Overlay,
        Self::Window,
        Self::ListView,
        Self::Element,
        Self::ElementIndex,
        Self::ElementText,
        Self::ElementMarker,
        Self::InputBar,
        Self::Prompt,
        Self::Entry,
        Self::Toast,
        Self::ToastSummary,
        Self::ToastBody,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Overlay => "overlay",
            Self::Window => "window",
            Self::ListView => "listview",
            Self::Element => "element",
            Self::ElementIndex => "element-index",
            Self::ElementText => "element-text",
            Self::ElementMarker => "element-marker",
            Self::InputBar => "inputbar",
            Self::Prompt => "prompt",
            Self::Entry => "entry",
            Self::Toast => "toast",
            Self::ToastSummary => "toast-summary",
            Self::ToastBody => "toast-body",
        }
    }

    /// Accepts `element-index` and `element_index`.
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.replace('_', "-");
        Self::ALL.into_iter().find(|widget| widget.as_str() == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    Normal,
    Selected,
    Hover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Location {
    #[default]
    Pointer,
    Center,
    North,
    NorthEast,
    East,
    SouthEast,
    South,
    SouthWest,
    West,
    NorthWest,
}

impl Location {
    pub fn parse(name: &str) -> Option<Self> {
        Some(
            match name.to_ascii_lowercase().replace(['-', '_'], " ").as_str() {
                "pointer" => Self::Pointer,
                "center" => Self::Center,
                "north" => Self::North,
                "north east" | "northeast" => Self::NorthEast,
                "east" => Self::East,
                "south east" | "southeast" => Self::SouthEast,
                "south" => Self::South,
                "south west" | "southwest" => Self::SouthWest,
                "west" => Self::West,
                "north west" | "northwest" => Self::NorthWest,
                _ => return None,
            },
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pointer => "pointer",
            Self::Center => "center",
            Self::North => "north",
            Self::NorthEast => "north east",
            Self::East => "east",
            Self::SouthEast => "south east",
            Self::South => "south",
            Self::SouthWest => "south west",
            Self::West => "west",
            Self::NorthWest => "north west",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

impl Orientation {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "horizontal" => Some(Self::Horizontal),
            "vertical" => Some(Self::Vertical),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn hex(rgb: u32) -> Self {
        Self::rgba((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255)
    }

    pub fn to_u32(self) -> u32 {
        (u32::from(self.r) << 24)
            | (u32::from(self.g) << 16)
            | (u32::from(self.b) << 8)
            | u32::from(self.a)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    pub fn all(value: f32) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }

    pub fn vh(vertical: f32, horizontal: f32) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }

    /// CSS shorthand: 1 to 4 values.
    pub fn from_shorthand(values: &[f32]) -> Option<Self> {
        Some(match *values {
            [all] => Self::all(all),
            [v, h] => Self::vh(v, h),
            [top, h, bottom] => Self {
                top,
                right: h,
                bottom,
                left: h,
            },
            [top, right, bottom, left] => Self {
                top,
                right,
                bottom,
                left,
            },
            _ => return None,
        })
    }

    pub fn horizontal(self) -> f32 {
        self.left + self.right
    }

    pub fn vertical(self) -> f32 {
        self.top + self.bottom
    }
}

/// The resolved look of one catalog element.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub padding: Edges,
    pub margin: Edges,
    pub border: Edges,
    pub border_radius: f32,
    pub background: Rgba,
    pub border_color: Rgba,
    pub text_color: Rgba,
    pub spacing: f32,
    pub orientation: Orientation,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub max_height: Option<f32>,
    pub font_size: Option<f32>,
    pub location: Option<Location>,
    pub text: Option<String>,
}

impl Style {
    /// A box that paints nothing and keeps `parent`'s text look, for layout
    /// primitives that have no catalog entry of their own.
    pub fn plain(parent: &Style, orientation: Orientation) -> Self {
        Self {
            padding: Edges::all(0.0),
            margin: Edges::all(0.0),
            border: Edges::all(0.0),
            border_radius: 0.0,
            background: Rgba::TRANSPARENT,
            border_color: parent.border_color,
            text_color: parent.text_color,
            spacing: parent.spacing,
            orientation,
            width: None,
            height: None,
            max_height: None,
            font_size: parent.font_size,
            location: None,
            text: None,
        }
    }

    /// Applies inline overrides. `inherit` is not meaningful here and is ignored.
    pub fn with(&self, props: &Props) -> Self {
        let mut style = self.clone();
        set(&mut style.padding, props.padding);
        set(&mut style.margin, props.margin);
        set(&mut style.border, props.border);
        set(&mut style.border_radius, props.border_radius);
        set(&mut style.background, props.background);
        set(&mut style.border_color, props.border_color);
        set(&mut style.text_color, props.text_color);
        set(&mut style.spacing, props.spacing);
        set(&mut style.orientation, props.orientation);
        style.width = props.width.or(style.width);
        style.height = props.height.or(style.height);
        style.max_height = props.max_height.or(style.max_height);
        style.font_size = props.font_size.or(style.font_size);
        style.location = props.location.or(style.location);
        if props.text.is_some() {
            style.text = props.text.clone();
        }
        style
    }
}

fn set<T: Copy>(slot: &mut T, value: Option<T>) {
    if let Some(value) = value {
        *slot = value;
    }
}

/// Sparse style properties. `inherit` names the properties that take the
/// parent element's resolved value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Props {
    pub padding: Option<Edges>,
    pub margin: Option<Edges>,
    pub border: Option<Edges>,
    pub border_radius: Option<f32>,
    pub background: Option<Rgba>,
    pub border_color: Option<Rgba>,
    pub text_color: Option<Rgba>,
    pub spacing: Option<f32>,
    pub orientation: Option<Orientation>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub max_height: Option<f32>,
    pub font_size: Option<f32>,
    pub location: Option<Location>,
    pub text: Option<String>,
    pub inherit: Vec<String>,
}

impl Props {
    pub const NAMES: [&'static str; 15] = [
        "padding",
        "margin",
        "border",
        "border_radius",
        "background_color",
        "border_color",
        "text_color",
        "spacing",
        "orientation",
        "width",
        "height",
        "max_height",
        "font_size",
        "location",
        "text",
    ];

    pub fn merge(&mut self, other: &Self) {
        overlay(&mut self.padding, other.padding);
        overlay(&mut self.margin, other.margin);
        overlay(&mut self.border, other.border);
        overlay(&mut self.border_radius, other.border_radius);
        overlay(&mut self.background, other.background);
        overlay(&mut self.border_color, other.border_color);
        overlay(&mut self.text_color, other.text_color);
        overlay(&mut self.spacing, other.spacing);
        overlay(&mut self.orientation, other.orientation);
        overlay(&mut self.width, other.width);
        overlay(&mut self.height, other.height);
        overlay(&mut self.max_height, other.max_height);
        overlay(&mut self.font_size, other.font_size);
        overlay(&mut self.location, other.location);
        if other.text.is_some() {
            self.text = other.text.clone();
        }
        for name in &other.inherit {
            if !self.inherit.contains(name) {
                self.inherit.push(name.clone());
            }
        }
    }
}

fn overlay<T: Copy>(slot: &mut Option<T>, value: Option<T>) {
    if let Some(value) = value {
        *slot = Some(value);
    }
}

/// One layer of a theme: `global` is the `defaults` fallback for every
/// element, then per-element and per-state properties.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThemeSpec {
    pub global: Props,
    pub widgets: HashMap<(Widget, State), Props>,
}

impl ThemeSpec {
    pub fn merge(&mut self, other: &Self) {
        self.global.merge(&other.global);
        for (key, props) in &other.widgets {
            self.widgets.entry(*key).or_default().merge(props);
        }
    }

    pub fn set(&mut self, widget: Widget, state: State, props: Props) {
        self.widgets.insert((widget, state), props);
    }

    fn props(&self, widget: Widget, state: State) -> Props {
        let mut props = Props::default();
        props.merge(&self.global);
        if let Some(base) = self.widgets.get(&(widget, State::Normal)) {
            props.merge(base);
        }
        if state != State::Normal {
            if let Some(over) = self.widgets.get(&(widget, state)) {
                props.merge(over);
            }
        }
        props
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    overlay: Style,
    window: Style,
    listview: Style,
    element: Style,
    element_selected: Style,
    element_hover: Style,
    element_index: Style,
    element_text: Style,
    element_marker: Style,
    inputbar: Style,
    prompt: Style,
    entry: Style,
    toast: Style,
    toast_summary: Style,
    toast_body: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self::resolve(&[])
    }
}

impl Theme {
    /// Merges `layers` in order over the built-in default and resolves every element.
    pub fn resolve(layers: &[&ThemeSpec]) -> Self {
        let mut spec = default_spec();
        for layer in layers {
            spec.merge(layer);
        }
        let overlay = resolve(&spec, Widget::Overlay, State::Normal, None);
        let window = resolve(&spec, Widget::Window, State::Normal, Some(&overlay));
        let listview = resolve(&spec, Widget::ListView, State::Normal, Some(&window));
        let inputbar = resolve(&spec, Widget::InputBar, State::Normal, Some(&window));
        let element = resolve(&spec, Widget::Element, State::Normal, Some(&listview));
        let toast = resolve_like(&spec, Widget::Toast, Widget::Window, Some(&overlay));
        let toast_summary = resolve_like(
            &spec,
            Widget::ToastSummary,
            Widget::ElementText,
            Some(&toast),
        );
        let toast_body = resolve_like(&spec, Widget::ToastBody, Widget::ElementText, Some(&toast));
        Self {
            toast,
            toast_summary,
            toast_body,
            overlay,
            window,
            listview,
            element_selected: resolve(&spec, Widget::Element, State::Selected, Some(&element)),
            element_hover: resolve(&spec, Widget::Element, State::Hover, Some(&element)),
            element_index: resolve(&spec, Widget::ElementIndex, State::Normal, Some(&element)),
            element_text: resolve(&spec, Widget::ElementText, State::Normal, Some(&element)),
            element_marker: resolve(&spec, Widget::ElementMarker, State::Normal, Some(&element)),
            element,
            prompt: resolve(&spec, Widget::Prompt, State::Normal, Some(&inputbar)),
            entry: resolve(&spec, Widget::Entry, State::Normal, Some(&inputbar)),
            inputbar,
        }
    }

    pub fn overlay(&self) -> &Style {
        &self.overlay
    }

    pub fn window(&self) -> &Style {
        &self.window
    }

    pub fn listview(&self) -> &Style {
        &self.listview
    }

    pub fn element(&self) -> &Style {
        &self.element
    }

    pub fn element_selected(&self) -> &Style {
        &self.element_selected
    }

    pub fn element_hover(&self) -> &Style {
        &self.element_hover
    }

    pub fn element_index(&self) -> &Style {
        &self.element_index
    }

    pub fn element_text(&self) -> &Style {
        &self.element_text
    }

    pub fn element_marker(&self) -> &Style {
        &self.element_marker
    }

    pub fn inputbar(&self) -> &Style {
        &self.inputbar
    }

    pub fn prompt(&self) -> &Style {
        &self.prompt
    }

    pub fn entry(&self) -> &Style {
        &self.entry
    }

    pub fn toast(&self) -> &Style {
        &self.toast
    }

    pub fn toast_summary(&self) -> &Style {
        &self.toast_summary
    }

    pub fn toast_body(&self) -> &Style {
        &self.toast_body
    }

    pub fn style(&self, widget: Widget) -> &Style {
        match widget {
            Widget::Overlay => &self.overlay,
            Widget::Window => &self.window,
            Widget::ListView => &self.listview,
            Widget::Element => &self.element,
            Widget::ElementIndex => &self.element_index,
            Widget::ElementText => &self.element_text,
            Widget::ElementMarker => &self.element_marker,
            Widget::InputBar => &self.inputbar,
            Widget::Prompt => &self.prompt,
            Widget::Entry => &self.entry,
            Widget::Toast => &self.toast,
            Widget::ToastSummary => &self.toast_summary,
            Widget::ToastBody => &self.toast_body,
        }
    }

    pub fn row_style(&self, selected: bool) -> &Style {
        if selected {
            &self.element_selected
        } else {
            &self.element
        }
    }
}

/// The embedded look every theme is merged over.
fn default_spec() -> ThemeSpec {
    let mut spec = ThemeSpec {
        global: Props {
            background: Some(Rgba::hex(0x2b2b2b)),
            text_color: Some(Rgba::hex(0xe8e8e8)),
            border_color: Some(Rgba::hex(0x555555)),
            ..Props::default()
        },
        widgets: HashMap::new(),
    };
    let transparent = || Props {
        background: Some(Rgba::TRANSPARENT),
        ..Props::default()
    };
    spec.set(Widget::Overlay, State::Normal, transparent());
    spec.set(
        Widget::Window,
        State::Normal,
        Props {
            border: Some(Edges::all(1.0)),
            border_radius: Some(0.0),
            padding: Some(Edges::all(0.0)),
            font_size: Some(14.0),
            max_height: Some(600.0),
            orientation: Some(Orientation::Vertical),
            ..Props::default()
        },
    );
    spec.set(
        Widget::ListView,
        State::Normal,
        Props {
            spacing: Some(0.0),
            ..Props::default()
        },
    );
    spec.set(
        Widget::Element,
        State::Normal,
        Props {
            orientation: Some(Orientation::Horizontal),
            padding: Some(Edges::vh(0.0, 8.0)),
            spacing: Some(8.0),
            height: Some(28.0),
            background: Some(Rgba::TRANSPARENT),
            ..Props::default()
        },
    );
    for state in [State::Selected, State::Hover] {
        spec.set(
            Widget::Element,
            state,
            Props {
                background: Some(Rgba::hex(0x3d5a80)),
                ..Props::default()
            },
        );
    }
    for widget in [Widget::ElementIndex, Widget::ElementMarker] {
        spec.set(
            widget,
            State::Normal,
            Props {
                text_color: Some(Rgba::hex(0x888888)),
                width: Some(16.0),
                background: Some(Rgba::TRANSPARENT),
                ..Props::default()
            },
        );
    }
    spec.set(Widget::ElementText, State::Normal, transparent());
    spec
}

fn resolve(spec: &ThemeSpec, widget: Widget, state: State, parent: Option<&Style>) -> Style {
    let mut props = fallback(widget);
    props.merge(&spec.props(widget, state));
    resolve_props(props, widget, parent)
}

/// Resolves `widget` over `like`'s look, so a palette that only styles the
/// popup still styles toasts. Sizes and placement are not borrowed.
fn resolve_like(spec: &ThemeSpec, widget: Widget, like: Widget, parent: Option<&Style>) -> Style {
    let mut props = fallback(like);
    props.merge(&spec.props(like, State::Normal));
    props.width = None;
    props.height = None;
    props.max_height = None;
    props.location = None;
    props.text = None;
    props.merge(&fallback(widget));
    if let Some(own) = spec.widgets.get(&(widget, State::Normal)) {
        props.merge(own);
    }
    resolve_props(props, widget, parent)
}

fn resolve_props(props: Props, widget: Widget, parent: Option<&Style>) -> Style {
    let inherit = |name: &str| props.inherit.iter().any(|item| item == name);
    macro_rules! pick {
        ($name:literal, $field:ident, $parent:expr) => {
            if inherit($name) {
                parent.and_then($parent).or(props.$field)
            } else {
                props.$field
            }
        };
    }
    Style {
        padding: pick!("padding", padding, |p: &Style| Some(p.padding)).unwrap_or(Edges::all(0.0)),
        margin: pick!("margin", margin, |p: &Style| Some(p.margin)).unwrap_or(Edges::all(0.0)),
        border: pick!("border", border, |p: &Style| Some(p.border)).unwrap_or(Edges::all(0.0)),
        border_radius: pick!("border_radius", border_radius, |p: &Style| Some(
            p.border_radius
        ))
        .unwrap_or(0.0),
        background: pick!("background_color", background, |p: &Style| Some(
            p.background
        ))
        .unwrap_or(Rgba::hex(0x2b2b2b)),
        border_color: pick!("border_color", border_color, |p: &Style| Some(
            p.border_color
        ))
        .unwrap_or(Rgba::hex(0x555555)),
        text_color: pick!("text_color", text_color, |p: &Style| Some(p.text_color))
            .unwrap_or(Rgba::hex(0xe8e8e8)),
        spacing: pick!("spacing", spacing, |p: &Style| Some(p.spacing)).unwrap_or(0.0),
        orientation: props.orientation.unwrap_or(match widget {
            Widget::Element => Orientation::Horizontal,
            _ => Orientation::Vertical,
        }),
        width: pick!("width", width, |p: &Style| p.width),
        height: pick!("height", height, |p: &Style| p.height),
        max_height: pick!("max_height", max_height, |p: &Style| p.max_height),
        font_size: pick!("font_size", font_size, |p: &Style| p.font_size),
        location: pick!("location", location, |p: &Style| p.location),
        text: if inherit("text") {
            parent
                .and_then(|p| p.text.clone())
                .or_else(|| props.text.clone())
        } else {
            props.text.clone()
        },
    }
}

fn fallback(widget: Widget) -> Props {
    let mut props = Props::default();
    match widget {
        Widget::Overlay => {
            props.background = Some(Rgba::TRANSPARENT);
        }
        Widget::Window => {
            props.border = Some(Edges::all(1.0));
            props.font_size = Some(14.0);
            props.max_height = Some(600.0);
        }
        Widget::Element => {
            props.padding = Some(Edges::vh(0.0, 8.0));
            props.spacing = Some(8.0);
            props.height = Some(28.0);
            props.orientation = Some(Orientation::Horizontal);
            props.background = Some(Rgba::TRANSPARENT);
        }
        Widget::ElementIndex | Widget::ElementMarker => {
            props.text_color = Some(Rgba::hex(0x888888));
            props.width = Some(16.0);
            props.background = Some(Rgba::TRANSPARENT);
        }
        Widget::ElementText | Widget::Prompt | Widget::Entry => {
            props.background = Some(Rgba::TRANSPARENT);
        }
        Widget::Toast => {
            props.padding = Some(Edges::vh(8.0, 12.0));
            props.spacing = Some(4.0);
            props.width = Some(360.0);
            props.orientation = Some(Orientation::Vertical);
        }
        Widget::ToastSummary => {
            props.background = Some(Rgba::TRANSPARENT);
            props.height = Some(20.0);
        }
        Widget::ToastBody => {
            props.background = Some(Rgba::TRANSPARENT);
            props.height = Some(36.0);
        }
        Widget::InputBar => {
            props.padding = Some(Edges::all(8.0));
            props.spacing = Some(8.0);
            props.height = Some(36.0);
            props.orientation = Some(Orientation::Horizontal);
            props.background = Some(Rgba::TRANSPARENT);
        }
        Widget::ListView => {}
    }
    props
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(entries: &[(Widget, State, Props)]) -> ThemeSpec {
        let mut spec = ThemeSpec::default();
        for (widget, state, props) in entries {
            spec.set(*widget, *state, props.clone());
        }
        spec
    }

    #[test]
    fn default_theme_uses_today_s_colors() {
        let theme = Theme::default();
        assert_eq!(theme.overlay().background, Rgba::TRANSPARENT);
        assert_eq!(theme.window().background, Rgba::hex(0x2b2b2b));
        assert_eq!(theme.element_selected().background, Rgba::hex(0x3d5a80));
        assert_eq!(theme.element().background, Rgba::TRANSPARENT);
        assert_eq!(theme.element_index().background, Rgba::TRANSPARENT);
        assert_eq!(theme.window().border, Edges::all(1.0));
    }

    #[test]
    fn layers_merge_over_the_default_in_order() {
        let palette = ThemeSpec {
            global: Props {
                background: Some(Rgba::hex(0x2e3440)),
                ..Props::default()
            },
            widgets: HashMap::new(),
        };
        let overrides = layer(&[(
            Widget::Element,
            State::Selected,
            Props {
                background: Some(Rgba::hex(0xff0000)),
                ..Props::default()
            },
        )]);
        let theme = Theme::resolve(&[&palette, &overrides]);
        assert_eq!(theme.window().background, Rgba::hex(0x2e3440));
        assert_eq!(theme.element_selected().background, Rgba::hex(0xff0000));
        assert_eq!(theme.element_selected().height, Some(28.0));
        // A per-element default still beats the palette's global fallback.
        assert_eq!(theme.element().background, Rgba::TRANSPARENT);
    }

    #[test]
    fn inherit_takes_the_parent_value() {
        let theme = Theme::resolve(&[&layer(&[
            (
                Widget::Window,
                State::Normal,
                Props {
                    background: Some(Rgba::hex(0x112233)),
                    location: Some(Location::NorthEast),
                    ..Props::default()
                },
            ),
            (
                Widget::ListView,
                State::Normal,
                Props {
                    inherit: vec!["background_color".into()],
                    ..Props::default()
                },
            ),
            (
                Widget::Element,
                State::Normal,
                Props {
                    text: Some("*".into()),
                    ..Props::default()
                },
            ),
            (
                Widget::ElementMarker,
                State::Normal,
                Props {
                    inherit: vec!["text".into()],
                    ..Props::default()
                },
            ),
        ])]);
        assert_eq!(theme.window().location, Some(Location::NorthEast));
        assert_eq!(theme.listview().background, theme.window().background);
        assert_eq!(theme.element_marker().text.as_deref(), Some("*"));
    }

    #[test]
    fn inline_overrides_replace_only_the_given_fields() {
        let base = Theme::default().element_text().clone();
        let style = base.with(&Props {
            text_color: Some(Rgba::hex(0x010203)),
            width: Some(40.0),
            ..Props::default()
        });
        assert_eq!(style.text_color, Rgba::hex(0x010203));
        assert_eq!(style.width, Some(40.0));
        assert_eq!(style.background, base.background);
    }

    #[test]
    fn toasts_follow_the_window_unless_styled_themselves() {
        let theme = Theme::resolve(&[&layer(&[(
            Widget::Window,
            State::Normal,
            Props {
                background: Some(Rgba::hex(0x112233)),
                border_radius: Some(8.0),
                width: Some(700.0),
                ..Props::default()
            },
        )])]);
        assert_eq!(theme.toast().background, Rgba::hex(0x112233));
        assert_eq!(theme.toast().border_radius, 8.0);
        assert_eq!(theme.toast().width, Some(360.0));
        assert_eq!(theme.toast().border, Edges::all(1.0));
        assert_eq!(theme.toast_body().background, Rgba::TRANSPARENT);

        let theme = Theme::resolve(&[&layer(&[
            (
                Widget::Window,
                State::Normal,
                Props {
                    background: Some(Rgba::hex(0x112233)),
                    ..Props::default()
                },
            ),
            (
                Widget::Toast,
                State::Normal,
                Props {
                    background: Some(Rgba::hex(0x445566)),
                    ..Props::default()
                },
            ),
        ])]);
        assert_eq!(theme.toast().background, Rgba::hex(0x445566));
    }

    #[test]
    fn widget_names_accept_dashes_and_underscores() {
        assert_eq!(Widget::parse("element-index"), Some(Widget::ElementIndex));
        assert_eq!(Widget::parse("element_marker"), Some(Widget::ElementMarker));
        assert_eq!(Widget::parse("sidebar"), None);
    }

    #[test]
    fn edges_follow_css_shorthand() {
        assert_eq!(Edges::from_shorthand(&[8.0]), Some(Edges::all(8.0)));
        assert_eq!(
            Edges::from_shorthand(&[0.0, 8.0]),
            Some(Edges::vh(0.0, 8.0))
        );
        assert_eq!(
            Edges::from_shorthand(&[1.0, 2.0, 3.0]),
            Some(Edges {
                top: 1.0,
                right: 2.0,
                bottom: 3.0,
                left: 2.0
            })
        );
        assert_eq!(Edges::from_shorthand(&[]), None);
    }

    #[test]
    fn location_names_accept_spaces_dashes_and_underscores() {
        assert_eq!(Location::parse("north east"), Some(Location::NorthEast));
        assert_eq!(Location::parse("south-west"), Some(Location::SouthWest));
        assert_eq!(Location::parse("north_west"), Some(Location::NorthWest));
        assert_eq!(Location::parse("middle"), None);
    }
}
