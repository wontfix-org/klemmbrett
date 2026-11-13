# Notifications

`notifications.serve` makes klemmbrett the desktop notification server: it
owns `org.freedesktop.Notifications` on the session bus, so `notify-send`,
browsers, and chat clients send their notifications to it. Each one goes
through a routing function that decides what happens to it:

| Decision | Effect |
| --- | --- |
| `SHOW` | A toast in the configured corner, without taking the keyboard |
| `DROP` | Nothing; the sender is told it expired |
| `STASH` | Kept as unread in the stash `serve` returns; the sender is told it expired |
| a `Stash` | Kept as unread in that stash |
| a list | All of them, such as `[SHOW, inbox]` |

```python
import klemmbrett as kb
from klemmbrett import layouts, ui
from klemmbrett import notifications as notify

work = notify.Stash("work")

def route(n):
    if n.app_name in {"Spotify", "Steam"}:
        return notify.DROP
    if n.critical:
        return [notify.SHOW, work] if n.app_name == "Slack" else notify.SHOW
    if n.app_name == "Slack":
        return work
    if n.category and n.category.startswith("im."):
        return notify.SHOW
    return notify.STASH

inbox = notify.serve(route, timeout=6, location="north-east")

row = ui.Element(
    ui.Index(),
    ui.Field("app_name", width=90, text_color="#888"),
    ui.Text("{summary}"),
)
kb.widget(
    "inbox",
    inbox.unread,
    layout=ui.Window(ui.InputBar(ui.Prompt(), ui.Entry()), ui.ListView(row), location="center"),
    search=["summary", "body", "app_name"],
)
kb.widget("work", work.unread, layout=layouts.filter, multi=True)
```

Without a route, every notification is shown. When the route raises, or
returns something it should not, the notification is shown and the error is
printed. The route runs on the handler thread with the other handlers, so
keep it quick.

## serve

```python
notify.serve(route=None, *, stash=None, timeout=5, location="north-east", replace=False, max_visible=5)
```

- `stash`: where `STASH` keeps notifications. Defaults to a `Stash` named
  `notifications`; `serve` returns it.
- `timeout`: seconds a toast stays when the sender leaves it to the server
  (an `expire_timeout` of -1). A sender's own timeout is honored, 0 never
  expires, and critical notifications stay until clicked.
- `location`: where toasts stack, by the names popups use (`north-east`,
  `south`, ...). `center` and `pointer` mean `north-east`. The gap from
  that corner is the theme's `toast` margin.
- `replace`: take the bus name from the running notification daemon. Without
  it, a running mako or dunst keeps the name and klemmbrett prints why it
  serves no notifications.
- `max_visible`: further toasts wait until one closes.

Call it at most once.

## Notification

The route gets a frozen dataclass:

| Field | |
| --- | --- |
| `id` | The id the sender got back |
| `app_name`, `app_icon`, `summary`, `body` | As sent |
| `actions` | `(key, label)` pairs; `default` is what a click invokes |
| `hints` | Hints as JSON-like data; byte arrays such as `image-data` become `{"bytes": n}` |
| `urgency` | `notify.LOW`, `NORMAL`, or `CRITICAL` (0–2) |
| `timeout` | Milliseconds: -1 the server default, 0 never |
| `replaces` | `True` when it replaces an earlier notification with the same id |
| `critical`, `category`, `desktop_entry` | Shortcuts into `urgency` and `hints` |

## Toasts

Left click invokes the `default` action, if the sender offered one, and
closes the toast. Right click dismisses it. Other actions are buttons on the
toast. Senders can replace and close their notifications as usual, and get
the `NotificationClosed` and `ActionInvoked` signals.

Toasts look like the popup window of the configured theme unless the theme
styles the `toast`, `toast_summary`, and `toast_body` elements
([theming](theming.md#toast-toast_summary-toast_body)). `toast.margin`
is how far the stack sits from the screen edges.

## Stashes

```python
notify.Stash(name="notifications", *, limit=500, path=None)
```

A `Stash` is a `providers.Store` of notifications, newest first, keyed by
notification id, stored in SQLite. `path` defaults to
`$XDG_DATA_HOME/klemmbrett/notifications.sqlite` (normally
`~/.local/share/klemmbrett/notifications.sqlite`), and every stash shares
that file. Only unread notifications are kept: marking one read removes it
from the stash and from the database. What remains is still there after a
restart. Past `limit`, the oldest fall off. Each record's label is
`app_name: summary`, its value the body (or the summary), and its data a
`notify.Stashed` with `id`, `app_name`, `summary`, `body`, `app_icon`,
`urgency`, `category`, `received` (a Unix time), and `read`. A notification
that replaces a stashed one takes its place and is unread again.

| | |
| --- | --- |
| `stash.unread` | The unread notifications, for a widget |
| `stash` itself | The same notifications |
| `stash.mark_read(*records_or_keys)` | Removes them. Returns how many were there |
| `stash.mark_all_read()` | |
| `stash.dismiss(*records_or_keys)` | Removes them |
| `stash.unread_count` | |
| `stash.stash(notification)` | Stashes by hand, for example from a `SHOW` route |

Picking a row in a widget showing a stash (or `stash.unread`) removes it,
unless the widget has its own `on_select`. Because a stash is a store,
widgets showing it reload as notifications arrive and are read, and
`stash.subscribe(callback)` can update a status bar.

From the shell, or anything that speaks Varlink:

```sh
kb stash notifications --unread        # one JSON record per line
kb stash work --mark-read 41 42
kb stash work --all
kb watch                               # includes every arriving notification
```

A status bar can count unread notifications without polling by running
`kb watch` and re-reading the stash on each `changed` event of a
widget showing it, or with [`klemmbrett-rpc`](rpc.md) from Python.
