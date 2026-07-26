# Firmware setup & build (ESP-IDF)

Reference for getting an ESP-IDF project to compile and flash on the XIAO
ESP32-S3. Written from the actual steps used to build the throwaway
`hello_world` smoke test. Read this before touching `firmware/`.

## Environment as installed

- Toolchain installed via **EIM** (ESP-IDF Installation Manager).
- ESP-IDF version: **v6.0.2**, at `~/.espressif/v6.0.2/esp-idf/`.
  - The firmware plan (`firmware-plan.md`) says v5.x. That is stale. We are
    on v6.0.2. v6.0 is a major bump with breaking API changes from v5 — when
    you copy example code or read docs, match them to **v6.x**, not v5.
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

- `hello_world` is a disposable setup check. The real firmware — the
  `minicole-monocle` BLE peripheral — goes in `firmware/`, scaffolded from
  the `bleprph` NimBLE example. See `firmware-plan.md` milestone 1.
- Every command above assumes `export.sh` was sourced in the current
  terminal. If `idf.py: command not found`, that's the reason.


 . ~/.espressif/v6.0.2/esp-idf/export.sh && cd ~/Documents/projects/minicole/hello-world && idf.py -p /dev/cu.usbmodem3101 flash monitor
