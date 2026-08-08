"""Wire format for the monocle Wi-Fi provisioning service.

Mirrors the firmware (``main/gatt_svr.c``) and the app
(``src-tauri/src/ble.rs``). Kept deliberately independent of both so the tests
fail when either drifts, rather than agreeing with a bug.

See ``docs/protocol.md`` for the authoritative description.
"""

from __future__ import annotations

from dataclasses import dataclass

# GATT contract. Byte-reversed in the firmware's BLE_UUID128_INIT calls.
SERVICE_UUID = "83486508-636c-4260-9119-c0ccc2004219"
WIFI_CREDS_UUID = "2c9b4a45-d3a5-4bf9-ac60-1f5f2e98db3c"
WIFI_STATE_UUID = "1ad1e743-dcae-422d-a7a8-68b4d695ac8b"

# 802.11 limits the firmware enforces.
SSID_MAX_LEN = 32
PASS_MAX_LEN = 63

# wifi_state values.
STATE_IDLE = 0
STATE_CONNECTING = 1
STATE_CONNECTED = 2
STATE_FAILED = 3

STATE_NAMES = {
    STATE_IDLE: "idle",
    STATE_CONNECTING: "connecting",
    STATE_CONNECTED: "connected",
    STATE_FAILED: "failed",
}

# Disconnect reasons worth naming in assertion output.
WIFI_REASONS = {
    15: "4-way handshake timeout (wrong password)",
    201: "no AP found (wrong SSID, or 5 GHz only)",
    202: "auth failed",
    203: "assoc failed",
    204: "handshake timeout",
    205: "connection failed",
}


def encode_creds(ssid: str, password: str) -> bytes:
    """Builds ``[ssid_len][ssid][pass_len][pass]``.

    Lengths count *bytes*, not characters — the firmware indexes bytes.
    """
    ssid_bytes = ssid.encode()
    pass_bytes = password.encode()

    if not 1 <= len(ssid_bytes) <= SSID_MAX_LEN:
        raise ValueError(f"ssid must be 1-{SSID_MAX_LEN} bytes, got {len(ssid_bytes)}")
    if len(pass_bytes) > PASS_MAX_LEN:
        raise ValueError(f"password must be 0-{PASS_MAX_LEN} bytes, got {len(pass_bytes)}")

    return (
        bytes([len(ssid_bytes)])
        + ssid_bytes
        + bytes([len(pass_bytes)])
        + pass_bytes
    )


@dataclass(frozen=True)
class WifiState:
    state: int
    ip: str | None = None
    reason: int | None = None

    @property
    def name(self) -> str:
        return STATE_NAMES.get(self.state, f"unknown({self.state})")

    def __str__(self) -> str:
        if self.ip:
            return f"{self.name} {self.ip}"
        if self.reason is not None:
            described = WIFI_REASONS.get(self.reason, "unknown reason")
            return f"{self.name} (reason={self.reason}: {described})"
        return self.name


def decode_state(payload: bytes) -> WifiState:
    """Parses a wifi_state notification. Raises ValueError if malformed."""
    if not payload:
        raise ValueError("empty wifi_state payload")

    state, rest = payload[0], payload[1:]

    if state == STATE_CONNECTED:
        if len(rest) < 4:
            raise ValueError(f"connected state needs 4 address bytes, got {len(rest)}")
        return WifiState(state, ip=".".join(str(b) for b in rest[:4]))

    if state == STATE_FAILED:
        if not rest:
            raise ValueError("failed state needs a reason byte")
        return WifiState(state, reason=rest[0])

    if state not in (STATE_IDLE, STATE_CONNECTING):
        raise ValueError(f"unknown wifi_state {state}")

    return WifiState(state)


# Payloads the firmware must reject. Each targets one bounds check in
# gatt_svr_chr_access_wifi(); a crash or an accepted write is a real defect.
MALFORMED_PAYLOADS: list[tuple[str, bytes]] = [
    ("empty", b""),
    ("one byte, no room for anything", b"\x01"),
    ("two bytes, below the 3-byte minimum", b"\x01a"),
    ("ssid_len 0 is not a network", b"\x00\x00"),
    ("ssid_len claims 255, payload has 2", b"\xffab"),
    ("ssid_len overruns by one", b"\x03ab\x00"),
    ("ssid_len exceeds the 32-byte maximum", bytes([33]) + b"a" * 33 + b"\x00"),
    ("pass_len claims 255, payload has 2", b"\x01a\xffxy"),
    ("pass_len overruns by one", b"\x01a\x03xy"),
    ("pass_len exceeds the 63-byte maximum", b"\x01a" + bytes([64]) + b"x" * 64),
    ("truncated after ssid, no pass_len", b"\x02ab"),
]
