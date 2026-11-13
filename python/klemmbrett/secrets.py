"""Password agents: widgets that answer secret questions.

Give an agent as a widget's provider, and that widget asks the questions
the agent receives::

    from klemmbrett import secrets, layouts

    kb.widget("unlock", secrets.SystemdAgent())       # systemd-ask-password --user
    kb.widget("ssh", secrets.Askpass())                # klemmbrett-askpass
    kb.widget("gpg", secrets.Pinentry(), layout=layouts.secret)
    kb.widget("admin", secrets.Polkit())               # pkexec, systemctl, udisks, ...

Answers go straight back to the asker. They never reach Python, `watch`,
handlers, history, or usage, so agent widgets take no `on_select`,
`on_highlight`, `multi`, `match`, or `rank`.

`route` sends a question to another widget, chosen from what is known
about it (never the answer). The target shows it with its own layout::

    def admin(question: secrets.Question) -> str | None:
        if question.action_id.startswith("org.freedesktop.udisks2."):
            return "mount"
        return None  # this widget

    kb.widget("admin", secrets.Polkit(route=admin))
    kb.widget("mount", layout=layouts.secret)

At most one widget may hold a `SystemdAgent` or `Polkit` agent. Several
may hold `Askpass` or `Pinentry`; the first answers, unless the asker's
`KLEMMBRETT_WIDGET` names another.
"""

from __future__ import annotations

from collections.abc import Callable, Mapping
from dataclasses import dataclass, field
from types import MappingProxyType
from typing import Any

__all__ = ["Agent", "Askpass", "Pinentry", "Polkit", "Question", "SystemdAgent"]


@dataclass(frozen=True)
class Question:
    """What a `route` function knows about a question."""

    #: `"systemd"`, `"askpass"`, `"pinentry"`, or `"polkit"`.
    kind: str
    #: Identifies the question to its asker, such as `cryptsetup:/dev/sda2`
    #: or a polkit action id.
    id: str
    title: str
    message: str
    details: Mapping[str, str] = field(default_factory=dict)
    #: The process asking, when known.
    pid: int | None = None

    @property
    def action_id(self) -> str:
        """The polkit action, such as `org.freedesktop.systemd1.manage-units`."""
        return self.details.get("action_id", "")

    @property
    def program(self) -> str:
        """The program polkit runs (`pkexec`), when it says."""
        return self.details.get("program", "") or self.details.get("command_line", "")

    @classmethod
    def _from_wire(cls, wire: dict[str, Any]) -> Question:
        return cls(
            kind=wire.get("kind", ""),
            id=wire.get("id", ""),
            title=wire.get("title", ""),
            message=wire.get("message", ""),
            details=MappingProxyType(dict(wire.get("details") or {})),
            pid=wire.get("pid"),
        )


Route = Callable[[Question], "str | None"]


class Agent:
    """Base of the agents; use one of its subclasses."""

    kind = ""
    #: Whether more than one widget may hold this kind of agent.
    shared = False

    def __init__(self, *, route: Route | None = None) -> None:
        if type(self) is Agent:
            raise TypeError("use secrets.SystemdAgent, Askpass, Pinentry, or Polkit")
        if route is not None and not callable(route):
            raise TypeError(f"{type(self).__name__}: route must be callable, got {route!r}")
        self.route = route

    def _route(self, question: Question) -> str | None:
        if self.route is None:
            return None
        target = self.route(question)
        if target is not None and not isinstance(target, str):
            raise TypeError(f"route returned {target!r}, not a widget name or None")
        return target

    def _wire(self) -> dict[str, Any]:
        return {"kind": self.kind, "route": self.route is not None}

    def __repr__(self) -> str:
        route = "" if self.route is None else f"route={self.route!r}"
        return f"secrets.{type(self).__name__}({route})"


class SystemdAgent(Agent):
    """Answers `systemd-ask-password --user`: services decrypting keys,
    `systemd-creds --user`, and anything else asking on
    `$XDG_RUNTIME_DIR/systemd/ask-password`."""

    kind = "systemd"


class Askpass(Agent):
    """Answers `klemmbrett-askpass`, set as `SSH_ASKPASS`, `SUDO_ASKPASS`,
    or `GIT_ASKPASS`."""

    kind = "askpass"
    shared = True


class Pinentry(Agent):
    """Answers `klemmbrett-pinentry`, set as gpg-agent's `pinentry-program`."""

    kind = "pinentry"
    shared = True


class Polkit(Agent):
    """The session's polkit authentication agent, for `pkexec`,
    `systemctl`, `udisksctl`, and other privileged actions. Only one agent
    serves a session: stop others (polkit-gnome, lxpolkit, ...) first."""

    kind = "polkit"
