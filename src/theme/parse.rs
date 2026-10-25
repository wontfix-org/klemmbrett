//! HCL documents for the popup catalog, sources, and widget recipes.

use super::{color, units, Edges, Location, Orientation, Rgba, Widget};
use hcl::eval::{Context, Evaluate, FuncArgs, FuncDef, ParamType};
use hcl::{Block, Body, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum State {
    Normal,
    Selected,
    Hover,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Props {
    pub enabled: Option<bool>,
    pub padding: Option<Edges>,
    pub margin: Option<Edges>,
    pub border: Option<Edges>,
    pub border_radius: Option<f32>,
    pub background: Option<Rgba>,
    pub border_color: Option<Rgba>,
    pub text_color: Option<Rgba>,
    pub spacing: Option<f32>,
    pub orientation: Option<Orientation>,
    pub children: Option<Vec<Widget>>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub max_height: Option<f32>,
    pub font_size: Option<f32>,
    pub location: Option<Location>,
    pub inherit: Vec<String>,
}

impl Props {
    pub(super) fn merge(&mut self, other: &Self) {
        overlay(&mut self.enabled, other.enabled);
        overlay(&mut self.padding, other.padding);
        overlay(&mut self.margin, other.margin);
        overlay(&mut self.border, other.border);
        overlay(&mut self.border_radius, other.border_radius);
        overlay(&mut self.background, other.background);
        overlay(&mut self.border_color, other.border_color);
        overlay(&mut self.text_color, other.text_color);
        overlay(&mut self.spacing, other.spacing);
        overlay(&mut self.orientation, other.orientation);
        if other.children.is_some() {
            self.children = other.children.clone();
        }
        overlay(&mut self.width, other.width);
        overlay(&mut self.height, other.height);
        overlay(&mut self.max_height, other.max_height);
        overlay(&mut self.font_size, other.font_size);
        overlay(&mut self.location, other.location);
        for name in &other.inherit {
            if !self.inherit.iter().any(|existing| existing == name) {
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

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct ThemeSpec {
    pub global: Props,
    pub widgets: std::collections::HashMap<(Widget, State), Props>,
}

impl ThemeSpec {
    pub(super) fn merge(&mut self, other: Self) {
        self.global.merge(&other.global);
        for (key, props) in other.widgets {
            self.widgets.entry(key).or_default().merge(&props);
        }
    }

    pub(super) fn props(&self, widget: Widget, state: State) -> Props {
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
pub enum HclValue {
    Number(f32),
    String(String),
    Bool(bool),
    List(Vec<String>),
}

impl HclValue {
    pub fn as_ident(&self) -> Option<&str> {
        self.as_text()
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::String(name) => Some(name),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[String]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f32> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WidgetRecipe {
    pub location: Option<Location>,
    pub(super) spec: ThemeSpec,
    raw: Option<Body>,
    locals: Value,
}

impl Default for WidgetRecipe {
    fn default() -> Self {
        Self {
            location: None,
            spec: ThemeSpec::default(),
            raw: None,
            locals: Value::Object(Default::default()),
        }
    }
}

impl WidgetRecipe {
    pub(super) fn evaluate_for_source(
        &self,
        source_name: Option<&str>,
        source_kind: Option<&str>,
    ) -> Result<Self, String> {
        let Some(raw) = &self.raw else {
            return Ok(self.clone());
        };
        let evaluated = evaluate_body(raw, &self.locals, source_name, source_kind)?;
        let mut recipe = widget_from_body(&evaluated)?;
        recipe.raw = self.raw.clone();
        recipe.locals = self.locals.clone();
        Ok(recipe)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct HclDocument {
    pub configuration: BTreeMap<String, HclValue>,
    pub sources: Vec<RawSource>,
    pub theme_refs: Vec<String>,
    pub named_widgets: BTreeMap<String, WidgetRecipe>,
    pub(super) theme: ThemeSpec,
    locals: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawSource {
    pub name: String,
    pub properties: BTreeMap<String, HclValue>,
}

pub fn parse_document(text: &str) -> Result<HclDocument, String> {
    parse_document_at(text, Path::new("."))
}

pub fn parse_document_at(text: &str, base_dir: &Path) -> Result<HclDocument, String> {
    parse_document_stack(text, base_dir, &mut Vec::new())
}

fn parse_document_stack(
    text: &str,
    base_dir: &Path,
    stack: &mut Vec<PathBuf>,
) -> Result<HclDocument, String> {
    let raw = parse_hcl(text)?;
    let mut doc = HclDocument {
        locals: locals_from_body(&raw)?,
        ..HclDocument::default()
    };
    ingest_raw(&raw, &mut doc, base_dir, stack, true)?;
    let evaluated = evaluate_body(&raw, &doc.locals, None, None)?;
    ingest_evaluated(&evaluated, &mut doc, true)?;
    Ok(doc)
}

pub(super) fn parse_spec(text: &str) -> Result<ThemeSpec, String> {
    parse_spec_at(text, Path::new("."))
}

pub(super) fn parse_spec_at(text: &str, base_dir: &Path) -> Result<ThemeSpec, String> {
    let doc = parse_document_at(text, base_dir)?;
    if !doc.configuration.is_empty()
        || !doc.sources.is_empty()
        || !doc.theme_refs.is_empty()
        || !doc.named_widgets.is_empty()
    {
        return Err(
            "configuration, source, widget, and theme belong in the config file, not a theme"
                .into(),
        );
    }
    Ok(doc.theme)
}

pub fn parse_widget_recipe(text: &str) -> Result<WidgetRecipe, String> {
    let raw = parse_hcl(text)?;
    let locals = locals_from_body(&raw)?;
    let evaluated = evaluate_body(&raw, &locals, None, None)?;
    let mut recipe = widget_from_body(&evaluated)?;
    recipe.raw = Some(raw);
    recipe.locals = locals;
    Ok(recipe)
}

fn parse_hcl(text: &str) -> Result<Body, String> {
    hcl::parse(text).map_err(|err| err.to_string())
}

fn locals_from_body(body: &Body) -> Result<Value, String> {
    let mut locals = hcl::Map::new();
    let empty = Value::Object(hcl::Map::new());
    let ctx = eval_context(&empty, None, None);
    for block in body.blocks() {
        if block.identifier() != "locals" {
            continue;
        }
        reject_labels(block, "locals")?;
        let evaluated = block.body().evaluate(&ctx).map_err(eval_err)?;
        for attr in evaluated.attributes() {
            locals.insert(attr.key().to_owned(), to_value(attr.expr())?);
        }
    }
    Ok(Value::Object(locals))
}

fn evaluate_body(
    body: &Body,
    locals: &Value,
    source_name: Option<&str>,
    source_kind: Option<&str>,
) -> Result<Body, String> {
    let ctx = eval_context(locals, source_name, source_kind);
    body.evaluate(&ctx).map_err(eval_err)
}

fn eval_err(err: impl std::fmt::Display) -> String {
    err.to_string()
}

fn eval_context<'a>(
    locals: &'a Value,
    source_name: Option<&'a str>,
    source_kind: Option<&'a str>,
) -> Context<'a> {
    let mut ctx = Context::new();
    ctx.declare_var("local", locals.clone());
    ctx.declare_var(
        "monitor",
        Value::from_iter([
            ("width", Value::from(f64::from(units::MONITOR_WIDTH))),
            ("height", Value::from(f64::from(units::MONITOR_HEIGHT))),
        ]),
    );
    let mut source = hcl::Map::new();
    source.insert("name".into(), Value::from(source_name.unwrap_or_default()));
    source.insert("type".into(), Value::from(source_kind.unwrap_or_default()));
    ctx.declare_var("source", Value::Object(source));
    declare_funcs(&mut ctx);
    ctx
}

fn declare_funcs(ctx: &mut Context<'_>) {
    ctx.declare_func(
        "px",
        FuncDef::builder()
            .param(ParamType::Number)
            .build(|args| Ok(Value::from(f64::from(number(&args[0]))))),
    );
    ctx.declare_func(
        "em",
        FuncDef::builder()
            .param(ParamType::Number)
            .build(|args| Ok(Value::from(f64::from(number(&args[0]) * units::EM_PX)))),
    );
    ctx.declare_func(
        "ch",
        FuncDef::builder()
            .param(ParamType::Number)
            .build(|args| Ok(Value::from(f64::from(number(&args[0]) * units::CH_PX)))),
    );
    ctx.declare_func(
        "mm",
        FuncDef::builder()
            .param(ParamType::Number)
            .build(|args| Ok(Value::from(f64::from(number(&args[0]) * units::MM_PX)))),
    );
    ctx.declare_func(
        "pct",
        FuncDef::builder().param(ParamType::Number).build(|args| {
            Ok(Value::from(f64::from(
                number(&args[0]) / 100.0 * units::MONITOR_WIDTH,
            )))
        }),
    );
    ctx.declare_func(
        "vpct",
        FuncDef::builder().param(ParamType::Number).build(|args| {
            Ok(Value::from(f64::from(
                number(&args[0]) / 100.0 * units::MONITOR_HEIGHT,
            )))
        }),
    );
    ctx.declare_func(
        "inherit",
        FuncDef::builder().build(|_| Ok(Value::from("inherit"))),
    );
    ctx.declare_func(
        "transparent",
        FuncDef::builder().build(|_| Ok(color_value(Rgba::TRANSPARENT))),
    );
    ctx.declare_func(
        "hex",
        FuncDef::builder().param(ParamType::String).build(|args| {
            let hex = args[0].as_str().unwrap().trim_start_matches('#');
            parse_hex(hex)
                .map(color_value)
                .ok_or_else(|| format!("invalid hex color {hex}"))
        }),
    );
    ctx.declare_func(
        "color",
        FuncDef::builder().param(ParamType::String).build(|args| {
            let name = args[0].as_str().unwrap();
            if name.eq_ignore_ascii_case("transparent") {
                return Ok(color_value(Rgba::TRANSPARENT));
            }
            color::named_color(name)
                .map(color_value)
                .ok_or_else(|| format!("unknown color {name}"))
        }),
    );
    ctx.declare_func(
        "rgb",
        FuncDef::builder()
            .params([ParamType::Number, ParamType::Number, ParamType::Number])
            .build(|args| {
                Ok(color_value(Rgba {
                    r: channel(&args[0]),
                    g: channel(&args[1]),
                    b: channel(&args[2]),
                    a: 255,
                }))
            }),
    );
    ctx.declare_func(
        "rgba",
        FuncDef::builder()
            .params([
                ParamType::Number,
                ParamType::Number,
                ParamType::Number,
                ParamType::Number,
            ])
            .build(|args| {
                Ok(color_value(Rgba {
                    r: channel(&args[0]),
                    g: channel(&args[1]),
                    b: channel(&args[2]),
                    a: channel(&args[3]),
                }))
            }),
    );
    ctx.declare_func(
        "hsl",
        FuncDef::builder()
            .params([ParamType::Number, ParamType::Number, ParamType::Number])
            .build(|args| {
                Ok(color_value(color::hsl(
                    number(&args[0]),
                    unit(&args[1]),
                    unit(&args[2]),
                    255,
                )))
            }),
    );
    ctx.declare_func(
        "hsla",
        FuncDef::builder()
            .params([
                ParamType::Number,
                ParamType::Number,
                ParamType::Number,
                ParamType::Number,
            ])
            .build(|args| {
                Ok(color_value(color::hsl(
                    number(&args[0]),
                    unit(&args[1]),
                    unit(&args[2]),
                    channel(&args[3]),
                )))
            }),
    );
    ctx.declare_func(
        "hwb",
        FuncDef::builder()
            .params([ParamType::Number, ParamType::Number, ParamType::Number])
            .build(|args| {
                Ok(color_value(color::hwb(
                    number(&args[0]),
                    unit(&args[1]),
                    unit(&args[2]),
                    255,
                )))
            }),
    );
    ctx.declare_func(
        "cmyk",
        FuncDef::builder()
            .params([
                ParamType::Number,
                ParamType::Number,
                ParamType::Number,
                ParamType::Number,
            ])
            .build(|args| {
                Ok(color_value(color::cmyk(
                    unit(&args[0]),
                    unit(&args[1]),
                    unit(&args[2]),
                    unit(&args[3]),
                    255,
                )))
            }),
    );
    ctx.declare_func(
        "env",
        FuncDef::builder()
            .param(ParamType::String)
            .variadic_param(ParamType::Any)
            .build(env_fn),
    );
}

fn env_fn(args: FuncArgs) -> Result<Value, String> {
    let name = args[0].as_str().unwrap();
    if let Ok(value) = std::env::var(name) {
        if let Some(color) = color::named_color(&value) {
            return Ok(color_value(color));
        }
        if let Ok(number) = value.parse::<f64>() {
            return Ok(Value::from(number));
        }
        return Ok(Value::from(value));
    }
    args.get(1)
        .cloned()
        .ok_or_else(|| format!("environment variable {name} is unset"))
}

fn number(value: &Value) -> f32 {
    value.as_f64().unwrap_or(0.0) as f32
}

fn channel(value: &Value) -> u8 {
    number(value).round().clamp(0.0, 255.0) as u8
}

fn unit(value: &Value) -> f32 {
    let n = number(value);
    if n > 1.0 {
        (n / 100.0).clamp(0.0, 1.0)
    } else {
        n.clamp(0.0, 1.0)
    }
}

fn color_value(color: Rgba) -> Value {
    Value::from_iter([
        ("__color", Value::from(true)),
        ("r", Value::from(color.r)),
        ("g", Value::from(color.g)),
        ("b", Value::from(color.b)),
        ("a", Value::from(color.a)),
    ])
}

fn ingest_raw(
    body: &Body,
    doc: &mut HclDocument,
    base_dir: &Path,
    stack: &mut Vec<PathBuf>,
    allow_config: bool,
) -> Result<(), String> {
    for block in body.blocks() {
        match block.identifier() {
            "import" => import_block(block, doc, base_dir, stack)?,
            "widget" if allow_config => {
                let name = one_label(block, "widget")?;
                let mut recipe = WidgetRecipe {
                    raw: Some(block.body().clone()),
                    locals: doc.locals.clone(),
                    ..WidgetRecipe::default()
                };
                let evaluated = evaluate_body(block.body(), &doc.locals, None, None)?;
                let parsed = widget_from_body(&evaluated)?;
                recipe.location = parsed.location;
                recipe.spec = parsed.spec;
                doc.named_widgets.insert(name, recipe);
            }
            _ => {}
        }
    }
    Ok(())
}

fn import_block(
    block: &Block,
    doc: &mut HclDocument,
    base_dir: &Path,
    stack: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let name = one_label(block, "import")?;
    let optional = match block.body().iter().find_map(|item| match item {
        hcl::Structure::Attribute(attr) if attr.key() == "optional" => Some(attr.expr()),
        _ => None,
    }) {
        Some(expr) => to_value(expr)?.as_bool().unwrap_or(false),
        None => false,
    };
    let Some(path) = resolve_hcl_path(&name, base_dir) else {
        return if optional {
            Ok(())
        } else {
            Err(format!("failed to read import {name}"))
        };
    };
    if stack.iter().any(|seen| seen == &path) {
        return Err(format!("cyclic import {}", path.display()));
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) if optional => return Ok(()),
        Err(err) => return Err(format!("failed to read import {}: {err}", path.display())),
    };
    stack.push(path.clone());
    let imported = parse_document_stack(&text, path.parent().unwrap_or(base_dir), stack)
        .map_err(|err| format!("invalid import {}: {err}", path.display()))?;
    stack.pop();
    merge_document(doc, imported);
    Ok(())
}

fn ingest_evaluated(body: &Body, doc: &mut HclDocument, allow_config: bool) -> Result<(), String> {
    for block in body.blocks() {
        match block.identifier() {
            "import" | "locals" | "widget" => {}
            "theme" if allow_config => {
                doc.theme_refs.push(one_label(block, "theme")?);
            }
            "configuration" if allow_config => {
                reject_labels(block, "configuration")?;
                for (key, value) in attributes_as_values(block.body())? {
                    doc.configuration.insert(key, value);
                }
            }
            "source" if allow_config => {
                let name = one_label(block, "source")?;
                doc.sources.push(RawSource {
                    name,
                    properties: attributes_as_values(block.body())?,
                });
            }
            "media" => ingest_media(block, &mut doc.theme)?,
            "defaults" => {
                reject_labels(block, "defaults")?;
                apply_block(&mut doc.theme.global, block.body(), None)?;
            }
            other => apply_catalog(&mut doc.theme, other, block)?,
        }
    }
    for attr in body.attributes() {
        return Err(format!(
            "unknown top-level attribute {}; use a block",
            attr.key()
        ));
    }
    Ok(())
}

fn ingest_media(block: &Block, spec: &mut ThemeSpec) -> Result<(), String> {
    reject_labels(block, "media")?;
    if !media_matches(block.body())? {
        return Ok(());
    }
    for nested in block.body().blocks() {
        match nested.identifier() {
            "defaults" => {
                reject_labels(nested, "defaults")?;
                apply_block(&mut spec.global, nested.body(), None)?;
            }
            other => apply_catalog(spec, other, nested)?,
        }
    }
    Ok(())
}

fn media_matches(body: &Body) -> Result<bool, String> {
    for attr in body.attributes() {
        let key = attr.key();
        let value = to_value(attr.expr())?;
        let value = &value;
        let ok = match key {
            "min_width" => units::MONITOR_WIDTH >= value_number(value, key)?,
            "max_width" => units::MONITOR_WIDTH < value_number(value, key)?,
            "min_height" => units::MONITOR_HEIGHT >= value_number(value, key)?,
            "max_height" => units::MONITOR_HEIGHT < value_number(value, key)?,
            "min_aspect_ratio" => {
                units::MONITOR_WIDTH / units::MONITOR_HEIGHT >= value_number(value, key)?
            }
            "max_aspect_ratio" => {
                units::MONITOR_WIDTH / units::MONITOR_HEIGHT < value_number(value, key)?
            }
            "monitor_id" => match value {
                Value::Number(n) => n.as_f64() == Some(0.0),
                Value::String(name) => name == "0",
                _ => false,
            },
            "enabled" => match value {
                Value::Bool(flag) => *flag,
                Value::String(name) => name == "1" || name.eq_ignore_ascii_case("true"),
                _ => false,
            },
            other => {
                return Err(format!("unknown media constraint {other}"));
            }
        };
        if !ok {
            return Ok(false);
        }
    }
    Ok(true)
}

fn widget_from_body(body: &Body) -> Result<WidgetRecipe, String> {
    let mut recipe = WidgetRecipe::default();
    for attr in body.attributes() {
        if attr.key() != "location" {
            return Err(format!(
                "unknown widget property {}; supported: location",
                attr.key()
            ));
        }
        recipe.location = Some(value_location(&to_value(attr.expr())?)?);
    }
    for block in body.blocks() {
        match block.identifier() {
            "locals" => {}
            "media" => ingest_media(block, &mut recipe.spec)?,
            "defaults" => {
                reject_labels(block, "defaults")?;
                apply_block(&mut recipe.spec.global, block.body(), None)?;
            }
            other => apply_catalog(&mut recipe.spec, other, block)?,
        }
    }
    Ok(recipe)
}

fn apply_catalog(spec: &mut ThemeSpec, name: &str, block: &Block) -> Result<(), String> {
    let widget = parse_widget(name).ok_or_else(|| {
        format!(
            "unknown block {name}; supported: configuration, source, widget, theme, import, locals, defaults, media, overlay, window, listview, element, element-index, element-text, inputbar, prompt, entry"
        )
    })?;
    let state = match block.labels().first().map(|label| label.as_str()) {
        None => State::Normal,
        Some(label) => {
            if widget != Widget::Element {
                return Err(format!(
                    "state {label} is only valid on element, not {}",
                    widget.as_str()
                ));
            }
            parse_state(label).ok_or_else(|| format!("unknown state {label}"))?
        }
    };
    if block.labels().len() > 1 {
        return Err(format!("{name} takes at most one label"));
    }
    apply_block(
        spec.widgets.entry((widget, state)).or_default(),
        block.body(),
        None,
    )
}

fn to_value(expr: &hcl::Expression) -> Result<Value, String> {
    let empty = Value::Object(hcl::Map::new());
    let ctx = eval_context(&empty, None, None);
    expr.evaluate(&ctx).map_err(eval_err)
}

fn apply_block(props: &mut Props, body: &Body, axis: Option<units::Axis>) -> Result<(), String> {
    if body.blocks().next().is_some() {
        return Err("style blocks cannot contain nested blocks".into());
    }
    for attr in body.attributes() {
        apply_prop(props, attr.key(), &to_value(attr.expr())?, axis)?;
    }
    Ok(())
}

fn apply_prop(
    props: &mut Props,
    name: &str,
    value: &Value,
    axis: Option<units::Axis>,
) -> Result<(), String> {
    if is_inherit(value) {
        if !props.inherit.iter().any(|item| item == name) {
            props.inherit.push(name.to_owned());
        }
        return Ok(());
    }
    let axis = axis.unwrap_or_else(|| units::axis_for_property(name));
    match name {
        "enabled" => props.enabled = Some(value_bool(value, name)?),
        "padding" => props.padding = Some(value_edges(value, name)?),
        "margin" => props.margin = Some(value_edges(value, name)?),
        "border" => props.border = Some(value_edges(value, name)?),
        "border_radius" => props.border_radius = Some(value_number(value, name)?),
        "border_color" => props.border_color = Some(value_color(value, name)?),
        "background_color" => props.background = Some(value_color(value, name)?),
        "background_image" => {
            if let Ok(color) = value_color(value, name) {
                if props.background.is_none() {
                    props.background = Some(color);
                }
            }
        }
        "text_color" => props.text_color = Some(value_color(value, name)?),
        "spacing" => props.spacing = Some(value_number(value, name)?),
        "orientation" => props.orientation = Some(value_orientation(value)?),
        "children" => props.children = Some(value_widgets(value)?),
        "width" => props.width = Some(value_number_on(value, name, axis)?),
        "height" => props.height = Some(value_number_on(value, name, axis)?),
        "max_height" => props.max_height = Some(value_number_on(value, name, axis)?),
        "font_size" => props.font_size = Some(value_number_on(value, name, axis)?),
        "location" => props.location = Some(value_location(value)?),
        other => {
            return Err(format!(
                "unknown property {other}; supported: enabled, padding, margin, border, border_radius, border_color, background_color, background_image, text_color, spacing, orientation, children, width, height, max_height, font_size, location"
            ));
        }
    }
    Ok(())
}

fn is_inherit(value: &Value) -> bool {
    value.as_str() == Some("inherit")
}

fn value_bool(value: &Value, name: &str) -> Result<bool, String> {
    value
        .as_bool()
        .ok_or_else(|| format!("{name} must be true or false"))
}

fn value_number(value: &Value, name: &str) -> Result<f32, String> {
    value_number_on(value, name, units::Axis::Horizontal)
}

fn value_number_on(value: &Value, name: &str, axis: units::Axis) -> Result<f32, String> {
    if let Some(n) = value.as_f64() {
        return Ok(n as f32);
    }
    if let Some(edges) = try_edges(value, axis) {
        return Ok(edges.top);
    }
    Err(format!("{name} must be a number"))
}

fn value_edges(value: &Value, name: &str) -> Result<Edges, String> {
    try_edges(value, units::Axis::Horizontal)
        .ok_or_else(|| format!("{name} must be a length or a list of 1-4 lengths"))
}

fn try_edges(value: &Value, axis: units::Axis) -> Option<Edges> {
    if let Some(n) = value.as_f64() {
        return Some(Edges::all(n as f32));
    }
    let items = value.as_array()?;
    let nums: Option<Vec<f32>> = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            item.as_f64().map(|n| {
                if items.len() == 2 && index == 0 {
                    let _ = axis;
                    n as f32
                } else {
                    n as f32
                }
            })
        })
        .collect();
    edges_from_nums(&nums?)
}

fn value_color(value: &Value, name: &str) -> Result<Rgba, String> {
    if let Some(color) = color_from_value(value) {
        return Ok(color);
    }
    if let Some(text) = value.as_str() {
        if text.eq_ignore_ascii_case("transparent") {
            return Ok(Rgba::TRANSPARENT);
        }
        if let Some(color) = color::named_color(text) {
            return Ok(color);
        }
        if let Some(color) = parse_hex(text.trim_start_matches('#')) {
            return Ok(color);
        }
    }
    Err(format!("{name} must be a color"))
}

fn color_from_value(value: &Value) -> Option<Rgba> {
    let object = value.as_object()?;
    if object.get("__color")?.as_bool()? {
        return Some(Rgba {
            r: object.get("r")?.as_f64()? as u8,
            g: object.get("g")?.as_f64()? as u8,
            b: object.get("b")?.as_f64()? as u8,
            a: object.get("a")?.as_f64()? as u8,
        });
    }
    None
}

fn value_location(value: &Value) -> Result<Location, String> {
    let name = match value {
        Value::String(name) => name.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| item.as_str().map(ToOwned::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| "location must be a name or list of names".to_string())?
            .join(" "),
        _ => return Err("location must be a name".into()),
    };
    Location::parse(&name).ok_or_else(|| format!("expected location, found {name}"))
}

fn value_orientation(value: &Value) -> Result<Orientation, String> {
    match value.as_str() {
        Some("horizontal") => Ok(Orientation::Horizontal),
        Some("vertical") => Ok(Orientation::Vertical),
        _ => Err("orientation must be \"horizontal\" or \"vertical\"".into()),
    }
}

fn value_widgets(value: &Value) -> Result<Vec<Widget>, String> {
    let items = value
        .as_array()
        .ok_or_else(|| "children must be a list of widget names".to_string())?;
    items
        .iter()
        .map(|item| {
            let name = item
                .as_str()
                .ok_or_else(|| format!("expected widget name, found {item}"))?;
            parse_widget(name).ok_or_else(|| format!("unknown widget {name} in children"))
        })
        .collect()
}

fn attributes_as_values(body: &Body) -> Result<BTreeMap<String, HclValue>, String> {
    if body.blocks().next().is_some() {
        return Err("this block cannot contain nested blocks".into());
    }
    let mut properties = BTreeMap::new();
    for attr in body.attributes() {
        properties.insert(
            attr.key().to_owned(),
            value_to_hcl(&to_value(attr.expr())?)?,
        );
    }
    Ok(properties)
}

fn value_to_hcl(value: &Value) -> Result<HclValue, String> {
    if let Some(n) = value.as_f64() {
        return Ok(HclValue::Number(n as f32));
    }
    if let Some(flag) = value.as_bool() {
        return Ok(HclValue::Bool(flag));
    }
    if let Some(text) = value.as_str() {
        return Ok(HclValue::String(text.to_owned()));
    }
    if let Some(items) = value.as_array() {
        let mut list = Vec::with_capacity(items.len());
        for item in items {
            let text = item
                .as_str()
                .ok_or_else(|| format!("expected a list of strings, found {item}"))?;
            list.push(text.to_owned());
        }
        return Ok(HclValue::List(list));
    }
    Err(format!(
        "expected number, string, boolean, or list, found {value}"
    ))
}

fn one_label(block: &Block, kind: &str) -> Result<String, String> {
    match block.labels() {
        [label] => Ok(label.as_str().to_owned()),
        _ => Err(format!("{kind} requires exactly one label")),
    }
}

fn reject_labels(block: &Block, kind: &str) -> Result<(), String> {
    if block.labels().is_empty() {
        Ok(())
    } else {
        Err(format!("{kind} does not take a label"))
    }
}

fn merge_document(into: &mut HclDocument, from: HclDocument) {
    into.configuration.extend(from.configuration);
    into.sources.extend(from.sources);
    into.theme_refs.extend(from.theme_refs);
    into.named_widgets.extend(from.named_widgets);
    into.theme.merge(from.theme);
}

fn resolve_hcl_path(name: &str, base_dir: &Path) -> Option<PathBuf> {
    let path = expand_tilde(name);
    let search = if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    };
    if search.is_file() {
        return Some(search);
    }
    if search.extension().is_some() {
        return None;
    }
    let candidate = search.with_extension("hcl");
    candidate.is_file().then_some(candidate)
}

fn expand_tilde(name: &str) -> PathBuf {
    if let Some(rest) = name.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(name)
}

fn parse_widget(name: &str) -> Option<Widget> {
    Some(match name {
        "overlay" => Widget::Overlay,
        "window" => Widget::Window,
        "listview" => Widget::ListView,
        "element" => Widget::Element,
        "element-index" => Widget::ElementIndex,
        "element-text" => Widget::ElementText,
        "inputbar" => Widget::InputBar,
        "prompt" => Widget::Prompt,
        "entry" => Widget::Entry,
        _ => return None,
    })
}

fn parse_state(name: &str) -> Option<State> {
    Some(match name {
        "normal" => State::Normal,
        "selected" => State::Selected,
        "hover" => State::Hover,
        _ => return None,
    })
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    let expand = |c: u8| (c << 4) | c;
    let digit = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok();
    let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    match hex.len() {
        3 => Some(Rgba {
            r: expand(digit(0)?),
            g: expand(digit(1)?),
            b: expand(digit(2)?),
            a: 255,
        }),
        4 => Some(Rgba {
            r: expand(digit(0)?),
            g: expand(digit(1)?),
            b: expand(digit(2)?),
            a: expand(digit(3)?),
        }),
        6 => Some(Rgba {
            r: pair(0)?,
            g: pair(2)?,
            b: pair(4)?,
            a: 255,
        }),
        8 => Some(Rgba {
            r: pair(0)?,
            g: pair(2)?,
            b: pair(4)?,
            a: pair(6)?,
        }),
        _ => None,
    }
}

fn edges_from_nums(nums: &[f32]) -> Option<Edges> {
    Some(match nums {
        [all] => Edges::all(*all),
        [v, h] => Edges::vh(*v, *h),
        [t, h, b] => Edges {
            top: *t,
            right: *h,
            bottom: *b,
            left: *h,
        },
        [t, r, b, l] => Edges {
            top: *t,
            right: *r,
            bottom: *b,
            left: *l,
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_and_padding_list() {
        let spec = parse_spec(
            r#"
                window {
                    background_color = hex("abc")
                    padding = [0, 8]
                    border = 1
                }
            "#,
        )
        .unwrap();
        let window = spec.props(Widget::Window, State::Normal);
        assert_eq!(
            window.background,
            Some(Rgba {
                r: 0xaa,
                g: 0xbb,
                b: 0xcc,
                a: 255
            })
        );
        assert_eq!(window.padding, Some(Edges::vh(0.0, 8.0)));
        assert_eq!(window.border, Some(Edges::all(1.0)));
    }

    #[test]
    fn parses_element_state_and_children() {
        let spec = parse_spec(
            r#"
                element "selected" { background_color = hex("010203") }
                element { children = ["element-text", "element-index"] }
            "#,
        )
        .unwrap();
        assert_eq!(
            spec.props(Widget::Element, State::Selected).background,
            Some(Rgba {
                r: 1,
                g: 2,
                b: 3,
                a: 255
            })
        );
        assert_eq!(
            spec.props(Widget::Element, State::Normal).children,
            Some(vec![Widget::ElementText, Widget::ElementIndex])
        );
    }

    #[test]
    fn comments_and_transparent_are_accepted() {
        let spec = parse_spec(
            r#"
                // line comment
                window {
                    background_color = transparent()
                }
            "#,
        )
        .unwrap();
        assert_eq!(
            spec.props(Widget::Window, State::Normal).background,
            Some(Rgba::TRANSPARENT)
        );
    }

    #[test]
    fn unknown_widget_is_rejected() {
        let error = parse_spec("search { padding = 0 }").unwrap_err();
        assert!(error.contains("unknown block search"), "{error}");
    }

    #[test]
    fn unknown_property_is_rejected() {
        let error = parse_spec("window { colour = hex(\"fff\") }").unwrap_err();
        assert!(error.contains("unknown property colour"), "{error}");
    }

    #[test]
    fn em_and_percent_resolve() {
        let spec = parse_spec(
            r#"
                window {
                    padding = em(1)
                    width = pct(10)
                    height = ch(2)
                }
            "#,
        )
        .unwrap();
        let window = spec.props(Widget::Window, State::Normal);
        assert_eq!(window.padding, Some(Edges::all(14.0)));
        assert_eq!(window.width, Some(192.0));
        assert_eq!(window.height, Some(18.0));
    }

    #[test]
    fn calc_local_and_quotes() {
        let spec = parse_spec(
            r#"
                locals { accent = hex("010203") }
                window {
                    width = pct(10) + 20
                    background_color = local.accent
                    border_color = local.accent
                    location = "south east"
                }
            "#,
        )
        .unwrap();
        let window = spec.props(Widget::Window, State::Normal);
        assert_eq!(window.width, Some(212.0));
        assert_eq!(
            window.background,
            Some(Rgba {
                r: 1,
                g: 2,
                b: 3,
                a: 255
            })
        );
        assert_eq!(
            window.border_color,
            Some(Rgba {
                r: 1,
                g: 2,
                b: 3,
                a: 255
            })
        );
        assert_eq!(window.location, Some(Location::SouthEast));
    }

    #[test]
    fn named_and_hsl_colors_parse() {
        let spec = parse_spec(
            r#"
                window { background_color = color("SeaGreen") }
                element "selected" { background_color = hsl(0, 100, 50) }
                listview { background_color = hwb(0, 0, 0) }
                entry { background_color = cmyk(0, 1, 1, 0) }
            "#,
        )
        .unwrap();
        assert_eq!(
            spec.props(Widget::Window, State::Normal).background,
            Some(Rgba {
                r: 0x2e,
                g: 0x8b,
                b: 0x57,
                a: 255
            })
        );
        assert_eq!(
            spec.props(Widget::Element, State::Selected).background,
            Some(Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255
            })
        );
        assert_eq!(
            spec.props(Widget::ListView, State::Normal).background,
            Some(Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255
            })
        );
        assert_eq!(
            spec.props(Widget::Entry, State::Normal).background,
            Some(Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255
            })
        );
    }

    #[test]
    fn import_merges_a_relative_file() {
        let dir = std::env::temp_dir().join(format!(
            "klemmbrett-import-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("extra.hcl"), "window { border = 4 }\n").unwrap();
        let spec = parse_spec_at(
            r#"import "extra.hcl" {}
window { padding = 2 }
"#,
            &dir,
        )
        .unwrap();
        let window = spec.props(Widget::Window, State::Normal);
        assert_eq!(window.border, Some(Edges::all(4.0)));
        assert_eq!(window.padding, Some(Edges::all(2.0)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn optional_import_ignores_a_missing_file() {
        let spec = parse_spec(
            r#"import "definitely-missing-klemmbrett.hcl" { optional = true }
window { padding = 3 }
"#,
        )
        .unwrap();
        assert_eq!(
            spec.props(Widget::Window, State::Normal).padding,
            Some(Edges::all(3.0))
        );
    }

    #[test]
    fn media_applies_matching_and_skips_the_rest() {
        let spec = parse_spec(
            r#"
                media {
                    min_width = 100
                    window { border = 3 }
                }
                media {
                    max_width = 100
                    window { padding = 9 }
                }
            "#,
        )
        .unwrap();
        let window = spec.props(Widget::Window, State::Normal);
        assert_eq!(window.border, Some(Edges::all(3.0)));
        assert_eq!(window.padding, None);
    }

    #[test]
    fn env_falls_back_to_a_named_color() {
        let spec = parse_spec(
            r#"window { background_color = env("KLEMMBRETT_UNSET_COLOR_TEST", color("red")) }"#,
        )
        .unwrap();
        assert_eq!(
            spec.props(Widget::Window, State::Normal).background,
            Some(Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255
            })
        );
    }

    #[test]
    fn parse_spec_rejects_config_blocks() {
        let error = parse_spec("configuration { label_max = 64 }").unwrap_err();
        assert!(error.contains("belong in the config file"), "{error}");
    }

    #[test]
    fn parse_spec_rejects_named_widget_blocks() {
        let error = parse_spec(r#"widget "filter" { location = "center" }"#).unwrap_err();
        assert!(error.contains("belong in the config file"), "{error}");
    }

    #[test]
    fn named_widget_parses_location_and_packing() {
        let doc = parse_document(
            r#"
                widget "filter" {
                    location = "center"
                    window { children = ["inputbar", "listview"] }
                    inputbar { children = ["prompt", "entry"] }
                }
            "#,
        )
        .unwrap();
        let recipe = &doc.named_widgets["filter"];
        assert_eq!(recipe.location, Some(Location::Center));
        assert_eq!(
            recipe
                .spec
                .props(Widget::Window, State::Normal)
                .children
                .as_deref(),
            Some([Widget::InputBar, Widget::ListView].as_slice())
        );
    }

    #[test]
    fn document_parses_config_sources_theme_and_strings() {
        let doc = parse_document(
            r#"
                configuration { label_max = 32 }
                source "history" {
                    type = "clipboard-history"
                    skip_whitespace_only = false
                }
                source "snippets" {
                    type = "file"
                    path = "snippets.yaml"
                    format = "yaml"
                }
                theme "nord" {}
                element "selected" { background_color = hex("010203") }
            "#,
        )
        .unwrap();
        assert_eq!(
            doc.configuration
                .get("label_max")
                .and_then(HclValue::as_number),
            Some(32.0)
        );
        assert_eq!(doc.sources.len(), 2);
        assert_eq!(doc.sources[0].name, "history");
        assert_eq!(
            doc.sources[1]
                .properties
                .get("path")
                .and_then(HclValue::as_text),
            Some("snippets.yaml")
        );
        assert_eq!(doc.theme_refs, vec!["nord"]);
        assert_eq!(
            doc.theme.props(Widget::Element, State::Selected).background,
            Some(Rgba {
                r: 1,
                g: 2,
                b: 3,
                a: 255
            })
        );
    }
}
