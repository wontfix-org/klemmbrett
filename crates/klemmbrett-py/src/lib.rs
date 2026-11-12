//! `klemmbrett._core`: the daemon, its built-in providers, and the operations
//! Python handlers call back into.
//!
//! Python hands configuration over as plain data (see `python/klemmbrett/_wire.py`):
//! theme specs are `{"selector": {property: value}}`, layout nodes are
//! `{"kind", "style", "children", "text"}`, and records are `(key, label, value)`
//! tuples.

use klemmbrett::cli::{self, Invocation};
use klemmbrett::{
    cmyk, hsl, hwb, parse_color, resolve_config_path, systemd, ui, AppConfig, ClipboardHistorySource,
    Edges, ExecutablesSource, FileFormat, FileSource, Handlers, KeySpec, Launch, Layout,
    LayoutBuilder, LayoutContext, LayoutSpec, Location, MultiSelect, Node, NodeKind, Orientation,
    Props, Record, RecordSource, Rgba, Runtime, State, Style, Theme, ThemeSpec,
    WaylandWindowsSource, Widget, WidgetDef, Widgets,
};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, OnceLock};

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

type RecordTuple = (String, String, String);

fn value_error(message: impl Into<String>) -> PyErr {
    PyValueError::new_err(message.into())
}

fn runtime_error(message: String) -> PyErr {
    PyRuntimeError::new_err(message)
}

fn runtime() -> PyResult<&'static Runtime> {
    RUNTIME.get().ok_or_else(|| {
        PyRuntimeError::new_err(
            "klemmbrett is not running; this only works from handlers and providers in the daemon",
        )
    })
}

fn to_tuple(record: Record) -> RecordTuple {
    (record.key, record.label, record.value)
}

fn from_tuple((key, label, value): RecordTuple) -> Record {
    Record { key, label, value }
}

// ---------------------------------------------------------------------------
// Theme and layout data
// ---------------------------------------------------------------------------

fn color(value: &Bound<'_, PyAny>) -> Result<Rgba, String> {
    if let Ok(text) = value.extract::<String>() {
        return parse_color(&text);
    }
    let (r, g, b, a) = value
        .extract::<(u8, u8, u8, u8)>()
        .map_err(|_| "expected a color string or an (r, g, b, a) tuple".to_string())?;
    Ok(Rgba::rgba(r, g, b, a))
}

fn edges(value: &Bound<'_, PyAny>) -> Result<Edges, String> {
    if let Ok(all) = value.extract::<f32>() {
        return Ok(Edges::all(all));
    }
    let values = value
        .extract::<Vec<f32>>()
        .map_err(|_| "expected a number or a list of 1 to 4 numbers".to_string())?;
    Edges::from_shorthand(&values).ok_or_else(|| "expected 1 to 4 numbers".to_string())
}

fn number(value: &Bound<'_, PyAny>) -> Result<f32, String> {
    value.extract::<f32>().map_err(|_| "expected a number".to_string())
}

fn props(dict: &Bound<'_, PyAny>, context: &str) -> PyResult<Props> {
    let dict = dict
        .cast::<PyDict>()
        .map_err(|_| value_error(format!("{context}: expected a dict of style properties")))?;
    let mut props = Props::default();
    for (key, value) in dict.iter() {
        let key: String = key.extract()?;
        let text = || {
            value
                .extract::<String>()
                .map_err(|_| "expected a string".to_string())
        };
        let result: Result<(), String> = (|| {
            match key.as_str() {
                "padding" => props.padding = Some(edges(&value)?),
                "margin" => props.margin = Some(edges(&value)?),
                "border" => props.border = Some(edges(&value)?),
                "border_radius" => props.border_radius = Some(number(&value)?),
                "background_color" => props.background = Some(color(&value)?),
                "border_color" => props.border_color = Some(color(&value)?),
                "text_color" => props.text_color = Some(color(&value)?),
                "spacing" => props.spacing = Some(number(&value)?),
                "orientation" => {
                    let name = text()?;
                    props.orientation = Some(Orientation::parse(&name).ok_or_else(|| {
                        format!("unknown orientation {name:?}; use horizontal or vertical")
                    })?);
                }
                "width" => props.width = Some(number(&value)?),
                "height" => props.height = Some(number(&value)?),
                "max_height" => props.max_height = Some(number(&value)?),
                "font_size" => props.font_size = Some(number(&value)?),
                "location" => props.location = Some(location(&text()?)?),
                "text" => props.text = Some(text()?),
                "inherit" => {
                    let names = value
                        .extract::<Vec<String>>()
                        .map_err(|_| "expected a list of property names".to_string())?;
                    for name in &names {
                        if !Props::NAMES.contains(&name.as_str()) {
                            return Err(format!("cannot inherit unknown property {name:?}"));
                        }
                    }
                    props.inherit = names;
                }
                other => {
                    return Err(format!(
                        "unknown style property {other:?}; expected one of {}",
                        Props::NAMES.join(", ")
                    ))
                }
            }
            Ok(())
        })();
        result.map_err(|err| value_error(format!("{context}: {key}: {err}")))?;
    }
    Ok(props)
}

fn location(name: &str) -> Result<Location, String> {
    Location::parse(name).ok_or_else(|| format!("unknown location {name:?}"))
}

fn selector(text: &str) -> Result<(Option<Widget>, State), String> {
    let (name, state) = match text.split_once(':') {
        Some((name, state)) => (name, Some(state)),
        None => (text, None),
    };
    let state = match state {
        None | Some("normal") => State::Normal,
        Some("selected") => State::Selected,
        Some("hover") => State::Hover,
        Some(other) => return Err(format!("unknown state {other:?} in selector {text:?}")),
    };
    if name == "defaults" {
        return if state == State::Normal {
            Ok((None, state))
        } else {
            Err("defaults has no states".into())
        };
    }
    let widget = Widget::parse(name).ok_or_else(|| {
        let names: Vec<&str> = Widget::ALL.iter().map(|widget| widget.as_str()).collect();
        format!(
            "unknown theme element {name:?}; expected defaults or one of {}",
            names.join(", ")
        )
    })?;
    Ok((Some(widget), state))
}

fn theme_spec(value: &Bound<'_, PyAny>) -> PyResult<ThemeSpec> {
    let dict = value
        .cast::<PyDict>()
        .map_err(|_| value_error("theme: expected a dict of selector to properties"))?;
    let mut spec = ThemeSpec::default();
    for (key, value) in dict.iter() {
        let key: String = key.extract()?;
        let (widget, state) = selector(&key).map_err(|err| value_error(format!("theme: {err}")))?;
        let props = props(&value, &format!("theme {key}"))?;
        match widget {
            None => spec.global.merge(&props),
            Some(widget) => spec.set(widget, state, props),
        }
    }
    Ok(spec)
}

fn optional_spec(value: Option<&Bound<'_, PyAny>>) -> PyResult<Option<ThemeSpec>> {
    match value {
        Some(value) if !value.is_none() => theme_spec(value).map(Some),
        _ => Ok(None),
    }
}

fn node(value: &Bound<'_, PyAny>) -> PyResult<Node> {
    let dict = value
        .cast::<PyDict>()
        .map_err(|_| value_error("layout: expected a node dict"))?;
    let kind: String = dict
        .get_item("kind")?
        .ok_or_else(|| value_error("layout: node without a kind"))?
        .extract()?;
    let kind = match kind.as_str() {
        "window" => NodeKind::Window,
        "inputbar" => NodeKind::InputBar,
        "prompt" => NodeKind::Prompt,
        "entry" => NodeKind::Entry,
        "listview" => NodeKind::ListView,
        "element" => NodeKind::Element,
        "marker" => NodeKind::Marker,
        "index" => NodeKind::Index,
        "text" => NodeKind::Text(match dict.get_item("text")? {
            Some(text) => text.extract()?,
            None => String::new(),
        }),
        "spacer" => NodeKind::Spacer,
        "row" => NodeKind::Row,
        "column" => NodeKind::Column,
        other => return Err(value_error(format!("layout: unknown node kind {other:?}"))),
    };
    let style = match dict.get_item("style")? {
        Some(style) if !style.is_none() => props(&style, &format!("layout {}", kind.name()))?,
        _ => Props::default(),
    };
    let children = match dict.get_item("children")? {
        Some(children) if !children.is_none() => children
            .try_iter()?
            .map(|child| node(&child?))
            .collect::<PyResult<Vec<_>>>()?,
        _ => Vec::new(),
    };
    Ok(Node {
        kind,
        style,
        children,
    })
}

fn layout(value: &Bound<'_, PyAny>) -> PyResult<Layout> {
    let root = node(value)?;
    let location = match value.cast::<PyDict>()?.get_item("location")? {
        Some(name) if !name.is_none() => {
            Some(location(&name.extract::<String>()?).map_err(value_error)?)
        }
        _ => None,
    };
    Layout::new(root, location).map_err(|err| value_error(format!("layout: {err}")))
}

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------

/// A provider implemented in Rust. Python wraps these in `klemmbrett.providers`.
#[pyclass(frozen, module = "klemmbrett._core")]
struct NativeSource {
    inner: Arc<dyn RecordSource>,
    kind: &'static str,
}

#[pymethods]
impl NativeSource {
    #[staticmethod]
    #[pyo3(signature = (path, limit, coalesce_growing, skip_whitespace_only))]
    fn clipboard_history(
        path: PathBuf,
        limit: usize,
        coalesce_growing: bool,
        skip_whitespace_only: bool,
    ) -> PyResult<Self> {
        let source =
            ClipboardHistorySource::open(&path, limit, coalesce_growing, skip_whitespace_only)
                .map_err(value_error)?;
        Ok(Self {
            inner: Arc::new(source),
            kind: "ClipboardHistory",
        })
    }

    #[staticmethod]
    #[pyo3(signature = (path, format=None))]
    fn file(path: PathBuf, format: Option<&str>) -> PyResult<Self> {
        let format = match format {
            Some(name) => FileFormat::parse(name).map_err(value_error)?,
            None => FileFormat::for_path(&path),
        };
        let source = FileSource::open(path, format).map_err(value_error)?;
        Ok(Self {
            inner: Arc::new(source),
            kind: "File",
        })
    }

    #[staticmethod]
    fn executables() -> Self {
        Self {
            inner: Arc::new(ExecutablesSource),
            kind: "Executables",
        }
    }

    #[staticmethod]
    fn wayland_windows() -> Self {
        Self {
            inner: Arc::new(WaylandWindowsSource),
            kind: "WaylandWindows",
        }
    }

    /// Reads the current records, as the popup would.
    fn records(&self, py: Python<'_>) -> PyResult<Vec<RecordTuple>> {
        let inner = self.inner.clone();
        let records = py.detach(move || inner.records()).map_err(runtime_error)?;
        Ok(records.into_iter().map(to_tuple).collect())
    }

    fn __repr__(&self) -> String {
        format!("<native {} provider>", self.kind)
    }
}

/// Calls a Python function for records. Runs on a background thread.
struct PySource {
    fetch: Py<PyAny>,
}

impl RecordSource for PySource {
    fn records(&self) -> Result<Vec<Record>, String> {
        Python::attach(|py| {
            let result = self
                .fetch
                .call0(py)
                .and_then(|records| records.extract::<Vec<RecordTuple>>(py).map_err(PyErr::from));
            match result {
                Ok(records) => Ok(records.into_iter().map(from_tuple).collect()),
                Err(err) => {
                    let message = err.to_string();
                    err.print(py);
                    Err(message)
                }
            }
        })
    }
}

fn source(value: &Bound<'_, PyAny>) -> PyResult<Option<Arc<dyn RecordSource>>> {
    if value.is_none() {
        return Ok(None);
    }
    if let Ok(native) = value.cast::<NativeSource>() {
        return Ok(Some(native.get().inner.clone()));
    }
    if value.is_callable() {
        return Ok(Some(Arc::new(PySource {
            fetch: value.clone().unbind(),
        })));
    }
    Err(value_error(
        "provider: expected a native provider or a callable returning records",
    ))
}

// ---------------------------------------------------------------------------
// Layouts
// ---------------------------------------------------------------------------

/// Calls `build(widget, multi, prompt)` for every popup.
struct PyLayout {
    build: Py<PyAny>,
}

impl LayoutBuilder for PyLayout {
    fn build(&self, ctx: &LayoutContext) -> Result<Layout, String> {
        Python::attach(|py| {
            let result = self
                .build
                .call1(py, (ctx.widget.as_str(), ctx.multi, ctx.prompt))
                .and_then(|value| layout(value.bind(py)));
            result.map_err(|err| {
                let message = err.to_string();
                err.print(py);
                message
            })
        })
    }
}

fn layout_spec(value: Option<&Bound<'_, PyAny>>) -> PyResult<LayoutSpec> {
    let Some(value) = value.filter(|value| !value.is_none()) else {
        return Ok(LayoutSpec::Default);
    };
    if value.is_callable() {
        return Ok(LayoutSpec::Dynamic(Arc::new(PyLayout {
            build: value.clone().unbind(),
        })));
    }
    Ok(LayoutSpec::Static(Arc::new(layout(value)?)))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

enum Event {
    Select(String, Vec<Record>),
    Highlight(String, Record),
    Clipboard(String),
}

/// Queues events to one Python thread, so handlers run in the order they
/// happened and never block the popup.
struct PyHandlers {
    events: mpsc::Sender<Event>,
    clipboard: bool,
}

impl PyHandlers {
    fn spawn(dispatcher: Py<PyAny>, clipboard: bool) -> PyResult<Self> {
        let (events, queue) = mpsc::channel::<Event>();
        std::thread::Builder::new()
            .name("klemmbrett-handlers".into())
            .spawn(move || {
                for event in queue {
                    Python::attach(|py| {
                        let dispatcher = dispatcher.bind(py);
                        let result = match event {
                            Event::Select(widget, records) => {
                                let records: Vec<RecordTuple> =
                                    records.into_iter().map(to_tuple).collect();
                                dispatcher.call_method1("select", (widget, records))
                            }
                            Event::Highlight(widget, record) => {
                                dispatcher.call_method1("highlight", (widget, to_tuple(record)))
                            }
                            Event::Clipboard(text) => {
                                dispatcher.call_method1("clipboard", (text,))
                            }
                        };
                        if let Err(err) = result {
                            err.print(py);
                        }
                    });
                }
            })
            .map_err(|err| runtime_error(format!("failed to start the handler thread: {err}")))?;
        Ok(Self { events, clipboard })
    }

    fn send(&self, event: Event) {
        if self.events.send(event).is_err() {
            eprintln!("klemmbrett: the handler thread has stopped; dropping an event");
        }
    }
}

impl Handlers for PyHandlers {
    fn select(&self, widget: &str, records: Vec<Record>) {
        self.send(Event::Select(widget.to_owned(), records));
    }

    fn highlight(&self, widget: &str, record: Record) {
        self.send(Event::Highlight(widget.to_owned(), record));
    }

    fn clipboard(&self, text: &str) {
        if self.clipboard {
            self.send(Event::Clipboard(text.to_owned()));
        }
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

fn item<'py>(dict: &Bound<'py, PyDict>, key: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
    Ok(dict.get_item(key)?.filter(|value| !value.is_none()))
}

fn launch(value: Option<Bound<'_, PyAny>>) -> PyResult<Launch> {
    let Some(value) = value else {
        return Ok(Launch::Auto);
    };
    if let Ok(name) = value.extract::<String>() {
        return match name.as_str() {
            "auto" => Ok(Launch::Auto),
            "posix" => Ok(Launch::Posix),
            "systemd-run" | "systemd_run" => Ok(Launch::SystemdRun),
            other => Err(value_error(format!(
                "launch: unknown mode {other:?}; use auto, posix, systemd-run, or a command list"
            ))),
        };
    }
    let wrapper: Vec<String> = value
        .extract()
        .map_err(|_| value_error("launch: expected a mode name or a list of strings"))?;
    if wrapper.is_empty() {
        return Err(value_error("launch: the wrapper command must not be empty"));
    }
    Ok(Launch::Wrapper(wrapper))
}

fn multi(value: Option<Bound<'_, PyAny>>, widget: &str) -> PyResult<Option<MultiSelect>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let (select, deselect): (String, String) = value.extract()?;
    let key = |spec: &str| {
        KeySpec::parse(spec).map_err(|err| value_error(format!("widget {widget:?}: {err}")))
    };
    Ok(Some(MultiSelect {
        select: key(&select)?,
        deselect: key(&deselect)?,
    }))
}

fn widget_def(value: &Bound<'_, PyAny>) -> PyResult<WidgetDef> {
    let dict = value.cast::<PyDict>()?;
    let name: String = dict
        .get_item("name")?
        .ok_or_else(|| value_error("widget without a name"))?
        .extract()?;
    let context = |err: PyErr| {
        Python::attach(|py| {
            let message = format!("widget {name:?}: {}", err.value(py));
            PyErr::from_type(err.get_type(py), message)
        })
    };
    let source = match item(dict, "source")? {
        Some(value) => source(&value).map_err(context)?,
        None => None,
    };
    let mut def = WidgetDef::new(name.clone(), source);
    def.layout = layout_spec(item(dict, "layout")?.as_ref()).map_err(context)?;
    def.theme = optional_spec(item(dict, "theme")?.as_ref()).map_err(context)?;
    def.location = match item(dict, "location")? {
        Some(value) => Some(location(&value.extract::<String>()?).map_err(value_error).map_err(context)?),
        None => None,
    };
    def.multi = multi(item(dict, "multi")?, &name)?;
    def.highlight = match item(dict, "highlight")? {
        Some(value) => value.extract()?,
        None => false,
    };
    Ok(def)
}

fn app_config(config: &Bound<'_, PyAny>) -> PyResult<AppConfig> {
    let dict = config
        .cast::<PyDict>()
        .map_err(|_| value_error("the config loader must return a dict"))?;
    let defs = match item(dict, "widgets")? {
        Some(widgets) => widgets
            .try_iter()?
            .map(|widget| widget_def(&widget?))
            .collect::<PyResult<Vec<_>>>()?,
        None => Vec::new(),
    };
    let palette = optional_spec(item(dict, "theme")?.as_ref())?.unwrap_or_default();
    let overrides = optional_spec(item(dict, "overrides")?.as_ref())?.unwrap_or_default();
    let widgets = Widgets::new(defs, palette, overrides).map_err(value_error)?;
    let handlers: Arc<dyn Handlers> = match item(dict, "dispatcher")? {
        Some(dispatcher) => {
            let clipboard = match item(dict, "clipboard")? {
                Some(value) => value.extract()?,
                None => false,
            };
            Arc::new(PyHandlers::spawn(dispatcher.unbind(), clipboard)?)
        }
        None => Arc::new(klemmbrett::NoHandlers),
    };
    let mut app = AppConfig {
        launch: launch(item(dict, "launch")?)?,
        widgets,
        handlers,
        ..AppConfig::default()
    };
    if let Some(label_max) = item(dict, "label_max")? {
        app.label_max = label_max.extract()?;
        if app.label_max == 0 {
            return Err(value_error("label_max must be greater than zero"));
        }
    }
    Ok(app)
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Runs the command line. `load(path)` returns the configuration for the
/// daemon; `exe` is the command that starts klemmbrett again (for systemd).
#[pyfunction]
fn main(py: Python<'_>, argv: Vec<String>, load: Bound<'_, PyAny>, exe: Vec<String>) -> PyResult<i32> {
    let invocation = match cli::parse(&argv) {
        Ok(invocation) => invocation,
        Err(err) => {
            let code = err.exit_code();
            let _ = err.print();
            return Ok(code);
        }
    };
    match invocation {
        Invocation::Show(args) => Ok(py.detach(|| cli::run_popup(&args, false))),
        Invocation::Prompt(args) => Ok(py.detach(|| cli::run_popup(&args, true))),
        Invocation::InstallSystemd { config } => {
            let config = match config {
                Some(path) => Some(absolute(&path)?),
                None => None,
            };
            match py.detach(|| systemd::install(&exe, config.as_deref())) {
                Ok(()) => Ok(0),
                Err(err) => {
                    eprintln!("klemmbrett: {err}");
                    Ok(1)
                }
            }
        }
        Invocation::Daemon { config } => {
            let path = match resolve_config_path(config.as_deref()) {
                Ok(path) => path,
                Err(err) => {
                    eprintln!("klemmbrett: {err}");
                    return Ok(1);
                }
            };
            let loaded = load.call1((path,))?;
            let app = app_config(&loaded)?;
            drop(loaded);
            let result = py.detach(move || {
                ui::run(app, |runtime| {
                    let _ = RUNTIME.set(runtime);
                })
            });
            match result {
                Ok(()) => Ok(0),
                Err(err) => {
                    eprintln!("klemmbrett: {err}");
                    Ok(1)
                }
            }
        }
    }
}

fn absolute(path: &Path) -> PyResult<PathBuf> {
    std::path::absolute(path)
        .map_err(|err| value_error(format!("cannot resolve {}: {err}", path.display())))
}

// ---------------------------------------------------------------------------
// Operations for handlers
// ---------------------------------------------------------------------------

#[pyfunction]
fn copy(py: Python<'_>, text: String) -> PyResult<()> {
    let runtime = runtime()?;
    py.detach(|| runtime.copy(&text)).map_err(runtime_error)
}

#[pyfunction]
fn launch_command(py: Python<'_>, argv: Vec<String>) -> PyResult<()> {
    if argv.is_empty() {
        return Err(value_error("launch: the command must not be empty"));
    }
    let runtime = runtime()?;
    py.detach(|| runtime.launch(&argv)).map_err(runtime_error)
}

#[pyfunction]
fn activate_window(py: Python<'_>, key: String) -> PyResult<()> {
    let runtime = runtime()?;
    py.detach(|| runtime.activate_window(&key))
        .map_err(runtime_error)
}

#[pyfunction]
fn set_theme(py: Python<'_>, theme: Bound<'_, PyAny>) -> PyResult<()> {
    let spec = optional_spec(Some(&theme))?.unwrap_or_default();
    let runtime = runtime()?;
    py.detach(|| runtime.set_theme(spec)).map_err(runtime_error)
}

#[pyfunction]
#[pyo3(signature = (widget, records=None, pointer=None))]
fn show(
    py: Python<'_>,
    widget: String,
    records: Option<Vec<RecordTuple>>,
    pointer: Option<(i32, i32)>,
) -> PyResult<()> {
    let records = records.map(|records| records.into_iter().map(from_tuple).collect());
    let runtime = runtime()?;
    py.detach(|| runtime.show(&widget, records, pointer))
        .map_err(runtime_error)
}

#[pyfunction]
fn quit(py: Python<'_>) -> PyResult<()> {
    let runtime = runtime()?;
    py.detach(|| runtime.quit()).map_err(runtime_error)
}

#[pyfunction]
fn is_running() -> bool {
    RUNTIME.get().is_some()
}

/// `$XDG_DATA_HOME/klemmbrett/history.sqlite` or its `~/.local/share` fallback.
#[pyfunction]
fn default_history_path() -> PathBuf {
    klemmbrett::default_db_path()
}

// ---------------------------------------------------------------------------
// Helpers for the Python layer and its tests
// ---------------------------------------------------------------------------

fn rgba_tuple(color: Rgba) -> (u8, u8, u8, u8) {
    (color.r, color.g, color.b, color.a)
}

#[pyfunction(name = "parse_color")]
fn parse_color_py(text: &str) -> PyResult<(u8, u8, u8, u8)> {
    parse_color(text).map(rgba_tuple).map_err(value_error)
}

#[pyfunction(name = "hsl")]
#[pyo3(signature = (h, s, l, a=255))]
fn hsl_py(h: f32, s: f32, l: f32, a: u8) -> (u8, u8, u8, u8) {
    rgba_tuple(hsl(h, s, l, a))
}

#[pyfunction(name = "hwb")]
#[pyo3(signature = (h, w, b, a=255))]
fn hwb_py(h: f32, w: f32, b: f32, a: u8) -> (u8, u8, u8, u8) {
    rgba_tuple(hwb(h, w, b, a))
}

#[pyfunction(name = "cmyk")]
#[pyo3(signature = (c, m, y, k, a=255))]
fn cmyk_py(c: f32, m: f32, y: f32, k: f32, a: u8) -> (u8, u8, u8, u8) {
    rgba_tuple(cmyk(c, m, y, k, a))
}

fn style_dict<'py>(py: Python<'py>, style: &Style) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    let edges = |edges: Edges| (edges.top, edges.right, edges.bottom, edges.left);
    dict.set_item("padding", edges(style.padding))?;
    dict.set_item("margin", edges(style.margin))?;
    dict.set_item("border", edges(style.border))?;
    dict.set_item("border_radius", style.border_radius)?;
    dict.set_item("background_color", rgba_tuple(style.background))?;
    dict.set_item("border_color", rgba_tuple(style.border_color))?;
    dict.set_item("text_color", rgba_tuple(style.text_color))?;
    dict.set_item("spacing", style.spacing)?;
    dict.set_item(
        "orientation",
        match style.orientation {
            Orientation::Horizontal => "horizontal",
            Orientation::Vertical => "vertical",
        },
    )?;
    dict.set_item("width", style.width)?;
    dict.set_item("height", style.height)?;
    dict.set_item("max_height", style.max_height)?;
    dict.set_item("font_size", style.font_size)?;
    dict.set_item("location", style.location.map(Location::as_str))?;
    dict.set_item("text", style.text.clone())?;
    Ok(dict)
}

/// Resolves `layers` over the built-in default and returns every element's
/// final style, keyed like theme selectors.
#[pyfunction]
fn resolve_theme<'py>(py: Python<'py>, layers: Bound<'py, PyList>) -> PyResult<Bound<'py, PyDict>> {
    let specs = layers
        .iter()
        .filter(|layer| !layer.is_none())
        .map(|layer| theme_spec(&layer))
        .collect::<PyResult<Vec<_>>>()?;
    let theme = Theme::resolve(&specs.iter().collect::<Vec<_>>());
    let out = PyDict::new(py);
    for widget in Widget::ALL {
        out.set_item(widget.as_str(), style_dict(py, theme.style(widget))?)?;
    }
    out.set_item("element:selected", style_dict(py, theme.element_selected())?)?;
    out.set_item("element:hover", style_dict(py, theme.element_hover())?)?;
    Ok(out)
}

/// Validates a layout and returns its card size for `rows` records.
#[pyfunction]
#[pyo3(signature = (layout, rows, theme=None, label_max=klemmbrett::DEFAULT_LABEL_MAX))]
fn layout_size(
    layout: Bound<'_, PyAny>,
    rows: usize,
    theme: Option<Bound<'_, PyAny>>,
    label_max: usize,
) -> PyResult<(f32, f32)> {
    let layout = self::layout(&layout)?;
    let spec = optional_spec(theme.as_ref())?;
    let theme = Theme::resolve(&spec.iter().collect::<Vec<_>>());
    Ok(layout.card_size(&theme, rows, label_max))
}

#[pymodule]
fn _core(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<NativeSource>()?;
    module.add_function(wrap_pyfunction!(main, module)?)?;
    module.add_function(wrap_pyfunction!(copy, module)?)?;
    module.add_function(wrap_pyfunction!(launch_command, module)?)?;
    module.add_function(wrap_pyfunction!(activate_window, module)?)?;
    module.add_function(wrap_pyfunction!(set_theme, module)?)?;
    module.add_function(wrap_pyfunction!(show, module)?)?;
    module.add_function(wrap_pyfunction!(quit, module)?)?;
    module.add_function(wrap_pyfunction!(is_running, module)?)?;
    module.add_function(wrap_pyfunction!(default_history_path, module)?)?;
    module.add("DEFAULT_HISTORY_LIMIT", klemmbrett::DEFAULT_HISTORY_LIMIT)?;
    module.add("DEFAULT_LABEL_MAX", klemmbrett::DEFAULT_LABEL_MAX)?;
    module.add_function(wrap_pyfunction!(parse_color_py, module)?)?;
    module.add_function(wrap_pyfunction!(hsl_py, module)?)?;
    module.add_function(wrap_pyfunction!(hwb_py, module)?)?;
    module.add_function(wrap_pyfunction!(cmyk_py, module)?)?;
    module.add_function(wrap_pyfunction!(resolve_theme, module)?)?;
    module.add_function(wrap_pyfunction!(layout_size, module)?)?;
    Ok(())
}
