"""Solarized Dark — Ethan Schoonover

https://ethanschoonover.com/solarized/

base03 #002b36  base02 #073642  base01 #586e75  base0 #839496
base1  #93a1a1  blue   #268bd2
"""

from klemmbrett.style import Style, Theme, transparent

theme = Theme(
    name="solarized-dark",
    defaults=Style(
        background_color="#002b36",
        text_color="#839496",
        border_color="#073642",
    ),
    overlay=Style(background_color=transparent),
    window=Style(
        border=1,
        border_radius=0,
    ),
    element_selected=Style(
        background_color="#073642",
        text_color="#93a1a1",
    ),
    element_hover=Style(background_color="#073642"),
    element_index=Style(text_color="#586e75"),
)
