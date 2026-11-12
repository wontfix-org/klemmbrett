import dataclasses
import json
import textwrap

import pytest

import klemmbrett as kb
from klemmbrett import _registry, layouts, providers, themes, ui


def dispatcher():
    return _registry.wire()["dispatcher"]


def wire_widget(name):
    return next(w for w in _registry.wire()["widgets"] if w["name"] == name)


def test_records_default_key_and_label_and_are_frozen():
    record = kb.Record("v")
    assert (record.key, record.label) == ("v", "v")
    assert dataclasses.replace(record, value="w").value == "w"
    with pytest.raises(dataclasses.FrozenInstanceError):
        record.value = "x"  # type: ignore[misc]
    with pytest.raises(TypeError, match="string"):
        kb.Record(3)  # type: ignore[arg-type]


def test_decorator_sets_on_select(daemon):
    @kb.widget("links", providers.Static(["a"]))
    def links(record):
        kb.copy(record.value.upper())

    assert isinstance(links, kb.Widget)
    dispatcher().select("links", [("a", "a", "a", None)])
    assert daemon.calls == [("copy", "A")]


def test_default_select_prefers_the_provider_then_copies(daemon):
    kb.widget("run", providers.Executables())
    kb.widget("windows", providers.WaylandWindows())
    kb.widget("themes", providers.Themes())
    kb.widget("plain", providers.Static(["x"]))
    d = dispatcher()
    d.select("run", [("firefox", "firefox", "firefox", None)])
    d.select("windows", [("w1", "Title", "Title", {"title": "Title", "app_id": ""})])
    d.select("themes", [("nord", "nord", "nord", None)])
    d.select("themes", [("default", "default", "default", None)])
    d.select("plain", [("x", "x", "x", None)])
    assert daemon.calls == [
        ("launch", ["firefox"]),
        ("activate_window", "w1"),
        ("set_theme", themes.nord._wire()),
        ("set_theme", None),
        ("copy", "x"),
    ]


def test_class_providers_supply_select_and_clipboard_hooks(daemon):
    seen = []

    class Todo(kb.Provider):
        def records(self):
            return [{"value": "v", "label": "L"}]

        def select(self, record):
            seen.append(("select", record))

        def on_clipboard(self, text):
            seen.append(("clip", text))

    todo = Todo()
    kb.widget("a", todo)
    kb.widget("b", todo, on_select=lambda record: seen.append(("own", record.value)))
    config = _registry.wire()
    assert config["clipboard"] is True
    config["dispatcher"].select("a", [("v", "L", "v", None)])
    config["dispatcher"].select("b", [("v", "L", "v", None)])
    config["dispatcher"].clipboard("hello")
    assert seen == [("select", kb.Record("v", label="L")), ("own", "v"), ("clip", "hello")]
    fetch = wire_widget("a")["source"]
    assert fetch() == [("v", "L", "v", None)]


def test_function_providers_normalize_records():
    @kb.provider
    def bookmarks():
        return ["plain", kb.Record("https://x", label="X"), {"value": "v", "key": "k"}]

    kb.widget("marks", bookmarks)
    kb.widget("lambda", lambda: ["y"])
    assert wire_widget("marks")["source"]() == [
        ("plain", "plain", "plain", None),
        ("https://x", "X", "https://x", None),
        ("k", "v", "v", None),
    ]
    assert wire_widget("lambda")["source"]() == [("y", "y", "y", None)]
    with pytest.raises(TypeError, match="no value"):
        kb.widget("bad", lambda: [{"label": "x"}])
        wire_widget("bad")["source"]()


def test_multi_select_runs_in_marked_order_and_survives_errors(daemon, capsys):
    @kb.widget("pick", providers.Static(["a", "b", "c"]), multi=True)
    def pick(record):
        if record.value == "b":
            raise RuntimeError("boom")
        kb.copy(record.value)

    dispatcher().select("pick", [("c", "c", "c", None), ("b", "b", "b", None), ("a", "a", "a", None)])
    assert daemon.calls == [("copy", "c"), ("copy", "a")]
    err = capsys.readouterr().err
    assert "widget 'pick': on_select failed" in err
    assert "RuntimeError: boom" in err
    assert wire_widget("pick")["multi"] == ("tab", "shift-tab")


def test_highlight_handlers(daemon):
    palettes = kb.widget("themes", providers.Themes(), layout=layouts.filter)
    assert wire_widget("themes")["highlight"] is False

    @palettes.on_highlight
    def preview(record):
        kb.set_theme(record.value)

    assert preview is palettes
    assert wire_widget("themes")["highlight"] is True
    dispatcher().highlight("themes", ("dracula", "dracula", "dracula", None))
    assert daemon.calls == [("set_theme", themes.dracula._wire())]


def test_layouts_are_static_or_called_per_open():
    static = ui.Window(ui.ListView(ui.Element(ui.Text("{value}"))))
    kb.widget("static", layout=static)
    kb.widget("dynamic", layout=layouts.filter, multi=True)
    kb.widget("default")
    assert wire_widget("static")["layout"] == static._wire()
    assert wire_widget("default")["layout"] is None
    build = wire_widget("dynamic")["layout"]
    assert build("dynamic", True, False) == layouts.filter(layouts.LayoutContext("dynamic", True, False))._wire()

    kb.widget("broken", layout=lambda ctx: "nope")
    with pytest.raises(TypeError, match="not a ui.Window"):
        wire_widget("broken")["layout"]("broken", False, False)


def test_configure_validates_and_themes_by_name():
    kb.configure(label_max=10, launch=["uwsm", "app", "--"], theme="nord")
    config = _registry.wire()
    assert config["label_max"] == 10
    assert config["launch"] == ["uwsm", "app", "--"]
    assert config["theme"] == themes.nord._wire()
    with pytest.raises(ValueError, match="label_max"):
        kb.configure(label_max=0)
    with pytest.raises(ValueError, match="launch"):
        kb.configure(launch="nohup")
    with pytest.raises(KeyError, match="unknown theme"):
        kb.configure(theme="nope")
    with pytest.raises(KeyError, match="unknown theme"):
        kb.widget("w", theme="nope")


def test_duplicate_and_empty_names_are_rejected():
    kb.widget("a")
    with pytest.raises(ValueError, match="more than once"):
        kb.widget("a")
    with pytest.raises(ValueError, match="non-empty"):
        kb.widget(" ")


def test_runtime_operations_check_their_arguments(daemon):
    with pytest.raises(TypeError, match="list"):
        kb.launch("xdg-open https://x")
    with pytest.raises(TypeError, match="string"):
        kb.copy(3)  # type: ignore[arg-type]
    kb.show("links", ["a", {"value": "b", "label": "B"}], pointer=(1, 2))
    assert daemon.calls == [("show", ("links", [("a", "a", "a", None), ("b", "B", "b", None)], (1, 2)))]


def test_without_a_daemon_operations_raise():
    with pytest.raises(RuntimeError, match="not running"):
        kb.copy("x")


def test_load_runs_config_files_relative_to_their_directory(tmp_path):
    (tmp_path / "links.json").write_text(json.dumps([{"label": "Docs", "value": "https://docs"}]))
    (tmp_path / "helpers.py").write_text("PREFIX = '>'\n")
    config = tmp_path / "config.py"
    config.write_text(
        textwrap.dedent(
            """
            import klemmbrett as kb
            from klemmbrett import providers, layouts
            from helpers import PREFIX

            kb.configure(label_max=32)
            kb.widget("links", providers.File("links.json"), layout=layouts.filter)

            @kb.widget("echo", lambda: [PREFIX + "x"])
            def echo(record):
                kb.copy(record.value)
            """
        )
    )
    loaded = _registry.load(str(config))
    assert loaded["label_max"] == 32
    assert [w["name"] for w in loaded["widgets"]] == ["links", "echo"]
    links = loaded["widgets"][0]["source"]
    assert links.records() == [("https://docs", "Docs", "https://docs", None)]
    assert loaded["widgets"][1]["source"]() == [(">x", ">x", ">x", None)]

    empty = _registry.load(None)
    assert empty["widgets"] == []


def test_broken_files_fail_when_the_config_loads(tmp_path):
    (tmp_path / "bad.json").write_text("{not json")
    kb.widget("bad", providers.File(tmp_path / "bad.json"))
    with pytest.raises(ValueError, match="invalid JSON"):
        _registry.wire()


def test_cli_help_exits_zero(capfd):
    from klemmbrett.__main__ import main

    assert main(["--help"]) == 0
    assert "Usage: klemmbrett" in capfd.readouterr().out
    assert main(["show"]) == 2
