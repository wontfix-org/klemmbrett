"""Tokyo Night

https://github.com/tokyo-night/tokyo-night-vscode-theme

bg      #1a1b26  highlight #292e42  fg       #c0caf5
comment #565f89  blue      #7aa2f7  surface  #16161e
"""

from klemmbrett.style import Style, Theme, transparent

theme = Theme(
    name="tokyo-night",
    defaults=Style(
        background_color="#1a1b26",
        text_color="#c0caf5",
        border_color="#292e42",
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
    element_selected=Style(background_color="#3d59a1"),
    element_hover=Style(background_color="#292e42"),
    element_index=Style(text_color="#565f89"),
    inputbar=Style(
        background_color="#16161e",
        border_radius=10,
        padding=(8, 12),
        height=40,
    ),
    prompt=Style(text_color="#7aa2f7"),
)
