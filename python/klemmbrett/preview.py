"""Preview text for a file, with a syntax chosen from its type."""

from __future__ import annotations

import os
import traceback
from dataclasses import dataclass, replace
from pathlib import Path

from . import _core
from ._record import Record
from .ui import RichText

__all__ = ["FilePreview", "file_preview", "file_preview_pane", "preview_file"]

#: How many leading bytes are checked for a NUL before a file is called binary.
_PROBE = 8192

#: The preview pane for [`preview_file`](preview_file): `{preview}` and `{language}`.
file_preview_pane = RichText("{preview}", language="{language}")


@dataclass(frozen=True)
class FilePreview:
    """How to show one file. `language` is empty when the text is plain."""

    text: str
    language: str

    def apply(self, record: Record) -> Record:
        """`record` with this preview stored on `data`."""
        data = dict(record.data) if isinstance(record.data, dict) else {}
        data["preview"] = self.text
        data["language"] = self.language
        return replace(record, data=data)


def file_preview(path: str | os.PathLike[str]) -> FilePreview:
    """Reads `path` and probes its type.

    A text file comes back with the syntax known for its name or extension,
    or, when that misses, for its first line (a shebang or mode line). A
    file that is not text, or not a file, is described in plain text. A
    read error is the traceback, also plain.
    """
    file = Path(path)
    try:
        if not file.is_file():
            return FilePreview(f"{file} is not a file", "")
        data = file.read_bytes()
    except Exception:
        return FilePreview(traceback.format_exc(), "")
    if b"\x00" in data[:_PROBE]:
        return FilePreview(f"binary file ({len(data)} bytes)", "")
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return FilePreview(f"binary file ({len(data)} bytes)", "")
    language = _core.syntax_name(str(file), text.split("\n", 1)[0][:_PROBE]) or ""
    return FilePreview(text, language)


def preview_file(record: Record) -> Record:
    """`record` with the preview of the file named by its value."""
    return file_preview(record.value).apply(record)
