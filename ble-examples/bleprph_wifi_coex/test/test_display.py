"""Tests for the display characteristic.

Split like the rest of this suite: the codec tests always run, the hardware
tests need the board.

What cannot be asserted from here: **what is actually on the panel.** The
characteristic is write-only and the firmware exposes no read-back, so these
tests prove the device accepts what it should, rejects what it should, and
survives both. Confirming that "connected" is legible on glass is a human
looking at it — see the manual checks at the bottom of this file.
"""

from __future__ import annotations

import asyncio

import pytest

from display import (
    DISPLAY_UUID,
    MALFORMED_PAYLOADS,
    OP_APPEND,
    OP_CLEAR,
    OP_SET,
    TEXT_MAX,
    encode,
)
from protocol import WIFI_STATE_UUID, decode_state

# --- codec -------------------------------------------------------------------


class TestEncode:
    def test_op_leads_the_payload(self):
        assert encode(OP_SET, "hi") == b"\x01hi"

    def test_clear_carries_no_text(self):
        assert encode(OP_CLEAR) == b"\x00"

    def test_append_is_distinct_from_set(self):
        # Streaming tokens depend on these not being confused: one replaces
        # the screen, the other adds to it.
        assert encode(OP_APPEND, "x")[0] != encode(OP_SET, "x")[0]

    def test_lengths_count_bytes_not_characters(self):
        # "é" is two UTF-8 bytes. Counting characters would let a payload
        # overrun what a single ATT write can carry.
        assert len(encode(OP_SET, "é")) == 1 + 2

    def test_maximum_text_fits_one_att_write(self):
        payload = encode(OP_SET, "x" * TEXT_MAX)
        assert len(payload) == 253, "an ATT write at MTU 256 carries 253 bytes"

    def test_text_beyond_the_maximum_is_refused(self):
        # The app splits rather than truncating: a cut inside a UTF-8
        # character loses exactly what the wearer was meant to read.
        with pytest.raises(ValueError):
            encode(OP_SET, "x" * (TEXT_MAX + 1))

    def test_unknown_op_is_refused(self):
        with pytest.raises(ValueError):
            encode(9, "hi")


# --- hardware ----------------------------------------------------------------


@pytest.mark.hardware
class TestGattLayout:
    async def test_characteristic_is_present(self, client):
        uuids = {c.uuid.lower() for c in client.services.characteristics.values()}
        assert DISPLAY_UUID in uuids, f"display missing; found {sorted(uuids)}"

    async def test_display_is_write_only(self, client):
        display = client.services.get_characteristic(DISPLAY_UUID)
        assert "write" in display.properties
        # Nothing reads the panel back; a readable one would only be state to
        # keep in sync.
        assert "read" not in display.properties
        assert "notify" not in display.properties

    async def test_display_cannot_be_read(self, client):
        with pytest.raises(Exception):
            await client.read_gatt_char(DISPLAY_UUID)


@pytest.mark.hardware
class TestWrites:
    async def test_set_is_accepted(self, client):
        await client.write_gatt_char(DISPLAY_UUID, encode(OP_SET, "test"), response=True)

    async def test_clear_is_accepted(self, client):
        await client.write_gatt_char(DISPLAY_UUID, encode(OP_CLEAR), response=True)

    async def test_append_is_accepted(self, client):
        await client.write_gatt_char(DISPLAY_UUID, encode(OP_SET, "a"), response=True)
        await client.write_gatt_char(DISPLAY_UUID, encode(OP_APPEND, "b"), response=True)

    async def test_full_size_write_lands_in_one_transaction(self, client):
        """253 bytes must not need a long write, which the firmware does not
        reassemble."""
        await client.write_gatt_char(
            DISPLAY_UUID, encode(OP_SET, "x" * TEXT_MAX), response=True
        )

    async def test_empty_text_is_accepted(self, client):
        # A set with no text is how the app blanks the panel without clearing.
        await client.write_gatt_char(DISPLAY_UUID, encode(OP_SET, ""), response=True)

    async def test_rapid_writes_do_not_wedge_the_device(self, client):
        """The render queue is four deep and drops when full; overflowing it
        must cost frames, not the connection."""
        for i in range(20):
            await client.write_gatt_char(
                DISPLAY_UUID, encode(OP_SET, f"frame {i}"), response=True
            )

        # Still serving GATT afterwards.
        await client.write_gatt_char(DISPLAY_UUID, encode(OP_SET, "done"), response=True)


@pytest.mark.hardware
class TestMalformedWrites:
    @pytest.mark.parametrize(
        "description,payload",
        MALFORMED_PAYLOADS,
        ids=[d for d, _ in MALFORMED_PAYLOADS],
    )
    async def test_rejected(self, client, description, payload):
        with pytest.raises(Exception):
            await client.write_gatt_char(DISPLAY_UUID, payload, response=True)

    async def test_device_survives_every_malformed_payload(self, client, states):
        """A rejected write must not take the device with it.

        Replays all of them, then proves the chip is still serving GATT — an
        unhandled length or op would show up here as a dead connection rather
        than a failed assertion above.
        """
        for _description, payload in MALFORMED_PAYLOADS:
            try:
                await client.write_gatt_char(DISPLAY_UUID, payload, response=True)
            except Exception:
                pass  # rejection is the expected case; we care about survival

        await client.write_gatt_char(DISPLAY_UUID, encode(OP_SET, "alive"), response=True)
        assert client.is_connected


@pytest.mark.hardware
class TestServiceChanged:
    """The GATT table version is what stops a bonded central caching a table
    that no longer exists. If this characteristic is visible at all, the
    mechanism worked — a stale cache from before the display was added would
    make it absent."""

    async def test_display_is_visible_to_a_bonded_central(self, client):
        uuids = {c.uuid.lower() for c in client.services.characteristics.values()}
        assert DISPLAY_UUID in uuids, (
            "the display characteristic is missing from a bonded central's view.\n"
            "  Either MONOCLE_GATT_VERSION was not bumped when it was added, or\n"
            "  the Service Changed indication is not reaching this machine.\n"
            "  Forgetting the device in System Settings > Bluetooth is the\n"
            "  workaround; fixing the version is the fix."
        )


# --- not covered here --------------------------------------------------------
#
# - **What the panel shows.** Write-only characteristic, no read-back: the
#   only way to know "connected" is legible is to look at it. Worth doing
#   after any change to the renderer in display.c.
# - **Wrapping and scrolling.** Same reason. The interesting cases to eyeball
#   are a word longer than 21 characters, text past 8 lines, and an append
#   that overflows the 512-byte buffer (the oldest text should scroll off, not
#   the newest).
# - **The encryption gate.** display is WRITE_ENC, but CoreBluetooth pairs
#   transparently on the first refused write, so a host-side test cannot see
#   the refusal. Verify in the serial log: `encryption change event; status=0`
#   must precede `display write:`.
