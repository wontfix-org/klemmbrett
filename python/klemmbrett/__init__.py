"""Keep the Wayland clipboard and primary selection in sync, and pick values
from popup widgets configured in Python.

The daemon reads `$XDG_CONFIG_HOME/klemmbrett/config.py`::

    import klemmbrett as kb
    from klemmbrett import layouts, providers, themes

    kb.configure(theme=themes.nord)
    kb.widget("history", providers.ClipboardHistory(), layout=layouts.filter)
"""

from __future__ import annotations

from . import layouts, notifications, providers, rpc, themes, ui
from ._record import Record
from ._registry import Widget, configure, widget
from ._schema import SchemaError
from ._runtime import activate_window, copy, is_running, launch, quit, set_theme, show
from .layouts import LayoutContext
from .providers import Provider, provider
from .style import Style, Theme, ch, cmyk, em, hsl, hwb, inherit, mm, pct, px, rgb, transparent

__all__ = [
    "LayoutContext",
    "Provider",
    "Record",
    "SchemaError",
    "Style",
    "Theme",
    "Widget",
    "activate_window",
    "ch",
    "cmyk",
    "configure",
    "copy",
    "em",
    "hsl",
    "hwb",
    "inherit",
    "is_running",
    "launch",
    "layouts",
    "mm",
    "notifications",
    "pct",
    "provider",
    "providers",
    "px",
    "quit",
    "rgb",
    "rpc",
    "set_theme",
    "show",
    "themes",
    "transparent",
    "ui",
    "widget",
]
