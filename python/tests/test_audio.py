"""PulseAudio and PipeWire sinks and sources."""

from __future__ import annotations

import json

import pytest

from klemmbrett import audio, providers
from klemmbrett import ranking as rk
from klemmbrett._record import Record

PULSE_INFO = json.dumps(
    {
        "server_name": "pulseaudio",
        "default_sink_name": "speakers",
        "default_source_name": "mic",
    }
)
PULSE_SINKS = json.dumps(
    [
        {"name": "speakers", "description": "Speakers"},
        {"name": "hdmi", "description": "HDMI"},
    ]
)
PIPEWIRE = """\
PipeWire 'pipewire-0' [1.2.7, mbra@box, cookie:123]
 └─ Clients:
        31. pipewire                            [1.2.7, mbra@box, pid:900]

Audio
 ├─ Devices:
 │      40. Built-in Audio                      [alsa]
 ├─ Sinks:
 │  *   48. Speakers                            [vol: 0.40]
 │      60. HDMI                                [vol: 1.00]
 ├─ Sources:
 │      49. Microphone                          [vol: 1.00]
 ├─ Filters:
 └─ Streams:

Video
 ├─ Devices:
 │      70. Camera                              [v4l2]
 ├─ Sinks:
 │      72. Video Sink
 ├─ Sources:
 │  *   71. Camera                              [vol: 1.00]
 └─ Streams:
"""
PIPEWIRE_NAMES = """\
Audio
 ├─ Sinks:
 │  *   48. alsa_output.speakers
 │      60. alsa_output.hdmi
 ├─ Sources:
 │      49. alsa_input.mic
Video
 ├─ Sources:
 │  *   71. v4l2_input.camera
"""


def test_pulse_and_pipewire_sinks_are_both_listed_and_one_is_default():
    rows = {
        record.key: record
        for record in audio.devices_from(
            "sink",
            pulse_info=PULSE_INFO,
            pulse_list=PULSE_SINKS,
            pipewire=PIPEWIRE,
            pipewire_names=PIPEWIRE_NAMES,
        )
    }
    assert rows["pulse:speakers"].label == "Speakers (PulseAudio)"
    assert rows["pulse:speakers"].data["default"] is True
    assert rows["pipewire:alsa_output.speakers"].label == "Speakers (PipeWire)"
    assert rows["pipewire:alsa_output.speakers"].data["id"] == "48"
    assert rows["pipewire:alsa_output.speakers"].data["default"] is True
    assert "pipewire:v4l2_input.camera" not in rows
    assert [record.label for record in rows.values()] == [
        "Speakers (PipeWire)",
        "Speakers (PulseAudio)",
        "HDMI (PipeWire)",
        "HDMI (PulseAudio)",
    ]


def test_pipewire_pulse_rows_are_left_out_when_wpctl_answered():
    info = json.dumps({"server_name": "PulseAudio (on PipeWire 1.2.7)", "default_sink_name": "speakers"})
    rows = audio.devices_from(
        "sink",
        pulse_info=info,
        pulse_list=PULSE_SINKS,
        pipewire=PIPEWIRE,
        pipewire_names=PIPEWIRE_NAMES,
    )
    assert [record.label for record in rows] == ["Speakers", "HDMI"]
    assert [record.data["backend"] for record in rows] == ["pipewire", "pipewire"]


def test_sources_skip_the_video_section():
    rows = audio.devices_from(
        "source",
        pulse_info=PULSE_INFO,
        pulse_list=json.dumps([{"name": "mic", "description": "Microphone"}]),
        pipewire=PIPEWIRE,
        pipewire_names=PIPEWIRE_NAMES,
    )
    assert [record.value for record in rows] == ["mic", "alsa_input.mic"]
    assert all(record.data["kind"] == "source" for record in rows)


def test_a_missing_server_adds_no_rows():
    assert audio.devices_from("sink", pulse_info=None, pulse_list=None, pipewire=None, pipewire_names=None) == []


def test_activate_sets_the_default_on_the_matching_server():
    commands = []
    audio.activate(
        Record(
            "speakers",
            data={"backend": "pulse", "kind": "sink", "name": "speakers", "id": "speakers"},
        ),
        commands.append,
    )
    audio.activate(
        Record("alsa_output.speakers", data={"backend": "pipewire", "kind": "sink", "id": "48"}),
        commands.append,
    )
    assert commands == [
        ["pactl", "set-default-sink", "speakers"],
        ["wpctl", "set-default", "48"],
    ]
    with pytest.raises(RuntimeError):
        audio.activate(Record("nope"), commands.append)


def test_audio_sinks_and_sources_rank_by_use():
    assert providers.defaults(providers.AudioSinks()) == (rk.fuzzy(), rk.frecency() + rk.learned())
    assert providers.defaults(providers.AudioSources()) == (rk.fuzzy(), rk.frecency() + rk.learned())
