"""Fixtures for driving the monocle over BLE from the host.

Hardware tests are skipped unless a device is found, so the codec tests still
run on a machine with no chip attached (or in CI).
"""

from __future__ import annotations

import asyncio
import os

import pytest
import pytest_asyncio

from protocol import WIFI_STATE_UUID, WifiState, decode_state

DEFAULT_DEVICE_NAME = "nimble-bleprph"
SCAN_TIMEOUT_S = 10.0


def pytest_addoption(parser):
    parser.addoption(
        "--device-name",
        default=os.environ.get("MONOCLE_NAME", DEFAULT_DEVICE_NAME),
        help="BLE advertised name to look for.",
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


@pytest.fixture(scope="session")
def credentials(request) -> tuple[str, str]:
    ssid = request.config.getoption("--ssid")
    if not ssid:
        pytest.skip("no --ssid given; pass one to exercise the join path")
    return ssid, request.config.getoption("--password") or ""


@pytest_asyncio.fixture(loop_scope="session", scope="session")
async def device(request):
    """Finds the peripheral once per session."""
    from bleak import BleakScanner

    name = request.config.getoption("--device-name")
    found = await BleakScanner.find_device_by_name(name, timeout=SCAN_TIMEOUT_S)
    if found is None:
        pytest.skip(
            f"no BLE device named {name!r} within {SCAN_TIMEOUT_S:.0f}s.\n"
            "  The usual cause is that something else is already connected: "
            "bleprph accepts one\n"
            "  central and stops advertising, so nRF Connect or the minicole "
            "app holding the link\n"
            "  makes the chip invisible here. Disconnect it there first.\n"
            "  Otherwise: check the board is powered, flashed, and that the "
            "name matches\n"
            "  (--device-name / MONOCLE_NAME)."
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
