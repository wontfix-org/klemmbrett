"""What `config.py` declares, and how the daemon calls back into it."""

from __future__ import annotations

import os
import runpy
import sys
import traceback
from collections.abc import Callable, Iterable, Sequence
from pathlib import Path
from typing import Any

from . import _core, _runtime, notifications, providers, secrets
from . import ranking as rk
from ._record import Record, from_wire, to_record
from . import layouts
from .layouts import LayoutContext
from .style import Theme
from .ui import Window

Handler = Callable[[Record], Any]
Layout = Window | Callable[[LayoutContext], Window]

_LAUNCH_MODES = ("auto", "posix", "systemd-run")

#: `match=` and `rank=` left out: the provider's defaults apply.
_PROVIDER_DEFAULT: Any = object()


class Settings:
    def __init__(self) -> None:
        self.label_max: int = _core.DEFAULT_LABEL_MAX
        self.launch: str | list[str] = "auto"
        self.theme: str | Theme | None = None
        self.overrides: Theme | None = None
        self.session: bool | None = None
        self.outputs: bool | None = None
        self.terminal: list[str] | None = None


class Registry:
    def __init__(self) -> None:
        self.settings = Settings()
        self.widgets: dict[str, Widget] = {}
        self.picker: Picker | None = None


registry = Registry()


def configure(
    *,
    label_max: int | None = None,
    launch: str | Sequence[str] | None = None,
    theme: str | Theme | None = None,
    overrides: Theme | None = None,
    session: bool | None = None,
    outputs: bool | None = None,
    terminal: Sequence[str] | None = None,
) -> None:
    """Daemon-wide settings. Only the arguments given are changed.

    - `label_max`: characters shown per row before the label is shortened.
    - `launch`: how `kb.launch` starts programs: `"auto"` (a systemd scope when
      running as a unit, else a double fork), `"posix"`, `"systemd-run"`, or a
      wrapper command list such as `["uwsm", "app", "--"]`.
    - `theme`: a palette name, a `Theme`, or `None` for the built-in look.
    - `overrides`: applied over every widget's theme, and kept by `set_theme`.
    - `session`: subscribe to the logind session. Left unset, klemmbrett probes
      and logs a warning when the session is missing. `True` aborts startup
      instead. `False` does not subscribe.
    - `outputs`: subscribe to `zxdg_output_v1` and keep the output layout.
      Left unset, a missing protocol logs a warning and placement reads the
      layout once per popup. `True` aborts startup instead. `False` does not
      subscribe.
    - `terminal`: command placed in front of `Exec` for desktop entries with
      `Terminal=true`, such as `["foot", "-e"]`. Left unset, those entries
      are not listed.
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
    if session is not None:
        if not isinstance(session, bool):
            raise TypeError(f"session must be true or false, got {session!r}")
        settings.session = session
    if outputs is not None:
        if not isinstance(outputs, bool):
            raise TypeError(f"outputs must be true or false, got {outputs!r}")
        settings.outputs = outputs
    if terminal is not None:
        if isinstance(terminal, str) or not isinstance(terminal, Sequence):
            raise TypeError("terminal must be a list of strings")
        command = [os.fspath(part) for part in terminal]
        if not command or not all(part.strip() for part in command):
            raise ValueError("terminal must be a non-empty list of strings")
        settings.terminal = command


class Picker:
    """The popup `kb` opens when it has no subcommand. Not a named widget."""

    def __init__(
        self,
        *,
        layout: Layout | None = None,
        theme: str | Theme | None = None,
        location: str | None = None,
        multi: bool = False,
        select: str | None = None,
        deselect: str | None = None,
        search: Sequence[str] | None = None,
        match: rk.Matcher | None = None,
        rank: rk.Rank | None = None,
        prefer: str = "blend",
        prompt: str | None = None,
    ) -> None:
        if layout is None:
            layout = layouts.filter
        if not (isinstance(layout, Window) or callable(layout)):
            raise TypeError(
                f"picker: layout must be a ui.Window or a function returning one, got {layout!r}"
            )
        if isinstance(search, str):
            search = [search]
        if search is not None and not all(isinstance(path, str) and path for path in search):
            raise TypeError("picker: search must be a list of paths such as 'sender.name'")
        if theme is not None:
            _runtime.theme_wire(theme)
        if multi or select or deselect:
            multi_keys: tuple[str, str] | None = (select or "tab", deselect or "shift-tab")
        else:
            multi_keys = None
        if match is None:
            match = rk.fuzzy()
        if not isinstance(match, rk.Matcher):
            raise TypeError(f"picker: match must be a matcher such as rk.fuzzy(), got {match!r}")
        if rank is not None and not isinstance(rank, rk.Rank):
            raise TypeError(f"picker: rank must be rankers such as rk.frecency(), got {rank!r}")
        if prefer not in rk.PREFER:
            raise ValueError(f"picker: prefer must be one of {', '.join(rk.PREFER)}")
        if prompt is not None and not isinstance(prompt, str):
            raise TypeError(f"picker: prompt must be a string, got {prompt!r}")
        self.layout = layout
        self.theme = theme
        self.location = location
        self.multi = multi_keys
        self.search = list(search or ())
        self.match = match
        self.rank = rank
        self.prefer = prefer
        #: `None` leaves a layout's own prompt text; a blank one becomes `"> "`.
        self.prompt = prompt

    def _wire(self) -> dict[str, Any]:
        return {
            "layout": _layout(self.layout),
            "theme": None if self.theme is None else _runtime.theme_wire(self.theme),
            "location": self.location,
            "multi": self.multi,
            "search": self.search,
            "ranking": rk.wire(self.match, self.rank, self.prefer),
            "prompt": self.prompt,
        }


def picker(
    *,
    layout: Layout | None = None,
    theme: str | Theme | None = None,
    location: str | None = None,
    multi: bool = False,
    select: str | None = None,
    deselect: str | None = None,
    search: Sequence[str] | None = None,
    match: rk.Matcher | None = None,
    rank: rk.Rank | None = None,
    prefer: str = "blend",
    prompt: str | None = None,
) -> None:
    """Configures the picker `kb` opens when it has no subcommand.

    There is no widget name and no provider: the choices come from the
    command, and the picked values are printed. `layout` defaults to
    `layouts.filter` and `match` to `ranking.fuzzy()`. `prompt` is the
    label in front of the filter; left out, a blank prompt becomes `"> "`.
    `multi`, `select`, `deselect`, `search`, `rank`, `prefer`, `theme`, and
    `location` mean the same as on `kb.widget`. Calling this twice is an
    error. Without it, those defaults are used.
    """
    if registry.picker is not None:
        raise ValueError("the picker is configured more than once")
    registry.picker = Picker(
        layout=layout,
        theme=theme,
        location=location,
        multi=multi,
        select=select,
        deselect=deselect,
        search=search,
        match=match,
        rank=rank,
        prefer=prefer,
        prompt=prompt,
    )


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
        search: Sequence[str] | None = None,
        match: rk.Matcher | None = _PROVIDER_DEFAULT,
        rank: rk.Rank | None = _PROVIDER_DEFAULT,
        prefer: str = "blend",
    ) -> None:
        if not isinstance(name, str) or not name.strip():
            raise ValueError("widget name must be a non-empty string")
        self.agent: secrets.Agent | None = None
        if isinstance(provider, secrets.Agent):
            unsupported = [
                what
                for what, given in (
                    ("on_select", on_select is not None),
                    ("on_highlight", on_highlight is not None),
                    ("multi", bool(multi or select or deselect)),
                    ("search", bool(search)),
                    ("match", match is not _PROVIDER_DEFAULT),
                    ("rank", rank is not _PROVIDER_DEFAULT),
                )
                if given
            ]
            if unsupported:
                raise TypeError(
                    f"widget {name!r}: {', '.join(unsupported)} cannot be used with "
                    f"{provider!r}; answers go to the asker only"
                )
            self.agent, provider, rank = provider, None, None
        if isinstance(search, str):
            search = [search]
        if search is not None and not all(isinstance(path, str) and path for path in search):
            raise TypeError(f"widget {name!r}: search must be a list of paths such as 'sender.name'")
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
        suggested_match, suggested_rank = providers.defaults(self.provider)
        if match is _PROVIDER_DEFAULT:
            match = suggested_match
        if rank is _PROVIDER_DEFAULT:
            rank = suggested_rank
        if match is None:
            match = rk.substring()
        if not isinstance(match, rk.Matcher):
            raise TypeError(f"widget {name!r}: match must be a matcher such as rk.fuzzy(), got {match!r}")
        if rank is not None and not isinstance(rank, rk.Rank):
            raise TypeError(f"widget {name!r}: rank must be rankers such as rk.frecency(), got {rank!r}")
        if prefer not in rk.PREFER:
            raise ValueError(f"widget {name!r}: prefer must be one of {', '.join(rk.PREFER)}")
        self.match = match
        self.rank = rank
        self.prefer = prefer
        self.layout = layout
        self.theme = theme
        self.location = location
        self.multi = multi_keys
        self.search = list(search or ())
        self._on_select = _handler(on_select, "on_select")
        self._on_highlight = _handler(on_highlight, "on_highlight")
        self._shown: dict[str, Record] = {}

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
        if records is not None:
            records = [to_record(record) for record in records]
            self._remember(records)
        _runtime.show(self.name, records, pointer=pointer)

    def changed(self) -> None:
        """Reloads this widget if it is open."""
        _runtime.refresh([self.name])

    def _remember(self, records: list[Record]) -> None:
        self._shown = {record.key: record for record in records}

    def _original(self, wire: tuple[str, str, str, Any]) -> Record:
        """The record as Python gave it, data and all, if it is still known."""
        record = from_wire(wire)
        known = self._shown.get(record.key)
        if known is not None and known.value == record.value and known.label == record.label:
            return known
        return record

    def __repr__(self) -> str:
        return f"<Widget {self.name!r} provider={self.provider!r}>"

    def _wire(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "source": _source(self.provider, self),
            "layout": _layout(self.layout),
            "theme": None if self.theme is None else _runtime.theme_wire(self.theme),
            "location": self.location,
            "multi": self.multi,
            "highlight": self._on_highlight is not None,
            "search": self.search,
            "ranking": rk.wire(self.match, self.rank, self.prefer),
            "agent": None if self.agent is None else self.agent._wire(),
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
    search: Sequence[str] | None = None,
    match: rk.Matcher | None = _PROVIDER_DEFAULT,
    rank: rk.Rank | None = _PROVIDER_DEFAULT,
    prefer: str = "blend",
) -> Widget:
    """Declares a popup widget.

    - `provider`: where rows come from. Without one, every `show` must pass records.
    - `layout`: a `ui.Window`, or a function of a `LayoutContext` returning one.
      Defaults to `layouts.menu`.
    - `theme`: replaces the configured theme for this widget.
    - `location`: where the popup opens, overriding the layout's choice.
    - `multi`: mark rows with Tab / Shift-Tab (or `select` / `deselect` keys).
    - `on_select`, `on_highlight`: functions of a `Record`.
    - `search`: what typing filters on, as paths such as `"label"` or
      `"sender.name"`. Defaults to the label.
    - `match`: how typing matches rows, such as `ranking.fuzzy()`. `None` is
      `ranking.substring()`. Left out, the provider's default applies.
    - `rank`: how past picks order rows, such as `ranking.frecency()`. `None`
      keeps the provider's order. Left out, the provider's default applies.
    - `prefer`: `"blend"`, `"match"`, or `"usage"`; see docs/ranking.md.
    """
    if name in registry.widgets:
        raise ValueError(f"widget {name!r} is configured more than once")
    if isinstance(provider, secrets.Agent) and not provider.shared:
        for other in registry.widgets.values():
            if other.agent is not None and other.agent.kind == provider.kind:
                raise ValueError(
                    f"widget {name!r}: widget {other.name!r} already answers "
                    f"{provider.kind} questions; use route= to send some elsewhere"
                )
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
        search=search,
        match=match,
        rank=rank,
        prefer=prefer,
    )
    registry.widgets[name] = created
    return created


def _source(provider: Any, widget: Widget) -> Any:
    if provider is None:
        return None
    rank = widget.rank if widget.rank is not None and widget.rank.uses_python() else None
    if rank is None and isinstance(provider, providers._Native) and provider.schema is None:
        return provider.native()

    def fetch() -> Any:
        records = [to_record(record) for record in provider.records()]
        widget._remember(records)
        wires = [record._wire() for record in records]
        if rank is None:
            return wires
        return wires, rank.scores(records)

    return fetch


def _tree(provider: Any) -> list[Any]:
    """`provider` and every provider it is made of."""
    found: list[Any] = []
    pending = [provider]
    while pending:
        current = pending.pop()
        if current is None or any(current is seen for seen in found):
            continue
        found.append(current)
        sources = getattr(current, "_sources", None)
        if callable(sources):
            pending.extend(sources())
    return found


def _changed_hook(widgets: dict[str, Widget]) -> Callable[[Any], None]:
    def changed(provider: Any) -> None:
        names = [
            name
            for name, widget in widgets.items()
            if any(part is provider for part in _tree(widget.provider))
        ]
        if names:
            _runtime.refresh(names)

    return changed


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

    def select(self, name: str, records: list[tuple[str, str, str, Any]]) -> None:
        widget = self.widgets[name]
        for record in records:
            _guard(f"widget {name!r}: on_select", widget.select, widget._original(record))

    def highlight(self, name: str, record: tuple[str, str, str, Any]) -> None:
        widget = self.widgets[name]
        _guard(f"widget {name!r}: on_highlight", widget.highlight, widget._original(record))

    def clipboard(self, text: str) -> None:
        for hook in self.hooks:
            _guard(f"{hook!r}", hook, text)

    def notification(self, wire: dict[str, Any]) -> None:
        _guard("notification routing", notifications._dispatch, wire)

    def route_secret(self, name: str, wire: dict[str, Any]) -> str | None:
        """Where the agent widget `name` sends a question; `None` keeps it."""
        agent = self.widgets[name].agent
        if agent is None:
            return None
        try:
            return agent._route(secrets.Question._from_wire(wire))
        except Exception:
            print(f"klemmbrett: widget {name!r}: route failed:", file=sys.stderr)
            traceback.print_exc()
            return None

    def call(self, method: str, parameters: dict[str, Any]) -> dict[str, Any]:
        try:
            return notifications._call(method, parameters or {})
        except Exception as err:
            traceback.print_exc()
            return {"error": "Failed", "parameters": {"message": f"{type(err).__name__}: {err}"}}


def _guard(what: str, function: Callable[..., Any], *args: Any) -> None:
    try:
        function(*args)
    except Exception:
        print(f"klemmbrett: {what} failed:", file=sys.stderr)
        traceback.print_exc()


def _clipboard_hooks(widgets: Iterable[Widget]) -> list[Callable[[str], Any]]:
    hooks = []
    seen: list[Any] = []
    for widget in widgets:
        for provider in _tree(widget.provider):
            hook = getattr(provider, "on_clipboard", None)
            if callable(hook) and not any(provider is other for other in seen):
                seen.append(provider)
                hooks.append(hook)
    return hooks


def reset() -> None:
    global registry
    registry = Registry()
    providers._config_dir = None
    providers._on_changed = None
    notifications._reset()


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
    providers._on_changed = _changed_hook(widgets)
    return {
        "label_max": settings.label_max,
        "launch": settings.launch,
        "theme": _runtime.theme_wire(settings.theme),
        "overrides": None if settings.overrides is None else settings.overrides._wire(),
        "widgets": [widget._wire() for widget in widgets.values()],
        "dispatcher": Dispatcher(widgets, hooks),
        "clipboard": bool(hooks),
        "session": settings.session,
        "outputs": settings.outputs,
        "notifications": notifications._wire(),
        "route_notifications": notifications._routed(),
        "picker": (registry.picker or Picker())._wire(),
    }


def load(path: str | None) -> dict[str, Any]:
    """Called by the daemon with the configuration path, or `None` for none."""
    if path is None:
        reset()
    else:
        run_config(path)
    return wire()
