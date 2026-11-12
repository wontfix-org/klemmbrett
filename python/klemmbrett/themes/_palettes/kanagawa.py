"""Kanagawa wave

https://github.com/rebelot/kanagawa.nvim

sumiInk1 #1f1f28  sumiInk3 #363646  fujiWhite #dcd7ba
fujiGray #727169  waveBlue #2d4f67  crystal   #7e9cd8
"""

from klemmbrett.style import Style, Theme, transparent

theme = Theme(
    name="kanagawa",
    defaults=Style(
        background_color="#1f1f28",
        text_color="#dcd7ba",
        border_color="#363646",
    ),
    overlay=Style(background_color=transparent),
    window=Style(
        border=1,
        border_radius=16,
        padding=10,
        spacing=8,
    ),
    listview=Style(spacing=4),
    element=Style(
        padding=(4, 10),
        border_radius=8,
        height=32,
    ),
    element_selected=Style(background_color="#2d4f67"),
    element_hover=Style(background_color="#2a2a37"),
    element_index=Style(text_color="#727169"),
    inputbar=Style(
        background_color="#16161d",
        border_radius=10,
        padding=(8, 12),
        height=40,
    ),
    prompt=Style(text_color="#7e9cd8"),
)
