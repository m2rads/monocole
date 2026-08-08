"""Wi-Fi data plane tests, and the throughput measurement the architecture
rests on.

Requires hardware plus real credentials, since the chip has to actually join a
network before it has an address to serve on:

    pytest --ssid "MyNetwork" --password "hunter2" -m hardware -s

``-s`` is worth passing: the throughput test prints its number.
"""

from __future__ import annotations

import socket
import struct
import time

import pytest

from dataplane import (
    FRAME_BULK_DATA,
    FRAME_ECHO_REQ,
    HEADER_LEN,
    MAX_PAYLOAD,
    DataPlane,
    encode_frame,
)
from protocol import STATE_CONNECTED, WIFI_CREDS_UUID, encode_creds

JOIN_TIMEOUT_S = 45.0

# The firmware's MONOCLE_IDLE_TIMEOUT_MS.
IDLE_TIMEOUT_S = 30.0


# --- framing (no hardware) ---------------------------------------------------


class TestFraming:
    def test_length_covers_the_type_byte(self):
        assert encode_frame(FRAME_ECHO_REQ, b"hi") == b"\x00\x00\x00\x03\x01hi"

    def test_empty_payload_still_has_length_one(self):
        frame = encode_frame(FRAME_BULK_DATA)
        assert frame == b"\x00\x00\x00\x01\x04"
        assert len(frame) == HEADER_LEN

    def test_maximum_payload_length_is_representable(self):
        frame = encode_frame(FRAME_BULK_DATA, b"x" * MAX_PAYLOAD)
        length = struct.unpack(">I", frame[:4])[0]
        assert length == MAX_PAYLOAD + 1


# --- hardware ----------------------------------------------------------------


@pytest.fixture
async def device_ip(client, states, credentials) -> str:
    """Provisions the chip and returns the address it reports."""
    ssid, password = credentials
    await client.write_gatt_char(
        WIFI_CREDS_UUID, encode_creds(ssid, password), response=True
    )
    state = await states.wait_for(STATE_CONNECTED, timeout=JOIN_TIMEOUT_S)
    assert state.ip
    return state.ip


@pytest.mark.hardware
@pytest.mark.join
class TestDataPlane:
    async def test_server_accepts_a_connection(self, device_ip):
        with DataPlane(device_ip):
            pass  # connecting at all is the assertion

    async def test_echo_round_trips(self, device_ip):
        with DataPlane(device_ip) as plane:
            assert plane.echo(b"minicole") == b"minicole"

    async def test_echo_handles_a_full_size_payload(self, device_ip):
        """8 KB spans many TCP segments; the firmware must reassemble it."""
        payload = bytes(range(256)) * (MAX_PAYLOAD // 256)
        with DataPlane(device_ip) as plane:
            assert plane.echo(payload) == payload

    async def test_echo_is_repeatable_on_one_connection(self, device_ip):
        """Frame boundaries must stay aligned across many messages."""
        with DataPlane(device_ip) as plane:
            for i in range(20):
                payload = f"message-{i}".encode()
                assert plane.echo(payload) == payload

    async def test_reconnecting_works(self, device_ip):
        """SO_REUSEADDR: a second connection must not hit TIME_WAIT."""
        for _ in range(3):
            with DataPlane(device_ip) as plane:
                assert plane.echo(b"ping") == b"ping"

    async def test_oversized_frame_is_rejected_without_crashing(self, device_ip):
        """A length header the firmware must refuse rather than allocate for."""
        with DataPlane(device_ip) as plane:
            plane._sock.sendall(struct.pack(">IB", MAX_PAYLOAD + 100, FRAME_ECHO_REQ))
            plane._sock.settimeout(5.0)
            with pytest.raises((ConnectionError, socket.timeout, OSError)):
                plane._recv()

        # The server must still be serving after dropping that client.
        time.sleep(1.0)
        with DataPlane(device_ip) as plane:
            assert plane.echo(b"still alive") == b"still alive"


@pytest.mark.hardware
@pytest.mark.join
class TestThroughput:
    """The measurement the two-radio split depends on.

    BLE is connected throughout — that is the point. Numbers taken with BLE
    idle would not reflect the shipping configuration.
    """

    @pytest.mark.parametrize("size_kb", [16, 100, 512])
    async def test_bulk_transfer_is_correct_and_timed(self, device_ip, size_kb, states):
        total = size_kb * 1024
        with DataPlane(device_ip, timeout=60.0) as plane:
            transfer = plane.bulk(total)

        assert transfer.total_bytes == total
        print(
            f"\n  {size_kb:>4} KB in {transfer.elapsed_s * 1000:7.0f} ms  "
            f"= {transfer.mbps:5.2f} Mbps  ({transfer.frames} frames)"
        )

    async def test_typical_jpeg_still_is_fast_enough(self, device_ip):
        """100 KB is a representative QVGA/VGA still.

        The architecture claims this beats BLE's 1-3 s by enough to justify a
        second radio. If this fails, that premise needs revisiting — see
        docs/firmware-plan.md.
        """
        with DataPlane(device_ip, timeout=60.0) as plane:
            transfer = plane.bulk(100 * 1024)

        print(f"\n  100 KB still: {transfer.elapsed_s * 1000:.0f} ms "
              f"({transfer.mbps:.2f} Mbps)")
        assert transfer.elapsed_s < 1.0, (
            f"100 KB took {transfer.elapsed_s:.2f}s; BLE would manage this in "
            "1-3s, so the Wi-Fi plane is not earning its power budget"
        )


@pytest.mark.hardware
@pytest.mark.join
@pytest.mark.slow
class TestIdleTeardown:
    """The power model: Wi-Fi must not stay up between bursts."""

    async def test_radio_powers_down_when_idle(self, device_ip, states):
        # Nothing connects for longer than the idle timeout.
        state = await states.wait_for(0, timeout=IDLE_TIMEOUT_S + 20.0)
        assert state.state == 0, "expected an idle notification after teardown"

        # And the socket must genuinely be gone, not merely reported closed.
        with pytest.raises((ConnectionError, socket.timeout, OSError)):
            with DataPlane(device_ip, timeout=5.0) as plane:
                plane.echo(b"should not reach a powered-down radio")
