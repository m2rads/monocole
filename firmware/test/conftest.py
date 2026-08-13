"""Fixtures for driving the monocle over BLE from the host.

Hardware tests are skipped unless a device is found, so the codec tests still
run on a machine with no chip attached (or in CI).
"""

from __future__ import annotations

import asyncio
import os

import pytest
import pytest_asyncio

from protocol import SERVICE_UUID, WIFI_STATE_UUID, WifiState, decode_state

DEFAULT_DEVICE_NAME = "minicole-monocle"
SCAN_TIMEOUT_S = 10.0


def pytest_addoption(parser):
    parser.addoption(
        "--device-name",
        default=os.environ.get("MONOCLE_NAME", DEFAULT_DEVICE_NAME),
        help="The name the firmware advertises. Informational only — the "
        "device is found by its service UUID, which macOS does not cache "
        "the way it caches names.",
    )
    parser.addoption(
        "--ssid",
        default=os.environ.get("MONOCLE_SSID"),
        help="A network the chip can actually join. Enables the join tests.",
    )
    parser.addoption(
        "--password",
        default=os.environ.get("MONOCLE_PASSWORD"),
        help="Passphrase for --ssid. Omit for an open network.",
    )


def pytest_configure(config):
    config.addinivalue_line(
        "markers", "hardware: needs the chip powered, flashed, and in range"
    )
    config.addinivalue_line(
        "markers", "join: additionally needs real Wi-Fi credentials (--ssid)"
    )
    config.addinivalue_line(
        "markers", "slow: takes tens of seconds (idle timers)"
    )


@pytest.fixture(scope="session")
def credentials(request) -> tuple[str, str]:
    ssid = request.config.getoption("--ssid")
    if not ssid:
        pytest.skip("no --ssid given; pass one to exercise the join path")
    return ssid, request.config.getoption("--password") or ""


@pytest_asyncio.fixture(loop_scope="session", scope="session")
async def device(request):
    """Finds the peripheral once per session, by the service it advertises.

    Matching on the service UUID rather than the name is deliberate: macOS
    caches a bonded peripheral's name and keeps reporting the old one long
    after the firmware changed it, so a rename would silently stop the whole
    hardware suite from finding the board. The service UUID comes from the
    advertisement itself and is always current.
    """
    from bleak import BleakScanner

    def is_monocle(_device, adv) -> bool:
        return SERVICE_UUID in [uuid.lower() for uuid in adv.service_uuids]

    found = await BleakScanner.find_device_by_filter(
        is_monocle, timeout=SCAN_TIMEOUT_S
    )
    if found is None:
        pytest.skip(
            f"no device advertising {SERVICE_UUID} within "
            f"{SCAN_TIMEOUT_S:.0f}s.\n"
            "  The usual cause is that something else is already connected: "
            "the firmware accepts\n"
            "  one central and stops advertising, so nRF Connect or the "
            "minicole app holding the\n"
            "  link makes the chip invisible here. Disconnect it there first.\n"
            "  Otherwise: check the board is powered and flashed, and run "
            "scan.py to see what\n"
            "  is on the air."
        )
    return found


@pytest_asyncio.fixture(loop_scope="session")
async def client(device):
    """A fresh connection per test, so one failure can't cascade."""
    from bleak import BleakClient

    async with BleakClient(device) as connected:
        yield connected


class StateRecorder:
    """Collects wifi_state notifications for assertions."""

    def __init__(self):
        self._queue: asyncio.Queue[WifiState] = asyncio.Queue()
        self.raw: list[bytes] = []
        self.seen: list[WifiState] = []

    def handle(self, _characteristic, payload: bytearray) -> None:
        data = bytes(payload)
        self.raw.append(data)
        state = decode_state(data)  # raises on malformed — a real failure
        self.seen.append(state)
        self._queue.put_nowait(state)

    async def wait_for(self, *states: int, timeout: float) -> WifiState:
        """Waits for any of ``states``, ignoring others (e.g. 'connecting')."""
        deadline = asyncio.get_running_loop().time() + timeout
        while True:
            remaining = deadline - asyncio.get_running_loop().time()
            if remaining <= 0:
                raise AssertionError(
                    f"timed out after {timeout:.0f}s waiting for "
                    f"{[s for s in states]}; saw {[str(s) for s in self.seen]}"
                )
            state = await asyncio.wait_for(self._queue.get(), timeout=remaining)
            if state.state in states:
                return state


@pytest_asyncio.fixture(loop_scope="session")
async def states(client) -> StateRecorder:
    """Subscribes to wifi_state for the duration of a test."""
    recorder = StateRecorder()
    await client.start_notify(WIFI_STATE_UUID, recorder.handle)
    try:
        yield recorder
    finally:
        try:
            await client.stop_notify(WIFI_STATE_UUID)
        except Exception:
            pass  # the link may already be gone; not this test's concern
