# Control socket

The daemon answers [Varlink](https://varlink.org) calls on a Unix socket:
`--socket PATH`, else `$KLEMMBRETT_SOCKET`, else
`$XDG_RUNTIME_DIR/klemmbrett/varlink.sock`. The socket is only open to the
user running the daemon, and only one daemon runs per socket: a second one
exits with an error, and a socket left by a crashed daemon is replaced.
With the systemd units from `kb install-systemd`, systemd holds the socket
and starts the daemon on the first call.

Messages are JSON objects terminated by a NUL byte. `varlinkctl` from
systemd speaks the protocol:

```sh
sock=$XDG_RUNTIME_DIR/klemmbrett/varlink.sock
varlinkctl introspect $sock org.wontfix.klemmbrett
varlinkctl call $sock org.wontfix.klemmbrett.Widgets '{}'
varlinkctl call --more $sock org.wontfix.klemmbrett.Subscribe '{}'
```

From Python, the `klemmbrett-rpc` package is a client that needs only the
standard library, so it installs where the full `klemmbrett` package is not
wanted, such as the system Python a window manager runs on:

```sh
pip install klemmbrett-rpc
```

```python
import klemmbrett_rpc as rpc

with rpc.Client() as client:
    picked = client.prompt("snippets", ["one", {"value": "two", "data": {"n": 2}}])
    client.notify("backup", "Backup finished", "42 GB", urgency=0)
    for event in client.subscribe(["inbox"]):
        print(event["kind"], event.get("widget"))
```

`rpc.AsyncClient` has the same methods as coroutines (`subscribe` is an
async iterator); open it with `await rpc.AsyncClient.connect()`. Errors are
raised as `rpc.VarlinkError` with `error`, `name`, and `parameters`, and a
missing daemon as `ConnectionError`. Inside a configuration, `klemmbrett.rpc`
is the same client; `klemmbrett` depends on `klemmbrett-rpc`.

`klemmbrett-rpc` is versioned on its own, following this interface: while it
is 0.x, a minor bump (0.1 to 0.2) is an incompatible change to the interface
or the client, and a patch bump is anything compatible, such as a new method.
Each `klemmbrett` release requires the range it was built against, such as
`klemmbrett-rpc>=0.1,<0.2`. It is released with
`scripts/release --package rpc` (tags `klemmbrett-rpc-vX.Y.Z`), before the
`klemmbrett` release that needs it.

## Methods

All methods are in the interface `org.wontfix.klemmbrett`. Optional
parameters may be left out or `null`.

| Method | Parameters | Reply |
| --- | --- | --- |
| `Show` | `widget`, `pointer?`, `records?` | `{}` |
| `Prompt` | `widget`, `pointer?`, `records?` | `values`: picked values |
| `Pick` | `records`, `pointer?`, `query?`, `multi?`, `prompt?` | `values`: picked values |
| `Widgets` | | `widgets`: `name`, `provider`, `multi` |
| `Records` | `widget` | `records`: what the provider supplies now |
| `Subscribe` | `widgets?` | a stream of `event` (call with `more`) |
| `StashList` | `stash`, `unread_only?` | `records`: newest first |
| `StashMarkRead` | `stash`, `keys?` | `marked`: how many were unread |
| `Notify` | `app_name`, `summary`, `body?`, `urgency?`, `timeout?` | `id` |
| `AskSecret` | `kind`, `widget?`, `id?`, `title?`, `message?`, `prompt?`, `error?`, `echo?`, `mode?`, `timeout?` | `secret?`, `confirmed` |

A record is `{"key"?, "label"?, "value", "data"?}`; `key` and `label`
default to the value, and `data` is any JSON value. Unknown fields are an
error.

`pointer` is `{"x", "y"}` in the compositor's global layout (the same space
qtile's `get_mouse_position()` and Sway output `rect` values use). The popup
opens on the output containing it, flipping or sliding to stay there.
Without one, the focused window's output is used, then the primary output,
then the first output, and a `pointer` location is treated as `center`.

Without `records`, or with an empty list, the widget's provider supplies the
rows. `Show` runs the widget's `on_select` as usual. `Prompt` waits until
the popup closes, returns the picked values, and runs no handlers.
Cancelling, or opening another popup, returns no values.

`Pick` is the picker `kb` opens when it has no subcommand. It does not name
a widget: the daemon uses `kb.picker()` (a centered fuzzy filter when that
was left out). `records` is required and may be empty. `query` is the text
the filter starts with. `multi` overrides the configured multi-select for
this call (`null` keeps it). `prompt` replaces the label in front of the
filter. Like `Prompt`, it returns the picked values and runs no handlers.

`Notify` sends a notification through the configured routing, like
`notify-send` would; `urgency` is 0–2 and `timeout` is in milliseconds.

`AskSecret` asks a password question in the widget serving `kind`
(`askpass` or `pinentry`), or in `widget`, and waits for the answer; it is
what `klemmbrett-askpass` and `klemmbrett-pinentry` call. `echo` is `no`,
`masked` (the default), or `yes`; `mode` is `secret` (the default, replying
with `secret`), `confirm`, or `message`; `timeout` is in seconds. The
answer goes to the caller only: no event carries it. Cancelling is the
`Cancelled` error. See [`secrets.md`](secrets.md).

## Events

`Subscribe` must be called with `"more": true`. The first reply is a `ready`
event, then one reply per event, until the client disconnects. With
`widgets`, only events about those widgets arrive.

| `kind` | Fields | When |
| --- | --- | --- |
| `ready` | | The subscription is in place |
| `changed` | `widget` | A provider under the widget reported a change |
| `selected` | `widget`, `records` | Rows were picked (not for `Prompt`) |
| `notification` | `notification` | A notification arrived, before routing; not sent when filtering by widget |

## Errors

| Error | Parameters |
| --- | --- |
| `org.wontfix.klemmbrett.UnknownWidget` | `widget`, `known` |
| `org.wontfix.klemmbrett.NoProvider` | `widget` |
| `org.wontfix.klemmbrett.InvalidRecord` | `path` (`records[2]`), `message` |
| `org.wontfix.klemmbrett.UnknownStash` | `stash`, `known` |
| `org.wontfix.klemmbrett.NotificationsDisabled` | |
| `org.wontfix.klemmbrett.Busy` | (a password prompt is open) |
| `org.wontfix.klemmbrett.Cancelled` | |
| `org.wontfix.klemmbrett.NoAgent` | `kind` |
| `org.wontfix.klemmbrett.Failed` | `message` |
| `org.varlink.service.InvalidParameter` | `parameter` |
| `org.varlink.service.MethodNotFound` | `method` |
| `org.varlink.service.ExpectedMore` | |

The full interface, with comments, is
[`src/org.wontfix.klemmbrett.varlink`](../src/org.wontfix.klemmbrett.varlink).
