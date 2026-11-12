"""mbra — qtile focus, tmux status, and Alacritty

focus   #60845b  unfocused #50594b  current #0f0f0f
pane    #B9B9B9  terminal  #000000  text    #e5e5e5
"""

from klemmbrett.style import Style, Theme, transparent

theme = Theme(
    name="mbra",
    defaults=Style(
        background_color="#000000",
        text_color="#e5e5e5",
        border_color="#50594b",
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
    element_selected=Style(
        background_color="#60845b",
        text_color="#0f0f0f",
    ),
    element_hover=Style(background_color="#50594b"),
    element_index=Style(text_color="#b9b9b9"),
    inputbar=Style(
        background_color="#0f0f0f",
        border_radius=10,
        padding=(8, 12),
        height=40,
    ),
    prompt=Style(text_color="#60845b"),
)
