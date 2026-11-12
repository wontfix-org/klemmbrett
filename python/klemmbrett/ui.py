"""Layout nodes. A layout is a `Window` tree, rendered by the daemon.

Where nodes may appear:

- `Window` holds at most one `InputBar` and one `ListView`, plus `Text`,
  `Spacer`, `Row`, and `Column`.
- `InputBar` holds `Prompt` and `Entry`, plus the primitives.
- `ListView` holds exactly one `Element`, the template for every row.
- `Element` holds `Marker`, `Index`, and the primitives.

`Text` templates expand `{label}`, `{key}`, and `{value}` inside an element,
and `{widget}`, `{query}`, and `{count}` elsewhere. `{{` and `}}` are literal
braces. Style keyword arguments override the theme for that node only::

    ui.Window(
        ui.InputBar(ui.Prompt(), ui.Entry()),
        ui.ListView(ui.Element(ui.Index(), ui.Text("{label}"), ui.Spacer(), ui.Text("{key}"))),
        location="center",
    )
"""

from __future__ import annotations

from collections.abc import Iterable
from typing import Any

from .style import Style

__all__ = [
    "Node",
    "Window",
    "InputBar",
    "Prompt",
    "Entry",
    "ListView",
    "Element",
    "Marker",
    "Index",
    "Text",
    "Spacer",
    "Row",
    "Column",
]


class Node:
    kind = ""
    leaf = False

    def __init__(
        self,
        *children: Node,
        children_: Iterable[Node] = (),
        style: Style | None = None,
        **props: Any,
    ) -> None:
        nodes = tuple(children) + tuple(children_)
        if self.leaf and nodes:
            raise TypeError(f"{type(self).__name__} takes no children")
        for child in nodes:
            if not isinstance(child, Node):
                raise TypeError(f"{type(self).__name__}: expected a layout node, got {child!r}")
        self.children = nodes
        self.style = (style or Style()) | Style(**props)

    def __repr__(self) -> str:
        parts = [repr(child) for child in self.children]
        parts += [f"{name}={value!r}" for name, value in self.style.items()]
        return f"{type(self).__name__}({', '.join(parts)})"

    def __eq__(self, other: object) -> bool:
        if type(other) is not type(self):
            return NotImplemented
        return self._wire() == other._wire()  # type: ignore[attr-defined]

    __hash__ = None  # type: ignore[assignment]

    def _wire(self) -> dict[str, Any]:
        return {
            "kind": self.kind,
            "style": self.style._wire(),
            "children": [child._wire() for child in self.children],
        }


def _children(args: tuple[Node, ...], kwargs: dict[str, Any]) -> dict[str, Any]:
    if "children" in kwargs:
        kwargs["children_"] = kwargs.pop("children")
    return kwargs


class Window(Node):
    """The popup card. `location` places it (overridden by the widget's own)."""

    kind = "window"

    def __init__(self, *children: Node, location: str | None = None, **kwargs: Any) -> None:
        super().__init__(*children, **_children(children, kwargs))
        self.location = location

    def _wire(self) -> dict[str, Any]:
        return {**super()._wire(), "location": self.location}

    def __repr__(self) -> str:
        text = super().__repr__()
        return text if self.location is None else f"{text[:-1]}, location={self.location!r})"


class InputBar(Node):
    kind = "inputbar"

    def __init__(self, *children: Node, **kwargs: Any) -> None:
        super().__init__(*children, **_children(children, kwargs))


class ListView(Node):
    kind = "listview"

    def __init__(self, *children: Node, **kwargs: Any) -> None:
        super().__init__(*children, **_children(children, kwargs))


class Element(Node):
    """One row; its children are drawn for every record."""

    kind = "element"

    def __init__(self, *children: Node, **kwargs: Any) -> None:
        super().__init__(*children, **_children(children, kwargs))


class Row(Node):
    kind = "row"

    def __init__(self, *children: Node, **kwargs: Any) -> None:
        super().__init__(*children, **_children(children, kwargs))


class Column(Node):
    kind = "column"

    def __init__(self, *children: Node, **kwargs: Any) -> None:
        super().__init__(*children, **_children(children, kwargs))


class Prompt(Node):
    """The prompt: its `text` style property, else the widget name."""

    kind = "prompt"
    leaf = True


class Entry(Node):
    """The text being typed."""

    kind = "entry"
    leaf = True


class Marker(Node):
    """The multi-select mark of a row."""

    kind = "marker"
    leaf = True


class Index(Node):
    """The accelerator of a row (1-9, then a-z)."""

    kind = "index"
    leaf = True


class Spacer(Node):
    """Takes up the remaining space along its parent's orientation."""

    kind = "spacer"
    leaf = True


class Text(Node):
    kind = "text"
    leaf = True

    def __init__(self, template: str, **kwargs: Any) -> None:
        if not isinstance(template, str):
            raise TypeError(f"Text: expected a template string, got {template!r}")
        super().__init__(**kwargs)
        self.template = template

    def _wire(self) -> dict[str, Any]:
        return {**super()._wire(), "text": self.template}

    def __repr__(self) -> str:
        extra = "".join(f", {name}={value!r}" for name, value in self.style.items())
        return f"Text({self.template!r}{extra})"
