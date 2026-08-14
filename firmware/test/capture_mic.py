#!/usr/bin/env python3
"""Records from the monocle's microphone and writes a WAV.

Milestone 3's gate: prove the capture path produces intelligible audio
*before* a wake word detector is wired to it, so that a silent pipeline never
has two possible causes.

Needs a firmware built with CONFIG_MONOCLE_MIC_DUMP_AT_BOOT=y:

    idf.py menuconfig      # Monocle Configuration -> Dump ... at boot
    idf.py -p PORT flash

Then — with `idf.py monitor` closed, because only one process can read the
port and a monitor will swallow the dump:

    .venv/bin/python capture_mic.py --port /dev/cu.usbmodem1101 --reset

The board prints a few thousand lines of base64 during the transfer. That is
the audio, and it is meant for this script rather than for a human to read.

It resets the board, waits for the dump, writes mic.wav, and reports what the
samples look like. **The verdict is still yours** — the checks below catch a
dead or stuck microphone, not a quiet or distorted one. Listen to the file.
"""

from __future__ import annotations

import argparse
import base64
import math
import re
import struct
import sys
import time
import wave

BEGIN = "---BEGIN MONOCLE PCM"
END = "---END MONOCLE PCM---"

# A dump line and nothing else. Log output from other tasks has spaces, colons
# and parentheses, so it never matches.
B64_LINE = re.compile(r"^[A-Za-z0-9+/]+={0,2}$")


def capture(port: str, baud: int, timeout_s: float,
            reset: bool = False) -> tuple[int, bytes]:
    """Listens for the board's dump, returning (sample_rate, pcm bytes).

    Does **not** reset the board by default. Driving DTR/RTS on this board
    pulls GPIO0 low at the wrong moment and drops it into ROM download mode —
    it then sits at "waiting for download", printing nothing, which looks
    exactly like a dead or wedged chip. Tap the RST button instead; that is one
    press against a failure mode that costs half an hour to diagnose.
    """
    import serial

    with serial.Serial(port, baud, timeout=0.5) as s:
        # Opening the port asserts both lines on macOS, which on its own can
        # hold the board in reset. Release them immediately, always.
        s.dtr = False
        s.rts = False

        if reset:
            s.rts = True        # EN low
            time.sleep(0.15)
            s.rts = False       # release, with GPIO0 already high
        else:
            print("waiting for a reboot — tap the RST button on the board")

        rate = 0
        expected = 0
        chunks: list[str] = []
        skipped: list[str] = []
        collecting = False
        deadline = time.time() + timeout_s

        while time.time() < deadline:
            try:
                raw = s.readline()
            except serial.SerialException as err:
                # Almost always a second reader on the same port: only one
                # process can own it, and the other one is getting the dump.
                raise SystemExit(
                    f"lost the serial port: {err}\n\n"
                    "  Close `idf.py monitor` (Ctrl-]) and any other terminal "
                    "reading this port,\n"
                    "  then run this again. The dump can only go to one "
                    "reader, and a monitor\n"
                    "  will happily swallow it."
                ) from None
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
                if skipped:
                    print(f"  ignored {len(skipped)} non-base64 line(s) in the "
                          f"stream, e.g. {skipped[0][:60]!r}")
                pcm = base64.b64decode("".join(chunks))
                # A dropped line can leave an odd byte, which is half a sample.
                pcm = pcm[:len(pcm) - (len(pcm) % 2)]
                if expected and len(pcm) != expected * 2:
                    lost = expected - len(pcm) // 2
                    print(f"  WARNING: {lost} samples "
                          f"({lost / expected:.1%}) lost to the console")
                return rate, pcm

            if collecting:
                if not line:
                    continue        # blank separators are not a problem
                # Anything that is not pure base64 is another task logging over
                # the top of the stream. Drop it rather than corrupting the
                # decode; a hole in the audio beats an exception.
                if B64_LINE.match(line) and len(line) % 4 == 0:
                    chunks.append(line)
                else:
                    skipped.append(line)
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

    # A PDM mic sits on a DC offset, so raw peak and RMS mostly measure that.
    # What says whether sound arrived is the swing *around* the mean.
    dc = sum(samples) / len(samples)
    ac = [s - dc for s in samples]

    peak = max(abs(s) for s in ac)
    rms = math.sqrt(sum(s * s for s in ac) / len(ac))
    distinct = len(set(samples))

    print(f"\n{len(samples)} samples  dc_offset={dc:.0f}  "
          f"peak={peak:.0f}  rms={rms:.0f}  distinct values={distinct}")

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
    # Speech at arm's length lands in the thousands. A few hundred is a room
    # tone or the mic's own noise floor — real, but not evidence it can hear.
    if rms < 500:
        print(f"INCONCLUSIVE: rms {rms:.0f} is a noise floor, not speech.\n"
              "  The mic is alive — the samples vary, so it is clocked and "
              "wired.\n"
              "  But nothing here shows it picking up a voice. Re-run and "
              "talk at it;\n"
              "  speaking near the board should put rms in the thousands.")
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
    parser.add_argument(
        "--reset", action="store_true",
        help="toggle RTS to reset the board. Off by default: on this board it "
             "can land in ROM download mode instead of rebooting. Prefer RST.",
    )
    args = parser.parse_args()

    rate, pcm = capture(args.port, args.baud, args.timeout, args.reset)

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
