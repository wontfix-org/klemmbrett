"""Gruvbox Dark (medium contrast) — morhetz

https://github.com/morhetz/gruvbox

dark0 #282828  dark1 #3c3836  dark2 #504945
fg    #ebdbb2  gray  #928374
"""

from klemmbrett.style import Style, Theme, transparent

theme = Theme(
    name="gruvbox-dark",
    defaults=Style(
        background_color="#282828",
        text_color="#ebdbb2",
        border_color="#504945",
    ),
    overlay=Style(background_color=transparent),
    window=Style(
        border=1,
        border_radius=2,
    ),
    element_selected=Style(background_color="#3c3836"),
    element_hover=Style(background_color="#3c3836"),
    element_index=Style(text_color="#928374"),
)
