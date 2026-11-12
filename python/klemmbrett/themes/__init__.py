"""The shipped palettes, as `Theme` objects.

Use them by attribute (`themes.nord`, `themes.tokyo_night`) or by name
(`themes.get("tokyo-night")`), and merge your own changes with `|`::

    kb.configure(theme=themes.nord | Theme(window=Style(border_radius=0)))
"""

from __future__ import annotations

import importlib

from ..style import Theme

__all__ = ["NAMES", "names", "get"]

#: Palette names, as `kb.set_theme` and the `Themes` provider use them.
NAMES = (
    "catppuccin-mocha",
    "dracula",
    "everforest",
    "gruvbox-dark",
    "kanagawa",
    "mbra",
    "nord",
    "rose-pine",
    "solarized-dark",
    "solarized-light",
    "tokyo-night",
)


def names() -> tuple[str, ...]:
    return NAMES


def get(name: str) -> Theme:
    """A palette by name; `-` and `_` are interchangeable."""
    normalized = name.replace("_", "-")
    if normalized not in NAMES:
        raise KeyError(f"unknown theme {name!r}; expected default or one of {', '.join(NAMES)}")
    module = importlib.import_module(f"._palettes.{normalized.replace('-', '_')}", __name__)
    return module.theme


def __getattr__(attribute: str) -> Theme:
    if attribute.replace("_", "-") in NAMES:
        return get(attribute)
    raise AttributeError(f"module {__name__!r} has no attribute {attribute!r}")


def __dir__() -> list[str]:
    return [*globals(), *(name.replace("-", "_") for name in NAMES)]
