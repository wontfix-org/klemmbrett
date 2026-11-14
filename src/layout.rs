//! Declarative popup layouts: a tree of catalog elements and primitives.
//!
//! A layout decides which elements appear and in what order. A [`Theme`]
//! decides how each catalog element looks, and a node's own `style` is applied
//! on top of that.

use crate::data::DataPath;
use crate::history::shorten_label;
use crate::theme::{Location, Orientation, Props, Style, Theme};
use std::sync::Arc;

const BASE_FONT_SIZE: f32 = 14.0;
const BASE_CHAR_WIDTH: f32 = 9.0;
const WIDTH_SLACK: f32 = 14.0;
const MIN_CARD_WIDTH: f32 = 200.0;
const MAX_CARD_WIDTH: f32 = 520.0;
const MARKER_WIDTH: f32 = 16.0;
const INDEX_WIDTH: f32 = 16.0;

#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    Window,
    InputBar,
    Prompt,
    Entry,
    ListView,
    Element,
    Marker,
    Index,
    /// `{name}` fields are expanded when drawn; `{{` and `}}` are literal braces.
    Text(String),
    /// One value from the row's record, `fallback` when the path reaches nothing.
    Field {
        path: DataPath,
        fallback: String,
    },
    Spacer,
    Row,
    Column,
}

impl NodeKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::InputBar => "inputbar",
            Self::Prompt => "prompt",
            Self::Entry => "entry",
            Self::ListView => "listview",
            Self::Element => "element",
            Self::Marker => "marker",
            Self::Index => "index",
            Self::Text(_) => "text",
            Self::Field { .. } => "field",
            Self::Spacer => "spacer",
            Self::Row => "row",
            Self::Column => "column",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: NodeKind,
    pub style: Props,
    pub children: Vec<Node>,
}

impl Node {
    pub fn new(kind: NodeKind, children: Vec<Node>) -> Self {
        Self {
            kind,
            style: Props::default(),
            children,
        }
    }

    pub fn leaf(kind: NodeKind) -> Self {
        Self::new(kind, Vec::new())
    }

    pub fn text(template: &str) -> Self {
        Self::leaf(NodeKind::Text(template.to_owned()))
    }
}

/// Where a node sits, which decides the children it may have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Area {
    Window,
    InputBar,
    Element,
}

impl Area {
    fn name(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::InputBar => "inputbar",
            Self::Element => "element",
        }
    }

    fn allows(self, kind: &NodeKind, nested: bool) -> bool {
        use NodeKind::*;
        match (self, kind) {
            (_, Text(_) | Spacer | Row | Column) => true,
            (Area::Window, InputBar | ListView) => !nested,
            (Area::InputBar, Prompt | Entry) => true,
            (Area::Element, Marker | Index | Field { .. }) => true,
            _ => false,
        }
    }

    fn expected(self) -> &'static str {
        match self {
            Area::Window => "inputbar, listview, text, spacer, row, or column",
            Area::InputBar => "prompt, entry, text, spacer, row, or column",
            Area::Element => "marker, index, field, text, spacer, row, or column",
        }
    }
}

/// Context a layout builder sees when a widget opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutContext {
    pub widget: String,
    pub multi: bool,
    /// `true` for a `Prompt`, which returns values instead of running handlers.
    pub prompt: bool,
}

/// A validated layout tree rooted at a `window` node.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    root: Node,
    location: Option<Location>,
    inputbar: bool,
    listview: bool,
}

impl Layout {
    pub fn new(root: Node, location: Option<Location>) -> Result<Self, String> {
        if root.kind != NodeKind::Window {
            return Err(format!(
                "a layout must be rooted at a window, not {}",
                root.kind.name()
            ));
        }
        let mut layout = Self {
            root,
            location,
            inputbar: false,
            listview: false,
        };
        for child in &layout.root.children {
            match child.kind {
                NodeKind::InputBar if layout.inputbar => {
                    return Err("a window may contain only one inputbar".into())
                }
                NodeKind::ListView if layout.listview => {
                    return Err("a window may contain only one listview".into())
                }
                NodeKind::InputBar => layout.inputbar = true,
                NodeKind::ListView => layout.listview = true,
                _ => {}
            }
            validate(child, Area::Window, false)?;
        }
        Ok(layout)
    }

    /// The list-only card at the pointer, with accelerators.
    pub fn menu(multi: bool) -> Self {
        Self::new(
            Node::new(NodeKind::Window, vec![listview(multi)]),
            Some(Location::Pointer),
        )
        .expect("menu layout")
    }

    /// An input bar over the list, centered: typing filters the rows.
    pub fn filter(multi: bool) -> Self {
        Self::new(
            Node::new(NodeKind::Window, vec![inputbar(), listview(multi)]),
            Some(Location::Center),
        )
        .expect("filter layout")
    }

    /// An input bar only: Enter submits the typed text.
    pub fn prompt() -> Self {
        Self::new(
            Node::new(NodeKind::Window, vec![inputbar()]),
            Some(Location::Center),
        )
        .expect("prompt layout")
    }

    pub fn root(&self) -> &Node {
        &self.root
    }

    pub fn location(&self) -> Option<Location> {
        self.location
    }

    /// `Some` replaces every prompt's text. `None` fills a prompt that has
    /// none, using `default`, and leaves one that already has text.
    pub fn apply_prompt(&mut self, text: Option<&str>, default: &str) {
        fn walk(node: &mut Node, text: Option<&str>, default: &str) {
            if matches!(node.kind, NodeKind::Prompt) {
                match text {
                    Some(text) => node.style.text = Some(text.to_owned()),
                    None if node.style.text.is_none() => {
                        node.style.text = Some(default.to_owned());
                    }
                    None => {}
                }
            }
            for child in &mut node.children {
                walk(child, text, default);
            }
        }
        walk(&mut self.root, text, default);
    }

    pub fn has_inputbar(&self) -> bool {
        self.inputbar
    }

    pub fn has_listview(&self) -> bool {
        self.listview
    }

    /// The row template inside the listview.
    pub fn element(&self) -> Option<&Node> {
        self.root
            .children
            .iter()
            .find(|node| node.kind == NodeKind::ListView)
            .and_then(|list| list.children.first())
    }

    pub fn window_style(&self, theme: &Theme) -> Style {
        theme.window().with(&self.root.style)
    }

    pub fn listview_style(&self, theme: &Theme) -> Style {
        let props = self
            .root
            .children
            .iter()
            .find(|node| node.kind == NodeKind::ListView)
            .map(|node| &node.style);
        match props {
            Some(props) => theme.listview().with(props),
            None => theme.listview().clone(),
        }
    }

    pub fn row_style(&self, theme: &Theme, selected: bool) -> Style {
        match self.element() {
            Some(element) => theme.row_style(selected).with(&element.style),
            None => theme.row_style(selected).clone(),
        }
    }

    pub fn row_extent(&self, theme: &Theme) -> f32 {
        self.row_style(theme, false).height.unwrap_or(28.0) + self.listview_style(theme).spacing
    }

    pub fn card_size(&self, theme: &Theme, rows: usize, label_max: usize) -> (f32, f32) {
        let window = self.window_style(theme);
        let width = window.width.unwrap_or_else(|| {
            (label_max as f32 * self.char_width(theme) + self.width_chrome(theme))
                .clamp(MIN_CARD_WIDTH, MAX_CARD_WIDTH)
        });
        let height = if self.listview {
            (rows.max(1) as f32 * self.row_extent(theme) + self.height_chrome(theme))
                .min(window.max_height.unwrap_or(600.0))
        } else {
            self.height_chrome(theme)
        };
        (width, height)
    }

    fn char_width(&self, theme: &Theme) -> f32 {
        let font = theme
            .element_text()
            .font_size
            .or(self.window_style(theme).font_size)
            .unwrap_or(BASE_FONT_SIZE);
        BASE_CHAR_WIDTH * (font / BASE_FONT_SIZE)
    }

    fn width_chrome(&self, theme: &Theme) -> f32 {
        let window = self.window_style(theme);
        let element = self.row_style(theme, false);
        let mut chrome =
            element.padding.horizontal() + window.padding.horizontal() + window.border.horizontal();
        if let Some(node) = self.element() {
            chrome += gaps(&node.children, element.spacing);
            chrome += node
                .children
                .iter()
                .map(|child| fixed_width(child, theme, &element))
                .sum::<f32>();
        }
        chrome + WIDTH_SLACK
    }

    fn height_chrome(&self, theme: &Theme) -> f32 {
        let window = self.window_style(theme);
        let mut chrome = window.padding.vertical() + window.border.vertical();
        for child in &self.root.children {
            chrome += match &child.kind {
                NodeKind::ListView => 0.0,
                NodeKind::InputBar => {
                    let style = theme.inputbar().with(&child.style);
                    style.height.unwrap_or_else(|| {
                        style.padding.vertical() + self.char_width(theme).max(BASE_FONT_SIZE)
                    })
                }
                _ => {
                    let style = Style::plain(&window, Orientation::Vertical).with(&child.style);
                    style
                        .height
                        .unwrap_or_else(|| self.row_extent(theme) + style.padding.vertical())
                }
            };
        }
        chrome + gaps(&self.root.children, window.spacing)
    }
}

fn gaps(children: &[Node], spacing: f32) -> f32 {
    if children.len() > 1 {
        spacing * (children.len() - 1) as f32
    } else {
        0.0
    }
}

/// Width a row child takes regardless of the label: fixed columns and
/// explicit widths. Text without a width shares what `label_max` asks for.
fn fixed_width(node: &Node, theme: &Theme, parent: &Style) -> f32 {
    match &node.kind {
        NodeKind::Marker => theme
            .element_marker()
            .with(&node.style)
            .width
            .unwrap_or(MARKER_WIDTH),
        NodeKind::Index => theme
            .element_index()
            .with(&node.style)
            .width
            .unwrap_or(INDEX_WIDTH),
        NodeKind::Text(_) | NodeKind::Field { .. } => {
            theme.element_text().with(&node.style).width.unwrap_or(0.0)
        }
        NodeKind::Row | NodeKind::Column => {
            let orientation = if node.kind == NodeKind::Row {
                Orientation::Horizontal
            } else {
                Orientation::Vertical
            };
            let style = Style::plain(parent, orientation).with(&node.style);
            if let Some(width) = style.width {
                return width;
            }
            let widths = node
                .children
                .iter()
                .map(|child| fixed_width(child, theme, &style));
            let inner = if orientation == Orientation::Horizontal {
                widths.sum::<f32>() + gaps(&node.children, style.spacing)
            } else {
                widths.fold(0.0, f32::max)
            };
            inner + style.padding.horizontal()
        }
        _ => node.style.width.unwrap_or(0.0),
    }
}

fn validate(node: &Node, area: Area, nested: bool) -> Result<(), String> {
    if !area.allows(&node.kind, nested) {
        return Err(format!(
            "{} cannot contain {}; expected {}",
            area.name(),
            node.kind.name(),
            if nested {
                area.expected().replace("inputbar, listview, ", "")
            } else {
                area.expected().to_owned()
            }
        ));
    }
    match &node.kind {
        NodeKind::InputBar => node
            .children
            .iter()
            .try_for_each(|child| validate(child, Area::InputBar, false)),
        NodeKind::ListView => match node.children.as_slice() {
            [element] if element.kind == NodeKind::Element => element
                .children
                .iter()
                .try_for_each(|child| validate(child, Area::Element, false)),
            _ => Err("a listview must contain exactly one element (the row template)".into()),
        },
        NodeKind::Row | NodeKind::Column => node
            .children
            .iter()
            .try_for_each(|child| validate(child, area, true)),
        kind if !node.children.is_empty() => Err(format!("{} cannot have children", kind.name())),
        _ => Ok(()),
    }
}

fn inputbar() -> Node {
    Node::new(
        NodeKind::InputBar,
        vec![Node::leaf(NodeKind::Prompt), Node::leaf(NodeKind::Entry)],
    )
}

fn listview(multi: bool) -> Node {
    let mut row = Vec::new();
    if multi {
        row.push(Node::leaf(NodeKind::Marker));
    }
    row.push(Node::leaf(NodeKind::Index));
    row.push(Node::text("{label}"));
    Node::new(NodeKind::ListView, vec![Node::new(NodeKind::Element, row)])
}

/// Expands `{name}` from `fields`. Unknown names stay as written.
pub fn expand(template: &str, fields: &[(&str, &str)]) -> String {
    expand_with(template, |name| {
        fields
            .iter()
            .find(|(field, _)| *field == name)
            .map(|(_, value)| (*value).to_owned())
    })
}

/// Expands `{name}` in a row template: `key`, `label`, `value`, or a path
/// into the record's data, each shortened like a label. Names that reach
/// nothing stay as written.
pub fn expand_record(template: &str, record: &crate::Record, label_max: usize) -> String {
    expand_with(template, |name| {
        let path = DataPath::parse(name).ok()?;
        record
            .text(&path)
            .map(|text| shorten_label(&text, label_max))
    })
}

fn expand_with(template: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(index) = rest.find(['{', '}']) {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if tail.starts_with('{') {
            if let Some(end) = tail.find('}') {
                if let Some(value) = lookup(&tail[1..end]) {
                    out.push_str(&value);
                    rest = &tail[end + 1..];
                    continue;
                }
            }
        }
        out.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// How a widget gets its layout each time it opens.
#[derive(Clone, Default)]
pub enum LayoutSpec {
    /// [`Layout::menu`].
    #[default]
    Default,
    Static(Arc<Layout>),
    Dynamic(Arc<dyn LayoutBuilder>),
}

pub trait LayoutBuilder: Send + Sync {
    fn build(&self, ctx: &LayoutContext) -> Result<Layout, String>;
}

/// Builds [`Layout::filter`], so multi-select markers follow the open.
struct FilterLayout;

impl LayoutBuilder for FilterLayout {
    fn build(&self, ctx: &LayoutContext) -> Result<Layout, String> {
        Ok(Layout::filter(ctx.multi))
    }
}

impl LayoutSpec {
    /// The filter layout, rebuilt for each open so `ctx.multi` is honored.
    pub fn filter() -> Self {
        Self::Dynamic(Arc::new(FilterLayout))
    }

    pub fn build(&self, ctx: &LayoutContext) -> Result<Arc<Layout>, String> {
        match self {
            Self::Default => Ok(Arc::new(Layout::menu(ctx.multi))),
            Self::Static(layout) => Ok(layout.clone()),
            Self::Dynamic(builder) => builder.build(ctx).map(Arc::new),
        }
    }
}

impl std::fmt::Debug for LayoutSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Default => f.write_str("Default"),
            Self::Static(layout) => f.debug_tuple("Static").field(layout).finish(),
            Self::Dynamic(_) => f.write_str("Dynamic"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Rgba, State, ThemeSpec, Widget};

    #[test]
    fn menu_matches_the_historic_card_size() {
        let theme = Theme::default();
        let menu = Layout::menu(false);
        assert_eq!(menu.card_size(&theme, 1, 16), (200.0, 30.0));
        assert_eq!(menu.card_size(&theme, 1, 20), (236.0, 30.0));
        assert_eq!(menu.card_size(&theme, 10, 64), (520.0, 282.0));
        assert_eq!(menu.card_size(&theme, 100, 64), (520.0, 600.0));
    }

    #[test]
    fn filter_spec_follows_multi_and_prompt_text_fills_only_blanks() {
        let ctx = LayoutContext {
            widget: "picker".into(),
            multi: true,
            prompt: true,
        };
        let layout = LayoutSpec::filter().build(&ctx).unwrap();
        assert!(format!("{layout:?}").contains("Marker"));
        let mut layout = Layout::filter(false);
        layout.apply_prompt(None, "> ");
        assert!(format!("{layout:?}").contains("\"> \""));
        layout.apply_prompt(Some("go> "), "> ");
        assert!(format!("{layout:?}").contains("\"go> \""));
        assert!(!format!("{layout:?}").contains("\"> \""));
    }

    #[test]
    fn filter_adds_an_inputbar_and_centers() {
        let theme = Theme::default();
        let filter = Layout::filter(false);
        assert!(filter.has_inputbar());
        assert!(filter.has_listview());
        assert_eq!(filter.location(), Some(Location::Center));
        let (width, height) = filter.card_size(&theme, 1, 16);
        assert_eq!(width, 200.0);
        assert!(height > Layout::menu(false).card_size(&theme, 1, 16).1);
    }

    #[test]
    fn prompt_height_does_not_depend_on_rows() {
        let theme = Theme::default();
        let prompt = Layout::prompt();
        assert!(!prompt.has_listview());
        let (_, short) = prompt.card_size(&theme, 0, 16);
        let (_, tall) = prompt.card_size(&theme, 50, 16);
        assert_eq!(short, tall);
        assert!(short > 0.0);
    }

    #[test]
    fn multi_select_marker_widens_the_card() {
        let theme = Theme::default();
        let with = Layout::filter(true).card_size(&theme, 1, 16).0;
        let without = Layout::filter(false).card_size(&theme, 1, 16).0;
        assert!(with > without, "{with} > {without}");
    }

    #[test]
    fn dropping_the_index_shrinks_the_card() {
        let theme = Theme::default();
        let without = Layout::new(
            Node::new(
                NodeKind::Window,
                vec![Node::new(
                    NodeKind::ListView,
                    vec![Node::new(NodeKind::Element, vec![Node::text("{label}")])],
                )],
            ),
            None,
        )
        .unwrap();
        assert!(
            without.card_size(&theme, 1, 20).0 < Layout::menu(false).card_size(&theme, 1, 20).0
        );
    }

    #[test]
    fn node_styles_override_the_theme() {
        let mut root = Node::new(NodeKind::Window, vec![listview(false)]);
        root.style.width = Some(300.0);
        let layout = Layout::new(root, None).unwrap();
        assert_eq!(layout.card_size(&Theme::default(), 1, 16).0, 300.0);

        let mut spec = ThemeSpec::default();
        spec.set(
            Widget::Element,
            State::Selected,
            Props {
                background: Some(Rgba::hex(0x010203)),
                ..Props::default()
            },
        );
        let theme = Theme::resolve(&[&spec]);
        assert_eq!(
            layout.row_style(&theme, true).background,
            Rgba::hex(0x010203)
        );
    }

    #[test]
    fn misplaced_nodes_are_rejected() {
        let error = Layout::new(Node::leaf(NodeKind::Row), None).unwrap_err();
        assert!(error.contains("rooted at a window"), "{error}");

        let error = Layout::new(
            Node::new(NodeKind::Window, vec![Node::leaf(NodeKind::Marker)]),
            None,
        )
        .unwrap_err();
        assert!(error.contains("window cannot contain marker"), "{error}");

        let error = Layout::new(
            Node::new(
                NodeKind::Window,
                vec![Node::new(NodeKind::ListView, Vec::new())],
            ),
            None,
        )
        .unwrap_err();
        assert!(error.contains("exactly one element"), "{error}");

        let error = Layout::new(
            Node::new(
                NodeKind::Window,
                vec![Node::new(NodeKind::Row, vec![listview(false)])],
            ),
            None,
        )
        .unwrap_err();
        assert!(
            error.contains("row cannot contain")
                || error.contains("window cannot contain listview"),
            "{error}"
        );

        let error = Layout::new(
            Node::new(NodeKind::Window, vec![inputbar(), inputbar()]),
            None,
        )
        .unwrap_err();
        assert!(error.contains("only one inputbar"), "{error}");

        let error = Layout::new(
            Node::new(
                NodeKind::Window,
                vec![Node::new(NodeKind::Text("x".into()), vec![Node::text("y")])],
            ),
            None,
        )
        .unwrap_err();
        assert!(error.contains("text cannot have children"), "{error}");
    }

    #[test]
    fn rows_and_columns_nest_inside_an_element() {
        let element = Node::new(
            NodeKind::Element,
            vec![
                Node::leaf(NodeKind::Index),
                Node::new(
                    NodeKind::Column,
                    vec![Node::text("{label}"), Node::text("{key}")],
                ),
            ],
        );
        let layout = Layout::new(
            Node::new(
                NodeKind::Window,
                vec![Node::new(NodeKind::ListView, vec![element])],
            ),
            None,
        )
        .unwrap();
        assert!(layout.element().is_some());
    }

    #[test]
    fn templates_expand_known_fields_and_keep_the_rest() {
        let fields = [("label", "Site"), ("key", "url")];
        assert_eq!(expand("{label} ({key})", &fields), "Site (url)");
        assert_eq!(expand("{{label}} {missing}", &fields), "{label} {missing}");
        assert_eq!(expand("open {", &fields), "open {");
        assert_eq!(expand("a}b", &fields), "a}b");
    }

    #[test]
    fn row_templates_reach_into_record_data() {
        let record = crate::Record::new("v").with_data(serde_json::json!({
            "sender": {"name": "Ada Lovelace"},
            "tags": ["a", "b"],
        }));
        assert_eq!(
            expand_record("{label}: {sender.name} [{tags[*]}] {nope}", &record, 64),
            "v: Ada Lovelace [a, b] {nope}"
        );
        assert_eq!(expand_record("{sender.name}", &record, 5), "Ada …");
    }

    #[test]
    fn fields_belong_in_rows() {
        let field = || {
            Node::leaf(NodeKind::Field {
                path: DataPath::parse("sender.name").unwrap(),
                fallback: String::new(),
            })
        };
        let row = Node::new(
            NodeKind::ListView,
            vec![Node::new(NodeKind::Element, vec![field()])],
        );
        assert!(Layout::new(Node::new(NodeKind::Window, vec![row]), None).is_ok());
        let error = Layout::new(Node::new(NodeKind::Window, vec![field()]), None).unwrap_err();
        assert!(error.contains("window cannot contain field"), "{error}");
    }
}
