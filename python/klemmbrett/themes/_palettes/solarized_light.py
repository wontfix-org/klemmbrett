"""Solarized Light — Ethan Schoonover

https://ethanschoonover.com/solarized/

base3 #fdf6e3  base2 #eee8d5  base1 #93a1a1  base00 #657b83
base01 #586e75
"""

from klemmbrett.style import Style, Theme, transparent

theme = Theme(
    name="solarized-light",
    defaults=Style(
        background_color="#fdf6e3",
        text_color="#657b83",
        border_color="#eee8d5",
    ),
    overlay=Style(background_color=transparent),
    window=Style(
        border=1,
        border_radius=0,
    ),
    element_selected=Style(
        background_color="#eee8d5",
        text_color="#586e75",
    ),
    element_hover=Style(background_color="#eee8d5"),
    element_index=Style(text_color="#93a1a1"),
)
