"""NetworkManager connection profiles from nmcli."""

from __future__ import annotations

import pytest

from klemmbrett import network, providers
from klemmbrett import ranking as rk
from klemmbrett._record import Record

# nmcli -t -e yes -f NAME,UUID,TYPE,DEVICE,ACTIVE connection show
# Cafe\: guest is the name "Cafe: guest". odd\\net is the name "odd\net".
# An empty device and "--" both mean the profile is not on a device.
LISTING = """\
Wired connection 1:11111111-1111-1111-1111-111111111111:802-3-ethernet:enp0s3:yes
Home:22222222-2222-2222-2222-222222222222:802-11-wireless:wlp2s0:yes
Cafe\\: guest:33333333-3333-3333-3333-333333333333:802-11-wireless:--:no
Work:44444444-4444-4444-4444-444444444444:vpn::no
odd\\\\net:55555555-5555-5555-5555-555555555555:wireguard:wg0:yes
lo:66666666-6666-6666-6666-666666666666:loopback:lo:yes
Phone:77777777-7777-7777-7777-777777777777:gsm:--:no
"""


def test_connections_unescape_names_and_list_active_ones_first():
    rows = {record.key: record for record in network.connections_from(LISTING)}
    home = rows["22222222-2222-2222-2222-222222222222"]
    assert home.label == "Home (wifi, on)"
    assert home.value == "Home"
    assert home.data == {
        "name": "Home",
        "uuid": "22222222-2222-2222-2222-222222222222",
        "type": "802-11-wireless",
        "kind": "wifi",
        "device": "wlp2s0",
        "active": True,
    }
    cafe = rows["33333333-3333-3333-3333-333333333333"]
    assert cafe.label == "Cafe: guest (wifi, off)"
    assert cafe.data["name"] == "Cafe: guest"
    assert cafe.data["device"] == ""
    assert cafe.data["active"] is False
    work = rows["44444444-4444-4444-4444-444444444444"]
    assert work.label == "Work (vpn, off)"
    assert work.data["device"] == ""
    assert work.data["type"] == "vpn"
    odd = rows["55555555-5555-5555-5555-555555555555"]
    assert odd.data["name"] == "odd\\net"
    assert odd.label == "odd\\net (wireguard, on)"
    assert rows["77777777-7777-7777-7777-777777777777"].data["kind"] == "mobile"
    assert [record.label for record in network.connections_from(LISTING)] == [
        "Home (wifi, on)",
        "lo (loopback, on)",
        "odd\\net (wireguard, on)",
        "Wired connection 1 (ethernet, on)",
        "Cafe: guest (wifi, off)",
        "Phone (mobile, off)",
        "Work (vpn, off)",
    ]


def test_a_blank_listing_and_a_bad_row_add_nothing(caplog):
    assert network.connections_from(None) == []
    assert network.connections_from("   \n") == []
    assert network.connections_from("only-three:fields:here\n") == []
    assert "3 fields" in caplog.text


def test_toggle_brings_an_inactive_connection_up_and_an_active_one_down():
    commands = []
    network.toggle(
        Record("Home", key="uuid-1", data={"uuid": "uuid-1", "active": False}),
        commands.append,
    )
    network.toggle(
        Record("Home", key="uuid-1", data={"active": True}),
        commands.append,
    )
    assert commands == [
        ["nmcli", "connection", "up", "uuid", "uuid-1"],
        ["nmcli", "connection", "down", "uuid", "uuid-1"],
    ]
    with pytest.raises(RuntimeError):
        network.toggle(Record("nope", key=""), commands.append)


def test_select_toggles_through_nmcli(monkeypatch):
    commands = []
    monkeypatch.setattr(providers._runtime, "run", lambda args, **kwargs: commands.append(list(args)))
    providers.NetworkConnections().select(
        Record("Home", key="uuid-1", data={"uuid": "uuid-1", "active": True})
    )
    assert commands == [["nmcli", "connection", "down", "uuid", "uuid-1"]]


def test_network_connections_rank_by_use():
    assert providers.defaults(providers.NetworkConnections()) == (rk.fuzzy(), rk.frecency() + rk.learned())
