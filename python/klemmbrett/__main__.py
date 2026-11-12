"""The `klemmbrett` command."""

from __future__ import annotations

import os
import shutil
import signal
import sys

from . import _core, _registry


def _self_command() -> list[str]:
    """How systemd should start klemmbrett again: the console script when it
    is on PATH as invoked, else this interpreter running the module."""
    script = sys.argv[0] if sys.argv else ""
    if script and os.path.basename(script) == "klemmbrett":
        resolved = shutil.which(script) if os.sep not in script else script
        if resolved:
            return [os.path.abspath(resolved)]
    return [sys.executable, "-m", "klemmbrett"]


def main(argv: list[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    # The daemon runs in Rust; let Ctrl-C end it like any other program.
    signal.signal(signal.SIGINT, signal.SIG_DFL)
    return _core.main(["klemmbrett", *args], _registry.load, _self_command())


if __name__ == "__main__":
    sys.exit(main())
