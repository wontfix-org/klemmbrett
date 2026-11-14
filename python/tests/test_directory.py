"""A directory tree shown as relative paths or as one folder at a time."""

from __future__ import annotations

import os

import pytest

import klemmbrett as kb
from klemmbrett import _registry, providers, ranking as rk


def _tree(root):
    (root / "sub" / "c").mkdir(parents=True)
    (root / "a.txt").write_text("a")
    (root / ".hidden").write_text("h")
    (root / "sub" / "b.txt").write_text("b")
    (root / "sub" / "c" / "d.txt").write_text("d")
    (root / "filelink").symlink_to("a.txt")
    (root / "link").symlink_to("sub", target_is_directory=True)
    return providers.Directory(root)


def test_flat_lists_files_by_relative_path_and_skips_real_directories(tmp_path):
    tree = _tree(tmp_path)
    records = tree.flat().records()
    assert [record.label for record in records] == [
        ".hidden",
        "a.txt",
        "filelink",
        "link",
        "sub/b.txt",
        "sub/c/d.txt",
    ]
    found = {record.label: record for record in records}
    assert found["sub/b.txt"].value == str(tmp_path / "sub" / "b.txt")
    assert found["sub/b.txt"].data == {"name": "b.txt", "relative": "sub/b.txt"}
    assert "children" not in found["sub/b.txt"].data
    assert providers.defaults(tree.flat()) == (rk.fuzzy(), None)


def test_browse_steps_into_a_directory_and_back(tmp_path, daemon):
    tree = _tree(tmp_path)
    kb.widget("browse", tree.browse())
    root = {record.label: record for record in tree.browse().records()}
    assert list(root) == ["sub", ".hidden", "a.txt", "filelink", "link"]
    assert ".." not in root
    assert root["sub"].data == {"name": "sub", "relative": "sub"}

    _registry.wire()["dispatcher"].select("browse", [root["sub"]._wire()])
    assert daemon.calls[-1][0] == "show"
    widget, rows, pointer = daemon.calls[-1][1]
    assert widget == "browse"
    assert pointer is None
    labels = [label for _key, label, _value, _data in rows]
    assert labels == ["..", "c", "b.txt"]

    parent = rows[0]
    daemon.calls.clear()
    _registry.wire()["dispatcher"].select("browse", [parent])
    _widget, back, _pointer = daemon.calls[-1][1]
    assert [label for _key, label, _value, _data in back] == [
        "sub",
        ".hidden",
        "a.txt",
        "filelink",
        "link",
    ]


def test_selecting_a_file_opens_it(tmp_path, daemon):
    tree = _tree(tmp_path)
    kb.widget("files", tree.flat())
    record = next(record for record in tree.flat().records() if record.label == "a.txt")
    _registry.wire()["dispatcher"].select("files", [record._wire()])
    assert daemon.calls == [("launch", ["xdg-open", str(tmp_path / "a.txt")])]

    kb.widget("browse", tree.browse())
    daemon.calls.clear()
    opened = next(record for record in tree.browse().records() if record.label == "a.txt")
    _registry.wire()["dispatcher"].select("browse", [opened._wire()])
    assert daemon.calls == [("launch", ["xdg-open", str(tmp_path / "a.txt")])]


def test_browse_without_a_widget_refuses_to_guess(tmp_path):
    record = next(record for record in _tree(tmp_path).browse().records() if record.label == "sub")
    with pytest.raises(RuntimeError, match="widget"):
        providers.Directory(tmp_path).browse().select(record)


def test_missing_and_non_directory_roots_are_rejected(tmp_path):
    missing = tmp_path / "missing"
    with pytest.raises(FileNotFoundError):
        providers.Directory(missing).flat().records()
    file = tmp_path / "a.txt"
    file.write_text("a")
    with pytest.raises(NotADirectoryError):
        providers.Directory(file).browse().records()


def test_an_unreadable_subdirectory_is_skipped(tmp_path):
    if os.geteuid() == 0:
        pytest.skip("root can read a mode 000 directory")
    blocked = tmp_path / "blocked"
    blocked.mkdir()
    (blocked / "secret.txt").write_text("x")
    (tmp_path / "visible.txt").write_text("x")
    blocked.chmod(0)
    try:
        labels = [record.label for record in providers.Directory(tmp_path).flat().records()]
    finally:
        blocked.chmod(0o755)
    assert labels == ["visible.txt"]


def test_a_relative_directory_is_resolved_beside_the_config(tmp_path, monkeypatch):
    notes = tmp_path / "notes"
    notes.mkdir()
    (notes / "one.txt").write_text("1")
    monkeypatch.setattr(providers, "_config_dir", tmp_path)
    records = providers.Directory("notes").flat().records()
    assert records[0].value == str(notes / "one.txt")
