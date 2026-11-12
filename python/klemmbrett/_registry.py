"""What `config.py` declares, and how the daemon calls back into it."""

from __future__ import annotations

import os
import runpy
import sys
import traceback
from collections.abc import Callable, Iterable, Sequence
from pathlib import Path
from typing import Any

from . import _core, _runtime, providers
from ._record import Record, from_wire, to_record
from .layouts import LayoutContext
from .style import Theme
from .ui import Window

Handler = Callable[[Record], Any]
Layout = Window | Callable[[LayoutContext], Window]

_LAUNCH_MODES = ("auto", "posix", "systemd-run")


class Settings:
    def __init__(self) -> None:
        self.label_max: int = _core.DEFAULT_LABEL_MAX
        self.launch: str | list[str] = "auto"
        self.theme: str | Theme | None = None
        self.overrides: Theme | None = None


class Registry:
    def __init__(self) -> None:
        self.settings = Settings()
        self.widgets: dict[str, Widget] = {}


registry = Registry()


def configure(
    *,
    label_max: int | None = None,
    launch: str | Sequence[str] | None = None,
    theme: str | Theme | None = None,
    overrides: Theme | None = None,
) -> None:
    """Daemon-wide settings. Only the arguments given are changed.

    - `label_max`: characters shown per row before the label is shortened.
    - `launch`: how `kb.launch` starts programs: `"auto"` (a systemd scope when
      running as a unit, else a double fork), `"posix"`, `"systemd-run"`, or a
      wrapper command list such as `["uwsm", "app", "--"]`.
    - `theme`: a palette name, a `Theme`, or `None` for the built-in look.
    - `overrides`: applied over every widget's theme, and kept by `set_theme`.
    """
    settings = registry.settings
    if label_max is not None:
        if isinstance(label_max, bool) or not isinstance(label_max, int) or label_max < 1:
            raise ValueError("label_max must be a positive integer")
        settings.label_max = label_max
    if launch is not None:
        if isinstance(launch, str):
            if launch not in _LAUNCH_MODES:
                raise ValueError(
                    f"launch must be one of {', '.join(_LAUNCH_MODES)}, or a command list"
                )
            settings.launch = launch
        else:
            wrapper = [os.fspath(part) for part in launch]
            if not wrapper or not all(part.strip() for part in wrapper):
                raise ValueError("launch: the wrapper must be a non-empty list of strings")
            settings.launch = wrapper
    if theme is not None:
        _runtime.theme_wire(theme)
        settings.theme = theme
    if overrides is not None:
        if not isinstance(overrides, Theme):
            overrides = Theme(overrides)
        settings.overrides = overrides


class Widget:
    """A popup bound to a provider. Returned by `kb.widget`.

    Calling it with a function sets `on_select`, so it works as a decorator::

        @kb.widget("links", providers.File("links.yaml"))
        def links(record):
            kb.copy(record.value)
    """

    def __init__(
        self,
        name: str,
        provider: Any,
        *,
        layout: Layout | None,
        theme: str | Theme | None,
        location: str | None,
        multi: bool,
        select: str | None,
        deselect: str | None,
        on_select: Handler | None,
        on_highlight: Handler | None,
    ) -> None:
        if not isinstance(name, str) or not name.strip():
            raise ValueError("widget name must be a non-empty string")
        if layout is not None and not (isinstance(layout, Window) or callable(layout)):
            raise TypeError(
                f"widget {name!r}: layout must be a ui.Window or a function returning one"
            )
        if theme is not None:
            _runtime.theme_wire(theme)
        if multi or select or deselect:
            multi_keys: tuple[str, str] | None = (select or "tab", deselect or "shift-tab")
        else:
            multi_keys = None
        self.name = name
        self.provider = providers.as_provider(provider)
        self.layout = layout
        self.theme = theme
        self.location = location
        self.multi = multi_keys
        self._on_select = _handler(on_select, "on_select")
        self._on_highlight = _handler(on_highlight, "on_highlight")

    def __call__(self, function: Handler) -> Widget:
        return self.on_select(function)

    def on_select(self, function: Handler) -> Widget:
        """Decorator: runs for each picked record (each marked one, in order)."""
        self._on_select = _handler(function, "on_select")
        return self

    def on_highlight(self, function: Handler) -> Widget:
        """Decorator: runs when the highlighted row changes."""
        self._on_highlight = _handler(function, "on_highlight")
        return self

    def select(self, record: Record) -> None:
        """What picking `record` does: `on_select`, else the provider's
        `select`, else copying the value."""
        if self._on_select is not None:
            self._on_select(record)
        elif callable(select := getattr(self.provider, "select", None)):
            select(record)
        else:
            _runtime.copy(record.value)

    def highlight(self, record: Record) -> None:
        if self._on_highlight is not None:
            self._on_highlight(record)

    def show(self, records: Iterable[Any] | None = None, *, pointer: tuple[int, int] | None = None) -> None:
        """Opens this widget in the running daemon."""
        _runtime.show(self.name, records, pointer=pointer)

    def __repr__(self) -> str:
        return f"<Widget {self.name!r} provider={self.provider!r}>"

    def _wire(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "source": _source(self.provider),
            "layout": _layout(self.layout),
            "theme": None if self.theme is None else _runtime.theme_wire(self.theme),
            "location": self.location,
            "multi": self.multi,
            "highlight": self._on_highlight is not None,
        }


def _handler(function: Handler | None, what: str) -> Handler | None:
    if function is not None and not callable(function):
        raise TypeError(f"{what} must be callable, got {function!r}")
    return function


def widget(
    name: str,
    provider: Any = None,
    *,
    layout: Layout | None = None,
    theme: str | Theme | None = None,
    location: str | None = None,
    multi: bool = False,
    select: str | None = None,
    deselect: str | None = None,
    on_select: Handler | None = None,
    on_highlight: Handler | None = None,
) -> Widget:
    """Declares a popup widget.

    - `provider`: where rows come from. Without one, every `show` must pass records.
    - `layout`: a `ui.Window`, or a function of a `LayoutContext` returning one.
      Defaults to `layouts.menu`.
    - `theme`: replaces the configured theme for this widget.
    - `location`: where the popup opens, overriding the layout's choice.
    - `multi`: mark rows with Tab / Shift-Tab (or `select` / `deselect` keys).
    - `on_select`, `on_highlight`: functions of a `Record`.
    """
    if name in registry.widgets:
        raise ValueError(f"widget {name!r} is configured more than once")
    created = Widget(
        name,
        provider,
        layout=layout,
        theme=theme,
        location=location,
        multi=multi,
        select=select,
        deselect=deselect,
        on_select=on_select,
        on_highlight=on_highlight,
    )
    registry.widgets[name] = created
    return created


def _source(provider: Any) -> Any:
    if provider is None:
        return None
    if isinstance(provider, providers._Native):
        return provider.native()

    def fetch() -> list[tuple[str, str, str]]:
        return [to_record(record)._wire() for record in provider.records()]

    return fetch


def _layout(layout: Layout | None) -> Any:
    if layout is None or isinstance(layout, Window):
        return None if layout is None else layout._wire()

    def build(widget: str, multi: bool, prompt: bool) -> dict[str, Any]:
        window = layout(LayoutContext(widget=widget, multi=multi, prompt=prompt))
        if not isinstance(window, Window):
            raise TypeError(f"layout {layout!r} returned {window!r}, not a ui.Window")
        return window._wire()

    return build


class Dispatcher:
    """Receives events from the daemon's handler thread, in order."""

    def __init__(self, widgets: dict[str, Widget], hooks: list[Callable[[str], Any]]) -> None:
        self.widgets = widgets
        self.hooks = hooks

    def select(self, name: str, records: list[tuple[str, str, str]]) -> None:
        widget = self.widgets[name]
        for record in records:
            _guard(f"widget {name!r}: on_select", widget.select, from_wire(record))

    def highlight(self, name: str, record: tuple[str, str, str]) -> None:
        widget = self.widgets[name]
        _guard(f"widget {name!r}: on_highlight", widget.highlight, from_wire(record))

    def clipboard(self, text: str) -> None:
        for hook in self.hooks:
            _guard(f"{hook!r}", hook, text)


def _guard(what: str, function: Callable[..., Any], *args: Any) -> None:
    try:
        function(*args)
    except Exception:
        print(f"klemmbrett: {what} failed:", file=sys.stderr)
        traceback.print_exc()


def _clipboard_hooks(widgets: Iterable[Widget]) -> list[Callable[[str], Any]]:
    hooks = []
    seen: set[int] = set()
    for widget in widgets:
        provider = widget.provider
        hook = getattr(provider, "on_clipboard", None)
        if callable(hook) and id(provider) not in seen:
            seen.add(id(provider))
            hooks.append(hook)
    return hooks


def reset() -> None:
    global registry
    registry = Registry()
    providers._config_dir = None


def run_config(path: str | os.PathLike[str]) -> None:
    """Runs a configuration file into a fresh registry."""
    path = Path(path).expanduser().resolve()
    reset()
    providers._config_dir = path.parent
    sys.path.insert(0, str(path.parent))
    try:
        runpy.run_path(str(path), run_name="__klemmbrett_config__")
    finally:
        if sys.path and sys.path[0] == str(path.parent):
            sys.path.pop(0)


def wire() -> dict[str, Any]:
    """The registry as the daemon reads it."""
    settings = registry.settings
    widgets = dict(registry.widgets)
    hooks = _clipboard_hooks(widgets.values())
    return {
        "label_max": settings.label_max,
        "launch": settings.launch,
        "theme": _runtime.theme_wire(settings.theme),
        "overrides": None if settings.overrides is None else settings.overrides._wire(),
        "widgets": [widget._wire() for widget in widgets.values()],
        "dispatcher": Dispatcher(widgets, hooks),
        "clipboard": bool(hooks),
    }


def load(path: str | None) -> dict[str, Any]:
    """Called by the daemon with the configuration path, or `None` for none."""
    if path is None:
        reset()
    else:
        run_config(path)
    return wire()
