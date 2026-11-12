"""Nord — arctic, north-bluish palette

https://www.nordtheme.com/docs/colors-and-palettes

nord0 #2e3440  nord1 #3b4252  nord2 #434c5e  nord3 #4c566a
nord4 #d8dee9  nord6 #eceff4  nord10 #5e81ac
"""

from klemmbrett.style import Style, Theme, transparent

theme = Theme(
    name="nord",
    defaults=Style(
        background_color="#2e3440",
        text_color="#eceff4",
        border_color="#3b4252",
    ),
    overlay=Style(background_color=transparent),
    window=Style(
        border=1,
        border_radius=4,
    ),
    element_selected=Style(background_color="#434c5e"),
    element_hover=Style(background_color="#3b4252"),
    element_index=Style(text_color="#4c566a"),
)
