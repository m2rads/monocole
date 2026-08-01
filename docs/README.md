# Minicole docs

Minicole is the desktop companion app for a smart monocle: the monocle sends
voice over BLE and stills over Wi-Fi, the app runs local inference
(llama.cpp), and generated tokens stream back to the monocle's display.

- [firmware-plan.md](firmware-plan.md) — the decisions: language, toolchain,
  transport split, what was rejected and why, and the build milestones. Start
  here for *what* we're building.
- [protocol.md](protocol.md) — the wire contract between `firmware/` and
  `src-tauri/`: the BLE GATT service, the Wi-Fi handoff, and the socket
  framing. **Update this first when the protocol changes.**
- [firmware-setup.md](firmware-setup.md) — how to activate ESP-IDF, build, and
  flash on the XIAO ESP32-S3. Read before touching `firmware/`.
- [firmware-learning-notes.md](firmware-learning-notes.md) — concepts and
  hard-won tooling fixes (clangd, NimBLE, flashing). The "why" behind the
  above; not a spec.

`ble-protocol.md` was folded into `protocol.md` on 2026-08-01, when media moved
off a BLE-only transport.
