# klemmbrett

Keeps the Wayland clipboard and primary selection holding the same text, and
remembers recent clips in an SQLite file so they can be pasted again.

A GTK popup lists the history. Entries longer than a configurable length are
shown shortened. The first 35 items can be chosen with accelerator keys
(`1`–`9`, then `a`–`z`); `Up`/`Down` and `Enter` also work, and `Escape`
closes the popup. An Ayatana AppIndicator entry in the system tray opens the
same list.

The popup takes keyboard focus as soon as it appears, without needing the
pointer. It is a `wlr-layer-shell` surface that requests exclusive keyboard
interactivity, which is the only way a Wayland surface opened from something
other than user input (here, a D-Bus call) can be focused: an `xdg_popup`
grab — what `GtkMenu` would use — requires the serial of a real input event,
which a D-Bus trigger does not have. Any compositor implementing
`wlr-layer-shell` works, including sway, Hyprland, river, niri, wayfire,
labwc, and qtile's Wayland backend. GNOME does not implement it.

## Build

Debian/Trixie packages needed in addition to a Rust toolchain:

```sh
sudo apt install \
    libgtk-3-dev \
    libgtk-layer-shell-dev \
    libayatana-appindicator3-dev \
    pkg-config
```

```sh
cargo build --release
```

## Run

```sh
klemmbrett
```

Optional flags:

| Flag | Default | Meaning |
| --- | --- | --- |
| `--history-limit` | `50` | Maximum rows kept in SQLite; older rows are deleted |
| `--label-max` | `64` | Maximum characters shown per menu item |
| `--db` | `$XDG_DATA_HOME/klemmbrett/history.sqlite` | History database path |

## D-Bus

The process owns `org.klemmbrett` on the session bus:

| | |
| --- | --- |
| Name | `org.klemmbrett` |
| Object | `/org/klemmbrett/History` |
| Interface | `org.klemmbrett.History` |
| Method | `ShowMenu(ii)` — `x`, `y` in global compositor coordinates |

`x` and `y` are the pointer location in the compositor's global layout (the
same space qtile's `get_mouse_position()` and Sway output `rect` values use).
The popup is placed on the output that contains that point.

```sh
busctl --user call \
    org.klemmbrett \
    /org/klemmbrett/History \
    org.klemmbrett.History \
    ShowMenu ii 120 80
```

## Sway key binding

Sway's stable IPC does not report the pointer. A small helper can still pass
coordinates: it tries `swaymsg -t get_cursor` (available on some builds), and
otherwise reads the pointer from a seat if the compositor includes it, falling
back to the focused output only if neither is present.

Put this next to your Sway config, for example as `~/.config/sway/clip-history.sh`:

```sh
#!/bin/sh
set -e
coords=$(python3 - <<'PY'
import json, subprocess, sys

def call(msg_type):
    return json.loads(subprocess.check_output(["swaymsg", "-t", msg_type]))

try:
    cursor = call("get_cursor")
    print(int(cursor["x"]), int(cursor["y"]))
    sys.exit(0)
except Exception:
    pass

try:
    for seat in call("get_seats"):
        cursor = seat.get("cursor") or {}
        if "x" in cursor and "y" in cursor:
            print(int(cursor["x"]), int(cursor["y"]))
            sys.exit(0)
except Exception:
    pass

# Last resort: the focused output's origin, so the menu still appears
# on the screen the pointer is most likely on.
for output in call("get_outputs"):
    if output.get("focused"):
        rect = output["rect"]
        print(int(rect["x"]), int(rect["y"]))
        break
PY
)
# shellcheck disable=SC2086
set -- $coords
exec busctl --user call \
    org.klemmbrett \
    /org/klemmbrett/History \
    org.klemmbrett.History \
    ShowMenu ii "$1" "$2"
```

```sh
chmod +x ~/.config/sway/clip-history.sh
```

In `~/.config/sway/config`:

```
bindsym $mod+v exec ~/.config/sway/clip-history.sh
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

async def _show_clip_history(x, y):
    bus = await MessageBus().connect()
    try:
        await bus.call(
            Message(
                destination="org.klemmbrett",
                path="/org/klemmbrett/History",
                interface="org.klemmbrett.History",
                member="ShowMenu",
                signature="ii",
                body=[int(x), int(y)],
            )
        )
    finally:
        bus.disconnect()

def show_clip_history(qtile):
    x, y = qtile.core.get_mouse_position()
    create_task(_show_clip_history(x, y))

keys = [
    Key(["mod4"], "v", lazy.function(show_clip_history)),
]
```

Autostart the daemon from the same config (`klemmbrett` in `spawn`) or from
your session so the bus name is already claimed when the binding runs.
