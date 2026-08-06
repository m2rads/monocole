"""Integration tests against the real chip over BLE.

Run with the board powered and flashed:

    pytest -m hardware
    pytest --ssid "MyNetwork" --password "hunter2"    # adds the join tests

Every test here connects fresh, so a failure in one does not poison the next.
"""

from __future__ import annotations

import asyncio

import pytest

from protocol import (
    MALFORMED_PAYLOADS,
    PASS_MAX_LEN,
    SSID_MAX_LEN,
    SERVICE_UUID,
    STATE_CONNECTED,
    STATE_CONNECTING,
    STATE_FAILED,
    WIFI_CREDS_UUID,
    WIFI_STATE_UUID,
    encode_creds,
)

pytestmark = pytest.mark.hardware

# Joining a network involves a scan, auth, DHCP; and a failure only surfaces
# after the firmware's retry budget is spent.
JOIN_TIMEOUT_S = 45.0
FAIL_TIMEOUT_S = 60.0


class TestGattLayout:
    """The service must look the way docs/protocol.md says it does."""

    async def test_service_is_present(self, client):
        uuids = {service.uuid.lower() for service in client.services}
        assert SERVICE_UUID in uuids, f"service missing; found {sorted(uuids)}"

    async def test_characteristics_are_present(self, client):
        uuids = {c.uuid.lower() for c in client.services.characteristics.values()}
        assert WIFI_CREDS_UUID in uuids
        assert WIFI_STATE_UUID in uuids

    async def test_creds_is_write_only(self, client):
        creds = client.services.get_characteristic(WIFI_CREDS_UUID)
        assert "write" in creds.properties
        # Readable credentials would hand the passphrase to any client.
        assert "read" not in creds.properties
        assert "notify" not in creds.properties

    async def test_state_is_notify_only(self, client):
        state = client.services.get_characteristic(WIFI_STATE_UUID)
        assert "notify" in state.properties
        assert "write" not in state.properties

    async def test_state_cannot_be_read(self, client):
        """Notify-only: the access callback rejects reads rather than
        returning stale or uninitialised data."""
        with pytest.raises(Exception):
            await client.read_gatt_char(WIFI_STATE_UUID)


class TestInputValidation:
    """Every rejection here corresponds to a bounds check in
    gatt_svr_chr_access_wifi(). A crash means the chip stops advertising and
    later tests fail to connect — which is itself the signal."""

    @pytest.mark.parametrize(
        "payload",
        [pytest.param(p, id=label) for label, p in MALFORMED_PAYLOADS],
    )
    async def test_malformed_payload_is_rejected(self, client, payload):
        with pytest.raises(Exception):
            await client.write_gatt_char(WIFI_CREDS_UUID, payload, response=True)

    async def test_device_survives_malformed_writes(self, client):
        """The chip must still be responsive after being fed garbage."""
        for _, payload in MALFORMED_PAYLOADS:
            try:
                await client.write_gatt_char(WIFI_CREDS_UUID, payload, response=True)
            except Exception:
                pass

        assert client.is_connected
        # Still serving GATT, not just holding the link open.
        assert client.services.get_characteristic(WIFI_CREDS_UUID) is not None

    async def test_maximum_sized_payload_is_accepted(self, client, states):
        """97 bytes must land in a single ATT write. This fails if the
        negotiated MTU is too small or the firmware's buffer is undersized."""
        payload = encode_creds("a" * SSID_MAX_LEN, "b" * PASS_MAX_LEN)
        assert len(payload) == 97

        await client.write_gatt_char(WIFI_CREDS_UUID, payload, response=True)

        # The network won't exist, but the write itself must be accepted and
        # move the state machine.
        state = await states.wait_for(
            STATE_CONNECTING, STATE_FAILED, timeout=JOIN_TIMEOUT_S
        )
        assert state.state in (STATE_CONNECTING, STATE_FAILED)


class TestProvisioning:
    async def test_valid_write_reports_connecting(self, client, states):
        await client.write_gatt_char(
            WIFI_CREDS_UUID, encode_creds("minicole-test-network", "irrelevant"), response=True
        )
        state = await states.wait_for(STATE_CONNECTING, timeout=10.0)
        assert state.state == STATE_CONNECTING

    async def test_unknown_network_reports_failure_with_reason(self, client, states):
        """A network that cannot exist must end in a reported failure, not
        silence and not an endless retry loop."""
        await client.write_gatt_char(
            WIFI_CREDS_UUID,
            encode_creds("minicole-no-such-network-9zq", "irrelevant"),
            response=True,
        )
        state = await states.wait_for(STATE_FAILED, timeout=FAIL_TIMEOUT_S)
        assert state.reason is not None, "failure must carry a reason byte"

    @pytest.mark.join
    async def test_joins_real_network_and_reports_address(
        self, client, states, credentials
    ):
        ssid, password = credentials
        await client.write_gatt_char(
            WIFI_CREDS_UUID, encode_creds(ssid, password), response=True
        )

        state = await states.wait_for(STATE_CONNECTED, timeout=JOIN_TIMEOUT_S)
        assert state.ip is not None

        octets = [int(part) for part in state.ip.split(".")]
        assert len(octets) == 4
        assert all(0 <= o <= 255 for o in octets)
        assert octets[0] != 0, f"{state.ip} is not a routable address"

    @pytest.mark.join
    async def test_connecting_precedes_connected(self, client, states, credentials):
        """The app relies on the ordering to show progress."""
        ssid, password = credentials
        await client.write_gatt_char(
            WIFI_CREDS_UUID, encode_creds(ssid, password), response=True
        )
        await states.wait_for(STATE_CONNECTED, timeout=JOIN_TIMEOUT_S)

        kinds = [s.state for s in states.seen]
        assert STATE_CONNECTING in kinds
        assert kinds.index(STATE_CONNECTING) < kinds.index(STATE_CONNECTED)

    @pytest.mark.join
    async def test_wrong_password_reports_failure(self, client, states, credentials):
        ssid, _ = credentials
        await client.write_gatt_char(
            WIFI_CREDS_UUID,
            encode_creds(ssid, "definitely-not-the-password"),
            response=True,
        )
        state = await states.wait_for(STATE_FAILED, timeout=FAIL_TIMEOUT_S)
        assert state.reason is not None

    @pytest.mark.join
    async def test_reprovisioning_switches_networks(self, client, states, credentials):
        """A second write must supersede the first, not be ignored because the
        chip is already associated."""
        ssid, password = credentials

        await client.write_gatt_char(
            WIFI_CREDS_UUID,
            encode_creds("minicole-no-such-network-9zq", "irrelevant"),
            response=True,
        )
        await states.wait_for(STATE_CONNECTING, timeout=10.0)

        await asyncio.sleep(1.0)
        await client.write_gatt_char(
            WIFI_CREDS_UUID, encode_creds(ssid, password), response=True
        )
        state = await states.wait_for(STATE_CONNECTED, timeout=JOIN_TIMEOUT_S)
        assert state.ip is not None


class TestNotificationHygiene:
    async def test_every_notification_decodes(self, client, states, credentials):
        """StateRecorder.handle raises on a malformed payload, so reaching the
        assertion at all proves the firmware never emitted a bad frame."""
        ssid, password = credentials
        await client.write_gatt_char(
            WIFI_CREDS_UUID, encode_creds(ssid, password), response=True
        )
        await states.wait_for(STATE_CONNECTED, STATE_FAILED, timeout=JOIN_TIMEOUT_S)

        assert states.raw, "no notifications arrived at all"
        for payload in states.raw:
            assert 1 <= len(payload) <= 5, f"unexpected frame size: {payload!r}"
