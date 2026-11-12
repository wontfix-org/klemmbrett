"""Record providers.

A provider is anything with a `records()` method, or a plain function
returning records. Records may be `Record` objects, strings (key, label, and
value all the same), or dicts with a `value` and optional `key` and `label`.

A provider may also define:

- `select(record)`: what a widget does with a picked record when it has no
  `on_select` of its own. Without it, the value is copied.
- `on_clipboard(text)`: called with every new clipboard text.
"""

from __future__ import annotations

import os
from collections.abc import Callable, Iterable, Mapping
from pathlib import Path
from typing import Any

from . import _core, _runtime
from ._record import Record, RecordLike, to_record

__all__ = [
    "Provider",
    "ClipboardHistory",
    "File",
    "Executables",
    "WaylandWindows",
    "Static",
    "Themes",
    "provider",
]


class Provider:
    """Base class for providers. Override `records()`; `select` and
    `on_clipboard` are optional."""

    def records(self) -> Iterable[RecordLike]:
        raise NotImplementedError

    def __repr__(self) -> str:
        return f"{type(self).__name__}()"


class _Native(Provider):
    """A provider whose records come from the daemon itself."""

    def __init__(self) -> None:
        self._source: _core.NativeSource | None = None

    def _open(self) -> _core.NativeSource:
        raise NotImplementedError

    def native(self) -> _core.NativeSource:
        if self._source is None:
            self._source = self._open()
        return self._source

    def records(self) -> list[Record]:
        return [Record(value=value, label=label, key=key) for key, label, value in self.native().records()]


class ClipboardHistory(_Native):
    """Recent clipboard texts, newest first, stored in SQLite.

    Every widget using the same instance shares one database connection, and
    each new clipboard text is recorded once.
    """

    def __init__(
        self,
        limit: int = _core.DEFAULT_HISTORY_LIMIT,
        *,
        path: str | os.PathLike[str] | None = None,
        coalesce_growing: bool = True,
        skip_whitespace_only: bool = True,
    ) -> None:
        super().__init__()
        if limit < 1:
            raise ValueError("ClipboardHistory: limit must be at least 1")
        self.limit = limit
        self.path = _resolve(path) if path is not None else Path(_core.default_history_path())
        self.coalesce_growing = coalesce_growing
        self.skip_whitespace_only = skip_whitespace_only

    def _open(self) -> _core.NativeSource:
        return _core.NativeSource.clipboard_history(
            self.path, self.limit, self.coalesce_growing, self.skip_whitespace_only
        )

    def __repr__(self) -> str:
        return f"ClipboardHistory(limit={self.limit}, path={str(self.path)!r})"


class File(_Native):
    """Records from a file, read again each time the widget opens.

    `format` is `text` (one value per non-empty line), `json`, or `yaml`.
    By default it follows the extension: `.json`, `.yaml`/`.yml`, else text.
    A relative path is relative to the configuration file.
    """

    def __init__(self, path: str | os.PathLike[str], format: str | None = None) -> None:  # noqa: A002
        super().__init__()
        self.path = _resolve(path)
        self.format = format

    def _open(self) -> _core.NativeSource:
        return _core.NativeSource.file(self.path, self.format)

    def __repr__(self) -> str:
        return f"File({str(self.path)!r})"


class Executables(_Native):
    """Programs on `PATH`; the first of each basename wins. Selecting launches it."""

    def _open(self) -> _core.NativeSource:
        return _core.NativeSource.executables()

    def select(self, record: Record) -> None:
        _runtime.launch([record.value])


class WaylandWindows(_Native):
    """Open toplevel windows. Selecting focuses the window."""

    def _open(self) -> _core.NativeSource:
        return _core.NativeSource.wayland_windows()

    def select(self, record: Record) -> None:
        _runtime.activate_window(record.key)


class Static(Provider):
    """A fixed list of records."""

    def __init__(self, records: Iterable[RecordLike]) -> None:
        self._records = [to_record(record) for record in records]

    def records(self) -> list[Record]:
        return list(self._records)

    def __repr__(self) -> str:
        return f"Static({self._records!r})"


class Themes(Provider):
    """`default` and every shipped palette. Selecting switches the theme.

    Attach `kb.set_theme` as `on_highlight` to preview while moving::

        kb.widget("themes", providers.Themes(), layout=layouts.filter,
                  on_highlight=lambda record: kb.set_theme(record.value))
    """

    def records(self) -> list[Record]:
        from . import themes

        return [Record("default"), *(Record(name) for name in themes.names())]

    def select(self, record: Record) -> None:
        _runtime.set_theme(record.value)


class _Function(Provider):
    def __init__(self, fetch: Callable[[], Iterable[RecordLike]]) -> None:
        self._fetch = fetch
        self.__name__ = getattr(fetch, "__name__", type(self).__name__)
        self.__doc__ = getattr(fetch, "__doc__", None)
        self.__wrapped__ = fetch

    def records(self) -> Iterable[RecordLike]:
        return self._fetch()

    def __call__(self) -> Iterable[RecordLike]:
        return self._fetch()

    def __repr__(self) -> str:
        return f"<provider {self.__name__}>"


def provider(fetch: Callable[[], Iterable[RecordLike]]) -> Provider:
    """Turns a function returning records into a provider::

        @kb.provider
        def bookmarks():
            return [kb.Record(url, label=title) for title, url in load()]
    """
    if isinstance(fetch, Provider):
        return fetch
    if not callable(fetch):
        raise TypeError(f"provider: expected a function, got {fetch!r}")
    return _Function(fetch)


def as_provider(value: Any) -> Provider | None:
    """Accepts a `Provider`, an object with `records()`, or a function."""
    if value is None or isinstance(value, Provider):
        return value
    if callable(getattr(value, "records", None)):
        return value
    if callable(value):
        return _Function(value)
    if isinstance(value, Iterable) and not isinstance(value, str | bytes | Mapping):
        return Static(value)
    raise TypeError(
        f"expected a provider, a function returning records, or a list of records, got {value!r}"
    )


_config_dir: Path | None = None


def _resolve(path: str | os.PathLike[str]) -> Path:
    expanded = Path(path).expanduser()
    if expanded.is_absolute():
        return expanded
    return (_config_dir or Path.cwd()) / expanded
