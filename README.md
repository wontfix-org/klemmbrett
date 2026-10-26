# klemmbrett

Keeps the Wayland clipboard and primary selection holding the same text. Named
popup widgets can provide values to copy, open, or run, including optional
SQLite clipboard history, text/JSON/YAML files, executables on `PATH`, and
Wayland windows to switch to.

A GPUI popup lists a configured widget. Entries longer than a configurable length are
shown shortened. A widget can pack either today's accelerator menu or a
filter: `layout: filter` adds an input bar and types into a case-insensitive
label filter; without that, the first 35 items can be chosen with accelerator keys
(`1`–`9`, then `a`–`z`). `Up`/`Down` and `Enter` also work, and `Escape`
closes the popup. A StatusNotifierItem tray icon has a Widgets submenu.

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
klemmbrett --config /path/to/config.hcl
```

Install a systemd user unit bound to `graphical-session.target` so the
daemon starts with the compositor and stops when the session ends:

```sh
klemmbrett --install-systemd
```

That writes `$XDG_CONFIG_HOME/systemd/user/klemmbrett.service` (normally
`~/.config/systemd/user/klemmbrett.service`) as a `Type=dbus` unit for
`org.wontfix.klemmbrett`, and a session D-Bus service file at
`$XDG_DATA_HOME/dbus-1/services/org.wontfix.klemmbrett.service`. The unit is
pulled in by `graphical-session.target` so history is recorded from
login. The D-Bus file is a fallback: a `Show` call still starts the
unit if the bus name is missing. If this process can see
`WAYLAND_DISPLAY`, that value is imported into the systemd user manager.
Pass `--config` as well to pin that file in `ExecStart`.

To open a configured widget without spelling out `busctl`:

```sh
klemmbrett show history
klemmbrett show history 120 80
```

`x` and `y` are compositor global coordinates. Omit both when there is no
pointer: the popup is then centered on the primary output, or the first
output if the compositor does not mark one. The running daemon (or D-Bus
activation) still serves the request.

## Configuration

The default file is `$XDG_CONFIG_HOME/klemmbrett/config.hcl` (normally
`~/.config/klemmbrett/config.hcl`). If it is missing, clipboard/primary
sync still runs, but no popup widgets or history database are created. A
leftover `config.toml` in that directory is rejected with a convert hint.

```hcl
configuration {
  label_max = 64
  launch    = "auto"
}

theme "nord" {}

widget "history" "clipboard-history" {
  layout   = "filter"
}

widget "snippets" "file" {
  path     = "snippets.yaml"
  format   = "yaml"
  action   = "xdg-open"
}

widget "run" "executables" {
  layout   = "filter"
}

widget "windows" "wayland-windows" {
  layout   = "filter"
}
```

Selecting a row runs that widget's transforms, then its action. `launch`
defaults to `auto` (`systemd-run --user --scope` when klemmbrett is a
systemd unit, otherwise a POSIX double-fork). Override with
`launch = "posix"`, `launch = "systemd-run"`, or a wrapper list.

Widgets, transforms, and actions are documented in
[`docs/widgets.md`](docs/widgets.md).

## Popup theme

`theme "nord" {}` sets a file-wide palette; `theme = "dracula"` on a widget
replaces it for that widget. `layout = "menu"` and `layout = "filter"` are
shipped. Catalog, lengths, colors, and packing are in
[`docs/theming.md`](docs/theming.md).

```hcl
theme "nord" {}

element "selected" { background_color = hex("5e81ac") }
```

Shipped palettes (`theme "nord" {}` or `theme = "nord"`):

| File | Palette |
| --- | --- |
| `themes/solarized-dark.hcl` | [Solarized](https://ethanschoonover.com/solarized/) dark (`base03` / `base0`) |
| `themes/solarized-light.hcl` | Solarized light (`base3` / `base00`) |
| `themes/nord.hcl` | [Nord](https://www.nordtheme.com/docs/colors-and-palettes) polar night |
| `themes/gruvbox-dark.hcl` | [Gruvbox](https://github.com/morhetz/gruvbox) dark, medium contrast |
| `themes/catppuccin-mocha.hcl` | [Catppuccin](https://github.com/catppuccin/palette) Mocha |
| `themes/dracula.hcl` | [Dracula](https://draculatheme.com) |
| `themes/tokyo-night.hcl` | [Tokyo Night](https://github.com/tokyo-night/tokyo-night-vscode-theme), rounded card |
| `themes/rose-pine.hcl` | [Rosé Pine](https://rosepinetheme.com), rounded card |
| `themes/everforest.hcl` | [Everforest](https://github.com/sainnhe/everforest) dark, rounded card |
| `themes/kanagawa.hcl` | [Kanagawa](https://github.com/rebelot/kanagawa.nvim) wave, rounded card |
| `themes/mbra.hcl` | Personal moss-on-black (qtile `#60845b`, Alacritty black) |

## D-Bus

The process owns `org.wontfix.klemmbrett` on the session bus:

| | |
| --- | --- |
| Name | `org.wontfix.klemmbrett` |
| Object | `/org/wontfix/klemmbrett` |
| Interface | `org.wontfix.klemmbrett` |
| Method | `Show(sa(ii))` — widget name, then zero or one `(x, y)` pair |

`x` and `y` are the pointer location in the compositor's global layout (the
same space qtile's `get_mouse_position()` and Sway output `rect` values use).
The popup is placed on the output that contains that point, flipping or
sliding so it stays on that output. An empty array means no pointer: the
primary output is used, and a `pointer` location is treated as `center`.

```sh
klemmbrett show history 120 80
```

That is the same call as:

```sh
busctl --user call \
    org.wontfix.klemmbrett \
    /org/wontfix/klemmbrett \
    org.wontfix.klemmbrett \
    Show sa(ii) history 1 120 80
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
subprocess.check_call(["klemmbrett", "show", "history", str(x), str(y)])
```

```sh
chmod +x ~/.config/sway/clip-history.py
```

In `~/.config/sway/config`:

```
bindsym $mod+v exec ~/.config/sway/clip-history.py
```

Without pointer coordinates the popup is centered on the primary output:

```
bindsym $mod+v exec klemmbrett show history
```

## Qtile key binding

Qtile's Wayland backend exposes the pointer directly. `lazy.spawn` can run
`klemmbrett show history`, or the example below uses
[dbus-fast](https://github.com/Bluetooth-Devices/dbus-fast) on Qtile's asyncio
loop (`libqtile.utils.create_task`) so the binding does not fork a helper.
Qtile already talks to D-Bus through dbus-fast (notifications, StatusNotifier,
and related widgets), so this matches the compositor's own stack.

```python
from dbus_fast import Message, MessageType
from dbus_fast.aio import MessageBus
from libqtile.config import Key
from libqtile.lazy import lazy
from libqtile.log_utils import logger
from libqtile.utils import create_task

_bus = None

async def _session_bus():
    global _bus
    if _bus is None or not _bus.connected:
        _bus = await MessageBus().connect()
    return _bus

async def _show_widget(widget, x, y):
    global _bus
    message = Message(
        destination="org.wontfix.klemmbrett",
        path="/org/wontfix/klemmbrett",
        interface="org.wontfix.klemmbrett",
        member="Show",
        signature="sa(ii)",
        body=[widget, [(int(x), int(y))]],
    )
    try:
        reply = await (await _session_bus()).call(message)
    except Exception:
        # The cached connection can go stale; drop it and try once more.
        _bus = None
        reply = await (await _session_bus()).call(message)
    if reply.message_type == MessageType.ERROR:
        logger.error(
            "Klemmbrett Show failed: %s %s",
            reply.error_name,
            reply.body,
        )

def show_clip_history(qtile):
    x, y = qtile.core.get_mouse_position()
    create_task(_show_widget("history", x, y))

keys = [
    Key(["mod4"], "v", lazy.function(show_clip_history)),
]
```

Autostart the daemon from the same config (`klemmbrett` in `spawn`) or from
your session so the bus name is already claimed when the binding runs.
