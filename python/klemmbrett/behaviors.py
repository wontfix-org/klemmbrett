"""Behaviors a list popup runs around its own keys.

Pass them to `kb.widget` or `kb.picker` as `behaviors=`. `None` uses the
provider's `default_behaviors`. `[]` turns them off.

`multi=True` is `behaviors=[bh.Multi()]`.
`run_query=True` is `behaviors=[bh.RunCommands()]`.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

_KINDS = {
    "multi": ("select", "deselect"),
    "run_commands": ("forget", "terminal"),
}


@dataclass(frozen=True)
class Behavior:
    """A key binding the list runs before its own keys."""

    def _spec(self) -> dict[str, str]:
        raise NotImplementedError


@dataclass(frozen=True)
class Multi(Behavior):
    """Tab marks the highlighted row and moves down. Shift-Tab marks it
    and moves up.

    `select="tab"` and `deselect="shift-tab"` are the defaults.
    `deselect="btab"` is the same shift-tab. Modifiers are written
    `shift-delete`, `ctrl-space`, and `alt-enter`.
    """

    select: str = "tab"
    deselect: str = "shift-tab"

    def _spec(self) -> dict[str, str]:
        return {
            "kind": "multi",
            "select": self.select,
            "deselect": self.deselect,
        }


@dataclass(frozen=True)
class RunCommands(Behavior):
    """Offer a typed line, remember it, and bind the run keys.

    `forget="shift-delete"` drops a remembered line. A `PATH` row stays.
    `terminal="shift-enter"` runs the selection in the configured terminal.
    """

    forget: str = "shift-delete"
    terminal: str = "shift-enter"

    def _spec(self) -> dict[str, str]:
        return {
            "kind": "run_commands",
            "forget": self.forget,
            "terminal": self.terminal,
        }


def normalize(where: str, behaviors: Any) -> tuple[Behavior, ...]:
    """Checks `behaviors` and returns them in order.

    `where` names the widget in errors, such as ``widget 'links'`` or
    ``picker``. A single behavior is not a list. Unknown kinds and keys
    such as ``shift-`` or ``tab-enter`` are rejected.
    """
    if isinstance(behaviors, (str, Behavior)):
        raise TypeError(f"{where}: behaviors must be a list, got {behaviors!r}")
    try:
        items = tuple(behaviors)
    except TypeError:
        raise TypeError(
            f"{where}: behaviors must be a list, got {behaviors!r}"
        ) from None
    checked: list[Behavior] = []
    for item in items:
        if not isinstance(item, Behavior):
            raise TypeError(
                f"{where}: behaviors must be Multi or RunCommands, got {item!r}"
            )
        spec = item._spec()
        kind = spec.get("kind")
        fields = _KINDS.get(str(kind))
        if fields is None:
            raise ValueError(f"{where}: unknown behavior {kind!r}")
        for field in fields:
            _key(where, spec.get(field))
        checked.append(item)
    return tuple(checked)


def _key(where: str, spec: Any) -> None:
    """Accepts `tab`, `shift-tab`, `btab`, `ctrl-space`, `shift-delete`."""
    if not isinstance(spec, str):
        raise TypeError(f"{where}: invalid key {spec!r}")
    key = None
    for part in spec.split("-"):
        name = part.lower()
        if name == "":
            raise ValueError(f"{where}: invalid key {spec!r}")
        if name in {"shift", "ctrl", "control", "alt"}:
            continue
        if name == "btab":
            if key is not None:
                raise ValueError(f"{where}: invalid key {spec!r}")
            key = "tab"
            continue
        if key is not None:
            raise ValueError(f"{where}: invalid key {spec!r}")
        key = name
    if key is None:
        raise ValueError(f"{where}: invalid key {spec!r}")
