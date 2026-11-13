"""A Varlink client for the running daemon, for scripts, bars, and
compositor configurations. It needs nothing but the standard library::

    import klemmbrett_rpc as rpc

    with rpc.Client() as client:
        client.show("history", pointer=(120, 40))
        for event in client.subscribe(["inbox"]):
            print(event)

`AsyncClient` does the same with asyncio, for Qtile and other event loops.
The socket is `$KLEMMBRETT_SOCKET`, else `$XDG_RUNTIME_DIR/klemmbrett/varlink.sock`.
The interface is described by `varlinkctl introspect <socket> org.wontfix.klemmbrett`.
"""

from __future__ import annotations

import asyncio
import json
import os
import socket
from collections.abc import AsyncIterator, Iterable, Iterator
from pathlib import Path
from typing import Any

from ._records import RecordLike, SupportsJson, _encode_record

__all__ = ["INTERFACE", "AsyncClient", "Client", "RecordLike", "SupportsJson", "VarlinkError", "socket_path"]

INTERFACE = "org.wontfix.klemmbrett"


class VarlinkError(Exception):
    """An error reply. `error` is its qualified name, such as
    `org.wontfix.klemmbrett.UnknownWidget`."""

    def __init__(self, error: str, parameters: dict[str, Any] | None = None) -> None:
        self.error = error
        self.parameters = parameters or {}
        detail = self.parameters.get("message") or (json.dumps(self.parameters) if self.parameters else "")
        super().__init__(f"{error}: {detail}" if detail else error)

    @property
    def name(self) -> str:
        """The error name without its interface."""
        return self.error.rsplit(".", 1)[-1]


def socket_path(path: str | os.PathLike[str] | None = None) -> Path:
    if path is not None:
        return Path(path)
    if explicit := os.environ.get("KLEMMBRETT_SOCKET"):
        return Path(explicit)
    runtime = os.environ.get("XDG_RUNTIME_DIR") or f"/tmp/klemmbrett-{os.getuid()}"
    return Path(runtime) / "klemmbrett" / "varlink.sock"


def _method(name: str) -> str:
    return name if "." in name else f"{INTERFACE}.{name}"


def _encode(method: str, parameters: dict[str, Any], more: bool = False) -> bytes:
    message: dict[str, Any] = {"method": _method(method)}
    parameters = {name: value for name, value in parameters.items() if value is not None}
    if parameters:
        message["parameters"] = parameters
    if more:
        message["more"] = True
    return json.dumps(message, separators=(",", ":")).encode() + b"\0"


def _decode(raw: bytes) -> tuple[dict[str, Any], bool]:
    reply = json.loads(raw)
    if "error" in reply:
        raise VarlinkError(reply["error"], reply.get("parameters"))
    return reply.get("parameters") or {}, bool(reply.get("continues"))


def _records(records: Iterable[RecordLike] | None) -> list[dict[str, Any]] | None:
    if records is None:
        return None
    return [_encode_record(record) for record in records] or None


def _pointer(pointer: tuple[int, int] | None) -> dict[str, int] | None:
    return None if pointer is None else {"x": pointer[0], "y": pointer[1]}


class Client:
    """A blocking connection to the daemon. Calls go one after another."""

    def __init__(self, path: str | os.PathLike[str] | None = None, *, timeout: float | None = None) -> None:
        self.path = socket_path(path)
        self._socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._socket.settimeout(timeout)
        try:
            self._socket.connect(str(self.path))
        except (FileNotFoundError, ConnectionRefusedError) as err:
            self._socket.close()
            raise ConnectionError(f"klemmbrett is not running ({self.path}: {err.strerror})") from err
        self._buffer = b""

    def close(self) -> None:
        self._socket.close()

    def __enter__(self) -> Client:
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    def _read(self) -> bytes:
        while b"\0" not in self._buffer:
            chunk = self._socket.recv(65536)
            if not chunk:
                raise ConnectionError("klemmbrett closed the connection")
            self._buffer += chunk
        message, self._buffer = self._buffer.split(b"\0", 1)
        return message

    def call(self, method: str, **parameters: Any) -> dict[str, Any]:
        """One call, by name (`Show`) or qualified name."""
        self._socket.sendall(_encode(method, parameters))
        return _decode(self._read())[0]

    def stream(self, method: str, **parameters: Any) -> Iterator[dict[str, Any]]:
        """A call with `more`: every reply until the last."""
        self._socket.sendall(_encode(method, parameters, more=True))
        while True:
            reply, continues = _decode(self._read())
            yield reply
            if not continues:
                return

    def show(
        self,
        widget: str,
        records: Iterable[RecordLike] | None = None,
        *,
        pointer: tuple[int, int] | None = None,
    ) -> None:
        self.call("Show", widget=widget, pointer=_pointer(pointer), records=_records(records))

    def prompt(
        self,
        widget: str,
        records: Iterable[RecordLike] | None = None,
        *,
        pointer: tuple[int, int] | None = None,
    ) -> list[str]:
        """Waits for the popup to close and returns the picked values."""
        reply = self.call("Prompt", widget=widget, pointer=_pointer(pointer), records=_records(records))
        return list(reply.get("values", []))

    def widgets(self) -> list[dict[str, Any]]:
        return list(self.call("Widgets").get("widgets", []))

    def records(self, widget: str) -> list[dict[str, Any]]:
        return list(self.call("Records", widget=widget).get("records", []))

    def subscribe(self, widgets: Iterable[str] | None = None) -> Iterator[dict[str, Any]]:
        """Events as they happen: `changed`, `selected`, and `notification`."""
        names = None if widgets is None else list(widgets)
        for reply in self.stream("Subscribe", widgets=names):
            event = reply.get("event", {})
            if event.get("kind") != "ready":
                yield event

    def notify(
        self,
        app_name: str,
        summary: str,
        body: str | None = None,
        *,
        urgency: int | None = None,
        timeout: int | None = None,
    ) -> int:
        reply = self.call(
            "Notify", app_name=app_name, summary=summary, body=body, urgency=urgency, timeout=timeout
        )
        return int(reply["id"])

    def stash(self, name: str = "notifications", *, unread_only: bool = False) -> list[dict[str, Any]]:
        return list(self.call("StashList", stash=name, unread_only=unread_only).get("records", []))

    def mark_read(self, name: str = "notifications", keys: Iterable[str] | None = None) -> int:
        """Marks stashed notifications read; without keys, all of them."""
        names = None if keys is None else list(keys)
        return int(self.call("StashMarkRead", stash=name, keys=names).get("marked", 0))


class AsyncClient:
    """The asyncio counterpart of `Client`. Open it with `await AsyncClient.connect()`."""

    def __init__(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter, path: Path) -> None:
        self._reader = reader
        self._writer = writer
        self.path = path

    @classmethod
    async def connect(cls, path: str | os.PathLike[str] | None = None) -> AsyncClient:
        resolved = socket_path(path)
        try:
            reader, writer = await asyncio.open_unix_connection(str(resolved), limit=1 << 24)
        except (FileNotFoundError, ConnectionRefusedError) as err:
            raise ConnectionError(f"klemmbrett is not running ({resolved}: {err.strerror})") from err
        return cls(reader, writer, resolved)

    async def close(self) -> None:
        self._writer.close()
        await self._writer.wait_closed()

    async def __aenter__(self) -> AsyncClient:
        return self

    async def __aexit__(self, *_: object) -> None:
        await self.close()

    async def _read(self) -> bytes:
        try:
            return (await self._reader.readuntil(b"\0"))[:-1]
        except asyncio.IncompleteReadError as err:
            raise ConnectionError("klemmbrett closed the connection") from err

    async def call(self, method: str, **parameters: Any) -> dict[str, Any]:
        self._writer.write(_encode(method, parameters))
        await self._writer.drain()
        return _decode(await self._read())[0]

    async def stream(self, method: str, **parameters: Any) -> AsyncIterator[dict[str, Any]]:
        self._writer.write(_encode(method, parameters, more=True))
        await self._writer.drain()
        while True:
            reply, continues = _decode(await self._read())
            yield reply
            if not continues:
                return

    async def show(
        self,
        widget: str,
        records: Iterable[RecordLike] | None = None,
        *,
        pointer: tuple[int, int] | None = None,
    ) -> None:
        await self.call("Show", widget=widget, pointer=_pointer(pointer), records=_records(records))

    async def prompt(
        self,
        widget: str,
        records: Iterable[RecordLike] | None = None,
        *,
        pointer: tuple[int, int] | None = None,
    ) -> list[str]:
        reply = await self.call("Prompt", widget=widget, pointer=_pointer(pointer), records=_records(records))
        return list(reply.get("values", []))

    async def subscribe(self, widgets: Iterable[str] | None = None) -> AsyncIterator[dict[str, Any]]:
        names = None if widgets is None else list(widgets)
        async for reply in self.stream("Subscribe", widgets=names):
            event = reply.get("event", {})
            if event.get("kind") != "ready":
                yield event

    async def notify(
        self,
        app_name: str,
        summary: str,
        body: str | None = None,
        *,
        urgency: int | None = None,
        timeout: int | None = None,
    ) -> int:
        reply = await self.call(
            "Notify", app_name=app_name, summary=summary, body=body, urgency=urgency, timeout=timeout
        )
        return int(reply["id"])
