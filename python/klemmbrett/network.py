"""NetworkManager connection profiles.

``nmcli connection show`` lists saved profiles. Picking one turns an active
profile off and an inactive one on. ``nmcli`` escapes ``:`` as ``\\:`` and
``\\`` as ``\\\\`` in terse rows, and prints ``yes`` or ``no`` for
``ACTIVE`` when the locale is C.
"""

from __future__ import annotations

import logging
import os
import subprocess
from collections.abc import Callable
from typing import Any

from ._record import Record

logger = logging.getLogger("klemmbrett")

# nmcli TYPE values that are awkward to read. Others are shown as printed,
# for example `vpn`, `wireguard`, `tun`, `bridge`, `vlan`, `loopback`.
_KINDS = {
    "802-11-wireless": "wifi",
    "802-3-ethernet": "ethernet",
    "802-11-olpc-mesh": "mesh",
    "gsm": "mobile",
    "cdma": "mobile",
}

_LIST = [
    "nmcli",
    "-c",
    "no",
    "-t",
    "-e",
    "yes",
    "-f",
    "NAME,UUID,TYPE,DEVICE,ACTIVE",
    "connection",
    "show",
]


def connections() -> list[Record]:
    """Profiles ``nmcli`` listed. An absent or failing ``nmcli`` gives none."""
    return connections_from(_capture())


def connections_from(text: str | None) -> list[Record]:
    """Rows from ``nmcli -c no -t -e yes -f NAME,UUID,TYPE,DEVICE,ACTIVE connection show``.

    Accepted lines, as ``nmcli`` prints them::

        Wired connection 1:<uuid>:802-3-ethernet:enp0s3:yes
        Cafe\\: guest:<uuid>:802-11-wireless:--:no
        Work:<uuid>:vpn::no
        odd\\\\net:<uuid>:wireguard:wg0:yes

    The second name is ``Cafe: guest`` and the fourth is ``odd\\net``.
    ``--`` and an empty device both mean no device. ``yes`` means on.
    """
    if not text or not text.strip():
        return []
    rows = []
    seen: set[str] = set()
    for line in text.splitlines():
        if not line.strip():
            continue
        record = _record(line)
        if record is None:
            continue
        if record.key in seen:
            logger.warning("nmcli listed connection %s twice", record.key)
            continue
        seen.add(record.key)
        rows.append(record)
    rows.sort(
        key=lambda record: (
            not record.data["active"],
            record.data["name"].casefold(),
            record.key,
        )
    )
    return rows


def toggle(record: Record, run: Callable[..., Any]) -> None:
    """Turns an active connection off and an inactive one on.

    ``run`` waits for ``nmcli``. The uuid is ``data["uuid"]``, or the record
    key when that is missing. ``data["active"]`` must be ``True`` to turn a
    connection off; anything else brings it up.
    """
    data = record.data if isinstance(record.data, dict) else {}
    uuid = data.get("uuid")
    if not isinstance(uuid, str) or not uuid:
        uuid = record.key
    if not uuid:
        raise RuntimeError(f"{record.value!r} is not a NetworkManager connection")
    action = "down" if data.get("active") is True else "up"
    run(["nmcli", "connection", action, "uuid", uuid])


def _capture() -> str | None:
    env = os.environ.copy()
    # nmcli translates "yes" and "no". C keeps ACTIVE stable. LANGUAGE
    # overrides LC_ALL for gettext, so both are set.
    env["LC_ALL"] = "C"
    env["LANG"] = "C"
    env["LANGUAGE"] = "C"
    try:
        result = subprocess.run(
            _LIST,
            check=True,
            capture_output=True,
            text=True,
            timeout=3,
            env=env,
        )
    except OSError:
        return None
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or "").strip()
        logger.warning("nmcli could not list connections: %s", detail or error.returncode)
        return None
    except subprocess.TimeoutExpired:
        logger.warning("nmcli timed out listing connections")
        return None
    return result.stdout


def _record(line: str) -> Record | None:
    fields = _split_terse(line)
    if len(fields) != 5:
        logger.warning("nmcli connection row has %s fields: %s", len(fields), line)
        return None
    name, uuid, nm_type, device, active_text = fields
    if not name or not uuid:
        logger.warning("nmcli connection row has no name or uuid: %s", line)
        return None
    if device == "--":
        device = ""
    active = active_text.casefold() == "yes"
    kind = _KINDS.get(nm_type, nm_type)
    return Record(
        key=uuid,
        label=_label(name, kind, active),
        value=name,
        data={
            "name": name,
            "uuid": uuid,
            "type": nm_type,
            "kind": kind,
            "device": device,
            "active": active,
        },
    )


def _label(name: str, kind: str, active: bool) -> str:
    state = "on" if active else "off"
    if kind:
        return f"{name} ({kind}, {state})"
    return f"{name} ({state})"


def _split_terse(line: str) -> list[str]:
    """Splits one terse row on colons that are not escaped.

    ``Cafe\\: guest:<uuid>:802-11-wireless:--:no`` has the name ``Cafe: guest``.
    ``odd\\\\net:<uuid>:wireguard:wg0:yes`` has the name ``odd\\net``.
    """
    fields: list[str] = []
    current: list[str] = []
    escaped = False
    for character in line:
        if escaped:
            current.append(character)
            escaped = False
            continue
        if character == "\\":
            escaped = True
            continue
        if character == ":":
            fields.append("".join(current))
            current = []
            continue
        current.append(character)
    if escaped:
        current.append("\\")
    fields.append("".join(current))
    return fields
