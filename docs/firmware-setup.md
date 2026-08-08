# Firmware setup & build (ESP-IDF)

Reference for getting an ESP-IDF project to compile and flash on the XIAO
ESP32-S3. Written from the actual steps used to build the throwaway
`hello_world` smoke test. Read this before touching `firmware/`.

## Environment as installed

- Toolchain installed via **EIM** (ESP-IDF Installation Manager).
- ESP-IDF version: **v6.0.2**, at `~/.espressif/v6.0.2/esp-idf/`.
  - v6.0 is a major bump with breaking API changes from v5 — when you copy
    example code or read docs, match them to **v6.x**, not v5.
- There is a second, stray checkout at `~/esp/esp-idf/`. Ignore it. Use only
  the EIM-managed `~/.espressif/v6.0.2/esp-idf/`. Two IDF installs on one
  machine is how you waste an afternoon building against the wrong one.
- Board target: `esp32s3`.

## The SDK is not your project

ESP-IDF (`~/.espressif/v6.0.2/esp-idf/`) is the compiler + build system +
libraries. Your code does not live there. You activate the SDK into a
terminal, then build your project from wherever it actually lives.

`export.sh` is the bridge: it sets `IDF_PATH` and puts `idf.py` + the Xtensa
toolchain on `PATH` **for that terminal only**. It does not persist. New
terminal = source it again. There is no global install.

## Prerequisites

- Xcode Command Line Tools (already present — the Tauri/Rust build needs
  them). `xcode-select --install` if missing.
- Working `python3` with a functional `venv` module. A uv-managed or broken
  Python on `PATH` fails EIM's sanity check and later tooling. `python3 -m
  venv /tmp/x` must succeed.
- A USB-C **data** cable. Many cables are charge-only and will silently not
  enumerate a serial port. This wastes more time than any software problem.

## Steps that worked

Example was copied out of the SDK tree, not edited in place:

```bash
. ~/.espressif/v6.0.2/esp-idf/export.sh        # activate SDK (every new terminal)
cp -r $IDF_PATH/examples/get-started/hello_world .
cd hello_world
```

The copied example drags along Espressif's CI harness —
`build-test-rules.yml`, `pytest_hello_world.py`, `sdkconfig.ci`. None of it
is yours. Delete it or ignore it. Only these matter: `CMakeLists.txt`,
`main/CMakeLists.txt`, `main/*.c`.

Build:

```bash
idf.py set-target esp32s3      # regenerates build/, writes sdkconfig; run once per target change
idf.py build                   # first build is slow: it compiles the whole SDK for this target
```

First `idf.py build` took a few minutes (543 build steps) and produced
`build/hello_world.bin` (~145 KB). A clean compile means the toolchain,
compiler, and target are all correct. `build/` and `sdkconfig` are generated
— do not commit them.

## Flashing (needs the board plugged in)

Find the port:

```bash
ls /dev/cu.*                   # XIAO enumerates as /dev/cu.usbmodemXXXX
```

Flash + open serial monitor:

```bash
idf.py -p /dev/cu.usbmodemXXXX flash monitor
```

Expected: boot log, then `Hello world!` and a reboot countdown every ~10 s.
Exit the monitor with **Ctrl-]**.

## When flashing fails

- **No `/dev/cu.usbmodem*` appears:** wrong cable (charge-only) or wrong
  port on the board. Swap the cable first — it's the usual cause.
- **`Failed to connect` / timeout:** force download mode. Hold **BOOT**, tap
  **RESET**, release **BOOT**, re-run the flash command.
- **Permission / busy port:** a serial monitor from a previous run is still
  holding the port. Close it (Ctrl-]).

## Notes

- `hello_world` and the copies under `ble-examples/` are disposable setup
  checks. The real firmware — the `minicole-monocle` peripheral — goes in
  `firmware/`, scaffolded from the `bleprph` NimBLE example. See
  `firmware-plan.md` milestone 2.
- Copied examples default to target `esp32`. On the XIAO you must run `idf.py
  set-target esp32s3` **before** the first build, or flashing fails with
  `This chip is ESP32-S3, not ESP32`. `set-target` also resets `sdkconfig` to
  defaults — put anything you set in menuconfig into `sdkconfig.defaults` so
  it survives.
- Every command above assumes `export.sh` was sourced in the current
  terminal. If `idf.py: command not found`, that's the reason.

---

# Cheat sheet: build & flash any example

Run these four in order, from the example's directory.

### 1. Activate the SDK — every new terminal

```bash
. ~/.espressif/v6.0.2/esp-idf/export.sh
```

Leading `.` (= `source`). Not `./export.sh` — that exports into a subshell and
does nothing.

### 2. Set the target — once per project

```bash
idf.py set-target esp32s3
```

**`esp32s3`, no hyphen.** `esp32-s3` is not a valid name: the command fails,
`sdkconfig` is never written, and the target silently stays `esp32`. That's the
cause of `--chip;esp32` in flash errors.

### 3. Find your port — it changes between sessions

```bash
ls /dev/cu.usbmodem*
```

Use whatever it prints. **Never type `PORT` literally** — that's a placeholder
in Espressif's READMEs, and you get `Could not open PORT`.

### 4. Build, flash, watch

```bash
idf.py build
idf.py -p /dev/cu.usbmodem3101 flash monitor
```

Exit the monitor with **Ctrl-]**.

## One-liner

```bash
. ~/.espressif/v6.0.2/esp-idf/export.sh && idf.py -p $(ls /dev/cu.usbmodem* | head -1) flash monitor
```

## Gotchas, in the order they bite

| Symptom | Cause | Fix |
|---|---|---|
| `idf.py: command not found` | new terminal | step 1 |
| `Could not open PORT` | typed `PORT` literally | step 3 |
| `--chip;esp32` in the error | target never set (hyphen typo) | step 2 |
| `This chip is ESP32-S3, not ESP32` | built for the wrong target | step 2, then rebuild |
| menuconfig settings vanished | `set-target` resets `sdkconfig` | put them in `sdkconfig.defaults` |
| `Failed to connect` / timeout | not in download mode | hold **BOOT**, tap **RESET**, release **BOOT**, reflash |
| port busy | old monitor still open | Ctrl-] in the other terminal |
| no `/dev/cu.usbmodem*` at all | charge-only USB cable | swap the cable first |

## Wi-Fi examples need credentials

Examples with Wi-Fi (e.g. `bleprph_wifi_coex`) ship placeholders —
`myssid` / `mypassword`. Set real ones before flashing:

```bash
idf.py menuconfig     # → Example Configuration → WiFi SSID / WiFi Password
```

The ESP32-S3 is **2.4 GHz only**. It cannot join a 5 GHz network, and it cannot
log into a captive portal. Public and coworking Wi-Fi typically fails on both
counts. Use a phone hotspot with "Maximize Compatibility" **on** (forces
2.4 GHz), or a home router.

To run hardware tests
```bash
.venv/bin/python -m pytest -m "hardware and join" --ssid "wifi name" --password "your pass here" -s
```
