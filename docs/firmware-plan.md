# Firmware plan

Board: Seeed XIAO ESP32-S3 Sense (camera + PDM mic), driving a micro-LED
display. The board may change; these decisions assume any ESP32-S3-class part.

Decisions first made 2026-07-05; transport revised 2026-08-01 (see
[Transport](#transport-ble-control-plane--wi-fi-data-plane) — the original
BLE-only v1 was dropped). The wire contract lives in
[protocol.md](protocol.md); build mechanics in
[firmware-setup.md](firmware-setup.md).

## Language & toolchain: C++ on ESP-IDF

**C++, not MicroPython.** The workload — camera DMA, continuous I2S mic
capture, audio compression, and sustained BLE notification throughput,
concurrently on a 240 MHz chip — is exactly what MicroPython is bad at. The
camera driver alone needs a custom MicroPython build, and streaming audio
hits the interpreter's performance ceiling immediately.

**ESP-IDF v6.0.2** (not the Arduino framework). We need its control anyway:
NimBLE stack tuning (MTU, connection interval, 2M PHY), I2S DMA for the mic,
the `esp32-camera` component, `esp_wifi`, OTA updates later. Arduino would only
be a temporary on-ramp we'd migrate off.

Match examples and docs to **v6.x**. v6.0 is a major bump with breaking API
changes from v5, and much of what you'll find online targets v5.

**NimBLE, not Bluedroid** — BLE-only design, and the smaller footprint matters
alongside audio, JPEG, and Wi-Fi buffers.

Tooling: `idf.py` CLI. Firmware lives in `firmware/` in this repo so protocol
constants stay next to the app that consumes them.

## Transport: BLE control plane + Wi-Fi data plane

Split by **traffic shape**, chosen for battery life on a head-worn device.
Full wire details in [protocol.md](protocol.md).

- **BLE, always on** — control, status, tokens to the display, and **voice**
  (16 kHz mono, IMA ADPCM, ~64 kbps). Voice is continuous but low-rate, and
  fits inside BLE's ~200 kbps budget on a radio that is already up.
- **Wi-Fi, on demand** — **JPEG stills only**. Brought up for a burst, torn
  down after an idle timeout. ~100 KB in tens of milliseconds instead of the
  1–3 s the same image costs over BLE.

The handoff replaces BluFi: the app writes credentials to a BLE
characteristic, the firmware joins and reports its DHCP address back over BLE,
the app opens a TCP socket. BLE stays connected throughout.

**Raw footage is out of scope.** The consumer of imagery is a multimodal LLM
that takes still frames, not streams. The product flow is: user speaks;
optionally a snapshot of what they're looking at goes along with the query.

### Rejected alternatives

- **BLE-only v1** (the original plan). Images were always its weak point at
  1–3 s per capture, and the Wi-Fi radio was sitting idle on the same chip.
- **All media over Wi-Fi** (the first revision). Uniform, but holds a
  100–200 mA radio up for every spoken utterance to carry 64 kbps. Voice on
  BLE keeps Wi-Fi's duty cycle near ~1%.
- **ESP-NOW** as the data plane. Lower overhead than Wi-Fi + TCP and needs no
  router, but a Mac cannot speak it without an ESP32 dongle.
- **BluFi.** Provisions credentials only — no data channel, so you open your
  own socket regardless. Assumes Espressif's *mobile* apps and pulls in
  Bluedroid. We own both sides; a custom characteristic is simpler.
- **SoftAP** as the initial topology. Deferred, not rejected — the Mac loses
  its own internet while connected. Same data-plane code either way.

## Throughput essentials

- Negotiate MTU 517, request 2M PHY, short connection interval (~15 ms).
- Stream notifications from a dedicated FreeRTOS task fed by ring buffers off
  the I2S/camera DMA — never from callbacks.
- Design BLE against a conservative ~200 kbps sustained budget. Voice uses
  roughly a third, which is why a dedicated BLE throughput test is no longer a
  gating milestone: the headroom is large now that images have moved off. It
  reduces to a check inside milestone 3.

## Milestones

1. ~~**Advertise + connect**~~ — **done.** The stock `bleprph` example builds,
   flashes, advertises, and the app's Connection tab connects to it. Proves the
   pairing path end to end; no app-side changes were needed.
2. **GATT skeleton** — scaffold `firmware/` from `bleprph`: advertise as
   `minicole-monocle`, define the real service and characteristics from
   protocol.md, enable bonding with keys in NVS. App side: freeze the UUIDs and
   add a `ScanFilter`.
3. **Voice path** — mic → I2S DMA → ADPCM → BLE notify → app writes a WAV to
   disk. Confirm sustained 64 kbps holds without drops (absorbs the old
   throughput test).
4. ~~**Wi-Fi handoff**~~ — **done.** Creds write → join → report IP over BLE →
   app opens a TCP socket → echo + bulk transfer, with idle teardown and
   `wifi_control` to power the plane back up. Throughput measurement pending on
   hardware; see `ble-examples/bleprph_wifi_coex/test/`.
5. **Photo path** — capture command over BLE `control` → JPEG over the socket →
   app verifies CRC and saves it.
6. **Full loop** — voice → STT → llama.cpp → tokens → monocle display.

`ble.rs` now does scan/connect/disconnect plus the Wi-Fi provisioning writes
and `wifi_state` subscription; `socket.rs` is the data-plane client. Milestones
2, 3 and 5 still need new Rust there — see `TODO(monocle-protocol)` and
`TODO(auto-reconnect)`. Milestone 6 depends on the STT stage, not started.

## Hardware config still to enable

Both known and both deferred (see firmware-learning-notes.md open threads):

- **8 MB flash** — the XIAO S3 has 8 MB but builds default to 2 MB. Set before
  building a real partition table; OTA and Wi-Fi will want the space.
- **PSRAM** — not yet enabled. Turn on before BLE + Wi-Fi + audio + camera
  buffers coexist.
