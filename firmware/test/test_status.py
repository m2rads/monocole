"""Integration tests for the voice and status characteristics.

Run with the board powered and flashed:

    pytest -m hardware

These cover the parts of the contract that exist today: both characteristics
are present and notify-only, and status reports the panel's geometry as soon as
anyone subscribes. Voice frames themselves are milestone 3 — there is nothing
producing them yet, which is exactly why the test below asserts silence rather
than skipping.
"""

from __future__ import annotations

import asyncio

import pytest

from adpcm import FRAME_LEN, decode_frame
from protocol import (
    STATUS_PANEL_GEOMETRY,
    STATUS_UUID,
    VOICE_UUID,
)

pytestmark = pytest.mark.hardware

# Generous: the notification is sent from the subscribe callback, so it is
# really a round trip plus one connection interval.
GEOMETRY_TIMEOUT_S = 5.0

# Long enough that a device wrongly streaming audio would show up.
VOICE_SILENCE_S = 3.0


class TestLayout:
    async def test_both_characteristics_are_present(self, client):
        uuids = {c.uuid.lower() for c in client.services.characteristics.values()}
        assert VOICE_UUID in uuids, (
            "voice characteristic missing — if this build added it, a bonded "
            "Mac may still be serving a cached table; check the boot log says "
            "'service changed characteristic at handle N'"
        )
        assert STATUS_UUID in uuids

    async def test_voice_is_notify_only(self, client):
        voice = client.services.get_characteristic(VOICE_UUID)
        assert "notify" in voice.properties
        # A readable or writable voice channel would let any client pull audio
        # off a microphone worn on someone's face.
        assert "read" not in voice.properties
        assert "write" not in voice.properties

    async def test_status_is_notify_only(self, client):
        status = client.services.get_characteristic(STATUS_UUID)
        assert "notify" in status.properties
        assert "read" not in status.properties
        assert "write" not in status.properties


class TestPanelGeometry:
    async def test_geometry_arrives_on_subscribe(self, client):
        received: asyncio.Queue[bytes] = asyncio.Queue()

        def on_notify(_handle, data: bytearray):
            received.put_nowait(bytes(data))

        await client.start_notify(STATUS_UUID, on_notify)
        try:
            payload = await asyncio.wait_for(
                received.get(), timeout=GEOMETRY_TIMEOUT_S
            )
        finally:
            await client.stop_notify(STATUS_UUID)

        assert payload[0] == STATUS_PANEL_GEOMETRY
        assert len(payload) == 3, f"expected event + cols + rows, got {payload!r}"

        cols, rows = payload[1], payload[2]
        # The panel is a 128x64 SSD1306 with a 6x8 cell today, but nothing
        # outside display.c is supposed to assume that — so this asserts the
        # values are sane rather than exact, and would survive the micro-LED
        # it is standing in for.
        assert 0 < cols <= 128
        assert 0 < rows <= 64


class TestVoice:
    async def test_no_frames_until_the_mic_exists(self, client):
        """Subscribing must be harmless, not a source of junk.

        Milestone 3 replaces this with a test that speaks and decodes. Until
        then the useful assertion is that nothing arrives: a device sending
        malformed frames from an uninitialised buffer would fail here.
        """
        received: list[bytes] = []

        await client.start_notify(
            VOICE_UUID, lambda _h, data: received.append(bytes(data))
        )
        try:
            await asyncio.sleep(VOICE_SILENCE_S)
        finally:
            await client.stop_notify(VOICE_UUID)

        # If frames ever do arrive here, they must at least be the shape the
        # protocol promises — so this test keeps working once the mic lands.
        for frame in received:
            assert len(frame) == FRAME_LEN, (
                f"frame is {len(frame)} bytes, expected {FRAME_LEN}"
            )
            decode_frame(frame)

        assert received == [], (
            f"{len(received)} voice frames arrived with no mic pipeline built"
        )
