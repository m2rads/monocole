# BLE protocol (v1 draft)

The contract between the monocle firmware (`firmware/`) and the desktop app
(`src-tauri/src/ble.rs`). Status: **draft — UUIDs and framing not yet
frozen.** Freeze only after the throughput test (milestone 2 in
[firmware-plan.md](firmware-plan.md)).

## Roles

The desktop app is the BLE **central**; the monocle is the **peripheral**.
Device advertises as `minicole-monocle`. Once the service UUID is frozen,
the app should filter scans by it (`ScanFilter` in `ble.rs`) instead of
showing all nearby devices.

## Service

One custom 128-bit service, five characteristics:

| Characteristic | Direction | Ops | Purpose |
|---|---|---|---|
| control | app → device | write | start/stop mic, capture photo, settings |
| audio | device → app | notify | ADPCM audio frames |
| image | device → app | notify | chunked JPEG stills |
| tokens | app → device | write | generated text for the LED display |
| status | device → app | notify | battery, state, errors |

## Framing

All notified payloads start with a small header (exact layout TBD after the
throughput test):

- **audio**: `[seq: u16][adpcm payload]` — 16 kHz mono source, IMA ADPCM
  (4-bit/sample, ~64 kbps). Sequence number detects drops.
- **image**: three frame types —
  `START { total_size, width, height }` → `DATA { seq, bytes }` →
  `END { crc32 }`. App reassembles, verifies CRC, drops the capture on
  mismatch and requests a retake.

## Link parameters

- MTU: negotiate 517 (macOS grants 512-byte payloads typically).
- PHY: request 2M.
- Connection interval: short (~15 ms) while streaming.
- Budget: assume ~200 kbps sustained until measured otherwise.

## Open questions

- Exact UUIDs (generate once, commit here and in firmware constants).
- ADPCM frame size vs. connection-event packing (tune after milestone 2).
- Whether `control` needs a response characteristic or write-with-response
  is enough.
- Battery/status payload schema.
