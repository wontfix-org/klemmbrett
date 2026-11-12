import pytest

import klemmbrett as kb
from klemmbrett import themes
from klemmbrett.style import Style, Theme, ch, em, inherit, mm, pct, px


def test_colors_accept_strings_and_helpers():
    style = Style(background_color="#2e3440", text_color="white", border_color=kb.rgb(1, 2, 3, 0.5))
    assert style["background_color"] == "#2e3440"
    assert style["border_color"] == (1, 2, 3, 128)
    assert kb.hsl(0, 1, 0.5) == (255, 0, 0, 255)
    assert kb.hwb(0, 1, 1) == (128, 128, 128, 255)
    assert kb.cmyk(0, 0, 0, 1, 0) == (0, 0, 0, 0)
    with pytest.raises(ValueError, match="unknown color"):
        Style(text_color="not-a-color")
    with pytest.raises(ValueError, match="invalid hex"):
        Style(text_color="#12345")


def test_lengths_and_units():
    assert px(3) == 3.0
    assert em(2) == 28.0
    assert ch(2) == 18.0
    assert mm(25.4) == pytest.approx(96.0)
    style = Style(width=pct(10), height=pct(10), padding=(4, ch(1)))
    wire = style._wire()
    assert wire["width"] == 192.0
    assert wire["height"] == 108.0
    assert wire["padding"] == [4.0, 9.0]
    with pytest.raises(ValueError, match="1 to 4"):
        Style(margin=(1, 2, 3, 4, 5))
    with pytest.raises(TypeError, match="expected a number"):
        Style(height="tall")


def test_unknown_properties_name_the_supported_ones():
    with pytest.raises(TypeError, match="background_color"):
        Style(background="#fff")


def test_styles_merge_right_to_left_like_dicts():
    merged = Style(height=10, spacing=2) | Style(height=20)
    assert dict(merged) == {"height": 20.0, "spacing": 2.0}
    assert dict({"spacing": 5} | Style(height=1)) == {"spacing": 5.0, "height": 1.0}


def test_inherit_is_sent_as_a_list_of_names():
    wire = Style(text_color=inherit, height=4)._wire()
    assert wire == {"height": 4.0, "inherit": ["text_color"]}


def test_theme_selectors_and_merging():
    theme = Theme(element_selected=Style(text_color="red"), element=Style(height=30))
    assert theme._wire() == {
        "element:selected": {"text_color": "red"},
        "element": {"height": 30.0},
    }
    merged = theme | Theme(element=Style(padding=2))
    assert dict(merged["element"]) == {"height": 30.0, "padding": (2.0,)}
    with pytest.raises(TypeError, match="unknown theme element"):
        Theme(sidebar=Style(height=1))


def test_resolve_cascades_over_the_default():
    resolved = Theme(defaults=Style(text_color="#ff0000")).resolve()
    assert resolved["element-text"]["text_color"] == (255, 0, 0, 255)
    inherited = Theme(
        element=Style(text_color="#00ff00"), element_index=Style(text_color=inherit)
    ).resolve()
    assert inherited["element-index"]["text_color"] == (0, 255, 0, 255)


@pytest.mark.parametrize(
    "name,background",
    [
        ("solarized-dark", 0x002B36),
        ("solarized-light", 0xFDF6E3),
        ("nord", 0x2E3440),
        ("gruvbox-dark", 0x282828),
        ("catppuccin-mocha", 0x1E1E2E),
        ("dracula", 0x282A36),
        ("tokyo-night", 0x1A1B26),
        ("rose-pine", 0x191724),
        ("everforest", 0x2D353B),
        ("kanagawa", 0x1F1F28),
        ("mbra", 0x000000),
    ],
)
def test_shipped_palettes_keep_their_window_background(name, background):
    r, g, b, a = themes.get(name).resolve()["window"]["background_color"]
    assert (r << 16 | g << 8 | b, a) == (background, 255)


def test_palettes_by_attribute_and_name():
    assert themes.tokyo_night is themes.get("tokyo-night") is themes.get("tokyo_night")
    assert repr(themes.nord) == "<Theme nord>"
    with pytest.raises(KeyError, match="unknown theme"):
        themes.get("nope")
    with pytest.raises(AttributeError):
        themes.nope  # noqa: B018
