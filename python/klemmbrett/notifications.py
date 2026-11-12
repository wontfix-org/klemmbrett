"""Desktop notifications: klemmbrett as the `org.freedesktop.Notifications` server.

`serve` takes the bus name (stop mako, dunst, or the like first, or pass
`replace=True`) and routes every notification through a function that
decides what happens to it::

    from klemmbrett import notifications as notify

    def route(n):
        if n.app_name == "Spotify":
            return notify.DROP
        if n.critical or n.app_name == "Signal":
            return notify.SHOW
        return notify.STASH

    inbox = notify.serve(route)
    kb.widget("inbox", inbox.unread, layout=layouts.filter, search=["summary", "body"])

`SHOW` draws a toast, `DROP` discards the notification, and `STASH` keeps
it as unread in the stash `serve` returns (or the one given as `stash=`).
A route may return a `Stash` to keep it there instead, or a list such as
`[notify.SHOW, inbox]` to do both. Without a route, or when the route
raises, the notification is shown.

Picking a stashed notification in a widget marks it read.
"""

from __future__ import annotations

import sys
import time
import traceback
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass, field, replace
from typing import Any, Union

from . import _runtime
from ._record import Record
from .providers import Store, View

__all__ = [
    "CRITICAL",
    "DROP",
    "LOW",
    "NORMAL",
    "SHOW",
    "STASH",
    "Decision",
    "Notification",
    "Stash",
    "Stashed",
    "serve",
]

LOW, NORMAL, CRITICAL = 0, 1, 2


class Decision(str):
    """What happens to a notification. Use `SHOW`, `DROP`, or `STASH`."""

    def __repr__(self) -> str:
        return f"notifications.{self.upper()}"


SHOW = Decision("show")
DROP = Decision("drop")
STASH = Decision("stash")

_EXPIRED = 1


@dataclass(frozen=True)
class Notification:
    """One notification as the sender sent it. `timeout` is in milliseconds:
    -1 means the server's default, 0 never expires."""

    id: int
    app_name: str
    summary: str
    body: str = ""
    app_icon: str = ""
    actions: tuple[tuple[str, str], ...] = ()
    hints: Mapping[str, Any] = field(default_factory=dict, hash=False)
    urgency: int = NORMAL
    timeout: int = -1
    replaces: bool = False

    @property
    def critical(self) -> bool:
        return self.urgency == CRITICAL

    @property
    def category(self) -> str | None:
        category = self.hints.get("category")
        return category if isinstance(category, str) else None

    @property
    def desktop_entry(self) -> str | None:
        entry = self.hints.get("desktop-entry")
        return entry if isinstance(entry, str) else None

    @classmethod
    def _from_wire(cls, wire: Mapping[str, Any]) -> Notification:
        return cls(
            id=wire["id"],
            app_name=wire["app_name"],
            summary=wire["summary"],
            body=wire.get("body", ""),
            app_icon=wire.get("app_icon", ""),
            actions=tuple((key, label) for key, label in wire.get("actions", ())),
            hints=dict(wire.get("hints") or {}),
            urgency=wire.get("urgency", NORMAL),
            timeout=wire.get("timeout", -1),
            replaces=wire.get("replaces", False),
        )


@dataclass(frozen=True)
class Stashed:
    """A stashed notification's record data. `received` is a Unix time."""

    id: int
    app_name: str
    summary: str
    body: str
    app_icon: str
    urgency: int
    category: str | None
    received: float
    read: bool = False


_stashes: dict[str, Stash] = {}


class Stash(Store):
    """Stashed notifications, newest first, keyed by notification id.

    Use `stash.unread` as a widget provider for what is still unread, or the
    stash itself for everything. Picking a row marks it read.
    """

    def __init__(self, name: str = "notifications", *, limit: int | None = 500) -> None:
        if not isinstance(name, str) or not name:
            raise ValueError("Stash: the name must be a non-empty string")
        if name in _stashes:
            raise ValueError(f"a stash named {name!r} already exists")
        super().__init__(schema=Stashed, limit=limit)
        self.name = name
        self.unread = View(self, filter=lambda record: not record.data.read)
        _stashes[name] = self

    def stash(self, notification: Notification) -> Record:
        """Keeps `notification` as unread. A notification replacing an
        earlier one takes its place and is unread again."""
        data = Stashed(
            id=notification.id,
            app_name=notification.app_name,
            summary=notification.summary,
            body=notification.body,
            app_icon=notification.app_icon,
            urgency=notification.urgency,
            category=notification.category,
            received=time.time(),
        )
        label = f"{notification.app_name}: {notification.summary}" if notification.app_name else notification.summary
        record = Record(notification.body or notification.summary, label=label, key=str(notification.id), data=data)
        return self.add(record, first=True)

    def mark_read(self, *records: Record | str) -> int:
        """Marks notifications read, given as records or keys. Returns how
        many were unread."""
        return self._set_read([record.key if isinstance(record, Record) else str(record) for record in records])

    def mark_all_read(self) -> int:
        return self._set_read(None)

    def _set_read(self, keys: Iterable[str] | None) -> int:
        marked = 0
        with self._lock:
            for key in list(self._records) if keys is None else keys:
                record = self._records.get(key)
                if record is not None and not record.data.read:
                    self._records[key] = replace(record, data=replace(record.data, read=True))
                    marked += 1
        if marked:
            self._notify()
        return marked

    def dismiss(self, *records: Record | str) -> None:
        """Removes notifications from the stash."""
        for record in records:
            self.remove(record.key if isinstance(record, Record) else str(record))

    @property
    def unread_count(self) -> int:
        return sum(1 for record in self.records() if not record.data.read)

    def select(self, record: Record) -> None:
        self.mark_read(record)

    def __repr__(self) -> str:
        return f"Stash({self.name!r}, {len(self)} notifications, {self.unread_count} unread)"


Route = Callable[[Notification], Union[Decision, Stash, Iterable[Union[Decision, Stash]], None]]


@dataclass
class _Settings:
    route: Route | None
    stash: Stash
    config: dict[str, Any]


_settings: _Settings | None = None


def serve(
    route: Route | None = None,
    *,
    stash: Stash | None = None,
    timeout: float = 5,
    location: str = "north-east",
    replace: bool = False,
    max_visible: int = 5,
) -> Stash:
    """Makes klemmbrett the notification server, and returns the stash that
    `STASH` keeps notifications in.

    - `route(notification)`: returns `SHOW`, `DROP`, `STASH`, a `Stash`, or
      a list of those. Without one, everything is shown.
    - `timeout`: seconds a toast stays when the sender leaves it to us.
    - `location`: where toasts stack, such as `"north-east"` or `"south"`.
    - `replace`: take the bus name from a running notification daemon.
    - `max_visible`: further toasts wait for one to close.
    """
    global _settings
    if _settings is not None:
        raise ValueError("notifications.serve is called more than once")
    if route is not None and not callable(route):
        raise TypeError(f"serve: route must be callable, got {route!r}")
    if not timeout > 0:
        raise ValueError("serve: timeout must be greater than zero")
    if max_visible < 1:
        raise ValueError("serve: max_visible must be at least 1")
    _settings = _Settings(
        route=route,
        stash=stash if stash is not None else _stashes.get("notifications") or Stash(),
        config={"timeout": timeout, "location": location, "replace": replace, "max_visible": max_visible},
    )
    return _settings.stash


def _reset() -> None:
    global _settings
    _settings = None
    _stashes.clear()


def _wire() -> dict[str, Any] | None:
    return None if _settings is None else dict(_settings.config)


def _routed() -> bool:
    return _settings is not None and _settings.route is not None


def _decisions(notification: Notification) -> list[Decision | Stash]:
    assert _settings is not None
    if _settings.route is None:
        return [SHOW]
    try:
        decided = _settings.route(notification)
    except Exception:
        print(f"klemmbrett: notification route failed for {notification!r}; showing it:", file=sys.stderr)
        traceback.print_exc()
        return [SHOW]
    if decided is None:
        return [SHOW]
    if isinstance(decided, Decision | Stash):
        return [decided]
    if isinstance(decided, str):
        if decided not in (SHOW, DROP, STASH):
            raise ValueError(f"route returned {decided!r}; use SHOW, DROP, or STASH")
        return [Decision(decided)]
    return list(decided)


def _dispatch(wire: Mapping[str, Any]) -> None:
    """Routes one notification from the daemon."""
    if _settings is None:
        return
    notification = Notification._from_wire(wire)
    try:
        decisions = _decisions(notification)
    except ValueError as err:
        print(f"klemmbrett: {err}; showing the notification", file=sys.stderr)
        decisions = [SHOW]
    shown = False
    for decision in decisions:
        if decision == SHOW:
            shown = True
        elif decision == STASH:
            _settings.stash.stash(notification)
        elif isinstance(decision, Stash):
            decision.stash(notification)
        elif decision != DROP:
            print(f"klemmbrett: route returned {decision!r}; use SHOW, DROP, STASH, or a Stash", file=sys.stderr)
            shown = True
    if shown:
        _runtime.backend.notification_show(notification.id)
    else:
        _runtime.backend.notification_close(notification.id, _EXPIRED)


def _call(method: str, parameters: Mapping[str, Any]) -> dict[str, Any]:
    """Answers the Varlink stash methods."""
    name = parameters.get("stash")
    stash = _stashes.get(name) if isinstance(name, str) else None
    if stash is None:
        return {"error": "UnknownStash", "parameters": {"stash": name, "known": sorted(_stashes)}}
    if method == "StashList":
        records = stash.unread.records() if parameters.get("unread_only") else stash.records()
        return {"ok": {"records": [record._json() for record in records]}}
    if method == "StashMarkRead":
        keys = parameters.get("keys")
        marked = stash.mark_all_read() if keys is None else stash.mark_read(*keys)
        return {"ok": {"marked": marked}}
    return {"error": "Failed", "parameters": {"message": f"{method} is not available"}}
