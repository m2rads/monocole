"""Codec tests. No hardware, no BLE — these always run.

Their job is to pin the wire format so the integration tests below are
asserting against something deliberate, and so a change to protocol.py is a
visible decision rather than an accident.
"""

import pytest

from protocol import (
    MALFORMED_PAYLOADS,
    PASS_MAX_LEN,
    SSID_MAX_LEN,
    STATE_CONNECTED,
    STATE_CONNECTING,
    STATE_FAILED,
    STATE_IDLE,
    decode_state,
    encode_creds,
)


class TestEncodeCreds:
    def test_length_prefixes_each_field(self):
        assert encode_creds("net", "pw") == b"\x03net\x02pw"

    def test_empty_password_means_open_network(self):
        assert encode_creds("cafe", "") == b"\x04cafe\x00"

    def test_lengths_count_bytes_not_characters(self):
        # "é" is two UTF-8 bytes. A character count here would desync the
        # firmware's pass_len offset and corrupt the passphrase.
        encoded = encode_creds("é", "")
        assert encoded[0] == 2
        assert len(encoded) == 1 + 2 + 1

    def test_maximum_sizes_are_accepted(self):
        encoded = encode_creds("a" * SSID_MAX_LEN, "b" * PASS_MAX_LEN)
        assert len(encoded) == 1 + SSID_MAX_LEN + 1 + PASS_MAX_LEN
        # Must still fit a single ATT write at the MTU the chip negotiates.
        assert len(encoded) <= 97

    @pytest.mark.parametrize(
        "ssid, password",
        [
            ("", "pw"),
            ("a" * (SSID_MAX_LEN + 1), "pw"),
            ("net", "b" * (PASS_MAX_LEN + 1)),
        ],
    )
    def test_rejects_out_of_range_fields(self, ssid, password):
        with pytest.raises(ValueError):
            encode_creds(ssid, password)

    def test_error_never_quotes_the_password(self):
        secret = "b" * (PASS_MAX_LEN + 1)
        with pytest.raises(ValueError) as excinfo:
            encode_creds("net", secret)
        assert secret not in str(excinfo.value)


class TestDecodeState:
    def test_connected_carries_dotted_quad(self):
        state = decode_state(bytes([STATE_CONNECTED, 192, 168, 4, 22]))
        assert state.state == STATE_CONNECTED
        assert state.ip == "192.168.4.22"

    def test_failed_carries_reason(self):
        state = decode_state(bytes([STATE_FAILED, 15]))
        assert state.state == STATE_FAILED
        assert state.reason == 15
        assert "wrong password" in str(state)

    @pytest.mark.parametrize("value", [STATE_IDLE, STATE_CONNECTING])
    def test_progress_states_carry_no_payload(self, value):
        assert decode_state(bytes([value])).state == value

    @pytest.mark.parametrize(
        "payload, why",
        [
            (b"", "empty"),
            (bytes([STATE_CONNECTED, 192, 168]), "address truncated"),
            (bytes([STATE_FAILED]), "reason missing"),
            (bytes([9]), "unknown state"),
        ],
    )
    def test_rejects_malformed(self, payload, why):
        with pytest.raises(ValueError):
            decode_state(payload)


def test_malformed_vectors_are_actually_malformed():
    """The fixtures fed to the firmware must not accidentally be valid.

    Without this, a typo could turn a rejection test into a test that the
    firmware accepts a well-formed payload — passing for the wrong reason.
    """
    for label, payload in MALFORMED_PAYLOADS:
        assert not _is_well_formed(payload), f"{label!r} is well-formed after all"


def _is_well_formed(payload: bytes) -> bool:
    """Reference implementation of the firmware's validation."""
    if len(payload) < 3:
        return False
    ssid_len = payload[0]
    if ssid_len == 0 or ssid_len > SSID_MAX_LEN or len(payload) < 2 + ssid_len:
        return False
    pass_len = payload[1 + ssid_len]
    return pass_len <= PASS_MAX_LEN and len(payload) >= 2 + ssid_len + pass_len
