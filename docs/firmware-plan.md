# Firmware plan

Decisions made 2026-07-05. Board: Seeed XIAO ESP32-S3 Sense (camera + PDM
mic), driving a micro-LED display. The board may change; these decisions
assume any ESP32-S3-class part.

## Language & toolchain: C++ on ESP-IDF

**C++, not MicroPython.** The workload — camera DMA, continuous I2S mic
capture, audio compression, and sustained BLE notification throughput,
concurrently on a 240 MHz chip — is exactly what MicroPython is bad at. The
camera driver alone needs a custom MicroPython build, and streaming audio
hits the interpreter's performance ceiling immediately.

**ESP-IDF v5.x directly** (not the Arduino framework). We will need its
control anyway: NimBLE stack tuning (MTU, connection interval, 2M PHY), I2S
DMA for the mic, the `esp32-camera` component, OTA updates later. Arduino
would only be a temporary on-ramp we'd migrate off.

Tooling: `idf.py` CLI + ESP-IDF VS Code extension. Firmware lives in
`firmware/` in this repo so protocol constants stay next to the app that
consumes them.

## Transport: BLE-only v1, Wi-Fi media plane later

**Raw footage over BLE is impossible — and unnecessary.** Tuned BLE on an
ESP32-S3 sustains roughly 200–400 kbps real-world. Raw 16 kHz/16-bit audio
is already 256 kbps; live video is megabits. But the consumer of "video" is
a multimodal LLM (e.g. Gemma 3) that takes **still images**, not streams.
The product flow is: user speaks; optionally a snapshot of what they're
looking at goes along with the query.

v1 (BLE only — matches the app's Connection tab and `ble.rs`):

- **Voice**: 16 kHz mono, IMA ADPCM compressed on-device (4:1 → ~64 kbps).
  Whisper is happy with 16 kHz input.
- **Images**: on-demand JPEG stills (QVGA/VGA, quality ~12), chunked over
  BLE notifications. Expect 1–3 s per capture — acceptable for v1.
- **Tokens back to the monocle display**: BLE notify; trivial bandwidth.

v2 (hybrid, when still latency / audio quality becomes limiting): BLE stays
as the pairing/control plane; the ESP32 joins Wi-Fi and opens a WebSocket to
a listener in the desktop app for the media plane. BLE hands over the
address/credentials. Standard wearable pattern — grow into it, don't start
with two radios' worth of firmware complexity.

## Throughput essentials (firmware side)

- Negotiate MTU 517, request 2M PHY, short connection interval (~15 ms).
- Stream notifications from a dedicated FreeRTOS task fed by ring buffers
  off the I2S/camera DMA — never from callbacks.
- Design against a conservative ~200 kbps sustained budget.

## Milestones

1. **Advertise + connect**: firmware advertises as `minicole-monocle`;
   connect from the app's Connection tab. Proves the pairing path.
2. **Throughput test**: flood notifications, measure real kbps on the Mac
   *before* freezing the protocol. This measurement de-risks everything.
3. **Audio path**: mic → ADPCM → notify → app writes a WAV to disk.
4. **Photo path**: capture command → chunked JPEG → app saves it.
5. **Full loop**: audio → STT → llama.cpp → tokens → monocle display.

App-side counterparts are marked `TODO(monocle-protocol)` and
`TODO(auto-reconnect)` in `src-tauri/src/ble.rs`.
