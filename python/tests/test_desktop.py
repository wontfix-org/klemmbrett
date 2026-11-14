"""Desktop-file application discovery."""

from __future__ import annotations

import stat

import pytest

import klemmbrett as kb
from klemmbrett import _runtime, desktop, providers
from klemmbrett._record import Record


def _write(root, relative, text) -> None:
    path = root / "applications" / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def _app(**kwargs):
    return desktop.applications(dirs=[], locales=[], desktops=[], path="", terminal=None, **kwargs)


def test_locale_chain_walks_from_the_specific_name():
    assert desktop.locale_chain("en_GB@euro") == ["en_GB@euro", "en_GB", "en@euro", "en"]
    assert desktop.locale_chain("C") == []


def test_entries_are_filtered_and_the_first_id_wins(tmp_path):
    home = tmp_path / "home"
    system = tmp_path / "system"
    _write(
        home,
        "browser.desktop",
        """
        [Desktop Entry]
        Type=Application
        Name=Local Browser
        Name[en_GB]=British Browser
        GenericName=Web
        Comment=Browse
        Keywords=web;net;
        Exec=firefox %u
        Icon=web
        """,
    )
    _write(
        system,
        "browser.desktop",
        """
        [Desktop Entry]
        Type=Application
        Name=System Browser
        Exec=system-firefox
        """,
    )
    _write(
        system,
        "tools/htop.desktop",
        """
        [Desktop Entry]
        Type=Application
        Name=htop
        Exec=htop
        Terminal=true
        DBusActivatable=true
        """,
    )
    _write(
        system,
        "hidden.desktop",
        """
        [Desktop Entry]
        Type=Application
        Name=Hidden
        Exec=hidden
        Hidden=true
        """,
    )
    _write(
        system,
        "quiet.desktop",
        """
        [Desktop Entry]
        Type=Application
        Name=Quiet
        Exec=quiet
        NoDisplay=true
        """,
    )
    _write(
        system,
        "link.desktop",
        """
        [Desktop Entry]
        Type=Link
        Name=Link
        URL=https://example.com
        """,
    )
    _write(
        system,
        "kde-only.desktop",
        """
        [Desktop Entry]
        Type=Application
        Name=KDE
        Exec=kde
        OnlyShowIn=KDE;
        """,
    )
    _write(
        system,
        "not-here.desktop",
        """
        [Desktop Entry]
        Type=Application
        Name=Elsewhere
        Exec=elsewhere
        NotShowIn=sway;
        """,
    )
    missing = system / "missing-bin"
    _write(
        system,
        "missing.desktop",
        f"""
        [Desktop Entry]
        Type=Application
        Name=Missing
        Exec={missing}
        TryExec={missing}
        """,
    )
    binary = tmp_path / "bin" / "present"
    binary.parent.mkdir()
    binary.write_text("#!/bin/sh\n")
    binary.chmod(binary.stat().st_mode | stat.S_IXUSR)
    _write(
        system,
        "present.desktop",
        """
        [Desktop Entry]
        Type=Application
        Name=Present
        Exec=present --title=%c "two words"
        TryExec=present
        Icon=box
        Keywords=a\\;b;c;
        """,
    )
    _write(
        system,
        "org.example.App.desktop",
        """
        [Desktop Entry]
        Type=Application
        Name=Example
        Exec=example %i
        Icon=example
        DBusActivatable=true
        """,
    )

    records = {
        record.key: record
        for record in desktop.applications(
            home=home,
            dirs=[system],
            desktops=["sway"],
            locales=["en_GB"],
            path=str(tmp_path / "bin"),
            terminal=["foot", "-e"],
        )
    }
    assert records["browser.desktop"].label == "British Browser"
    assert records["browser.desktop"].data["generic_name"] == "Web"
    assert records["browser.desktop"].data["keywords"] == ["web", "net"]
    assert records["browser.desktop"].data["argv"] == ["firefox"]
    assert "hidden.desktop" not in records
    assert "quiet.desktop" not in records
    assert "link.desktop" not in records
    assert "kde-only.desktop" not in records
    assert "not-here.desktop" not in records
    assert "missing.desktop" not in records
    assert records["tools-htop.desktop"].data["terminal"] is True
    assert "app_id" not in records["tools-htop.desktop"].data
    assert records["present.desktop"].data["argv"] == ["present", "--title=Present", "two words"]
    assert records["present.desktop"].data["keywords"] == ["a;b", "c"]
    assert records["org.example.App.desktop"].data["argv"] == ["example", "--icon", "example"]
    assert records["org.example.App.desktop"].data["app_id"] == "org.example.App"
    assert [record.label for record in desktop.applications(
        home=home,
        dirs=[system],
        desktops=["sway"],
        locales=["en_GB"],
        path=str(tmp_path / "bin"),
        terminal=["foot", "-e"],
    )] == ["British Browser", "Example", "htop", "Present"]


def test_terminal_entries_need_a_configured_terminal(tmp_path):
    home = tmp_path / "home"
    _write(
        home,
        "htop.desktop",
        """
        [Desktop Entry]
        Type=Application
        Name=htop
        Exec=htop
        Terminal=true
        """,
    )
    assert _app(home=home) == []
    found = desktop.applications(
        home=home, dirs=[], desktops=[], locales=[], path="", terminal=["foot", "-e"]
    )
    assert found[0].data["terminal"] is True


def test_launch_prefers_dbus_then_exec_and_wraps_a_terminal():
    record = Record(
        key="org.example.App.desktop",
        label="Example",
        value="example",
        data={"argv": ["example"], "app_id": "org.example.App"},
    )
    started: list[list[str]] = []
    assert desktop.launch(record, terminal=None, activate=lambda _app: True, start=started.append) is None
    assert started == []
    desktop.launch(record, terminal=None, activate=lambda _app: False, start=started.append)
    assert started == [["example"]]

    terminal = Record(
        key="htop.desktop",
        label="htop",
        value="htop",
        data={"argv": ["htop"], "terminal": True},
    )
    started.clear()
    desktop.launch(terminal, terminal=["foot", "-e"], activate=lambda _app: False, start=started.append)
    assert started == [["foot", "-e", "htop"]]


def test_launch_rejects_a_terminal_entry_without_a_terminal():
    record = Record("htop", data={"argv": ["htop"], "terminal": True})
    with pytest.raises(RuntimeError, match="terminal"):
        desktop.launch(record, terminal=None, activate=lambda _app: False, start=lambda _argv: None)


def test_applications_select_uses_the_configured_terminal(monkeypatch):
    started = []
    monkeypatch.setattr(_runtime, "activate_desktop", lambda _app_id: False)
    monkeypatch.setattr(_runtime, "launch", lambda argv: started.append(list(argv)))
    kb.configure(terminal=["foot", "-e"])
    providers.Applications().select(Record("htop", data={"argv": ["htop"], "terminal": True}))
    assert started == [["foot", "-e", "htop"]]


def test_activate_desktop_rejects_an_id_that_is_not_a_bus_name():
    assert _runtime.activate_desktop("htop") is False


def test_exec_field_codes_and_quotes():
    assert desktop._exec_argv('app "a b" %F %%', name="App", icon=None, location="/a.desktop") == [
        "app",
        "a b",
        "%",
    ]
    assert desktop._exec_argv(r"app\ name", name="App", icon=None, location="/a.desktop") == ["app name"]
