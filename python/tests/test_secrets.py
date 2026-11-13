import pytest

import klemmbrett as kb
from klemmbrett import _registry, layouts, secrets, ui


def wired(name):
    return next(w for w in _registry.wire()["widgets"] if w["name"] == name)


def test_agents_wire_their_kind_and_route():
    kb.widget("unlock", secrets.SystemdAgent())
    kb.widget("admin", secrets.Polkit(route=lambda question: None), layout=layouts.secret)
    unlock = wired("unlock")
    assert unlock["agent"] == {"kind": "systemd", "route": False}
    assert unlock["source"] is None
    assert wired("admin")["agent"] == {"kind": "polkit", "route": True}
    kb.widget("plain", ["a"])
    assert wired("plain")["agent"] is None


def test_agent_widgets_refuse_what_would_see_the_answer():
    with pytest.raises(TypeError, match="on_select, multi"):
        kb.widget("ssh", secrets.Askpass(), on_select=print, multi=True)
    with pytest.raises(TypeError, match="rank"):
        kb.widget("gpg", secrets.Pinentry(), rank=None)
    with pytest.raises(TypeError, match="route must be callable"):
        secrets.Polkit(route="admin")
    with pytest.raises(TypeError, match="subclasses|use secrets"):
        secrets.Agent()


def test_only_shared_agents_may_repeat():
    kb.widget("ssh", secrets.Askpass())
    kb.widget("git", secrets.Askpass())
    kb.widget("admin", secrets.Polkit())
    with pytest.raises(ValueError, match="already answers polkit"):
        kb.widget("admin2", secrets.Polkit())


def test_route_sees_the_question_and_failures_keep_the_widget(capsys):
    seen = []

    def route(question):
        seen.append(question)
        return "mount" if question.action_id.startswith("org.freedesktop.udisks2.") else None

    kb.widget("admin", secrets.Polkit(route=route))
    kb.widget("mount", layout=layouts.secret)
    dispatcher = _registry.wire()["dispatcher"]
    question = {
        "kind": "polkit",
        "id": "org.freedesktop.udisks2.filesystem-mount",
        "title": "Authentication required",
        "message": "Mount a disk",
        "details": {"action_id": "org.freedesktop.udisks2.filesystem-mount", "user": "me"},
        "pid": None,
    }
    assert dispatcher.route_secret("admin", question) == "mount"
    assert seen[0].details["user"] == "me"
    assert seen[0].kind == "polkit"
    with pytest.raises(TypeError):
        seen[0].details["user"] = "root"
    assert dispatcher.route_secret("admin", {**question, "details": {}}) is None
    assert dispatcher.route_secret("mount", question) is None

    kb.widget("broken", secrets.Askpass(route=lambda question: 42))
    assert _registry.wire()["dispatcher"].route_secret("broken", {"kind": "askpass"}) is None
    assert "route failed" in capsys.readouterr().err


def test_entry_mask_and_secret_layout():
    assert ui.Entry(mask="*")._wire()["style"]["text"] == "*"
    with pytest.raises(TypeError, match="mask"):
        ui.Entry(mask="")
    window = layouts.secret(layouts.LayoutContext(widget="gpg", multi=False, prompt=True))
    wire = window._wire()
    assert wire["location"] == "center"
    assert [child["kind"] for child in wire["children"]] == ["text", "text", "text", "inputbar"]
    assert wire["children"][-1]["children"][1]["style"]["text"] == "•"
