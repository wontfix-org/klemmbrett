"""Everforest dark, medium contrast

https://github.com/sainnhe/everforest

bg0   #2d353b  bg1  #343f44  bg2   #3d484d
fg    #d3c6aa  grey #859289  green #a7c080
aqua  #83c092  blue #7fbbb3
"""

from klemmbrett.style import Style, Theme, transparent

theme = Theme(
    name="everforest",
    defaults=Style(
        background_color="#2d353b",
        text_color="#d3c6aa",
        border_color="#3d484d",
    ),
    overlay=Style(background_color=transparent),
    window=Style(
        border=1,
        border_radius=14,
        padding=10,
        spacing=8,
    ),
    listview=Style(spacing=4),
    element=Style(
        padding=(4, 10),
        border_radius=8,
        height=32,
    ),
    element_selected=Style(
        background_color="#475258",
        text_color="#d3c6aa",
    ),
    element_hover=Style(background_color="#3d484d"),
    element_index=Style(text_color="#859289"),
    inputbar=Style(
        background_color="#343f44",
        border_radius=10,
        padding=(8, 12),
        height=40,
    ),
    prompt=Style(text_color="#a7c080"),
)
