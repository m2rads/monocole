# Minicole docs

Minicole is the desktop companion app for a smart monocle: the monocle sends
voice (and on-demand images) to the app, the app runs local inference
(llama.cpp), and generated tokens stream back to the monocle's display.

- [firmware-plan.md](firmware-plan.md) — firmware language/framework and
  transport decisions, plus the build milestones.
- [firmware-setup.md](firmware-setup.md) — how to activate ESP-IDF, build,
  and flash on the XIAO ESP32-S3. Start here before touching `firmware/`.
- [ble-protocol.md](ble-protocol.md) — the BLE GATT service and framing the
  monocle and the app share. Update this first when the protocol changes;
  it is the contract between `firmware/` and `src-tauri/src/ble.rs`.
