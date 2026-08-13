# Monocle firmware

Runs on a Seeed XIAO ESP32-S3 Sense. Talks to the desktop app over BLE, brings
Wi-Fi up on demand for bulk transfers, and drives the OLED the wearer reads.

Forked from ESP-IDF's `bluetooth/nimble/bleprph_wifi_coex` and rewritten; the
NimBLE scaffolding and the Apache headers are what remain of it.

- The wire contract is [`docs/protocol.md`](../docs/protocol.md). **Update it
  before changing anything on the air.**
- Decisions, rejected alternatives and milestones are in
  [`docs/firmware-plan.md`](../docs/firmware-plan.md).
- Toolchain setup and flashing help are in
  [`docs/firmware-setup.md`](../docs/firmware-setup.md).

## Build and flash

Needs ESP-IDF v6.0.2 on the path:

```bash
. ~/.espressif/v6.0.2/esp-idf/export.sh
idf.py build
idf.py -p /dev/cu.usbmodem3101 flash monitor      # port varies; ls /dev/cu.*
```

The target is `esp32s3` — **no hyphen**. `esp32-s3` fails quietly, leaves the
target at `esp32`, and surfaces later as `--chip;esp32` in a flash error.

`idf.py set-target` resets `sdkconfig`, so anything that must survive belongs
in `sdkconfig.defaults` — that is where the 8 MB flash size and the NimBLE bond
persistence live. `sdkconfig` and `sdkconfig.old` are gitignored because they
carry the Wi-Fi passphrase in plaintext.

## What is here

| | |
|---|---|
| `main/main.c` | BLE host, GAP events, Wi-Fi lifecycle, NVS credentials |
| `main/gatt_svr.c` | the GATT table and its write handlers |
| `main/tcp_server.c` | the Wi-Fi data plane: one client, length-prefixed frames |
| `main/display.c` | SSD1306 panel, font, wrapping, pagination |
| `test/` | host-side pytest suite driving the real chip over BLE |

## Two things that will bite you

**Adding a characteristic means bumping `MONOCLE_GATT_VERSION` in
`main/bleprph.h`.** A bonded central caches the attribute table forever, so a
new characteristic is simply invisible until the user forgets the device —
which looks exactly like a bug in whatever you just added. The version makes
the firmware send a Service Changed indication instead.

**One central at a time.** The firmware stops advertising while connected, so
the app or nRF Connect holding the link makes the board invisible to the test
suite. Disconnect there first.

## Tests

```bash
cd test
python3 -m venv .venv && .venv/bin/pip install -r requirements.txt
.venv/bin/python -m pytest -m "not hardware"      # codec only, no board
.venv/bin/python -m pytest -m hardware            # against the chip
```

See [`test/README.md`](test/README.md) for the tiers, what is covered, and the
checks that only a human looking at the panel can make.
