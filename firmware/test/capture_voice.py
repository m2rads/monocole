#!/usr/bin/env python3
"""Records a spoken utterance off the monocle over BLE and writes a WAV.

The milestone 3 end-to-end check: say the wake word, speak, and get back audio
that went mic -> AFE -> IMA ADPCM -> BLE notification -> here. It decodes with
firmware/test/adpcm.py, which is an independent implementation of the wire
format, so a firmware codec bug fails here rather than being mirrored.

    .venv/bin/python capture_voice.py

Then say "Jarvis" or "Hi ESP" and talk. Nothing else may hold the BLE link —
the firmware accepts one central, so quit the app or nRF Connect first.

The verdict on whether it is *intelligible* is yours; listen to voice.wav.
"""

from __future__ import annotations

import argparse
import asyncio
import sys

from adpcm import FRAME_LEN, SAMPLE_RATE, SAMPLES_PER_FRAME, decode_frame, frames_to_wav
from protocol import (
    SERVICE_UUID,
    STATUS_UUID,
    STATUS_VOICE_ENDED,
    STATUS_VOICE_STARTED,
    VOICE_UUID,
)

END_REASONS = {0: "silence", 1: "hit the 15 s cap", 2: "capture error"}


async def find_device(timeout: float):
    from bleak import BleakScanner

    def is_monocle(_device, adv) -> bool:
        return SERVICE_UUID in [u.lower() for u in adv.service_uuids]

    print("scanning...")
    device = await BleakScanner.find_device_by_filter(is_monocle, timeout=timeout)
    if device is None:
        raise SystemExit(
            f"no device advertising {SERVICE_UUID}.\n"
            "  The firmware accepts one central and stops advertising while "
            "connected, so the\n"
            "  app or nRF Connect holding the link makes it invisible here."
        )
    return device


async def capture(timeout: float, wait_s: float):
    from bleak import BleakClient

    device = await find_device(timeout)
    frames: list[bytes] = []
    malformed: list[int] = []
    started = asyncio.Event()
    ended: asyncio.Future = asyncio.get_running_loop().create_future()

    def on_status(_h, data: bytearray):
        if not data:
            return
        if data[0] == STATUS_VOICE_STARTED:
            print("wake word detected — talk now")
            started.set()
        elif data[0] == STATUS_VOICE_ENDED and not ended.done():
            reason = data[1] if len(data) > 1 else 255
            ended.set_result(reason)

    def on_voice(_h, data: bytearray):
        # Length is checked here rather than trusted: a frame of the wrong size
        # means the firmware and this decoder disagree about the wire format,
        # which is exactly what an independent implementation is for.
        if len(data) != FRAME_LEN:
            malformed.append(len(data))
            return
        frames.append(bytes(data))

    async with BleakClient(device) as client:
        await client.start_notify(STATUS_UUID, on_status)
        await client.start_notify(VOICE_UUID, on_voice)
        print(f'connected to {device.address} — say "Jarvis" or "Hi ESP"')

        try:
            await asyncio.wait_for(started.wait(), timeout=wait_s)
        except asyncio.TimeoutError:
            raise SystemExit(
                f"no wake word in {wait_s:.0f}s.\n"
                "  Check the serial log shows 'wake word detected' when you "
                "speak; if it does,\n"
                "  the problem is the status characteristic rather than the "
                "detector."
            ) from None

        try:
            reason = await asyncio.wait_for(ended, timeout=30)
        except asyncio.TimeoutError:
            reason = 255

        await client.stop_notify(VOICE_UUID)
        await client.stop_notify(STATUS_UUID)

    return frames, malformed, reason


def report(frames: list[bytes], malformed: list[int], reason: int, out: str) -> int:
    print(f"\nutterance ended: {END_REASONS.get(reason, 'no voice_ended event')}")

    if malformed:
        print(f"FAIL: {len(malformed)} frame(s) were the wrong size "
              f"(e.g. {malformed[0]} bytes, expected {FRAME_LEN})")
        return 1
    if not frames:
        print("FAIL: no voice frames arrived. The session started, so the "
              "detector works and\n  the encoder or the notify path does not.")
        return 1

    decoded = [decode_frame(f) for f in frames]
    seqs = [d.seq for d in decoded]
    expected = seqs[-1] - seqs[0] + 1
    lost = expected - len(seqs)

    frames_to_wav(out, decoded)
    seconds = expected * SAMPLES_PER_FRAME / SAMPLE_RATE

    samples = [s for d in decoded for s in d.samples]
    peak = max(abs(s) for s in samples)
    rms = (sum(float(s) * s for s in samples) / len(samples)) ** 0.5

    print(f"{len(frames)} frames, {seconds:.1f}s of audio  peak={peak}  rms={rms:.0f}")
    print(f"wrote {out}")

    if lost > 0:
        # Not fatal: the app inserts silence for gaps by design, and stale
        # audio is not worth retransmitting. But it is the number that says
        # whether BLE is keeping up with 64 kbps.
        print(f"  {lost} of {expected} frames lost ({lost / expected:.1%}) — "
              "gaps became silence")
    else:
        print("  no dropped frames")

    if rms < 200:
        print("SUSPECT: very quiet. Did you speak after the wake word?")
        return 1

    print("\nNow listen to it — that is the gate.")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", default="voice.wav")
    parser.add_argument("--scan-timeout", type=float, default=10.0)
    parser.add_argument("--wait", type=float, default=60.0,
                        help="how long to wait for the wake word")
    args = parser.parse_args()

    frames, malformed, reason = asyncio.run(
        capture(args.scan_timeout, args.wait)
    )
    return report(frames, malformed, reason, args.out)


if __name__ == "__main__":
    sys.exit(main())
