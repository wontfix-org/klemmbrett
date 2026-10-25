# klemmbrett

Keeps the Wayland clipboard and primary selection holding the same text. Named
popup sources can provide values to copy to the clipboard, including optional
SQLite clipboard history and text, JSON, or YAML files.

A GPUI popup lists a configured source. Entries longer than a configurable length are
shown shortened. A source can pack either today's accelerator menu or a
filter: `widget: filter` adds an input bar and types into a case-insensitive
label filter; without that, the first 35 items can be chosen with accelerator keys
(`1`–`9`, then `a`–`z`). `Up`/`Down` and `Enter` also work, and `Escape`
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
login. The D-Bus file is a fallback: a `ShowMenu` call still starts the
unit if the bus name is missing. If this process can see
`WAYLAND_DISPLAY`, that value is imported into the systemd user manager.
Pass `--config` as well to pin that file in `ExecStart`.

## Configuration

The default configuration is
`$XDG_CONFIG_HOME/klemmbrett/config.hcl` (normally
`~/.config/klemmbrett/config.hcl`). If it does not exist, clipboard/primary
selection syncing still runs, but no popup sources or history database are
initialized. A leftover `config.toml` in that directory is
rejected with a convert hint.

```hcl
configuration {
  label_max = 64
}

theme "nord" {}

widget "filter" {
  location = "center"
  window { children = ["inputbar", "listview"] }
  inputbar {
    children = ["prompt", "entry"]
    padding  = 8
    spacing  = 8
  }
}

source "history" {
  type                   = "clipboard-history"
  history_limit          = 50
  coalesce_growing       = true
  skip_whitespace_only   = true
  widget                 = "filter"
  action                 = "copy-to-clipboard"
}

source "snippets" {
  type   = "file"
  path   = "snippets.yaml"
  format = "yaml"
  widget = "menu"
  theme  = "dracula"
  action = "xdg-open"
}
```

Source names must be unique. Relative `db` and `path` values are resolved
relative to the configuration file. The history `db` defaults to
`$XDG_DATA_HOME/klemmbrett/history.sqlite`; `history_limit` defaults to 50,
`coalesce_growing` defaults to true, `skip_whitespace_only` defaults to
true, and `action` defaults to `copy-to-clipboard`. Coalescing replaces the newest row when a new selection contains it,
avoiding one row per character while a selection grows. Whitespace-only
selections (spaces, tabs, newlines) are not stored unless
`skip_whitespace_only` is set to false.

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
omitted, and keys must be unique within a source. The popup displays the label.
Selecting a row runs that source's `action` on the value. `copy-to-clipboard`
(the default) copies it to the clipboard. `xdg-open` launches the desktop
handler for the value (a URL, file, or anything else `xdg-open` accepts).

## Popup theme and widgets

A full catalog and property reference is in [`docs/theming.md`](docs/theming.md).

The same `config.hcl` styles the popup. `theme "nord" {}` is a file-wide
default palette; `theme = "dracula"` on a source replaces it for that source
only. Named `widget` blocks pack the tree and set `location` (`pointer`,
`center`, `north east`). `widget = "menu"` and `widget = "filter"` are
shipped. A source with no `widget` keeps the list-only card at the pointer.

If `window` packs an `inputbar`, typing filters labels and accelerators are
off. Otherwise `1`–`9` / `a`–`z` still pick a row.

```hcl
theme "nord" {}

element "selected" { background_color = hex("5e81ac") }
element { children = ["element-text", "element-index"] }
```

Catalog: `overlay`, `window`, `listview`, `element`, `element-index`,
`element-text`, `inputbar`, `prompt`, `entry`. `element` accepts the states
`selected` and `hover` as a label. `window` may pack `inputbar` and
`listview`; `inputbar` may pack `prompt` and `entry`; `element` may pack
`element-index` and `element-text` (order and `orientation` are honored).
`enabled = false` hides a widget.

Properties: `enabled`, `padding`, `margin`, `border`, `border_radius`,
`border_color`, `background_color`, `background_image`, `text_color`,
`spacing`, `orientation`, `children`, `width`, `height`, `max_height`,
`font_size`, `location`. Lengths are numbers (pixels) or `px()`, `em()`,
`ch()`, `mm()`, `pct()` / `vpct()` (of a 1920×1080 monitor at load). HCL
`+` / `-` / `*` / `/` replace `calc()`. Padding/border/margin take a number
or a list of 1–4 numbers. Colors are `hex()`, `rgb[a]()`, `hsl[a]()`,
`hwb()`, `cmyk()`, `color("SeaGreen")`, or `transparent()`. `locals` and
`local.name` replace CSS variables; `env("NAME", default)` and `inherit()`
work. `import "extra.hcl" {}` merges another file (`optional = true` if
missing); `media { min_width = 120 … }` is evaluated against that same
canvas.

`window { location = "north east" }` uses the 9-point grid; `pointer` sits
at the pointer. A source `location` still overrides.

`background_image` accepts a color and otherwise paints transparent. Extra
widgets (preview) are not implemented.

Shipped palettes (`theme "nord" {}` or `theme = "nord"`):

| File | Palette |
| --- | --- |
| `themes/solarized-dark.hcl` | [Solarized](https://ethanschoonover.com/solarized/) dark (`base03` / `base0`) |
| `themes/solarized-light.hcl` | Solarized light (`base3` / `base00`) |
| `themes/nord.hcl` | [Nord](https://www.nordtheme.com/docs/colors-and-palettes) polar night |
| `themes/gruvbox-dark.hcl` | [Gruvbox](https://github.com/morhetz/gruvbox) dark, medium contrast |
| `themes/catppuccin-mocha.hcl` | [Catppuccin](https://github.com/catppuccin/palette) Mocha |
| `themes/dracula.hcl` | [Dracula](https://draculatheme.com) |

## D-Bus

The process owns `org.wontfix.klemmbrett` on the session bus:

| | |
| --- | --- |
| Name | `org.wontfix.klemmbrett` |
| Object | `/org/wontfix/klemmbrett/Activate` |
| Interface | `org.wontfix.klemmbrett.Activate` |
| Method | `ShowMenu(sii)` — source name, then `x`, `y` in global compositor coordinates |

`x` and `y` are the pointer location in the compositor's global layout (the
same space qtile's `get_mouse_position()` and Sway output `rect` values use).
The popup is placed on the output that contains that point, flipping or
sliding so it stays on that output.

```sh
busctl --user call \
    org.wontfix.klemmbrett \
    /org/wontfix/klemmbrett/Activate \
    org.wontfix.klemmbrett.Activate \
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
        "org.wontfix.klemmbrett",
        "/org/wontfix/klemmbrett/Activate",
        "org.wontfix.klemmbrett.Activate",
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

async def _show_source(source, x, y):
    global _bus
    message = Message(
        destination="org.wontfix.klemmbrett",
        path="/org/wontfix/klemmbrett/Activate",
        interface="org.wontfix.klemmbrett.Activate",
        member="ShowMenu",
        signature="sii",
        body=[source, int(x), int(y)],
    )
    try:
        reply = await (await _session_bus()).call(message)
    except Exception:
        # The cached connection can go stale; drop it and try once more.
        _bus = None
        reply = await (await _session_bus()).call(message)
    if reply.message_type == MessageType.ERROR:
        logger.error(
            "Klemmbrett ShowMenu failed: %s %s",
            reply.error_name,
            reply.body,
        )

def show_clip_history(qtile):
    x, y = qtile.core.get_mouse_position()
    create_task(_show_source("history", x, y))

keys = [
    Key(["mod4"], "v", lazy.function(show_clip_history)),
]
```

Autostart the daemon from the same config (`klemmbrett` in `spawn`) or from
your session so the bus name is already claimed when the binding runs.
