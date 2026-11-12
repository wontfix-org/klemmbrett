from __future__ import annotations

from typing import Any

import pytest

from klemmbrett import _registry, _runtime


class FakeBackend:
    """Records the operations handlers ask the daemon for."""

    def __init__(self) -> None:
        self.calls: list[tuple[str, Any]] = []

    def copy(self, text: str) -> None:
        self.calls.append(("copy", text))

    def launch_command(self, argv: list[str]) -> None:
        self.calls.append(("launch", argv))

    def activate_window(self, key: str) -> None:
        self.calls.append(("activate_window", key))

    def set_theme(self, theme: Any) -> None:
        self.calls.append(("set_theme", theme))

    def show(self, widget: str, records: Any, pointer: Any) -> None:
        self.calls.append(("show", (widget, records, pointer)))

    def refresh(self, widgets: list[str]) -> None:
        self.calls.append(("refresh", widgets))

    def notification_show(self, id: int) -> None:  # noqa: A002
        self.calls.append(("notification_show", id))

    def notification_close(self, id: int, reason: int) -> None:  # noqa: A002
        self.calls.append(("notification_close", (id, reason)))

    def quit(self) -> None:
        self.calls.append(("quit", None))

    def is_running(self) -> bool:
        return True


@pytest.fixture
def daemon(monkeypatch: pytest.MonkeyPatch) -> FakeBackend:
    backend = FakeBackend()
    monkeypatch.setattr(_runtime, "backend", backend)
    return backend


@pytest.fixture(autouse=True)
def fresh_registry() -> None:
    _registry.reset()
