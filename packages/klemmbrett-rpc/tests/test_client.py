import asyncio
import enum
import json
import socket
import sys
import threading
from dataclasses import dataclass
from typing import NamedTuple

import pytest

import klemmbrett_rpc as rpc
from klemmbrett_rpc._records import _encode_record


class Sender(NamedTuple):
    name: str
    address: str = ""


class Folder(enum.Enum):
    INBOX = "inbox"


@dataclass(frozen=True)
class Mail:
    sender: Sender
    tags: tuple[str, ...]
    folder: Folder = Folder.INBOX


@pytest.fixture
def varlink_server(tmp_path):
    """A one-connection Varlink server answering from a script of replies."""
    path = tmp_path / "test.sock"
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(str(path))
    server.listen(1)
    received = []
    replies = {}

    def serve():
        conn, _ = server.accept()
        buffer = b""
        with conn:
            while True:
                chunk = conn.recv(4096)
                if not chunk:
                    return
                buffer += chunk
                while b"\0" in buffer:
                    raw, buffer = buffer.split(b"\0", 1)
                    message = json.loads(raw)
                    received.append(message)
                    for reply in replies.get(message["method"].rsplit(".", 1)[-1], [{}]):
                        conn.sendall(json.dumps(reply).encode() + b"\0")

    thread = threading.Thread(target=serve, daemon=True)
    thread.start()
    yield path, received, replies
    server.close()


def test_stands_alone():
    assert "klemmbrett" not in sys.modules


def test_client_calls_streams_and_raises(varlink_server):
    path, received, replies = varlink_server
    replies["Prompt"] = [{"parameters": {"values": ["a"]}}]
    replies["Pick"] = [{"parameters": {"values": ["b"]}}]
    replies["Subscribe"] = [
        {"parameters": {"event": {"kind": "ready"}}, "continues": True},
        {"parameters": {"event": {"kind": "changed", "widget": "w"}}, "continues": True},
        {"parameters": {"event": {"kind": "changed", "widget": "v"}}},
    ]
    replies["Show"] = [{"error": "org.wontfix.klemmbrett.UnknownWidget", "parameters": {"widget": "x", "known": []}}]
    with rpc.Client(path) as client:
        assert client.prompt("w", ["a", {"value": "b", "n": 1}], pointer=(1, 2)) == ["a"]
        assert client.pick(["a", "b"], query="b", multi=True, prompt="> ") == ["b"]
        assert [e["widget"] for e in client.subscribe(["w"])] == ["w", "v"]
        with pytest.raises(rpc.VarlinkError) as err:
            client.show("x")
    assert err.value.name == "UnknownWidget"
    assert received[0] == {
        "method": "org.wontfix.klemmbrett.Prompt",
        "parameters": {
            "widget": "w",
            "pointer": {"x": 1, "y": 2},
            "records": [
                {"key": "a", "label": "a", "value": "a"},
                {"key": "b", "label": "b", "value": "b", "data": {"n": 1}},
            ],
        },
    }
    assert received[1]["method"] == "org.wontfix.klemmbrett.Pick"
    assert "widget" not in received[1]["parameters"]
    assert received[1]["parameters"]["query"] == "b"
    assert received[1]["parameters"]["multi"] is True
    assert [row["value"] for row in received[1]["parameters"]["records"]] == ["a", "b"]
    assert received[2]["more"] is True


def test_async_client_calls_and_streams(varlink_server):
    path, received, replies = varlink_server
    replies["Notify"] = [{"parameters": {"id": 7}}]
    replies["Subscribe"] = [
        {"parameters": {"event": {"kind": "ready"}}, "continues": True},
        {"parameters": {"event": {"kind": "selected", "widget": "w"}}},
    ]

    async def run():
        async with await rpc.AsyncClient.connect(path) as client:
            note = await client.notify("app", "hi", urgency=0)
            events = [event async for event in client.subscribe()]
        return note, events

    note, events = asyncio.run(run())
    assert note == 7
    assert events == [{"kind": "selected", "widget": "w"}]
    assert received[0]["parameters"] == {"app_name": "app", "summary": "hi", "urgency": 0}


def test_client_reports_a_missing_daemon(tmp_path):
    with pytest.raises(ConnectionError, match="not running"):
        rpc.Client(tmp_path / "none.sock")


def test_socket_path_prefers_the_environment(monkeypatch, tmp_path):
    monkeypatch.setenv("KLEMMBRETT_SOCKET", str(tmp_path / "x.sock"))
    assert rpc.socket_path() == tmp_path / "x.sock"
    monkeypatch.delenv("KLEMMBRETT_SOCKET")
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    assert rpc.socket_path() == tmp_path / "klemmbrett" / "varlink.sock"


def test_encodes_strings_mappings_and_self_encoding_records():
    assert _encode_record("a") == {"key": "a", "label": "a", "value": "a"}
    assert _encode_record({"value": "v", "label": "L", "data": Mail(Sender("ann"), ("x",))}) == {
        "key": "v",
        "label": "L",
        "value": "v",
        "data": {"sender": {"name": "ann", "address": ""}, "tags": ["x"], "folder": "inbox"},
    }

    class Own:
        def _json(self):
            return {"key": "k", "label": "l", "value": "v"}

    assert isinstance(Own(), rpc.SupportsJson)
    assert _encode_record(Own()) == {"key": "k", "label": "l", "value": "v"}


@pytest.mark.parametrize(
    ("item", "message"),
    [
        ({"label": "x"}, "has no value"),
        ({"value": "v", "data": 1, "n": 2}, "both data and extra fields"),
        ({"value": "v", "key": 3}, "Record.key must be a string"),
        ({"value": "v", "n": float("nan")}, "not a JSON number"),
        ({"value": "v", "n": {1: 2}}, "is not a string"),
        ({"value": "v", "n": object()}, "not JSON-like data"),
        (42, "expected a Record"),
    ],
)
def test_rejects_bad_records(item, message):
    with pytest.raises(TypeError, match=message):
        _encode_record(item)
