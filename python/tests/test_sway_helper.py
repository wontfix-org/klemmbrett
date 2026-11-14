"""The Sway pointer helper."""

from __future__ import annotations

import subprocess
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[2] / "klemmbrett.data" / "scripts" / "klemmbrett-sway"


def test_sway_helper_requires_one_widget():
    missing = subprocess.run([SCRIPT], capture_output=True, text=True)
    assert missing.returncode == 2
    assert "usage: klemmbrett-sway WIDGET" in missing.stderr

    extra = subprocess.run([SCRIPT, "history", "run"], capture_output=True, text=True)
    assert extra.returncode == 2
