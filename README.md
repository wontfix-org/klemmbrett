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

Records can carry nested data that layouts and filters reach into, and
providers can be live: a `Store` changed from a handler, thread, or timer
reloads the open popup. klemmbrett can also be the desktop notification
server: a Python function decides whether each notification is shown as a
toast, dropped, or stashed as unread for a widget to list
([`docs/notifications.md`](docs/notifications.md)).

The popup takes keyboard focus as soon as it appears, without needing the
pointer. It is a `wlr-layer-shell` surface that requests exclusive keyboard
interactivity, which is the only way a Wayland surface opened from something
other than user input (here, a call on the control socket) can be focused: an
`xdg_popup` grab — what `GtkMenu` would use — requires the serial of a real
input event, which such a call does not have. Toasts are layer-shell
surfaces that never take the keyboard. Any compositor implementing
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

That writes `klemmbrett.service` and `klemmbrett.socket` to
`$XDG_CONFIG_HOME/systemd/user/` (normally `~/.config/systemd/user/`). The
service is a `Type=notify` unit pulled in by `graphical-session.target`, so
history is recorded from login. The socket unit holds the control socket,
`$XDG_RUNTIME_DIR/klemmbrett/varlink.sock`: a call made while the daemon
restarts waits instead of failing, and the first call starts the daemon if
it is not running. A D-Bus service file left by an earlier version is
removed. If this process can see `WAYLAND_DISPLAY`, that value is imported
into the systemd user manager. `ExecStart` is the `klemmbrett` script you
ran (or `python -m klemmbrett` with the same interpreter). Pass `--config`
as well to pin that file in `ExecStart`.

To open a configured widget:

```sh
klemmbrett show history
klemmbrett show history -x 120 -y 80
klemmbrett show snippets snippets.yaml --format yaml
klemmbrett prompt history
printf '%s\n' one two | klemmbrett prompt snippets -
```

`-x` and `-y` are compositor global coordinates. Omit both when there is
no pointer: the popup uses the output of the keyboard-focused window, then
the primary output, then the first output. `show` and `prompt` never read
the configuration; they ask the running daemon. `--socket PATH` (or
`$KLEMMBRETT_SOCKET`) picks a different socket for the daemon and every
command.

A positional path replaces the widget provider. `-` reads that input from
stdin. `--format text|json|yaml` selects the record format and defaults to
`text`. The formats are the same ones a `File` provider reads. Omitting the
path uses the widget's provider. A widget configured without a provider has
no records of its own, so `show` and `prompt` must pass a path or `-`.
`prompt` uses the same arguments, blocks until the popup closes, and prints
each selected value on its own line. Cancelling prints nothing and exits 1.

```sh
klemmbrett watch inbox          # JSON lines: changed, selected, notification
klemmbrett stash notifications --unread
klemmbrett stash notifications --mark-read 12 13
klemmbrett stash notifications --all
```

`watch` prints an event per line as it happens, optionally only for some
widgets; notifications are printed when no widgets are named. `stash` lists a
notification stash as JSON lines or marks entries read.

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

Widgets, providers, record data, live stores, handlers, and recipes are
documented in [`docs/widgets.md`](docs/widgets.md).

## Notifications

```python
from klemmbrett import notifications as notify

def route(n):
    if n.app_name == "Spotify":
        return notify.DROP
    if n.critical or n.app_name == "Signal":
        return notify.SHOW
    return notify.STASH

inbox = notify.serve(route, location="north-east")
kb.widget("inbox", inbox.unread, layout=layouts.filter, search=["summary", "body", "app_name"])
```

`serve` makes klemmbrett own `org.freedesktop.Notifications`, so stop mako,
dunst, or whatever else runs (or pass `replace=True`). Shown notifications
are toasts that never take the keyboard: left click runs the default action,
right click dismisses. Stashed ones wait as unread, and picking one in the
`inbox` widget marks it read. See [`docs/notifications.md`](docs/notifications.md).

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

## Control socket

The daemon answers [Varlink](https://varlink.org) calls on
`$XDG_RUNTIME_DIR/klemmbrett/varlink.sock`, interface
`org.wontfix.klemmbrett`: `Show`, `Prompt`, `Widgets`, `Records`,
`Subscribe`, `StashList`, `StashMarkRead`, and `Notify`. Records are JSON,
data and all. Only the user running the daemon may connect.

```sh
klemmbrett show history -x 120 -y 80
```

That is the same call as:

```sh
varlinkctl call $XDG_RUNTIME_DIR/klemmbrett/varlink.sock \
    org.wontfix.klemmbrett.Show '{"widget": "history", "pointer": {"x": 120, "y": 80}}'
```

`varlinkctl introspect` prints the interface. For Python scripts and event
loops, the [`klemmbrett-rpc`](packages/klemmbrett-rpc) package
(`pip install klemmbrett-rpc`, imported as `klemmbrett_rpc`) is a client that
needs only the standard library; inside a configuration it is also
`klemmbrett.rpc`. Methods, errors, and events are described in
[`docs/rpc.md`](docs/rpc.md).

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
`klemmbrett show history`, or the example below uses `klemmbrett_rpc` on
Qtile's asyncio loop (`libqtile.utils.create_task`) so the binding does not
fork a helper. Install `klemmbrett-rpc` for the interpreter Qtile runs on; it
needs only the standard library, not the full `klemmbrett` package.

```python
import klemmbrett_rpc as rpc
from libqtile.config import Key
from libqtile.lazy import lazy
from libqtile.log_utils import logger
from libqtile.utils import create_task

async def _show_widget(widget, x, y):
    try:
        async with await rpc.AsyncClient.connect() as client:
            await client.show(widget, pointer=(int(x), int(y)))
    except (ConnectionError, rpc.VarlinkError) as err:
        logger.error("klemmbrett Show failed: %s", err)

def show_clip_history(qtile):
    x, y = qtile.core.get_mouse_position()
    create_task(_show_widget("history", x, y))

keys = [
    Key(["mod4"], "v", lazy.function(show_clip_history)),
]
```

Autostart the daemon from the same config (`klemmbrett` in `spawn`) or with
the systemd units, so the socket is there when the binding runs.
