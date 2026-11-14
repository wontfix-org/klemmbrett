"""Installed applications, from desktop files.

`$XDG_DATA_HOME/applications` is read first, then each directory in
`$XDG_DATA_DIRS` (default `/usr/local/share:/usr/share`). The same
desktop-file id later in that order is ignored. An id is the filename, or
`subdir-name.desktop` when the file sits in a subdirectory.

An entry is shown when `Type=Application`, it is not `Hidden` or
`NoDisplay`, `TryExec` exists when set, and `OnlyShowIn` / `NotShowIn`
agree with `$XDG_CURRENT_DESKTOP`. `Terminal=true` entries are shown only
when a terminal command is configured. Selecting activates a
`DBusActivatable` entry on the session bus and, if that fails, launches
`Exec`. Field codes that would insert a file or URL are dropped.
"""

from __future__ import annotations

import os
from collections.abc import Callable, Sequence
from pathlib import Path

from ._record import Record

def applications(
    *,
    home: Path,
    dirs: Sequence[Path],
    desktops: Sequence[str],
    locales: Sequence[str],
    path: str,
    terminal: Sequence[str] | None,
) -> list[Record]:
    """Desktop applications visible in this environment."""
    seen: set[str] = set()
    found: list[Record] = []
    for root in (home / "applications", *(directory / "applications" for directory in dirs)):
        for desktop_id, file in _desktop_files(root):
            if desktop_id in seen:
                continue
            seen.add(desktop_id)
            record = _entry(
                file,
                desktop_id,
                desktops=desktops,
                locales=locales,
                path=path,
                terminal=terminal,
            )
            if record is not None:
                found.append(record)
    found.sort(key=lambda record: (record.label.casefold(), record.key))
    return found


def launch(
    record: Record,
    *,
    terminal: Sequence[str] | None,
    activate: Callable[[str], bool],
    start: Callable[[Sequence[str]], None],
) -> None:
    """Activates `record` over D-Bus, or launches its `Exec` line."""
    data = record.data if isinstance(record.data, dict) else {}
    app_id = data.get("app_id")
    if isinstance(app_id, str) and app_id and activate(app_id):
        return
    argv = data.get("argv")
    command = [part for part in argv if isinstance(part, str)] if isinstance(argv, list) else []
    if not command:
        raise RuntimeError(f"{record.key} has nothing to launch")
    if data.get("terminal"):
        if not terminal:
            raise RuntimeError("terminal is not configured")
        command = [*terminal, *command]
    start(command)


def data_home() -> Path:
    raw = os.environ.get("XDG_DATA_HOME")
    if raw:
        return Path(raw)
    return Path.home() / ".local" / "share"


def data_dirs() -> list[Path]:
    raw = os.environ.get("XDG_DATA_DIRS") or "/usr/local/share:/usr/share"
    return [Path(part) for part in raw.split(os.pathsep) if part]


def current_desktops() -> list[str]:
    return [part for part in os.environ.get("XDG_CURRENT_DESKTOP", "").split(":") if part]


def message_locales() -> list[str]:
    """Locale names to try for translated desktop-file keys, most specific first."""
    raw = os.environ.get("LC_ALL") or os.environ.get("LC_MESSAGES") or os.environ.get("LANG") or "C"
    return locale_chain(raw.split(".", 1)[0])


def locale_chain(name: str) -> list[str]:
    """`lang_COUNTRY@MODIFIER`, then the shorter forms, for a locale name."""
    if name in ("", "C", "POSIX"):
        return []
    base, _, modifier = name.partition("@")
    language, separator, country = base.partition("_")
    chain: list[str] = []

    def add(value: str) -> None:
        if value and value not in chain:
            chain.append(value)

    if separator and modifier:
        add(f"{language}_{country}@{modifier}")
    if separator:
        add(f"{language}_{country}")
    if modifier:
        add(f"{language}@{modifier}")
    add(language)
    return chain


def _desktop_files(root: Path) -> list[tuple[str, Path]]:
    found: list[tuple[str, Path]] = []
    if not root.is_dir():
        return found
    for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
        dirnames.sort()
        for name in sorted(filenames):
            if not name.endswith(".desktop"):
                continue
            file = Path(dirpath) / name
            if not file.is_file():
                continue
            relative = file.relative_to(root).as_posix()
            found.append((relative.replace("/", "-"), file))
    return found


def _entry(
    file: Path,
    desktop_id: str,
    *,
    desktops: Sequence[str],
    locales: Sequence[str],
    path: str,
    terminal: Sequence[str] | None,
) -> Record | None:
    try:
        text = file.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None
    entry = _parse(text)
    if entry is None or entry.get("Type") != "Application":
        return None
    if _flag(entry, "Hidden") or _flag(entry, "NoDisplay"):
        return None
    if not _visible_here(entry, desktops):
        return None
    try_exec = entry.get("TryExec", "")
    if try_exec and not _executable(try_exec, path):
        return None
    in_terminal = _flag(entry, "Terminal")
    if in_terminal and not terminal:
        return None
    name = _localized(entry, "Name", locales)
    if not name:
        return None
    argv = _exec_argv(
        entry.get("Exec", ""),
        name=name,
        icon=entry.get("Icon") or None,
        location=str(file),
    )
    app_id = _bus_name(desktop_id) if _flag(entry, "DBusActivatable") else None
    if not argv and app_id is None:
        return None
    data: dict[str, object] = {}
    if argv:
        data["argv"] = argv
    if app_id is not None:
        data["app_id"] = app_id
    if in_terminal:
        data["terminal"] = True
    for key, field in (("generic_name", "GenericName"), ("comment", "Comment")):
        value = _localized(entry, field, locales)
        if value:
            data[key] = value
    keywords = _localized_list(entry, "Keywords", locales)
    if keywords:
        data["keywords"] = keywords
    return Record(
        key=desktop_id,
        label=name,
        value=argv[0] if argv else app_id or desktop_id,
        data=data or None,
    )


def _parse(text: str) -> dict[str, str] | None:
    """Keys of the `Desktop Entry` group. Later groups are actions and are skipped."""
    group: str | None = None
    entry: dict[str, str] = {}
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("[") and line.endswith("]"):
            group = line[1:-1].strip()
            continue
        if group != "Desktop Entry" or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        if key:
            entry[key] = value.strip()
    return entry or None


def _localized(entry: dict[str, str], key: str, locales: Sequence[str]) -> str:
    for locale in locales:
        value = entry.get(f"{key}[{locale}]")
        if value:
            return _unescape(value)
    value = entry.get(key, "")
    return _unescape(value) if value else ""


def _localized_list(entry: dict[str, str], key: str, locales: Sequence[str]) -> list[str]:
    for locale in locales:
        raw = entry.get(f"{key}[{locale}]")
        if raw is not None:
            return _split_list(raw)
    raw = entry.get(key)
    return _split_list(raw) if raw else []


def _flag(entry: dict[str, str], key: str) -> bool:
    return entry.get(key, "").lower() == "true"


def _visible_here(entry: dict[str, str], desktops: Sequence[str]) -> bool:
    only = _split_list(entry["OnlyShowIn"]) if "OnlyShowIn" in entry else []
    if only and not any(name in only for name in desktops):
        return False
    hidden = _split_list(entry["NotShowIn"]) if "NotShowIn" in entry else []
    return not any(name in hidden for name in desktops)


def _executable(command: str, path: str) -> bool:
    candidate = Path(command)
    if candidate.is_absolute():
        return candidate.is_file() and os.access(candidate, os.X_OK)
    for directory in path.split(os.pathsep):
        if directory and _executable(str(Path(directory) / command), path=""):
            return True
    return False


def _bus_name(desktop_id: str) -> str | None:
    """The session-bus name for a desktop-file id, when it is one."""
    name = desktop_id.removesuffix(".desktop")
    if len(name) > 255 or "." not in name:
        return None
    for element in name.split("."):
        if not element or element[0].isdigit():
            return None
        if any(
            not (character.isascii() and (character.isalnum() or character == "_"))
            for character in element
        ):
            return None
    return name


def _unescape(value: str) -> str:
    return "".join(_split_list_chars(value, separator=None))


def _split_list(value: str) -> list[str]:
    return [item for item in _split_list_chars(value, separator=";") if item]


def _split_list_chars(value: str, *, separator: str | None) -> list[str]:
    items: list[str] = []
    buf: list[str] = []
    index = 0
    escapes = {"s": " ", "n": "\n", "t": "\t", "r": "\r", "\\": "\\"}
    while index < len(value):
        character = value[index]
        if character == "\\" and index + 1 < len(value):
            buf.append(escapes.get(value[index + 1], value[index + 1]))
            index += 2
            continue
        if separator is not None and character == separator:
            items.append("".join(buf))
            buf = []
            index += 1
            continue
        buf.append(character)
        index += 1
    items.append("".join(buf))
    return items


def _exec_argv(command: str, *, name: str, icon: str | None, location: str) -> list[str]:
    argv: list[str] = []
    for argument in _split_exec(command):
        argv.extend(_expand_code(argument, name=name, icon=icon, location=location))
    return argv


def _split_exec(command: str) -> list[str]:
    """Splits an `Exec` value. Double quotes group one argument; `\\` escapes."""
    arguments: list[str] = []
    buf: list[str] = []
    quoted = False
    index = 0
    while index < len(command):
        character = command[index]
        if character == '"':
            quoted = not quoted
            index += 1
            continue
        if character == "\\" and index + 1 < len(command):
            nxt = command[index + 1]
            if quoted and nxt == "\n":
                index += 2
                continue
            if quoted and nxt not in '"$`\\':
                buf.append(character)
                index += 1
                continue
            buf.append(nxt)
            index += 2
            continue
        if not quoted and character in " \t\n\r":
            if buf:
                arguments.append("".join(buf))
                buf = []
            index += 1
            continue
        buf.append(character)
        index += 1
    if buf:
        arguments.append("".join(buf))
    return arguments


_FILE_CODES = set("fFuUdDnNvVm")


def _expand_code(argument: str, *, name: str, icon: str | None, location: str) -> list[str]:
    if argument == "%i":
        return ["--icon", icon] if icon else []
    if argument == "%c":
        return [name]
    if argument == "%k":
        return [location]
    if argument == "%%":
        return ["%"]
    if len(argument) == 2 and argument[0] == "%" and argument[1] in _FILE_CODES:
        return []
    out: list[str] = []
    index = 0
    while index < len(argument):
        if argument[index] == "%" and index + 1 < len(argument):
            code = argument[index + 1]
            index += 2
            if code == "%":
                out.append("%")
            elif code == "c":
                out.append(name)
            elif code == "k":
                out.append(location)
            elif code == "i" and icon:
                out.append(icon)
            elif code in _FILE_CODES:
                continue
            continue
        out.append(argument[index])
        index += 1
    text = "".join(out)
    return [text] if text else []
