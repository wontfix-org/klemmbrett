import pytest

from klemmbrett import _core, layouts, themes, ui
from klemmbrett.layouts import LayoutContext


def size(window, rows, theme=None, label_max=16):
    wire = None if theme is None else theme._wire()
    return _core.layout_size(window._wire(), rows, wire, label_max)


def ctx(multi=False, prompt=False):
    return LayoutContext(widget="w", multi=multi, prompt=prompt)


def test_menu_keeps_the_historic_card_size():
    assert size(layouts.menu(ctx()), 1) == (200.0, 30.0)


def test_filter_and_prompt():
    filter_ = layouts.filter(ctx())
    assert filter_.location == "center"
    _, prompt_height = size(layouts.prompt(ctx()), 1)
    assert size(layouts.prompt(ctx()), 20)[1] == prompt_height
    assert size(filter_, 5)[1] > size(layouts.menu(ctx()), 5)[1]


def test_multi_adds_a_marker():
    assert size(layouts.menu(ctx(multi=True)), 1)[0] > size(layouts.menu(ctx()), 1)[0]


def test_themes_change_the_size():
    assert size(layouts.menu(ctx()), 3, themes.tokyo_night) != size(layouts.menu(ctx()), 3)


def test_custom_trees_validate():
    card = ui.Window(
        ui.InputBar(ui.Prompt(), ui.Entry()),
        ui.ListView(ui.Element(ui.Marker(), ui.Text("{label}"), ui.Spacer(), ui.Text("{key}", text_color="gray"))),
        ui.Text("{count} rows"),
        location="pointer",
        padding=4,
    )
    width, height = size(card, 2)
    assert width > 0 and height > 0
    assert card._wire()["style"] == {"padding": [4.0]}


@pytest.mark.parametrize(
    "window,message",
    [
        (ui.Window(ui.Element()), "window cannot contain element"),
        (ui.Window(ui.ListView(ui.Element()), ui.ListView(ui.Element())), "only one listview"),
        (ui.Window(ui.Row(ui.InputBar())), "inputbar"),
        (ui.Window(ui.ListView()), "element"),
    ],
)
def test_misplaced_nodes_are_rejected(window, message):
    with pytest.raises(ValueError, match=message):
        size(window, 1)


def test_leaves_take_no_children():
    with pytest.raises(TypeError, match="no children"):
        ui.Index(ui.Text("x"))
    with pytest.raises(TypeError, match="template"):
        ui.Text(3)


def test_children_keyword():
    assert ui.Window(children=[ui.ListView(ui.Element())]) == ui.Window(ui.ListView(ui.Element()))
