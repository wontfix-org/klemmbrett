"""File previews pick a syntax from the file's type."""

from __future__ import annotations

import os

import pytest

import klemmbrett as kb
from klemmbrett.preview import file_preview, file_preview_pane, preview_file


def test_a_named_type_is_its_syntax_and_a_shebang_fills_in(tmp_path):
    sql = tmp_path / "q.sql"
    sql.write_text("select 1\n")
    found = file_preview(sql)
    assert found.text == "select 1\n"
    assert found.language == "SQL"
    tool = tmp_path / "tool"
    tool.write_text("#!/usr/bin/env python3\nprint(1)\n")
    assert "python" in file_preview(tool).language.lower()
    notes = tmp_path / "notes.txt"
    notes.write_text("hello\n")
    plain = file_preview(notes)
    assert plain.text == "hello\n"
    assert plain.language == ""


def test_binary_and_non_files_are_plain(tmp_path):
    binary = tmp_path / "a.bin"
    binary.write_bytes(b"\x00\x01\x02")
    found = file_preview(binary)
    assert found.language == ""
    assert found.text == "binary file (3 bytes)"
    missing = file_preview(tmp_path / "gone.sql")
    assert missing.language == ""
    assert missing.text.endswith("is not a file")
    assert "Traceback" not in missing.text


def test_a_read_error_is_the_traceback(tmp_path):
    secret = tmp_path / "secret.sql"
    secret.write_text("select 1")
    secret.chmod(0)
    if os.access(secret, os.R_OK):
        pytest.skip("this user can read a mode 000 file")
    found = file_preview(secret)
    assert found.language == ""
    assert "PermissionError" in found.text


def test_preview_file_keeps_the_record_and_the_pane_reads_it(tmp_path):
    path = tmp_path / "q.sql"
    path.write_text("select 1\n")
    record = kb.Record(str(path), label="q.sql", data={"name": "q.sql", "relative": "q.sql"})
    mapped = preview_file(record)
    assert mapped.label == "q.sql"
    assert mapped.data["name"] == "q.sql"
    assert mapped.data["relative"] == "q.sql"
    assert mapped.data["preview"] == "select 1\n"
    assert mapped.data["language"] == "SQL"
    assert file_preview(path).apply(record) == mapped
    child = file_preview_pane._wire()
    assert child["kind"] == "richtext"
    assert child["text"] == "{preview}"
    assert child["language"] == "{language}"
