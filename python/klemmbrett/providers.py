"""Record providers.

A provider is anything with a `records()` method, or a plain function
returning records. Records may be `Record` objects, strings (key, label, and
value all the same), or dicts with a `value`, optional `key` and `label`,
and `data` (or any other keys, which become the data).

A provider may also define:

- `select(record)`: what a widget does with a picked record when it has no
  `on_select` of its own. Without it, the value is copied.
- `on_clipboard(text)`: called with every new clipboard text.
- `default_match` and `default_rank`: the `ranking` matcher and rankers a
  widget showing it uses unless it sets `match=` or `rank=`.

Call `changed()` on a provider when its records change: every open popup
showing it, directly or through a `View` or `Merge`, reloads, and
`kb watch` subscribers hear about it. A `Store` does this itself.
"""

from __future__ import annotations

import os
import sys
import threading
import traceback
from collections.abc import Callable, Iterable, Iterator, Mapping
from dataclasses import replace
from pathlib import Path
from typing import Any

from . import _core, _runtime
from . import ranking as rk
from ._record import Record, RecordLike, to_record, with_schema

__all__ = [
    "Provider",
    "ClipboardHistory",
    "File",
    "Executables",
    "WaylandWindows",
    "Static",
    "Store",
    "View",
    "Merge",
    "Themes",
    "Widgets",
    "provider",
]

# Set by the registry: called with a provider whose records changed.
_on_changed: Callable[[Provider], None] | None = None


class Provider:
    """Base class for providers. Override `records()`; `select` and
    `on_clipboard` are optional, and `default_match` and `default_rank` set
    how widgets showing it match and rank by default."""

    default_match: rk.Matcher | None = None
    default_rank: rk.Rank | None = None

    def records(self) -> Iterable[RecordLike]:
        raise NotImplementedError

    def changed(self) -> None:
        """Reloads open popups that show this provider's records."""
        if _on_changed is not None:
            _on_changed(self)

    def view(
        self,
        *,
        filter: Callable[[Record], bool] | None = None,  # noqa: A002
        sort: Callable[[Record], Any] | None = None,
        reverse: bool = False,
        map: Callable[[Record], RecordLike] | None = None,  # noqa: A002
        limit: int | None = None,
    ) -> View:
        """This provider's records, filtered, sorted, mapped, and cut to `limit`."""
        return View(self, filter=filter, sort=sort, reverse=reverse, map=map, limit=limit)

    def _sources(self) -> tuple[Provider, ...]:
        """Providers whose records this one is made of."""
        return ()

    def __repr__(self) -> str:
        return f"{type(self).__name__}()"


class _Native(Provider):
    """A provider whose records come from the daemon itself. With a
    `schema`, they pass through Python to be checked."""

    schema: Any = None

    def __init__(self) -> None:
        self._source: _core.NativeSource | None = None

    def _open(self) -> _core.NativeSource:
        raise NotImplementedError

    def native(self) -> _core.NativeSource:
        if self._source is None:
            self._source = self._open()
        return self._source

    def records(self) -> list[Record]:
        records = [
            Record(value=value, label=label, key=key, data=data)
            for key, label, value, data in self.native().records()
        ]
        if self.schema is None:
            return records
        return [with_schema(record, self.schema, repr(self)) for record in records]


class ClipboardHistory(_Native):
    """Recent clipboard texts, newest first, stored in SQLite.

    Every widget using the same instance shares one database connection, and
    each new clipboard text is recorded once.
    """

    default_match = rk.fuzzy(scheme="history")

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
    A relative path is relative to the configuration file. With a `schema`
    (a dataclass or NamedTuple), every record's data must match it.
    """

    default_match = rk.fuzzy()

    def __init__(
        self,
        path: str | os.PathLike[str],
        format: str | None = None,  # noqa: A002
        *,
        schema: Any = None,
    ) -> None:
        super().__init__()
        self.path = _resolve(path)
        self.format = format
        self.schema = schema

    def _open(self) -> _core.NativeSource:
        return _core.NativeSource.file(self.path, self.format)

    def __repr__(self) -> str:
        return f"File({str(self.path)!r})"


class Executables(_Native):
    """Programs on `PATH`; the first of each basename wins. Selecting launches it."""

    default_match = rk.fuzzy()
    default_rank = rk.frecency() + rk.learned()

    def _open(self) -> _core.NativeSource:
        return _core.NativeSource.executables()

    def select(self, record: Record) -> None:
        _runtime.launch([record.value])


class WaylandWindows(_Native):
    """Open toplevel windows. Selecting focuses the window."""

    default_match = rk.fuzzy()
    default_rank = rk.recent()

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


class Store(Provider):
    """Records kept in memory and changed from handlers, threads, or timers.

    Every change reloads open popups showing the store. Records are kept in
    order; `add` replaces a record with the same key where it is, or puts a
    new one last (`first=True`: first). Past `limit`, records fall off the
    other end. With a `schema`, every record's data must match it.
    """

    def __init__(
        self,
        records: Iterable[RecordLike] = (),
        *,
        schema: Any = None,
        limit: int | None = None,
    ) -> None:
        if limit is not None and limit < 1:
            raise ValueError("Store: limit must be at least 1")
        self.schema = schema
        self.limit = limit
        self._lock = threading.RLock()
        self._records: dict[str, Record] = {}
        self._subscribers: list[Callable[[Store], Any]] = []
        for record in records:
            self._put(self._prepare(record), first=False)

    def _prepare(self, item: RecordLike) -> Record:
        record = to_record(item)
        if self.schema is not None:
            record = with_schema(record, self.schema, type(self).__name__)
        return record

    def _put(self, record: Record, *, first: bool) -> None:
        if first:
            rest = {key: entry for key, entry in self._records.items() if key != record.key}
            self._records = {record.key: record, **rest}
        else:
            self._records[record.key] = record
        if self.limit is not None:
            while len(self._records) > self.limit:
                drop = next(reversed(self._records)) if first else next(iter(self._records))
                del self._records[drop]

    def add(self, item: RecordLike, *, first: bool = False) -> Record:
        record = self._prepare(item)
        with self._lock:
            self._put(record, first=first)
        self._notify()
        return record

    def extend(self, items: Iterable[RecordLike], *, first: bool = False) -> None:
        """Adds several records with one change notification."""
        records = [self._prepare(item) for item in items]
        with self._lock:
            for record in reversed(records) if first else records:
                self._put(record, first=first)
        self._notify()

    def update(self, key: str, **changes: Any) -> Record:
        """Replaces `label`, `value`, or `data` of the record with `key`."""
        unknown = set(changes) - {"label", "value", "data"}
        if unknown:
            raise TypeError(f"Store.update: unknown fields {sorted(unknown)}; use label, value, or data")
        with self._lock:
            if key not in self._records:
                raise KeyError(key)
            record = replace(self._records[key], **changes)
            if self.schema is not None and "data" in changes:
                record = with_schema(record, self.schema, type(self).__name__)
            self._records[key] = record
        self._notify()
        return record

    def remove(self, key: str) -> bool:
        with self._lock:
            removed = self._records.pop(key, None) is not None
        if removed:
            self._notify()
        return removed

    def clear(self) -> None:
        with self._lock:
            self._records = {}
        self._notify()

    def get(self, key: str, default: Record | None = None) -> Record | None:
        with self._lock:
            return self._records.get(key, default)

    def records(self) -> list[Record]:
        with self._lock:
            return list(self._records.values())

    def subscribe(self, callback: Callable[[Store], Any]) -> Callable[[], None]:
        """Calls `callback(store)` after every change. Returns an unsubscribe function."""
        with self._lock:
            self._subscribers.append(callback)

        def unsubscribe() -> None:
            with self._lock:
                if callback in self._subscribers:
                    self._subscribers.remove(callback)

        return unsubscribe

    def _notify(self) -> None:
        self.changed()
        with self._lock:
            subscribers = list(self._subscribers)
        for callback in subscribers:
            try:
                callback(self)
            except Exception:
                print(f"klemmbrett: {type(self).__name__} subscriber {callback!r} failed:", file=sys.stderr)
                traceback.print_exc()

    def __len__(self) -> int:
        with self._lock:
            return len(self._records)

    def __iter__(self) -> Iterator[Record]:
        return iter(self.records())

    def __contains__(self, key: object) -> bool:
        with self._lock:
            return key in self._records

    def __repr__(self) -> str:
        return f"{type(self).__name__}({len(self)} records)"


class View(Provider):
    """Another provider's records, filtered, sorted, mapped, and cut to
    `limit`, in that order. Picking a row hands it to the source's `select`."""

    def __init__(
        self,
        source: Any,
        *,
        filter: Callable[[Record], bool] | None = None,  # noqa: A002
        sort: Callable[[Record], Any] | None = None,
        reverse: bool = False,
        map: Callable[[Record], RecordLike] | None = None,  # noqa: A002
        limit: int | None = None,
    ) -> None:
        provider = as_provider(source)
        if provider is None:
            raise TypeError("View: the source must be a provider")
        if limit is not None and limit < 0:
            raise ValueError("View: limit must not be negative")
        self.source = provider
        self.filter = filter
        self.sort = sort
        self.reverse = reverse
        self.map = map
        self.limit = limit

    def records(self) -> list[Record]:
        records = [to_record(record) for record in self.source.records()]
        if self.filter is not None:
            records = [record for record in records if self.filter(record)]
        if self.sort is not None:
            records.sort(key=self.sort, reverse=self.reverse)
        elif self.reverse:
            records.reverse()
        if self.map is not None:
            records = [to_record(self.map(record)) for record in records]
        if self.limit is not None:
            records = records[: self.limit]
        return records

    def select(self, record: Record) -> None:
        _select_with(self.source, record)

    @property
    def default_match(self) -> rk.Matcher | None:  # type: ignore[override]
        return defaults(self.source)[0]

    @property
    def default_rank(self) -> rk.Rank | None:  # type: ignore[override]
        return defaults(self.source)[1]

    def _sources(self) -> tuple[Provider, ...]:
        return (self.source,)

    def __repr__(self) -> str:
        return f"View({self.source!r})"


class Merge(Provider):
    """The records of several providers, one after another. Keys must not
    repeat across them unless `prefix_keys` makes them `"0:key"`, `"1:key"`,
    and so on. Picking a row hands the original record to its provider."""

    def __init__(self, *sources: Any, prefix_keys: bool = False) -> None:
        providers = [as_provider(source) for source in sources]
        if not providers or any(provider is None for provider in providers):
            raise TypeError("Merge: pass one or more providers")
        self.sources: tuple[Provider, ...] = tuple(providers)  # type: ignore[arg-type]
        self.prefix_keys = prefix_keys
        self._origin: dict[str, tuple[Provider, Record]] = {}

    def records(self) -> list[Record]:
        origin: dict[str, tuple[Provider, Record]] = {}
        records = []
        for index, source in enumerate(self.sources):
            for item in source.records():
                original = to_record(item)
                record = replace(original, key=f"{index}:{original.key}") if self.prefix_keys else original
                if record.key in origin:
                    raise ValueError(
                        f"Merge: key {record.key!r} comes from more than one provider; "
                        "pass prefix_keys=True"
                    )
                origin[record.key] = (source, original)
                records.append(record)
        self._origin = origin
        return records

    def select(self, record: Record) -> None:
        source, original = self._origin.get(record.key, (None, record))
        _select_with(source, original)

    @property
    def default_match(self) -> rk.Matcher | None:  # type: ignore[override]
        return _agreed(defaults(source)[0] for source in self.sources)

    @property
    def default_rank(self) -> rk.Rank | None:  # type: ignore[override]
        return _agreed(defaults(source)[1] for source in self.sources)

    def _sources(self) -> tuple[Provider, ...]:
        return self.sources

    def __repr__(self) -> str:
        return f"Merge({', '.join(map(repr, self.sources))})"


def defaults(provider: Any) -> tuple[rk.Matcher | None, rk.Rank | None]:
    """The matcher and rankers `provider` suggests, `None` where it has no opinion."""
    if provider is None:
        return None, None
    return getattr(provider, "default_match", None), getattr(provider, "default_rank", None)


def _agreed(values: Iterable[Any]) -> Any:
    """The value when all are equal, else `None`."""
    values = list(values)
    if values and all(value == values[0] for value in values):
        return values[0]
    return None


def _select_with(source: Any, record: Record) -> None:
    select = getattr(source, "select", None)
    if callable(select):
        select(record)
    else:
        _runtime.copy(record.value)


class Widgets(Provider):
    """Every configured widget that can be opened. Selecting shows it.

    Agent widgets are left out: they open when an asker has a question, and
    picking one from a list would have nothing to show. Use it with a filter
    to jump to any other widget::

        kb.widget("widgets", providers.Widgets(), layout=layouts.filter)
    """

    default_match = rk.fuzzy()

    def records(self) -> list[Record]:
        from ._registry import registry

        return [
            Record(widget.name)
            for widget in registry.widgets.values()
            if widget.agent is None
        ]

    def select(self, record: Record) -> None:
        _runtime.show(record.value)


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
    def __init__(self, fetch: Callable[[], Iterable[RecordLike]], schema: Any = None) -> None:
        self._fetch = fetch
        self.schema = schema
        self.__name__ = getattr(fetch, "__name__", type(self).__name__)
        self.__doc__ = getattr(fetch, "__doc__", None)
        self.__wrapped__ = fetch

    def records(self) -> Iterable[RecordLike]:
        if self.schema is None:
            return self._fetch()
        return [with_schema(to_record(record), self.schema, repr(self)) for record in self._fetch()]

    def __call__(self) -> Iterable[RecordLike]:
        return self._fetch()

    def __repr__(self) -> str:
        return f"<provider {self.__name__}>"


def provider(
    fetch: Callable[[], Iterable[RecordLike]] | None = None, *, schema: Any = None
) -> Any:
    """Turns a function returning records into a provider::

        @kb.provider
        def bookmarks():
            return [kb.Record(url, label=title) for title, url in load()]

    With `@kb.provider(schema=Bookmark)`, every record's data must match the
    dataclass or NamedTuple `Bookmark`, and dicts become `Bookmark`s.
    """
    if fetch is None:
        return lambda function: provider(function, schema=schema)
    if isinstance(fetch, Provider):
        return fetch
    if not callable(fetch):
        raise TypeError(f"provider: expected a function, got {fetch!r}")
    return _Function(fetch, schema)


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
