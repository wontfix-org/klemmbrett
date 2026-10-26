//! HCL themes and layout recipes for the popup.
//!
//! A user theme is merged over the embedded default so omitted keys keep
//! today's look.

mod color;
mod parse;
mod units;

use parse::{parse_spec, State, ThemeSpec};
use std::path::{Path, PathBuf};

pub use parse::{
    parse_document, parse_document_at, parse_layout_recipe, HclDocument, HclValue, RawAction,
    RawActionInvocation, RawTransform, RawTransformInvocation, RawWidget,
};

const DEFAULT_HCL: &str = include_str!("default.hcl");
const BASE_FONT_SIZE: f32 = 14.0;
const BASE_CHAR_WIDTH: f32 = 9.0;
const WIDTH_SLACK: f32 = 14.0;
const MIN_CARD_WIDTH: f32 = 200.0;
const MAX_CARD_WIDTH: f32 = 520.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Widget {
    Overlay,
    Window,
    ListView,
    Element,
    ElementIndex,
    ElementText,
    InputBar,
    Prompt,
    Entry,
}

impl Widget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Overlay => "overlay",
            Self::Window => "window",
            Self::ListView => "listview",
            Self::Element => "element",
            Self::ElementIndex => "element-index",
            Self::ElementText => "element-text",
            Self::InputBar => "inputbar",
            Self::Prompt => "prompt",
            Self::Entry => "entry",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
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

impl Default for Location {
    fn default() -> Self {
        Self::Pointer
    }
}

impl Location {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().replace('-', " ").as_str() {
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
        })
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

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

    pub fn horizontal(self) -> f32 {
        self.left + self.right
    }

    pub fn vertical(self) -> f32 {
        self.top + self.bottom
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub enabled: bool,
    pub padding: Edges,
    pub margin: Edges,
    pub border: Edges,
    pub border_radius: f32,
    pub background: Rgba,
    pub border_color: Rgba,
    pub text_color: Rgba,
    pub spacing: f32,
    pub orientation: Orientation,
    pub children: Vec<Widget>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub max_height: Option<f32>,
    pub font_size: Option<f32>,
    pub location: Option<Location>,
}

impl Style {
    pub fn packed_children(&self, theme: &Theme) -> Vec<Widget> {
        self.children
            .iter()
            .copied()
            .filter(|widget| theme.style(*widget).enabled)
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Presentation {
    pub theme: Theme,
    pub location: Location,
}

impl Default for Presentation {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            location: Location::Pointer,
        }
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
    inputbar: Style,
    prompt: Style,
    entry: Style,
}

impl Default for Theme {
    fn default() -> Self {
        parse_theme_from_spec(parse_spec(DEFAULT_HCL).expect("embedded default.hcl"))
            .expect("embedded default.hcl")
    }
}

impl Theme {
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

    pub fn inputbar(&self) -> &Style {
        &self.inputbar
    }

    pub fn prompt(&self) -> &Style {
        &self.prompt
    }

    pub fn entry(&self) -> &Style {
        &self.entry
    }

    pub fn has_inputbar(&self) -> bool {
        self.window()
            .packed_children(self)
            .contains(&Widget::InputBar)
            && self.inputbar.enabled
    }

    pub fn has_listview(&self) -> bool {
        self.window()
            .packed_children(self)
            .contains(&Widget::ListView)
            && self.listview.enabled
    }

    pub fn style(&self, widget: Widget) -> &Style {
        match widget {
            Widget::Overlay => &self.overlay,
            Widget::Window => &self.window,
            Widget::ListView => &self.listview,
            Widget::Element => &self.element,
            Widget::ElementIndex => &self.element_index,
            Widget::ElementText => &self.element_text,
            Widget::InputBar => &self.inputbar,
            Widget::Prompt => &self.prompt,
            Widget::Entry => &self.entry,
        }
    }

    pub fn row_style(&self, selected: bool) -> &Style {
        if selected {
            &self.element_selected
        } else {
            &self.element
        }
    }

    pub fn row_extent(&self) -> f32 {
        self.element.height.unwrap_or(28.0) + self.listview.spacing
    }

    pub fn card_size(&self, rows: usize, label_max: usize) -> (f32, f32) {
        let width = self.window.width.unwrap_or_else(|| {
            (label_max as f32 * self.char_width() + self.width_chrome())
                .clamp(MIN_CARD_WIDTH, MAX_CARD_WIDTH)
        });
        let height = if self.has_listview() {
            (rows.max(1) as f32 * self.row_extent() + self.height_chrome())
                .min(self.window.max_height.unwrap_or(600.0))
        } else {
            self.height_chrome()
        };
        (width, height)
    }

    fn char_width(&self) -> f32 {
        let font = self
            .element_text
            .font_size
            .or(self.window.font_size)
            .unwrap_or(BASE_FONT_SIZE);
        BASE_CHAR_WIDTH * (font / BASE_FONT_SIZE)
    }

    fn width_chrome(&self) -> f32 {
        let mut chrome = self.element.padding.horizontal()
            + self.window.padding.horizontal()
            + self.window.border.horizontal();
        let children = self.element.packed_children(self);
        if children.len() > 1 {
            chrome += self.element.spacing * (children.len() - 1) as f32;
        }
        if children.contains(&Widget::ElementIndex) {
            chrome += self.element_index.width.unwrap_or(16.0);
        }
        chrome + WIDTH_SLACK
    }

    fn height_chrome(&self) -> f32 {
        let mut chrome = self.window.padding.vertical() + self.window.border.vertical();
        if self.has_inputbar() {
            chrome += self.inputbar_extent();
            if self.window.packed_children(self).len() > 1 {
                chrome += self.window.spacing;
            }
        }
        chrome
    }

    fn inputbar_extent(&self) -> f32 {
        self.inputbar.height.unwrap_or_else(|| {
            self.inputbar.padding.vertical() + self.char_width().max(BASE_FONT_SIZE)
        })
    }
}

/// Parses a user theme and merges it over the embedded default.
pub fn parse_theme(text: &str) -> Result<Theme, String> {
    theme_from_specs([parse_spec(text)?])
}

fn theme_from_specs(specs: impl IntoIterator<Item = ThemeSpec>) -> Result<Theme, String> {
    let mut spec = parse_spec(DEFAULT_HCL).expect("embedded default.hcl");
    for extra in specs {
        spec.merge(extra);
    }
    parse_theme_from_spec(spec)
}

pub fn theme_from_document(doc: &HclDocument, base_dir: &Path) -> Result<Theme, String> {
    presentation_for_widget(doc, None, None, None, None, None, base_dir)
        .map(|presentation| presentation.theme)
}

pub fn presentation_for_widget(
    doc: &HclDocument,
    widget_theme: Option<&str>,
    widget_layout: Option<&str>,
    widget_location: Option<Location>,
    widget_name: Option<&str>,
    widget_provider: Option<&str>,
    base_dir: &Path,
) -> Result<Presentation, String> {
    let mut specs = Vec::new();
    if let Some(name) = widget_theme {
        specs.push(load_theme_spec(name, base_dir)?);
    } else {
        for name in &doc.theme_refs {
            specs.push(load_theme_spec(name, base_dir)?);
        }
    }
    specs.push(doc.theme.clone());
    let mut recipe_location = None;
    if let Some(name) = widget_layout {
        let recipe = resolve_layout_recipe(name, &doc.named_layouts, base_dir)?
            .evaluate_for_widget(widget_name, widget_provider)?;
        recipe_location = recipe.location;
        specs.push(recipe.spec);
    }
    let theme = theme_from_specs(specs)?;
    let location = widget_location
        .or(recipe_location)
        .or(theme.window().location)
        .unwrap_or(Location::Pointer);
    Ok(Presentation { theme, location })
}

fn resolve_layout_recipe(
    name: &str,
    named: &std::collections::BTreeMap<String, parse::LayoutRecipe>,
    base_dir: &Path,
) -> Result<parse::LayoutRecipe, String> {
    if let Some(recipe) = named.get(name) {
        return Ok(recipe.clone());
    }
    load_layout_recipe(name, base_dir)
}

fn load_layout_recipe(name: &str, base_dir: &Path) -> Result<parse::LayoutRecipe, String> {
    if let Some(text) = shipped_layout(name) {
        return parse_layout_recipe(text);
    }
    let mut path = PathBuf::from(name);
    if path.is_relative() {
        path = base_dir.join(&path);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|err| format!("failed to read layout {}: {err}", path.display()))?;
    parse_layout_recipe(&text).map_err(|err| format!("invalid layout {}: {err}", path.display()))
}

fn shipped_layout(name: &str) -> Option<&'static str> {
    match name.trim_end_matches(".hcl") {
        "menu" => Some(include_str!("../../layouts/menu.hcl")),
        "filter" => Some(include_str!("../../layouts/filter.hcl")),
        "prompt" => Some(include_str!("../../layouts/prompt.hcl")),
        _ => None,
    }
}

fn load_theme_spec(name: &str, base_dir: &Path) -> Result<ThemeSpec, String> {
    if let Some(text) = shipped_theme(name) {
        return parse_spec(text);
    }
    let mut path = PathBuf::from(name);
    if path.is_relative() {
        path = base_dir.join(&path);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|err| format!("failed to read theme {}: {err}", path.display()))?;
    parse::parse_spec_at(&text, path.parent().unwrap_or(base_dir))
        .map_err(|err| format!("invalid theme {}: {err}", path.display()))
}

fn shipped_theme(name: &str) -> Option<&'static str> {
    match name.trim_end_matches(".hcl") {
        "solarized-dark" => Some(include_str!("../../themes/solarized-dark.hcl")),
        "solarized-light" => Some(include_str!("../../themes/solarized-light.hcl")),
        "nord" => Some(include_str!("../../themes/nord.hcl")),
        "gruvbox-dark" => Some(include_str!("../../themes/gruvbox-dark.hcl")),
        "catppuccin-mocha" => Some(include_str!("../../themes/catppuccin-mocha.hcl")),
        "dracula" => Some(include_str!("../../themes/dracula.hcl")),
        "tokyo-night" => Some(include_str!("../../themes/tokyo-night.hcl")),
        "rose-pine" => Some(include_str!("../../themes/rose-pine.hcl")),
        "everforest" => Some(include_str!("../../themes/everforest.hcl")),
        "kanagawa" => Some(include_str!("../../themes/kanagawa.hcl")),
        "mbra" => Some(include_str!("../../themes/mbra.hcl")),
        _ => None,
    }
}

fn parse_theme_from_spec(spec: ThemeSpec) -> Result<Theme, String> {
    let overlay = resolve(&spec, Widget::Overlay, State::Normal, None)?;
    let window = resolve(&spec, Widget::Window, State::Normal, Some(&overlay))?;
    let listview = resolve(&spec, Widget::ListView, State::Normal, Some(&window))?;
    let inputbar = resolve(&spec, Widget::InputBar, State::Normal, Some(&window))?;
    let element = resolve(&spec, Widget::Element, State::Normal, Some(&listview))?;
    let theme = Theme {
        overlay,
        window,
        listview,
        element: element.clone(),
        element_selected: resolve(&spec, Widget::Element, State::Selected, Some(&element))?,
        element_hover: resolve(&spec, Widget::Element, State::Hover, Some(&element))?,
        element_index: resolve(&spec, Widget::ElementIndex, State::Normal, Some(&element))?,
        element_text: resolve(&spec, Widget::ElementText, State::Normal, Some(&element))?,
        inputbar: inputbar.clone(),
        prompt: resolve(&spec, Widget::Prompt, State::Normal, Some(&inputbar))?,
        entry: resolve(&spec, Widget::Entry, State::Normal, Some(&inputbar))?,
    };
    validate_children(Widget::Window, &theme.window.children)?;
    validate_children(Widget::InputBar, &theme.inputbar.children)?;
    validate_children(Widget::Element, &theme.element.children)?;
    validate_children(Widget::Element, &theme.element_selected.children)?;
    validate_children(Widget::Element, &theme.element_hover.children)?;
    Ok(theme)
}

fn resolve(
    spec: &ThemeSpec,
    widget: Widget,
    state: State,
    parent: Option<&Style>,
) -> Result<Style, String> {
    let mut props = fallback(widget);
    props.merge(&spec.props(widget, state));
    Ok(Style {
        enabled: props.enabled.unwrap_or(true),
        padding: inherited(
            &props,
            "padding",
            parent,
            |p| Some(p.padding),
            props.padding,
        )
        .unwrap_or(Edges::all(0.0)),
        margin: inherited(&props, "margin", parent, |p| Some(p.margin), props.margin)
            .unwrap_or(Edges::all(0.0)),
        border: inherited(&props, "border", parent, |p| Some(p.border), props.border)
            .unwrap_or(Edges::all(0.0)),
        border_radius: inherited(
            &props,
            "border_radius",
            parent,
            |p| Some(p.border_radius),
            props.border_radius,
        )
        .unwrap_or(0.0),
        background: inherited(
            &props,
            "background_color",
            parent,
            |p| Some(p.background),
            props.background,
        )
        .unwrap_or(Rgba {
            r: 0x2b,
            g: 0x2b,
            b: 0x2b,
            a: 255,
        }),
        border_color: inherited(
            &props,
            "border_color",
            parent,
            |p| Some(p.border_color),
            props.border_color,
        )
        .unwrap_or(Rgba {
            r: 0x55,
            g: 0x55,
            b: 0x55,
            a: 255,
        }),
        text_color: inherited(
            &props,
            "text_color",
            parent,
            |p| Some(p.text_color),
            props.text_color,
        )
        .unwrap_or(Rgba {
            r: 0xe8,
            g: 0xe8,
            b: 0xe8,
            a: 255,
        }),
        spacing: inherited(
            &props,
            "spacing",
            parent,
            |p| Some(p.spacing),
            props.spacing,
        )
        .unwrap_or(0.0),
        orientation: props.orientation.unwrap_or(match widget {
            Widget::Element => Orientation::Horizontal,
            _ => Orientation::Vertical,
        }),
        children: props
            .children
            .clone()
            .unwrap_or_else(|| default_children(widget)),
        width: inherited(&props, "width", parent, |p| p.width, props.width),
        height: inherited(&props, "height", parent, |p| p.height, props.height),
        max_height: inherited(
            &props,
            "max_height",
            parent,
            |p| p.max_height,
            props.max_height,
        ),
        font_size: inherited(
            &props,
            "font_size",
            parent,
            |p| p.font_size,
            props.font_size,
        ),
        location: inherited(&props, "location", parent, |p| p.location, props.location),
    })
}

fn inherited<T: Copy>(
    props: &parse::Props,
    name: &str,
    parent: Option<&Style>,
    from_parent: impl Fn(&Style) -> Option<T>,
    value: Option<T>,
) -> Option<T> {
    if props.inherit.iter().any(|item| item == name) {
        parent.and_then(from_parent).or(value)
    } else {
        value
    }
}

fn fallback(widget: Widget) -> parse::Props {
    let mut props = parse::Props {
        children: Some(default_children(widget)),
        ..parse::Props::default()
    };
    match widget {
        Widget::Overlay => {
            props.background = Some(Rgba::TRANSPARENT);
        }
        Widget::Window => {
            props.border = Some(Edges::all(1.0));
            props.font_size = Some(BASE_FONT_SIZE);
            props.max_height = Some(600.0);
        }
        Widget::Element => {
            props.padding = Some(Edges::vh(0.0, 8.0));
            props.spacing = Some(8.0);
            props.height = Some(28.0);
            props.orientation = Some(Orientation::Horizontal);
            props.background = Some(Rgba::TRANSPARENT);
        }
        Widget::ElementIndex => {
            props.text_color = Some(Rgba {
                r: 0x88,
                g: 0x88,
                b: 0x88,
                a: 255,
            });
            props.width = Some(16.0);
            props.background = Some(Rgba::TRANSPARENT);
        }
        Widget::ElementText => {
            props.background = Some(Rgba::TRANSPARENT);
        }
        Widget::InputBar => {
            props.padding = Some(Edges::all(8.0));
            props.spacing = Some(8.0);
            props.height = Some(36.0);
            props.orientation = Some(Orientation::Horizontal);
            props.background = Some(Rgba::TRANSPARENT);
        }
        Widget::Prompt | Widget::Entry => {
            props.background = Some(Rgba::TRANSPARENT);
        }
        Widget::ListView => {}
    }
    props
}

fn default_children(widget: Widget) -> Vec<Widget> {
    match widget {
        Widget::Window => vec![Widget::ListView],
        Widget::InputBar => vec![Widget::Prompt, Widget::Entry],
        Widget::Element => vec![Widget::ElementIndex, Widget::ElementText],
        _ => Vec::new(),
    }
}

fn validate_children(parent: Widget, children: &[Widget]) -> Result<(), String> {
    let allowed = match parent {
        Widget::Window => &[Widget::InputBar, Widget::ListView][..],
        Widget::InputBar => &[Widget::Prompt, Widget::Entry][..],
        Widget::Element => &[Widget::ElementIndex, Widget::ElementText][..],
        _ => &[][..],
    };
    for child in children {
        if !allowed.contains(child) {
            return Err(format!(
                "{} cannot contain {}; allowed children: {}",
                parent.as_str(),
                child.as_str(),
                allowed
                    .iter()
                    .map(|widget| widget.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_matches_hardcoded_card_size() {
        let theme = Theme::default();
        assert_eq!(theme.card_size(1, 16), (200.0, 30.0));
        assert_eq!(theme.card_size(1, 20), (236.0, 30.0));
        assert_eq!(theme.card_size(10, 64), (520.0, 282.0));
        assert_eq!(theme.card_size(100, 64), (520.0, 600.0));
    }

    #[test]
    fn default_theme_uses_today_s_colors() {
        let theme = Theme::default();
        assert_eq!(theme.overlay().background, Rgba::TRANSPARENT);
        assert_eq!(
            theme.window().background,
            Rgba {
                r: 0x2b,
                g: 0x2b,
                b: 0x2b,
                a: 255
            }
        );
        assert_eq!(
            theme.element_selected().background,
            Rgba {
                r: 0x3d,
                g: 0x5a,
                b: 0x80,
                a: 255
            }
        );
        assert_eq!(
            theme.element().children,
            vec![Widget::ElementIndex, Widget::ElementText]
        );
        assert_eq!(theme.element().background, Rgba::TRANSPARENT);
        assert_eq!(theme.element_index().background, Rgba::TRANSPARENT);
    }

    #[test]
    fn user_theme_overrides_merge_over_default() {
        let theme = parse_theme(
            r#"
                element "selected" { background_color = hex("ff0000") }
                element { children = ["element-text", "element-index"] }
            "#,
        )
        .unwrap();
        assert_eq!(
            theme.element_selected().background,
            Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255
            }
        );
        assert_eq!(theme.element_selected().height, Some(28.0));
        assert_eq!(
            theme.element().children,
            vec![Widget::ElementText, Widget::ElementIndex]
        );
        assert_eq!(theme.window().border, Edges::all(1.0));
    }

    #[test]
    fn hiding_the_accelerator_shrinks_chrome() {
        let with_index = Theme::default().card_size(1, 20).0;
        let without = parse_theme("element-index { enabled = false }")
            .unwrap()
            .card_size(1, 20)
            .0;
        assert!(without < with_index, "{without} < {with_index}");
    }

    #[test]
    fn invalid_window_child_is_rejected() {
        let error = parse_theme(r#"window { children = ["element"] }"#).unwrap_err();
        assert!(error.contains("window cannot contain element"), "{error}");
    }

    #[test]
    fn window_location_and_inherit_are_applied() {
        let theme = parse_theme(
            r#"
                window {
                    location = "north east"
                    background_color = hex("112233")
                }
                listview { background_color = inherit() }
            "#,
        )
        .unwrap();
        assert_eq!(theme.window().location, Some(Location::NorthEast));
        assert_eq!(theme.listview().background, theme.window().background);
    }

    #[test]
    fn window_may_pack_an_inputbar() {
        let theme = parse_theme(r#"window { children = ["inputbar", "listview"] }"#).unwrap();
        assert_eq!(
            theme.window().children,
            vec![Widget::InputBar, Widget::ListView]
        );
        assert!(theme.has_inputbar());
        let (width, height) = theme.card_size(1, 16);
        assert_eq!(width, 200.0);
        assert!(height > Theme::default().card_size(1, 16).1);
    }

    #[test]
    fn shipped_filter_layout_centers_and_adds_inputbar() {
        let presentation = presentation_for_widget(
            &parse_document("").unwrap(),
            None,
            Some("filter"),
            None,
            None,
            None,
            Path::new("."),
        )
        .unwrap();
        assert_eq!(presentation.location, Location::Center);
        assert!(presentation.theme.has_inputbar());
        assert!(presentation.theme.has_listview());
        assert_eq!(
            presentation.theme.inputbar().children,
            vec![Widget::Prompt, Widget::Entry]
        );
    }

    #[test]
    fn shipped_prompt_layout_is_inputbar_only() {
        let presentation = presentation_for_widget(
            &parse_document("").unwrap(),
            None,
            Some("prompt"),
            None,
            None,
            None,
            Path::new("."),
        )
        .unwrap();
        assert_eq!(presentation.location, Location::Center);
        assert!(presentation.theme.has_inputbar());
        assert!(!presentation.theme.has_listview());
        assert_eq!(
            presentation.theme.window().children,
            vec![Widget::InputBar]
        );
        assert_eq!(
            presentation.theme.inputbar().children,
            vec![Widget::Prompt, Widget::Entry]
        );
        let (_, short) = presentation.theme.card_size(0, 16);
        let (_, tall) = presentation.theme.card_size(50, 16);
        assert_eq!(short, tall);
        assert!(short > 0.0);
        assert!(short < Theme::default().card_size(10, 16).1);
    }

    #[test]
    fn window_may_omit_listview() {
        let theme = parse_theme(r#"window { children = ["inputbar"] }"#).unwrap();
        assert_eq!(theme.window().children, vec![Widget::InputBar]);
        assert!(theme.has_inputbar());
        assert!(!theme.has_listview());
    }

    #[test]
    fn empty_user_theme_is_the_default() {
        assert_eq!(parse_theme("").unwrap(), Theme::default());
    }

    #[test]
    fn shipped_themes_parse_and_keep_default_layout() {
        let themes = [
            (
                "solarized-dark",
                include_str!("../../themes/solarized-dark.hcl"),
                Rgba {
                    r: 0x00,
                    g: 0x2b,
                    b: 0x36,
                    a: 255,
                },
            ),
            (
                "solarized-light",
                include_str!("../../themes/solarized-light.hcl"),
                Rgba {
                    r: 0xfd,
                    g: 0xf6,
                    b: 0xe3,
                    a: 255,
                },
            ),
            (
                "nord",
                include_str!("../../themes/nord.hcl"),
                Rgba {
                    r: 0x2e,
                    g: 0x34,
                    b: 0x40,
                    a: 255,
                },
            ),
            (
                "gruvbox-dark",
                include_str!("../../themes/gruvbox-dark.hcl"),
                Rgba {
                    r: 0x28,
                    g: 0x28,
                    b: 0x28,
                    a: 255,
                },
            ),
            (
                "catppuccin-mocha",
                include_str!("../../themes/catppuccin-mocha.hcl"),
                Rgba {
                    r: 0x1e,
                    g: 0x1e,
                    b: 0x2e,
                    a: 255,
                },
            ),
            (
                "dracula",
                include_str!("../../themes/dracula.hcl"),
                Rgba {
                    r: 0x28,
                    g: 0x2a,
                    b: 0x36,
                    a: 255,
                },
            ),
            (
                "tokyo-night",
                include_str!("../../themes/tokyo-night.hcl"),
                Rgba {
                    r: 0x1a,
                    g: 0x1b,
                    b: 0x26,
                    a: 255,
                },
            ),
            (
                "rose-pine",
                include_str!("../../themes/rose-pine.hcl"),
                Rgba {
                    r: 0x19,
                    g: 0x17,
                    b: 0x24,
                    a: 255,
                },
            ),
            (
                "everforest",
                include_str!("../../themes/everforest.hcl"),
                Rgba {
                    r: 0x2d,
                    g: 0x35,
                    b: 0x3b,
                    a: 255,
                },
            ),
            (
                "kanagawa",
                include_str!("../../themes/kanagawa.hcl"),
                Rgba {
                    r: 0x1f,
                    g: 0x1f,
                    b: 0x28,
                    a: 255,
                },
            ),
            (
                "mbra",
                include_str!("../../themes/mbra.hcl"),
                Rgba {
                    r: 0x00,
                    g: 0x00,
                    b: 0x00,
                    a: 255,
                },
            ),
        ];
        for (name, source, background) in themes {
            let theme = parse_theme(source).unwrap_or_else(|err| panic!("{name}: {err}"));
            assert_eq!(theme.window().background, background, "{name}");
            assert_eq!(
                theme.element().children,
                vec![Widget::ElementIndex, Widget::ElementText],
                "{name}"
            );
        }
    }

    #[test]
    fn stylish_themes_round_the_card_and_rows() {
        for name in ["tokyo-night", "rose-pine", "everforest", "kanagawa", "mbra"] {
            let theme = parse_theme(shipped_theme(name).unwrap())
                .unwrap_or_else(|err| panic!("{name}: {err}"));
            assert!(
                theme.window().border_radius >= 12.0,
                "{name} window radius {}",
                theme.window().border_radius
            );
            assert!(
                theme.element().border_radius >= 8.0,
                "{name} row radius {}",
                theme.element().border_radius
            );
            assert!(
                theme.inputbar().border_radius >= 8.0,
                "{name} inputbar radius {}",
                theme.inputbar().border_radius
            );
            assert!(
                theme.window().padding.horizontal() > 0.0,
                "{name} should inset the list"
            );
        }
    }
}
