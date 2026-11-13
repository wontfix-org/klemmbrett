# klemmbrett-rpc

A client for the [klemmbrett](https://github.com/wontfix-org/klemmbrett)
daemon's Varlink control socket, for scripts, status bars, and compositor
configurations. It needs nothing but the Python standard library, so it
installs where the full `klemmbrett` package (with its Rust extension) is not
wanted, such as the system Python a window manager runs on.

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

`AsyncClient` has the same methods as coroutines (`subscribe` is an async
iterator); open it with `await AsyncClient.connect()`. Errors are raised as
`VarlinkError` with `error`, `name`, and `parameters`, and a missing daemon
as `ConnectionError`.

The socket is `$KLEMMBRETT_SOCKET`, else
`$XDG_RUNTIME_DIR/klemmbrett/varlink.sock`. Methods, events, and errors are
described in [docs/rpc.md](https://github.com/wontfix-org/klemmbrett/blob/main/docs/rpc.md).

Records are strings (the value), dicts with a `value` and optional `key`,
`label`, and `data` (or other keys as data), or any object with a `_json()`
method, such as `klemmbrett.Record`. `data` may hold dataclasses, NamedTuples,
and enums.

Inside a klemmbrett configuration, `klemmbrett.rpc` is this same client.

## Versions

The version follows the `org.wontfix.klemmbrett` Varlink interface and this
client's API, not the daemon's releases:

- While the version is 0.x, a minor bump (0.1 to 0.2) is an incompatible
  change to the interface or the client, and a patch bump is anything
  compatible, such as a new method or event.
- From 1.0 on, the major version takes that role.

Each `klemmbrett` release requires the range it was built against, such as
`klemmbrett-rpc>=0.1,<0.2`. Against an older daemon, a method it lacks is
reported as `org.varlink.service.MethodNotFound`.
