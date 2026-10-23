# klemmbrett

Keeps the Wayland clipboard and primary selection holding the same text. Named
popup sources can provide values to copy to the clipboard, including optional
SQLite clipboard history and text, JSON, or YAML files.

A GPUI popup lists a configured source. Entries longer than a configurable length are
shown shortened. The first 35 items can be chosen with accelerator keys
(`1`–`9`, then `a`–`z`); `Up`/`Down` and `Enter` also work, and `Escape`
closes the popup. A StatusNotifierItem tray icon has a Sources submenu.

The popup takes keyboard focus as soon as it appears, without needing the
pointer. It is a `wlr-layer-shell` surface that requests exclusive keyboard
interactivity, which is the only way a Wayland surface opened from something
other than user input (here, a D-Bus call) can be focused: an `xdg_popup`
grab — what `GtkMenu` would use — requires the serial of a real input event,
which a D-Bus trigger does not have. Any compositor implementing
`wlr-layer-shell` works, including sway, Hyprland, river, niri, wayfire,
labwc, and qtile's Wayland backend. GNOME does not implement it.

## Build

Debian/Trixie packages needed in addition to a Rust toolchain (1.95 or newer):

```sh
sudo apt install \
    libxkbcommon-dev \
    libwayland-dev \
    libvulkan-dev \
    pkg-config
```

```sh
cargo build --release
```

## Run

```sh
klemmbrett
```

To use a different configuration file:

```sh
klemmbrett --config /path/to/config.toml
```

## Configuration

The default configuration is
`$XDG_CONFIG_HOME/klemmbrett/config.toml` (normally
`~/.config/klemmbrett/config.toml`). If it does not exist, clipboard/primary
selection syncing still runs, but no popup sources or history database are
initialized.

```toml
[popup]
label-max = 64

[[sources]]
name = "history"
type = "clipboard-history"
history-limit = 50
coalesce-growing = true

[[sources]]
name = "snippets"
type = "file"
path = "snippets.yaml"
format = "yaml"
```

Source names must be unique. Relative `db` and `path` values are resolved
relative to the configuration file. The history `db` defaults to
`$XDG_DATA_HOME/klemmbrett/history.sqlite`; `history-limit` defaults to 50,
and `coalesce-growing` defaults to true. Coalescing replaces the newest row
when a new selection contains it, avoiding one row per character while a
selection grows.

File sources are read again whenever their popup opens. `text` format uses one
non-empty line per record. JSON and YAML files contain an array whose records
may be strings:

```json
["plain value", {"key": "url", "label": "Project website", "value": "https://example.com"}]
```

or the equivalent YAML:

```yaml
- plain value
- key: url
  label: Project website
  value: https://example.com
```

Every record requires a value. Its key and label default to the value when
omitted, and keys must be unique within a source. The popup displays the label
and copies the value.

## D-Bus

The process owns `org.klemmbrett` on the session bus:

| | |
| --- | --- |
| Name | `org.klemmbrett` |
| Object | `/org/klemmbrett/History` |
| Interface | `org.klemmbrett.History` |
| Method | `ShowMenu(sii)` — source name, then `x`, `y` in global compositor coordinates |

`x` and `y` are the pointer location in the compositor's global layout (the
same space qtile's `get_mouse_position()` and Sway output `rect` values use).
The popup is placed on the output that contains that point, flipping or
sliding so it stays on that output.

```sh
busctl --user call \
    org.klemmbrett \
    /org/klemmbrett/History \
    org.klemmbrett.History \
    ShowMenu sii history 120 80
```

## Sway key binding

Sway's stable IPC does not report the pointer. A small helper can still pass
coordinates: it tries `swaymsg -t get_cursor` (available on some builds), and
otherwise reads the pointer from a seat if the compositor includes it, falling
back to the focused output only if neither is present.

Put this next to your Sway config, for example as `~/.config/sway/clip-history.py`:

```python
#!/usr/bin/env python3
import json
import subprocess
import sys


def call(msg_type):
    return json.loads(subprocess.check_output(["swaymsg", "-t", msg_type]))


def coords():
    try:
        cursor = call("get_cursor")
        return int(cursor["x"]), int(cursor["y"])
    except Exception:
        pass

    try:
        for seat in call("get_seats"):
            cursor = seat.get("cursor") or {}
            if "x" in cursor and "y" in cursor:
                return int(cursor["x"]), int(cursor["y"])
    except Exception:
        pass

    # Last resort: the focused output's origin, so the menu still appears
    # on the screen the pointer is most likely on.
    for output in call("get_outputs"):
        if output.get("focused"):
            rect = output["rect"]
            return int(rect["x"]), int(rect["y"])

    sys.exit("could not determine pointer coordinates")


x, y = coords()
subprocess.check_call(
    [
        "busctl",
        "--user",
        "call",
        "org.klemmbrett",
        "/org/klemmbrett/History",
        "org.klemmbrett.History",
        "ShowMenu",
        "sii",
        "history",
        str(x),
        str(y),
    ]
)
```

```sh
chmod +x ~/.config/sway/clip-history.py
```

In `~/.config/sway/config`:

```
bindsym $mod+v exec ~/.config/sway/clip-history.py
```

## Qtile key binding

Qtile's Wayland backend exposes the pointer directly. The example uses
[dbus-fast](https://github.com/Bluetooth-Devices/dbus-fast) on Qtile's asyncio
loop (`libqtile.utils.create_task`) so the binding does not fork `busctl`.
Qtile already talks to D-Bus through dbus-fast (notifications, StatusNotifier,
and related widgets), so this matches the compositor's own stack.

```python
from dbus_fast import Message
from dbus_fast.aio import MessageBus
from libqtile.config import Key
from libqtile.lazy import lazy
from libqtile.utils import create_task

async def _show_source(source, x, y):
    bus = await MessageBus().connect()
    try:
        await bus.call(
            Message(
                destination="org.klemmbrett",
                path="/org/klemmbrett/History",
                interface="org.klemmbrett.History",
                member="ShowMenu",
                signature="sii",
                body=[source, int(x), int(y)],
            )
        )
    finally:
        bus.disconnect()

def show_clip_history(qtile):
    x, y = qtile.core.get_mouse_position()
    create_task(_show_source("history", x, y))

keys = [
    Key(["mod4"], "v", lazy.function(show_clip_history)),
]
```

Autostart the daemon from the same config (`klemmbrett` in `spawn`) or from
your session so the bus name is already claimed when the binding runs.
