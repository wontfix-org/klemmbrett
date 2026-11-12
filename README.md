# klemmbrett

Keeps the Wayland clipboard and primary selection holding the same text. Named
popup widgets can provide values to copy, open, or run, including optional
SQLite clipboard history, text/JSON/YAML files, executables on `PATH`, and
Wayland windows to switch to.

A GPUI popup lists a configured widget. Entries longer than a configurable length are
shown shortened. A widget can pack either today's accelerator menu or a
filter: `layout=layouts.filter` adds an input bar and types into a case-insensitive
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

## Install

klemmbrett is a Python package with a Rust extension. It needs Python 3.11
or newer at runtime. To build it you also need a Rust toolchain (1.95 or
newer) and, on Debian/Trixie:

```sh
sudo apt install \
    libxkbcommon-dev \
    libwayland-dev \
    libvulkan-dev \
    pkg-config
```

Install the `klemmbrett` command from a checkout:

```sh
uv tool install .        # or: pipx install .
```

For development, `just dev` builds the extension into `.venv` with
`maturin develop`, and `just test` runs `cargo test` and `pytest`.
`cargo test` also runs end-to-end tests against private headless
compositors when they are installed. Those tests build `klemmbrett-py` and
run the package with `python3 -m klemmbrett`; set `KLEMMBRETT_BIN` to test an
installed `klemmbrett` instead. To check where the popup lands on several
compositors and distros, see [`docs/ui-tests.md`](docs/ui-tests.md).

## Run

```sh
klemmbrett
```

To use a different configuration file:

```sh
klemmbrett --config /path/to/config.py
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
`ExecStart` is the `klemmbrett` script you ran (or `python -m klemmbrett`
with the same interpreter). Pass `--config` as well to pin that file in
`ExecStart`.

To open a configured widget without spelling out `busctl`:

```sh
klemmbrett show history
klemmbrett show history -x 120 -y 80
klemmbrett show snippets snippets.yaml --format yaml
klemmbrett prompt history
printf '%s\n' one two | klemmbrett prompt snippets -
```

`-x` and `-y` are compositor global coordinates. Omit both when there is
no pointer: the popup uses the output of the keyboard-focused window, then
the primary output, then the first output. The running daemon (or D-Bus
activation) still serves the request. `show` and `prompt` never read the
configuration.

A positional path replaces the widget provider. `-` reads that input from
stdin. `--format text|json|yaml` selects the record format and defaults to
`text`. The formats are the same ones a `File` provider reads. Omitting the
path uses the widget's provider. A widget configured without a provider has
no records of its own, so `show` and `prompt` must pass a path or `-`.
`prompt` uses the same arguments, blocks until the popup closes, and prints
each selected value on its own line. Cancelling prints nothing and exits 1.

## Configuration

The configuration is a Python file, `$XDG_CONFIG_HOME/klemmbrett/config.py`
(normally `~/.config/klemmbrett/config.py`). If it is missing, clipboard and
primary sync still run, but there are no popup widgets or history. A
leftover `config.hcl` or `config.toml` in that directory is rejected with a
hint to convert it.

```python
import subprocess

import klemmbrett as kb
from klemmbrett import layouts, providers, themes

kb.configure(label_max=64, launch="auto", theme=themes.nord)

kb.widget("history", providers.ClipboardHistory(), layout=layouts.filter)
kb.widget("run", providers.Executables(), layout=layouts.filter)
kb.widget("windows", providers.WaylandWindows(), layout=layouts.filter)
kb.widget("pick", layout=layouts.filter)

@kb.widget("snippets", providers.File("snippets.yaml"))
def snippets(record):
    kb.copy(record.value)
    subprocess.run(["notify-send", "Copied", record.label])
```

`pick` has no provider. Open it with records, for example
`printf '%s\n' one two | klemmbrett show pick -`.

Selecting a row calls the widget's `on_select` with the `Record`. Without
one, the provider decides (`Executables` launches, `WaylandWindows` focuses
the window), and otherwise the value is copied. Handlers are ordinary Python:
`subprocess`, `re`, and `json` replace the old transforms and actions.
`kb.launch` starts detached programs. With `launch="auto"`, that is
`systemd-run --user --scope` when klemmbrett runs as a systemd unit, and a
POSIX double fork otherwise. `"posix"`, `"systemd-run"`, or a wrapper list
such as `["uwsm", "app", "--"]` override it.

Widgets, providers, handlers, and recipes are documented in
[`docs/widgets.md`](docs/widgets.md).

## Popup theme

`kb.configure(theme=themes.nord)` (or `theme="nord"`) sets the palette for
every widget, and `kb.widget(..., theme=...)` replaces it for one widget.
`kb.set_theme(name)` switches the palette while the daemon runs, which the
`Themes` provider does when a row is picked. `layouts.menu`,
`layouts.filter`, and `layouts.prompt` are shipped, and a layout can be any
`ui.Window` tree or a function returning one. Elements, properties, colors,
and lengths are described in [`docs/theming.md`](docs/theming.md).

```python
from klemmbrett.style import Style, Theme

kb.configure(theme=themes.nord | Theme(element_selected=Style(background_color="#5e81ac")))
```

Shipped palettes (`themes.nord`, `themes.get("tokyo-night")`, or by name):

| Name | Palette |
| --- | --- |
| `solarized-dark` | [Solarized](https://ethanschoonover.com/solarized/) dark (`base03` / `base0`) |
| `solarized-light` | Solarized light (`base3` / `base00`) |
| `nord` | [Nord](https://www.nordtheme.com/docs/colors-and-palettes) polar night |
| `gruvbox-dark` | [Gruvbox](https://github.com/morhetz/gruvbox) dark, medium contrast |
| `catppuccin-mocha` | [Catppuccin](https://github.com/catppuccin/palette) Mocha |
| `dracula` | [Dracula](https://draculatheme.com) |
| `tokyo-night` | [Tokyo Night](https://github.com/tokyo-night/tokyo-night-vscode-theme), rounded card |
| `rose-pine` | [Rosé Pine](https://rosepinetheme.com), rounded card |
| `everforest` | [Everforest](https://github.com/sainnhe/everforest) dark, rounded card |
| `kanagawa` | [Kanagawa](https://github.com/rebelot/kanagawa.nvim) wave, rounded card |
| `mbra` | Personal moss-on-black (qtile `#60845b`, Alacritty black) |

## D-Bus

The process owns `org.wontfix.klemmbrett` on the session bus:

| | |
| --- | --- |
| Name | `org.wontfix.klemmbrett` |
| Object | `/org/wontfix/klemmbrett` |
| Interface | `org.wontfix.klemmbrett` |
| Method | `Show(sa(ii)aa{ss})` — widget, zero or one `(x, y)` pair, then records |
| Method | `Prompt(sa(ii)aa{ss}) -> as` — same arguments; returns the selected values |

`x` and `y` are the pointer location in the compositor's global layout (the
same space qtile's `get_mouse_position()` and Sway output `rect` values use).
The popup is placed on the output that contains that point, flipping or
sliding so it stays on that output. An empty pointer array means no pointer: the focused window's output is
used, then the primary output, then the first output. A `pointer` location
is treated as `center`.

An empty record array uses the widget provider. A widget with no provider
rejects that empty array. A non-empty array replaces the provider rows.
Each dict needs `value`; omitted `key` and `label` become the value.
`Show` still runs the widget's `on_select`. `Prompt`
blocks until the popup closes and returns the selected values. It does not
run handlers. Cancelling, or opening another popup, returns
an empty array.

```sh
klemmbrett show history -x 120 -y 80
```

That is the same call as:

```sh
busctl --user call \
    org.wontfix.klemmbrett \
    /org/wontfix/klemmbrett \
    org.wontfix.klemmbrett \
    Show sa(ii)aa{ss} history 1 120 80 0
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
subprocess.check_call(["klemmbrett", "show", "history", "-x", str(x), "-y", str(y)])
```

```sh
chmod +x ~/.config/sway/clip-history.py
```

In `~/.config/sway/config`:

```
bindsym $mod+v exec ~/.config/sway/clip-history.py
```

Without pointer coordinates the popup opens on the focused window's output:

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
        signature="sa(ii)aa{ss}",
        body=[widget, [(int(x), int(y))], []],
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
