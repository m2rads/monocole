#!/usr/bin/env python3
"""Records from the monocle's microphone and writes a WAV.

Milestone 3's gate: prove the capture path produces intelligible audio
*before* a wake word detector is wired to it, so that a silent pipeline never
has two possible causes.

Needs a firmware built with CONFIG_MONOCLE_MIC_DUMP_AT_BOOT=y:

    idf.py menuconfig      # Monocle Configuration -> Dump ... at boot
    idf.py -p PORT flash

Then, with nothing else holding the serial port:

    .venv/bin/python capture_mic.py --port /dev/cu.usbmodem1101

It resets the board, waits for the dump, writes mic.wav, and reports what the
samples look like. **The verdict is still yours** — the checks below catch a
dead or stuck microphone, not a quiet or distorted one. Listen to the file.
"""

from __future__ import annotations

import argparse
import base64
import math
import struct
import sys
import time
import wave

BEGIN = "---BEGIN MONOCLE PCM"
END = "---END MONOCLE PCM---"


def capture(port: str, baud: int, timeout_s: float) -> tuple[int, bytes]:
    """Resets the board and returns (sample_rate, pcm bytes) from its dump."""
    import serial

    with serial.Serial(port, baud, timeout=0.5) as s:
        # The standard ESP32 auto-reset: hold EN low via RTS, release.
        s.dtr = False
        s.rts = True
        time.sleep(0.15)
        s.rts = False

        rate = 0
        expected = 0
        chunks: list[str] = []
        collecting = False
        deadline = time.time() + timeout_s

        while time.time() < deadline:
            raw = s.readline()
            if not raw:
                continue
            line = raw.decode("utf-8", "replace").strip()

            if line.startswith(BEGIN):
                # ---BEGIN MONOCLE PCM <rate> <samples>---
                fields = line.replace("-", " ").split()
                rate, expected = int(fields[3]), int(fields[4])
                collecting = True
                print(f"receiving {expected} samples at {rate} Hz...")
                continue

            if line.startswith(END):
                return rate, base64.b64decode("".join(chunks))

            if collecting:
                chunks.append(line)
            elif "monocle_mic" in line or "capturing" in line:
                print(f"  {line}")

    raise SystemExit(
        "timed out waiting for the dump.\n"
        "  Is the firmware built with CONFIG_MONOCLE_MIC_DUMP_AT_BOOT=y?\n"
        "  Is something else holding the serial port (idf.py monitor)?"
    )


def describe(samples: list[int]) -> int:
    """Prints what the audio looks like. Returns a process exit code."""
    if not samples:
        print("no samples at all — the capture stalled")
        return 1

    peak = max(abs(s) for s in samples)
    rms = math.sqrt(sum(float(s) * s for s in samples) / len(samples))
    distinct = len(set(samples))

    print(f"\n{len(samples)} samples  peak={peak}  rms={rms:.0f}  "
          f"distinct values={distinct}")

    # These catch a microphone that is not there, not clocked, or wired to the
    # wrong pin. They cannot tell good audio from bad — that needs your ears.
    if peak == 0:
        print("FAIL: every sample is zero. The mic is silent — check that the "
              "Sense expansion board is seated, and that CLK/DIN are GPIO42/41.")
        return 1
    if distinct < 16:
        print(f"FAIL: only {distinct} distinct values. That is a stuck or "
              "unclocked input rather than audio.")
        return 1
    if rms < 20:
        print("SUSPECT: nearly silent. Possible, if the room was quiet and "
              "nobody spoke — but re-run and talk at it before believing the "
              "pipeline works.")
        return 1
    if peak >= 32767:
        print("SUSPECT: clipping. The audio is there but the gain is too high "
              "for anything downstream to do well with it.")

    print("Looks like audio. Now listen to it — that is the actual gate.")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", default="/dev/cu.usbmodem1101")
    parser.add_argument("--baud", type=int, default=115200)
    parser.add_argument("--out", default="mic.wav")
    parser.add_argument("--timeout", type=float, default=60.0)
    args = parser.parse_args()

    rate, pcm = capture(args.port, args.baud, args.timeout)

    with wave.open(args.out, "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(rate)
        out.writeframes(pcm)
    print(f"wrote {args.out} ({len(pcm)} bytes)")

    samples = list(struct.unpack(f"<{len(pcm) // 2}h", pcm))
    return describe(samples)


if __name__ == "__main__":
    sys.exit(main())
