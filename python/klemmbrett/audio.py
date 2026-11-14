"""Audio sinks and sources from PulseAudio and PipeWire.

`pactl` lists the PulseAudio server. `wpctl` lists PipeWire. When the
PulseAudio server is PipeWire itself and `wpctl` returned devices, the
PulseAudio rows are the same graph and are left out. Picking a row makes
it the default sink or source.
"""

from __future__ import annotations

import json
import logging
import re
import subprocess
from collections.abc import Callable, Sequence
from typing import Any

from ._record import Record

logger = logging.getLogger("klemmbrett")

_HEADINGS = {"Audio", "Video", "Sinks", "Sources", "Devices", "Filters", "Streams", "Clients"}
_NODE = re.compile(r"^[│├└─\s]*(\*)?[│├└─\s]*(\d+)\.\s+(.+?)\s*$")


def devices(kind: str) -> list[Record]:
    """Sinks or sources that answered. `kind` is `sink` or `source`."""
    listed = "sinks" if kind == "sink" else "sources"
    return devices_from(
        kind,
        pulse_info=_capture(["pactl", "-f", "json", "info"]),
        pulse_list=_capture(["pactl", "-f", "json", "list", listed]),
        pipewire=_capture(["wpctl", "status"]),
        pipewire_names=_capture(["wpctl", "status", "-n"]),
    )


def devices_from(
    kind: str,
    *,
    pulse_info: str | None,
    pulse_list: str | None,
    pipewire: str | None,
    pipewire_names: str | None,
) -> list[Record]:
    """Rows for `kind` from the command output each tool printed."""
    info = _json_object(pulse_info)
    pulse = _pulse_devices(kind, info, _json_list(pulse_list))
    described = _wpctl_devices(kind, pipewire)
    names = {item["id"]: item["name"] for item in _wpctl_devices(kind, pipewire_names)}
    wire = [_with_name(item, names.get(item["id"], "")) for item in described]
    if wire and _pulse_is_pipewire(info):
        pulse = []
    both = bool(pulse) and bool(wire)
    rows = [_record_for("pulse", kind, item, item["name"], both) for item in pulse]
    rows.extend(_record_for("pipewire", kind, item, item["id"], both) for item in wire)
    rows.sort(key=lambda record: (not record.data["default"], record.label.casefold(), record.key))
    return rows


def activate(record: Record, run: Callable[..., Any]) -> None:
    """Makes `record` the default device. `run` waits for the command."""
    data = record.data if isinstance(record.data, dict) else {}
    backend = data.get("backend")
    kind = data.get("kind")
    if backend == "pipewire":
        run(["wpctl", "set-default", str(data.get("id") or record.value)])
        return
    if backend == "pulse" and kind in {"sink", "source"}:
        run(["pactl", f"set-default-{kind}", str(data.get("name") or record.value)])
        return
    raise RuntimeError(f"{record.key} is not an audio device")


def _capture(argv: Sequence[str]) -> str | None:
    try:
        result = subprocess.run(argv, check=True, capture_output=True, text=True, timeout=3)
    except (OSError, subprocess.SubprocessError):
        return None
    return result.stdout


def _json_object(text: str | None) -> dict[str, Any]:
    value = _load_json(text)
    return value if isinstance(value, dict) else {}


def _json_list(text: str | None) -> list[Any]:
    value = _load_json(text)
    return value if isinstance(value, list) else []


def _load_json(text: str | None) -> Any:
    if not text or not text.strip():
        return None
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        logger.warning("pactl returned invalid JSON")
        return None


def _pulse_is_pipewire(info: dict[str, Any]) -> bool:
    return "pipewire" in str(info.get("server_name") or "").casefold()


def _pulse_devices(kind: str, info: dict[str, Any], items: list[Any]) -> list[dict[str, Any]]:
    default = str(info.get(f"default_{kind}_name") or "")
    found = []
    for item in items:
        if not isinstance(item, dict):
            continue
        name = item.get("name")
        if not isinstance(name, str) or not name:
            continue
        description = item.get("description")
        found.append(
            {
                "name": name,
                "description": description if isinstance(description, str) and description else name,
                "default": name == default,
            }
        )
    return found


def _wpctl_devices(kind: str, text: str | None) -> list[dict[str, Any]]:
    if not text:
        return []
    want = "Sinks" if kind == "sink" else "Sources"
    media = ""
    section = ""
    found = []
    for line in text.splitlines():
        heading = _heading(line)
        if heading in {"Audio", "Video"}:
            media = heading
            section = ""
            continue
        if heading is not None:
            section = heading
            continue
        if media != "Audio" or section != want:
            continue
        node = _node(line)
        if node is not None:
            found.append(node)
    return found


def _heading(line: str) -> str | None:
    text = "".join(character for character in line if character not in "│├└─").strip()
    if text.endswith(":"):
        text = text[:-1].strip()
    if text in _HEADINGS:
        return text
    return None


def _node(line: str) -> dict[str, Any] | None:
    match = _NODE.match(line)
    if match is None:
        return None
    text = re.sub(r"\s*\[[^\]]*\]\s*$", "", match.group(3)).strip()
    if not text:
        return None
    return {"id": match.group(2), "name": text, "default": match.group(1) == "*"}


def _with_name(item: dict[str, Any], name: str) -> dict[str, Any]:
    description = item["name"]
    return {
        "id": item["id"],
        "name": name or item["id"],
        "description": description,
        "default": item["default"],
    }


def _record_for(
    backend: str,
    kind: str,
    item: dict[str, Any],
    identifier: str,
    both: bool,
) -> Record:
    return _record(
        backend,
        kind,
        item["name"],
        item["description"],
        item["default"],
        identifier,
        both,
    )


def _record(
    backend: str,
    kind: str,
    name: str,
    description: str,
    default: bool,
    identifier: str,
    both: bool,
) -> Record:
    label = description
    if both:
        label = f"{description} ({'PipeWire' if backend == 'pipewire' else 'PulseAudio'})"
    return Record(
        key=f"{backend}:{name}",
        label=label,
        value=name,
        data={
            "backend": backend,
            "kind": kind,
            "name": name,
            "description": description,
            "default": default,
            "id": identifier,
        },
    )
