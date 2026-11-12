"""Dracula

https://draculatheme.com

background #282a36  current-line/selection #44475a
foreground #f8f8f2  comment #6272a4
"""

from klemmbrett.style import Style, Theme, transparent

theme = Theme(
    name="dracula",
    defaults=Style(
        background_color="#282a36",
        text_color="#f8f8f2",
        border_color="#44475a",
    ),
    overlay=Style(background_color=transparent),
    window=Style(
        border=1,
        border_radius=4,
    ),
    element_selected=Style(background_color="#44475a"),
    element_hover=Style(background_color="#44475a"),
    element_index=Style(text_color="#6272a4"),
)
