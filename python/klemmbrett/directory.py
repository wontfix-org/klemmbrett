"""One directory walk, projected as a flat file list or one folder at a time.

Symlinks are listed and not followed, so a link to a directory does not
duplicate that directory and cannot cycle. An unreadable subdirectory is
skipped. Hidden names are included.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path

from ._record import Record


@dataclass(frozen=True)
class _Entry:
    relative: str
    name: str
    parent: str
    is_dir: bool
    absolute: str


class Index:
    """Entries under one root. `""` is the root directory."""

    def __init__(self, root: Path, entries: tuple[_Entry, ...]) -> None:
        self.root = root
        self.entries = entries
        self.directories = {""}
        self.directories.update(entry.relative for entry in entries if entry.is_dir)

    def is_dir(self, relative: str) -> bool:
        return relative in self.directories

    def flat(self) -> list[Record]:
        """Every file. The label is the relative path."""
        files = [entry for entry in self.entries if not entry.is_dir]
        files.sort(key=lambda entry: (entry.relative.casefold(), entry.relative))
        return [_record(entry, label=entry.relative) for entry in files]

    def browse(self, relative: str = "") -> list[Record]:
        """The entries of one directory, with `..` when it is not the root."""
        if not self.is_dir(relative):
            raise FileNotFoundError(self.root / relative)
        rows: list[Record] = []
        if relative:
            parent = Path(relative).parent.as_posix()
            if parent == ".":
                parent = ""
            rows.append(
                Record(
                    key="..",
                    label="..",
                    value=str(self.root if parent == "" else self.root / parent),
                    data={"name": "..", "relative": parent},
                )
            )
        children = [entry for entry in self.entries if entry.parent == relative]
        children.sort(key=lambda entry: (not entry.is_dir, entry.name.casefold(), entry.name))
        rows.extend(_record(entry, label=entry.name) for entry in children)
        return rows


def scan(root: Path) -> Index:
    """Walks `root`. Raises if it is missing or not a directory."""
    if not root.exists():
        raise FileNotFoundError(root)
    if not root.is_dir():
        raise NotADirectoryError(root)
    root = root.resolve()
    entries: list[_Entry] = []
    for dirpath, dirnames, filenames in os.walk(root, followlinks=False, onerror=_skip):
        current = Path(dirpath)
        relative_dir = current.relative_to(root).as_posix()
        if relative_dir == ".":
            relative_dir = ""
        leaves = list(filenames)
        real_dirs: list[str] = []
        for name in dirnames:
            if (current / name).is_symlink():
                leaves.append(name)
            else:
                real_dirs.append(name)
        dirnames[:] = real_dirs
        for name in real_dirs:
            entries.append(_entry(root, relative_dir, name, is_dir=True))
        for name in leaves:
            entries.append(_entry(root, relative_dir, name, is_dir=False))
    return Index(root, tuple(entries))


def _entry(root: Path, parent: str, name: str, *, is_dir: bool) -> _Entry:
    relative = f"{parent}/{name}" if parent else name
    return _Entry(relative, name, parent, is_dir, str(root / relative))


def _record(entry: _Entry, *, label: str) -> Record:
    return Record(
        key=entry.relative,
        label=label,
        value=entry.absolute,
        data={"name": entry.name, "relative": entry.relative},
    )


def _skip(_error: OSError) -> None:
    """Leaves an unreadable subdirectory out of the index."""
