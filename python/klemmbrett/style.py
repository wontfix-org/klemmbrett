"""Colors, lengths, and the `Style` and `Theme` objects that describe the popup's look.

A `Style` is a sparse set of properties. A `Theme` maps theme elements
(`window`, `element_selected`, ...) to styles. Both merge with `|`, the right
side winning per property, like dicts::

    from klemmbrett.style import Style, Theme, em

    dense = Theme(element=Style(height=em(2), padding=(2, 8)))
    theme = themes.nord | dense
"""

from __future__ import annotations

from collections.abc import Iterator, Mapping
from typing import Any

from . import _core

__all__ = [
    "PROPERTIES",
    "ELEMENTS",
    "Style",
    "Theme",
    "inherit",
    "rgb",
    "hsl",
    "hwb",
    "cmyk",
    "transparent",
    "px",
    "em",
    "ch",
    "mm",
    "pct",
]

PROPERTIES = (
    "padding",
    "margin",
    "border",
    "border_radius",
    "background_color",
    "border_color",
    "text_color",
    "spacing",
    "orientation",
    "width",
    "height",
    "max_height",
    "font_size",
    "location",
    "text",
)

#: Theme elements, as keyword arguments of `Theme`.
ELEMENTS = (
    "defaults",
    "overlay",
    "window",
    "listview",
    "element",
    "element_selected",
    "element_hover",
    "element_index",
    "element_text",
    "element_marker",
    "inputbar",
    "prompt",
    "entry",
    "toast",
    "toast_summary",
    "toast_body",
)

_COLORS = {"background_color", "border_color", "text_color"}
_EDGES = {"padding", "margin", "border"}
_VERTICAL = {"height", "max_height", "font_size"}

EM_PX = 14.0
CH_PX = 9.0
MM_PX = 96.0 / 25.4
MONITOR = (1920.0, 1080.0)


class _Inherit:
    """Take the value from the parent element instead of the theme."""

    _instance: _Inherit | None = None

    def __new__(cls) -> _Inherit:
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    def __repr__(self) -> str:
        return "inherit"

    def __reduce__(self) -> str:
        return "inherit"


inherit = _Inherit()

Color = str | tuple[int, int, int, int]


def _alpha(a: float) -> int:
    if isinstance(a, int) and not 0 <= a <= 1:
        raise ValueError("alpha is a float from 0 to 1")
    return round(max(0.0, min(1.0, float(a))) * 255)


def rgb(r: int, g: int, b: int, a: float = 1.0) -> tuple[int, int, int, int]:
    """An sRGB color from 0-255 channels and an alpha from 0 to 1."""
    for channel in (r, g, b):
        if not 0 <= channel <= 255:
            raise ValueError(f"color channel {channel} is outside 0-255")
    return (int(r), int(g), int(b), _alpha(a))


def hsl(h: float, s: float, l: float, a: float = 1.0) -> tuple[int, int, int, int]:  # noqa: E741
    """Hue in degrees; saturation, lightness, and alpha from 0 to 1."""
    return _core.hsl(h, s, l, _alpha(a))


def hwb(h: float, w: float, b: float, a: float = 1.0) -> tuple[int, int, int, int]:
    """Hue in degrees; whiteness, blackness, and alpha from 0 to 1."""
    return _core.hwb(h, w, b, _alpha(a))


def cmyk(c: float, m: float, y: float, k: float, a: float = 1.0) -> tuple[int, int, int, int]:
    """Channels and alpha from 0 to 1."""
    return _core.cmyk(c, m, y, k, _alpha(a))


transparent = (0, 0, 0, 0)


def px(value: float) -> float:
    return float(value)


def em(value: float) -> float:
    """Multiples of the default 14 px font size."""
    return value * EM_PX


def ch(value: float) -> float:
    """Multiples of the default character width (9 px)."""
    return value * CH_PX


def mm(value: float) -> float:
    return value * MM_PX


class pct(float):
    """A percentage of a 1920x1080 monitor: width for horizontal lengths,
    height for `height`, `max_height`, and `font_size`."""

    def __repr__(self) -> str:
        return f"pct({float(self):g})"


def _length(name: str, value: Any) -> float:
    if isinstance(value, pct):
        size = MONITOR[1] if name in _VERTICAL else MONITOR[0]
        return float(value) * size / 100.0
    if isinstance(value, bool) or not isinstance(value, int | float):
        raise TypeError(f"{name}: expected a number, got {value!r}")
    return float(value)


def _color(name: str, value: Any) -> Color:
    if isinstance(value, str):
        _core.parse_color(value)
        return value
    if (
        isinstance(value, tuple)
        and len(value) in (3, 4)
        and all(isinstance(channel, int) and 0 <= channel <= 255 for channel in value)
    ):
        return (*value[:3], value[3] if len(value) == 4 else 255)
    raise TypeError(f"{name}: expected a color string or an rgb() tuple, got {value!r}")


def _check(name: str, value: Any) -> Any:
    if name not in PROPERTIES:
        raise TypeError(f"unknown style property {name!r}; expected one of {', '.join(PROPERTIES)}")
    if value is inherit:
        return value
    if name in _COLORS:
        return _color(name, value)
    if name in _EDGES:
        values = value if isinstance(value, list | tuple) else (value,)
        if not 1 <= len(values) <= 4:
            raise ValueError(f"{name}: expected 1 to 4 lengths")
        return tuple(_length(name, item) for item in values)
    if name in ("orientation", "location", "text"):
        if not isinstance(value, str):
            raise TypeError(f"{name}: expected a string, got {value!r}")
        return value
    return _length(name, value)


class Style(Mapping[str, Any]):
    """Style properties for one element. Unset properties come from the theme.

    Lengths are pixels unless wrapped in `em`, `ch`, `mm`, or `pct`. Edges
    (`padding`, `margin`, `border`) take one to four lengths in CSS order.
    Any property may be `inherit` to take the parent element's value.
    """

    __slots__ = ("_props",)

    def __init__(self, props: Mapping[str, Any] | None = None, /, **kwargs: Any) -> None:
        merged = {**(props or {}), **kwargs}
        self._props = {name: _check(name, value) for name, value in merged.items()}

    def __getitem__(self, name: str) -> Any:
        return self._props[name]

    def __iter__(self) -> Iterator[str]:
        return iter(self._props)

    def __len__(self) -> int:
        return len(self._props)

    def __or__(self, other: Mapping[str, Any]) -> Style:
        if not isinstance(other, Mapping):
            return NotImplemented
        return Style({**self._props, **other})

    def __ror__(self, other: Mapping[str, Any]) -> Style:
        if not isinstance(other, Mapping):
            return NotImplemented
        return Style({**other, **self._props})

    def __eq__(self, other: object) -> bool:
        if isinstance(other, Style):
            return self._props == other._props
        return NotImplemented

    def __hash__(self) -> int:
        return hash(tuple(sorted(self._props.items(), key=lambda item: item[0])))

    def __repr__(self) -> str:
        fields = ", ".join(f"{name}={value!r}" for name, value in self._props.items())
        return f"Style({fields})"

    def _wire(self) -> dict[str, Any]:
        wire: dict[str, Any] = {}
        inherited = []
        for name, value in self._props.items():
            if value is inherit:
                inherited.append(name)
            else:
                wire[name] = list(value) if isinstance(value, tuple) and name in _EDGES else value
        if inherited:
            wire["inherit"] = inherited
        return wire


def _selector(element: str) -> str:
    for state in ("selected", "hover"):
        if element.endswith("_" + state):
            return f"{element.removesuffix('_' + state)}:{state}"
    return element


class Theme(Mapping[str, Style]):
    """Styles per theme element. Unset elements and properties keep the
    built-in look.

    `defaults` applies to every element that does not set a property itself.
    """

    __slots__ = ("_elements", "name")

    def __init__(
        self,
        elements: Mapping[str, Mapping[str, Any]] | None = None,
        /,
        *,
        name: str | None = None,
        **kwargs: Mapping[str, Any],
    ) -> None:
        merged = {**(elements or {}), **kwargs}
        for element in merged:
            if element not in ELEMENTS:
                raise TypeError(
                    f"unknown theme element {element!r}; expected one of {', '.join(ELEMENTS)}"
                )
        self._elements = {
            element: style if isinstance(style, Style) else Style(style)
            for element, style in merged.items()
        }
        self.name = name

    def __getitem__(self, element: str) -> Style:
        return self._elements[element]

    def __iter__(self) -> Iterator[str]:
        return iter(self._elements)

    def __len__(self) -> int:
        return len(self._elements)

    def __or__(self, other: Mapping[str, Mapping[str, Any]]) -> Theme:
        if not isinstance(other, Mapping):
            return NotImplemented
        merged = dict(self._elements)
        for element, style in other.items():
            merged[element] = merged[element] | style if element in merged else Style(style)
        return Theme(merged, name=getattr(other, "name", None) or self.name)

    def __eq__(self, other: object) -> bool:
        if isinstance(other, Theme):
            return self._elements == other._elements
        return NotImplemented

    def __hash__(self) -> int:
        return hash(tuple(sorted(self._elements.items(), key=lambda item: item[0])))

    def __repr__(self) -> str:
        if self.name:
            return f"<Theme {self.name}>"
        fields = ", ".join(f"{element}={style!r}" for element, style in self._elements.items())
        return f"Theme({fields})"

    def resolve(self) -> dict[str, dict[str, Any]]:
        """Every element's final style over the built-in default."""
        return _core.resolve_theme([self._wire()])

    def _wire(self) -> dict[str, dict[str, Any]]:
        return {_selector(element): style._wire() for element, style in self._elements.items()}
