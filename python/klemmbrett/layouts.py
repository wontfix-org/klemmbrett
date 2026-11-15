"""The shipped layouts. A layout is a `ui.Window`, or a function of a
`LayoutContext` returning one; functions run each time the widget opens."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from typing import overload

from .ui import Element, Entry, Index, InputBar, ListView, Marker, Node, Preview, Prompt, Text, Window

__all__ = ["LayoutContext", "menu", "filter", "prompt", "rows", "secret"]


@dataclass(frozen=True)
class LayoutContext:
    widget: str
    #: Multi-select keys are configured, so rows need a marker.
    multi: bool
    #: Opened by `kb prompt`, which prints values instead of running handlers.
    prompt: bool


def rows(ctx: LayoutContext) -> ListView:
    """The list with an accelerator and the label per row."""
    markers = [Marker()] if ctx.multi else []
    return ListView(Element(*markers, Index(), Text("{label}")))


def menu(ctx: LayoutContext) -> Window:
    """The list alone, at the pointer; rows are picked by accelerator or click."""
    return Window(rows(ctx), location="pointer")


@overload
def filter(ctx: LayoutContext, *, preview: Node | None = None) -> Window: ...  # noqa: A001


@overload
def filter(  # noqa: A001
    ctx: None = None, *, preview: Node | None = None
) -> Callable[[LayoutContext], Window]: ...


def filter(  # noqa: A001
    ctx: LayoutContext | None = None, *, preview: Node | None = None
) -> Window | Callable[[LayoutContext], Window]:
    """An input bar over the list, centered; typing filters the rows.

    `preview` is a pane to the right of the list. A `ui.Preview` is that
    pane; any other node is placed inside one. Called without a context,
    this returns a layout function::

        layout=layouts.filter(preview=ui.RichText("{preview}"))
    """
    if preview is not None and not isinstance(preview, Node):
        raise TypeError(f"filter: preview must be a layout node, got {preview!r}")

    def build(ctx: LayoutContext) -> Window:
        children: list[Node] = [InputBar(Prompt(), Entry()), rows(ctx)]
        if preview is not None:
            children.append(preview if isinstance(preview, Preview) else Preview(preview))
        return Window(*children, location="center")

    if ctx is None:
        return build
    return build(ctx)


def prompt(ctx: LayoutContext) -> Window:
    """An input bar only; Enter submits the typed text as the record."""
    return Window(InputBar(Prompt(), Entry()), location="center")


def secret(ctx: LayoutContext, *, mask: str = "•") -> Window:
    """A password card for `secrets` agents: the question's title, message,
    and last error (lines that are empty are left out), then the masked
    input bar. Agent widgets without a layout get the same card, which
    also handles confirmations and choices."""
    return Window(
        Text("{title}"),
        Text("{message}"),
        Text("{error}"),
        InputBar(Prompt(), Entry(mask=mask)),
        location="center",
    )
