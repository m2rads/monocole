#!/usr/bin/env python3
"""Lists nearby BLE devices. Run when the tests skip with 'device not found'."""

import asyncio
import sys

from bleak import BleakScanner

from protocol import SERVICE_UUID


async def main(timeout: float) -> int:
    devices = await BleakScanner.discover(timeout=timeout, return_adv=True)
    named = {d: adv for d, adv in devices.values() if d.name}

    print(f"{len(devices)} devices seen, {len(named)} advertising a name:\n")
    for device, adv in sorted(named.items(), key=lambda kv: kv[0].name or ""):
        marker = "  <-- monocle" if SERVICE_UUID in [
            u.lower() for u in adv.service_uuids
        ] else ""
        print(f"  {device.name:<32} {device.address}  rssi={adv.rssi}{marker}")

    if not named:
        print("  (nothing named — a connected peripheral stops advertising)")
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main(float(sys.argv[1]) if len(sys.argv) > 1 else 8.0)))
