import json
import socket
import threading
from dataclasses import dataclass
from typing import Literal, NamedTuple, Optional

import pytest

import klemmbrett as kb
from klemmbrett import _registry, notifications, providers, rpc, ui
from klemmbrett._schema import SchemaError, to_json, validate


def wire_widget(name):
    return next(w for w in _registry.wire()["widgets"] if w["name"] == name)


class Sender(NamedTuple):
    name: str
    address: str = ""


@dataclass(frozen=True)
class Mail:
    sender: Sender
    subject: str
    tags: list[str]
    size: float = 0
    folder: Literal["inbox", "spam"] = "inbox"
    thread: Optional[int] = None


def test_validate_builds_instances_and_names_the_failing_path():
    mail = validate(
        {"sender": {"name": "Ada"}, "subject": "Hi", "tags": ["work"], "size": 3},
        Mail,
    )
    assert mail == Mail(Sender("Ada"), "Hi", ["work"], 3)
    with pytest.raises(SchemaError) as err:
        validate({"sender": {"name": 7}, "subject": "Hi", "tags": []}, Mail)
    assert err.value.path == "data.sender.name"
    assert "expected str, got int 7" in str(err.value)
    with pytest.raises(SchemaError, match=r"data.tags\[1\]: expected str"):
        validate({"sender": {"name": "A"}, "subject": "", "tags": ["a", 2]}, Mail)
    with pytest.raises(SchemaError, match="missing 'subject'"):
        validate({"sender": {"name": "A"}, "tags": []}, Mail)
    with pytest.raises(SchemaError, match="unknown field 'cc'"):
        validate({"sender": {"name": "A"}, "subject": "", "tags": [], "cc": 1}, Mail)
    with pytest.raises(SchemaError, match="expected one of"):
        validate({"sender": {"name": "A"}, "subject": "", "tags": [], "folder": "x"}, Mail)
    with pytest.raises(SchemaError, match="expected a number"):
        validate({"sender": {"name": "A"}, "subject": "", "tags": [], "size": True}, Mail)


def test_to_json_flattens_dataclasses_and_namedtuples():
    mail = Mail(Sender("Ada", "a@x"), "Hi", ["work"], thread=4)
    assert to_json(mail) == {
        "sender": {"name": "Ada", "address": "a@x"},
        "subject": "Hi",
        "tags": ["work"],
        "size": 0,
        "folder": "inbox",
        "thread": 4,
    }
    with pytest.raises(SchemaError, match=r"data\.when: set"):
        to_json({"when": {1}})


def test_records_carry_data_from_dicts_and_keep_it_for_handlers(daemon):
    record = kb.Record("v", data=Mail(Sender("Ada"), "Hi", []))
    assert record._wire()[3]["sender"] == {"name": "Ada", "address": ""}
    from klemmbrett._record import to_record

    assert to_record({"value": "v", "n": 1}).data == {"n": 1}
    with pytest.raises(TypeError, match="both data and extra fields"):
        to_record({"value": "v", "data": {}, "n": 1})

    seen = []
    kb.widget("mail", providers.Static([record]), on_select=seen.append, search=["subject", "sender.name"])
    assert wire_widget("mail")["search"] == ["subject", "sender.name"]
    wire_widget("mail")["source"]()
    _registry.wire()["dispatcher"].select("mail", [record._wire()])
    assert seen == [record]
    assert isinstance(seen[0].data, Mail)


def test_schema_providers_check_every_record():
    @kb.provider(schema=Sender)
    def people():
        return [{"value": "a", "name": "Ada"}, {"value": "b", "name": 3}]

    kb.widget("people", people)
    with pytest.raises(SchemaError, match=r"record 'b': data.name"):
        wire_widget("people")["source"]()


def test_stores_upsert_trim_and_refresh_open_widgets(daemon):
    store = providers.Store(limit=2)
    kb.widget("direct", store)
    kb.widget("viewed", store.view(filter=lambda r: r.value != "b"))
    kb.widget("other", providers.Static(["x"]))
    _registry.wire()
    store.add("a")
    store.add("b")
    store.add({"value": "a2", "key": "a"})
    assert [r.value for r in store] == ["a2", "b"]
    store.add("c")
    assert [r.key for r in store] == ["b", "c"]
    store.add("d", first=True)
    assert [r.key for r in store] == ["d", "b"]
    assert store.update("b", label="B").label == "B"
    assert store.remove("b") and not store.remove("b")
    refreshes = [args for name, args in daemon.calls if name == "refresh"]
    assert refreshes and all(args == ["direct", "viewed"] for args in refreshes)
    with pytest.raises(KeyError):
        store.update("zzz", label="x")


def test_views_sort_map_and_limit_and_select_through_their_source(daemon):
    picked = []

    class Source(kb.Provider):
        def records(self):
            return ["b", "c", "a"]

        def select(self, record):
            picked.append(record.value)

    view = Source().view(sort=lambda r: r.value, reverse=True, limit=2, map=lambda r: r.value.upper())
    assert [r.value for r in view.records()] == ["C", "B"]
    view.select(kb.Record("C"))
    assert picked == ["C"]


def test_merge_routes_selection_to_the_original_provider(daemon):
    picked = []

    class Picky(kb.Provider):
        def records(self):
            return ["same"]

        def select(self, record):
            picked.append(record.key)

    merged = providers.Merge(Picky(), providers.Static(["same"]), prefix_keys=True)
    assert [r.key for r in merged.records()] == ["0:same", "1:same"]
    merged.select(kb.Record("same", key="0:same"))
    merged.select(kb.Record("same", key="1:same"))
    assert picked == ["same"]
    assert daemon.calls == [("copy", "same")]
    with pytest.raises(ValueError, match="prefix_keys"):
        providers.Merge(providers.Static(["a"]), providers.Static(["a"])).records()


def test_fields_wire_their_path_and_fallback():
    field = ui.Field("sender.name", fallback="?", text_color="#fff")
    assert field._wire()["kind"] == "field"
    assert field._wire()["text"] == "sender.name"
    assert field._wire()["fallback"] == "?"
    with pytest.raises(TypeError):
        ui.Field("")


def note(id, app="mail", summary="Hi", urgency=1, **rest):  # noqa: A002
    return {"id": id, "app_name": app, "summary": summary, "body": "", "app_icon": "",
            "actions": [], "hints": {}, "urgency": urgency, "timeout": -1, "replaces": False, **rest}


def test_routes_show_drop_and_stash(daemon, capsys):
    spam = notifications.Stash("spam")

    def route(n):
        if n.app_name == "noise":
            return notifications.DROP
        if n.critical:
            return [notifications.SHOW, spam]
        if n.app_name == "broken":
            raise RuntimeError("boom")
        return notifications.STASH

    inbox = notifications.serve(route, timeout=2.5, location="south")
    config = _registry.wire()
    assert config["notifications"] == {"timeout": 2.5, "location": "south", "replace": False, "max_visible": 5}
    assert config["route_notifications"] is True
    d = config["dispatcher"]
    d.notification(note(1, app="noise"))
    d.notification(note(2, urgency=2))
    d.notification(note(3))
    d.notification(note(4, app="broken"))
    assert [call for call in daemon.calls if call[0].startswith("notification")] == [
        ("notification_close", (1, 1)),
        ("notification_show", 2),
        ("notification_close", (3, 1)),
        ("notification_show", 4),
    ]
    assert "route failed" in capsys.readouterr().err
    assert [r.key for r in inbox] == ["3"]
    assert [r.key for r in spam] == ["2"]
    assert inbox.unread_count == 1


def test_stashes_mark_read_and_answer_varlink_calls(daemon):
    inbox = notifications.serve()
    assert _registry.wire()["route_notifications"] is False
    inbox.stash(notifications.Notification._from_wire(note(7, summary="one")))
    inbox.stash(notifications.Notification._from_wire(note(8, summary="two")))
    kb.widget("inbox", inbox.unread)
    d = _registry.wire()["dispatcher"]
    listed = d.call("StashList", {"stash": "notifications", "unread_only": True})
    assert [r["key"] for r in listed["ok"]["records"]] == ["8", "7"]
    assert listed["ok"]["records"][0]["data"]["summary"] == "two"
    d.select("inbox", [("8", "mail: two", "two", None)])
    assert [r.key for r in inbox.unread.records()] == ["7"]
    assert d.call("StashMarkRead", {"stash": "notifications", "keys": None}) == {"ok": {"marked": 1}}
    assert d.call("StashList", {"stash": "nope"})["error"] == "UnknownStash"
    with pytest.raises(ValueError, match="more than once"):
        notifications.serve()


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


def test_rpc_client_calls_streams_and_raises(varlink_server):
    path, received, replies = varlink_server
    replies["Prompt"] = [{"parameters": {"values": ["a"]}}]
    replies["Subscribe"] = [
        {"parameters": {"event": {"kind": "ready"}}, "continues": True},
        {"parameters": {"event": {"kind": "changed", "widget": "w"}}, "continues": True},
        {"parameters": {"event": {"kind": "changed", "widget": "v"}}},
    ]
    replies["Show"] = [{"error": "org.wontfix.klemmbrett.UnknownWidget", "parameters": {"widget": "x", "known": []}}]
    with rpc.Client(path) as client:
        assert client.prompt("w", ["a", {"value": "b", "n": 1}], pointer=(1, 2)) == ["a"]
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
    assert received[1]["more"] is True


def test_rpc_client_reports_a_missing_daemon(tmp_path):
    with pytest.raises(ConnectionError, match="not running"):
        rpc.Client(tmp_path / "none.sock")
