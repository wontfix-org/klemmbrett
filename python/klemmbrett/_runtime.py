"""Operations on the running daemon. Tests replace `backend`."""

from __future__ import annotations

import os
from collections.abc import Iterable, Mapping, Sequence
from typing import Any

from . import _core
from ._record import RecordLike, to_record

backend: Any = _core


def copy(text: str) -> None:
    """Puts `text` on the clipboard and the primary selection, and into history."""
    if not isinstance(text, str):
        raise TypeError(f"copy: expected a string, got {text!r}")
    backend.copy(text)


def launch(argv: Sequence[str | os.PathLike[str]]) -> None:
    """Starts a detached program the way the `launch` setting says."""
    if isinstance(argv, str | bytes):
        raise TypeError("launch: pass the command as a list, e.g. ['xdg-open', url]")
    backend.launch_command([os.fspath(part) for part in argv])


def activate_window(key: str) -> None:
    """Focuses the window a `WaylandWindows` record stands for."""
    backend.activate_window(key)


def theme_wire(theme: Any) -> dict[str, Any] | None:
    from . import themes
    from .style import Theme

    if theme is None or theme == "default":
        return None
    if isinstance(theme, str):
        theme = themes.get(theme)
    if isinstance(theme, Mapping) and not isinstance(theme, Theme):
        theme = Theme(theme)
    if not isinstance(theme, Theme):
        raise TypeError(f"expected a theme name or a Theme, got {theme!r}")
    return theme._wire()


def set_theme(theme: Any) -> None:
    """Replaces the configured theme until the daemon exits: a palette name,
    `"default"` (or `None`) for the built-in look, or a `Theme`. Widgets with
    their own theme keep it, and `overrides` still apply."""
    backend.set_theme(theme_wire(theme))


def show(
    widget: str,
    records: Iterable[RecordLike] | None = None,
    *,
    pointer: tuple[int, int] | None = None,
) -> None:
    """Opens a widget, with `records` in place of its provider's."""
    wire = None if records is None else [to_record(record)._wire() for record in records]
    backend.show(widget, wire, pointer)


def quit() -> None:  # noqa: A001
    """Stops the daemon."""
    backend.quit()


def is_running() -> bool:
    return bool(backend.is_running())
