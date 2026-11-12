"""Rosé Pine

https://rosepinetheme.com

base    #191724  surface #1f1d2e  overlay #26233a
text    #e0def4  muted   #6e6a86  iris    #c4a7e7
foam    #9ccfd8  rose    #ebbcba
"""

from klemmbrett.style import Style, Theme, transparent

theme = Theme(
    name="rose-pine",
    defaults=Style(
        background_color="#191724",
        text_color="#e0def4",
        border_color="#26233a",
    ),
    overlay=Style(background_color=transparent),
    window=Style(
        border=1,
        border_radius=18,
        padding=12,
        spacing=8,
    ),
    listview=Style(spacing=4),
    element=Style(
        padding=(4, 12),
        border_radius=10,
        height=32,
    ),
    element_selected=Style(
        background_color="#26233a",
        text_color="#e0def4",
        border=1,
        border_color="#c4a7e7",
    ),
    element_hover=Style(background_color="#1f1d2e"),
    element_index=Style(text_color="#6e6a86"),
    inputbar=Style(
        background_color="#1f1d2e",
        border_radius=12,
        padding=(8, 12),
        height=40,
    ),
    prompt=Style(text_color="#ebbcba"),
)
