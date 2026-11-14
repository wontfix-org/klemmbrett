import logging
import subprocess

import pytest

import klemmbrett as kb


def _messages(caplog: pytest.LogCaptureFixture) -> list[str]:
    return [record.message for record in caplog.records]


def test_run_logs_the_command_and_uncaptured_stderr(caplog: pytest.LogCaptureFixture):
    caplog.set_level(logging.INFO, logger="klemmbrett")

    with pytest.raises(subprocess.CalledProcessError) as raised:
        kb.run(["sh", "-c", "echo failed >&2; exit 3"])

    command = "run sh -c 'echo failed >&2; exit 3'"
    assert raised.value.returncode == 3
    assert raised.value.stderr == b"failed\n"
    assert _messages(caplog) == [command, f"{command}: failed"]
    assert {record.levelno for record in caplog.records} == {logging.INFO}
    assert {record.name for record in caplog.records} == {"klemmbrett"}


def test_run_returns_and_logs_each_stderr_line(caplog: pytest.LogCaptureFixture):
    caplog.set_level(logging.INFO, logger="klemmbrett")

    result = kb.run(["sh", "-c", "echo one >&2; echo two >&2"], text=True)

    command = "run sh -c 'echo one >&2; echo two >&2'"
    assert result.returncode == 0
    assert result.stderr == "one\ntwo\n"
    assert _messages(caplog) == [command, f"{command}: one", f"{command}: two"]


def test_run_can_keep_a_failing_exit():
    result = kb.run(["false"], check=False, stderr=subprocess.DEVNULL)
    assert result.returncode == 1


def test_run_leaves_caller_captured_stderr_out_of_the_log(caplog: pytest.LogCaptureFixture):
    caplog.set_level(logging.INFO, logger="klemmbrett")

    result = kb.run(
        ["sh", "-c", "echo out; echo err >&2"],
        capture_output=True,
        text=True,
    )

    assert (result.stdout, result.stderr) == ("out\n", "err\n")
    assert _messages(caplog) == ["run sh -c 'echo out; echo err >&2'"]


def test_run_leaves_discarded_stderr_out_of_the_log(caplog: pytest.LogCaptureFixture):
    caplog.set_level(logging.INFO, logger="klemmbrett")

    result = kb.run(["sh", "-c", "echo hidden >&2"], stderr=subprocess.DEVNULL)

    assert result.stderr is None
    assert _messages(caplog) == ["run sh -c 'echo hidden >&2'"]


def test_run_logs_a_quoted_command_and_not_stdin(caplog: pytest.LogCaptureFixture):
    caplog.set_level(logging.INFO, logger="klemmbrett")

    kb.run(["echo", "a b"], input=b"secret-token", stdout=subprocess.DEVNULL)

    assert _messages(caplog) == ["run echo 'a b'"]
    assert "secret-token" not in caplog.text
