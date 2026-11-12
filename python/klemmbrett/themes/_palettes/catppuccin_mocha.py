"""Catppuccin Mocha

https://github.com/catppuccin/palette

base    #1e1e2e  surface0 #313244  surface1 #45475a
text    #cdd6f4  overlay0 #6c7086  mauve    #cba6f7
"""

from klemmbrett.style import Style, Theme, transparent

theme = Theme(
    name="catppuccin-mocha",
    defaults=Style(
        background_color="#1e1e2e",
        text_color="#cdd6f4",
        border_color="#313244",
    ),
    overlay=Style(background_color=transparent),
    window=Style(
        border=1,
        border_radius=8,
    ),
    element_selected=Style(background_color="#45475a"),
    element_hover=Style(background_color="#313244"),
    element_index=Style(text_color="#6c7086"),
)
