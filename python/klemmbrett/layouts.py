"""The shipped layouts. A layout is a `ui.Window`, or a function of a
`LayoutContext` returning one; functions run each time the widget opens."""

from __future__ import annotations

from dataclasses import dataclass

from .ui import Element, Entry, Index, InputBar, ListView, Marker, Prompt, Text, Window

__all__ = ["LayoutContext", "menu", "filter", "prompt", "rows"]


@dataclass(frozen=True)
class LayoutContext:
    widget: str
    #: Multi-select keys are configured, so rows need a marker.
    multi: bool
    #: Opened by `klemmbrett prompt`, which prints values instead of running handlers.
    prompt: bool


def rows(ctx: LayoutContext) -> ListView:
    """The list with an accelerator and the label per row."""
    markers = [Marker()] if ctx.multi else []
    return ListView(Element(*markers, Index(), Text("{label}")))


def menu(ctx: LayoutContext) -> Window:
    """The list alone, at the pointer; rows are picked by accelerator or click."""
    return Window(rows(ctx), location="pointer")


def filter(ctx: LayoutContext) -> Window:  # noqa: A001
    """An input bar over the list, centered; typing filters the rows."""
    return Window(InputBar(Prompt(), Entry()), rows(ctx), location="center")


def prompt(ctx: LayoutContext) -> Window:
    """An input bar only; Enter submits the typed text as the record."""
    return Window(InputBar(Prompt(), Entry()), location="center")
