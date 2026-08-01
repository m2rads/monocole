# Firmware learning notes (BLE, Wi-Fi, tooling)

Concept + decision reference built from the first real firmware session:
getting a NimBLE beacon to build, flash, and advertise on the XIAO
ESP32-S3, plus the architecture the monocle is actually heading toward.
Pairs with `firmware-setup.md` (that one is environment/build mechanics;
this one is the "why" and the concepts). Read `firmware-plan.md` for the
product plan.

---

## 1. IDE tooling: clangd + Zed for ESP-IDF

The editor language server is separate from the compiler. `idf.py build`
uses GCC; Zed's autocomplete/errors come from **clangd**. Getting clangd to
understand ESP-IDF code took three fixes — expect to redo these per project.

### Use Espressif's clangd, not the system one
- Firmware targets **Xtensa**, which is **not** an upstream LLVM backend.
  Apple/mainline clangd (`/usr/bin/clangd`) has no Xtensa target and cannot
  parse ESP-IDF headers or flags (`-mlongcalls`, `target=xtensa`).
- Espressif ships its own LLVM fork with Xtensa support. Point Zed at it in
  `.zed/settings.json` via `lsp.clangd.binary.path`:
  `~/.espressif/tools/esp-clang/<ver>/esp-clang/bin/clangd`.

### Feed clangd the toolchain's system headers (`--query-driver`)
- Standard headers like `stdio.h` come from the Xtensa toolchain's newlib,
  not `/usr/include`. clangd only finds them by inspecting the compiler.
- Set `--query-driver=<glob to xtensa gcc>` as a clangd launch **argument**
  in `.zed/settings.json`. The glob must match the compiler the build
  actually used — if the target is `esp32` the driver is
  `xtensa-esp32-elf-gcc`; if `esp32s3`, `xtensa-esp32s3-elf-gcc`. A glob
  pinned to one target silently fails on another → "`stdio.h` not found".

### Strip GCC-only flags clangd rejects (`.clangd` file)
- The build's `compile_commands.json` contains GCC flags clang doesn't know
  (`-fno-malloc-dce`, `-fzero-init-padding-bits=all`, `-mlongcalls`, ...).
  clangd aborts the whole parse on the first unknown flag and anchors the
  error to **line 1** regardless of file contents.
- Fix: a `.clangd` file in the project root with `CompileFlags.Remove`
  listing those flags. Use a trailing `*` for flags carrying a value
  (`-fzero-init-padding-bits*`). Whack-a-mole: each new GCC-15 flag that
  shows up after a rebuild gets added here.

### Two hard preconditions clangd can't work without
1. **A build must have run** — `compile_commands.json` only exists after
   `idf.py build`. No build → clangd falls back to flagless parsing → no
   system headers.
2. **At least one `.c` file** — clangd infers a header's flags from a `.c`
   that includes it. A folder of only `.h` files has nothing to anchor to.

**Reload after changes:** Zed has `editor: restart language server` in the
command palette (⌘⇧P), but changing the clangd *binary* needs a full quit
(⌘Q) and reopen.

---

## 2. C + build-system basics (as they show up in ESP-IDF)

### `.h` header files
- Header = **declarations** (the interface: function prototypes, structs,
  `#define`s, `typedef`s). `.c` = **definitions** (the actual code).
- `#include "x.h"` literally pastes the header's text in. `<...>` = system/
  library headers on the include path; `"..."` = your own, searched locally.
- Include guards (`#ifndef X_H`/`#pragma once`) stop double-inclusion errors.
- In ESP-IDF: `#include "esp_wifi.h"`, `"host/ble_gap.h"` etc. pull in IDF's
  declarations; the compiled code is linked from IDF's libraries.

### `CMakeLists.txt` — two levels
- **Project-level** (`./CMakeLists.txt`): `include(...project.cmake)` hands
  control to ESP-IDF's build framework (needs `IDF_PATH`); `project(name)`
  names the output binary.
- **Component-level** (`main/CMakeLists.txt`): `idf_component_register(SRCS
  ... PRIV_REQUIRES bt nvs_flash INCLUDE_DIRS ./include)` declares one
  component's sources, dependencies, and header dirs. `bt` = the Bluetooth
  stack; `nvs_flash` = non-volatile storage (BLE stores bonding keys there).

### `app_main()` is mandatory
- Every ESP-IDF app must define `void app_main(void)` — FreeRTOS startup
  calls it as the entry point. Missing it = **link error** `undefined
  reference to 'app_main'` (compiles fine, fails at link). A `main/` with
  only headers and no `.c` hits exactly this.

---

## 3. Bluetooth concepts

### Two host stacks: Bluedroid vs NimBLE
- The BT stack splits into **controller** (radio/link layer, Espressif's,
  same either way) and **host** (GAP/GATT/L2CAP/security — pick one).
- **Bluedroid**: Classic BT **+** BLE. Heavy (more RAM/flash), verbose API.
  Required if you need Classic BT (A2DP audio, SPP, classic HID).
- **NimBLE**: **BLE only**. Lighter, cleaner API, Espressif's recommendation
  for BLE-only designs.
- **Monocle = NimBLE.** BLE-only plan; the smaller footprint matters
  alongside audio/JPEG/Wi-Fi buffers. Caveat: some old examples are written
  against Bluedroid — occasionally you translate GATT calls to NimBLE.

### Beacon vs connectable peripheral
- **Beacon**: broadcasts advertising packets only, **non-connectable**. Used
  for proximity/positioning (iBeacon = Apple UUID+major/minor; Eddystone =
  Google URL/ID). Teaches NimBLE **advertising** setup — the "advertise"
  half without connections.
- **Peripheral (`bleprph`)**: connectable, exposes a **GATT service** with
  characteristics, handles connect/disconnect + security. **This is the
  monocle's role.** Characteristics = the app writes Wi-Fi creds / reads IP
  and status. Notifications = the peripheral streams data out (voice,
  events) without being polled.

### Discovery reality check
- A beacon does **not** appear in the phone/computer's **system Bluetooth
  settings** — that screen is for pairable Classic/standard devices.
- Even a connectable custom peripheral generally won't show in system
  Bluetooth settings. Custom BLE devices are found by **scanner apps**
  (nRF Connect, LightBlue) and by **your own app** talking to the BLE APIs
  directly — which is what `ble.rs`/`use-ble.ts` already do (scan list lives
  inside minicole, not the OS menu).

### How wireless audio works (context)
- Headphones use **Classic BT** (A2DP for music, HFP for calls), not BLE.
  Audio is **compressed** (SBC/AAC/aptX/LDAC, ~200 kbps–~1 Mbps) so it's
  lighter than raw CD audio (1.4 Mbps). Mic/call path is low-bitrate
  (~64 kbps) — why calls sound worse.
- **LE Audio / LC3** (BT 5.2+) runs audio over **BLE** efficiently
  (~160 kbps). The modern BLE-native path — but IDF support is newer/less
  mature than plain GATT.
- For the monocle: voice at **ADPCM ~64 kbps over BLE is fine** (phone-call
  ballpark). **Images are the heavy part** — that's what justifies Wi-Fi.

---

## 4. The architecture: BLE control plane + Wi-Fi data plane

Decision: **skip the BLE-only-data v1**. BLE manages the connection and
bootstraps Wi-Fi; **Wi-Fi carries the heavy data**.

- **BLE = control plane (always on).** App scans, connects, manages session.
  Also the bootstrap channel: app writes Wi-Fi SSID/password to a
  characteristic; firmware writes its assigned IP + "ready" back. Stays
  connected for control/signaling.
- **Wi-Fi = data plane.** Firmware brings up `esp_wifi`, opens a socket
  server (TCP/WebSocket); app connects to the IP; bulk data flows there.
- **Handoff replaces BluFi:** BLE connect → app writes creds → firmware
  joins Wi-Fi, gets IP → firmware reports IP over BLE → app opens socket.
  BLE stays up throughout.

### BluFi — evaluated and dropped
- BluFi = Espressif's protocol for provisioning Wi-Fi creds over BLE. It
  **only provisions** — it does not give you a data channel; you still open
  your own socket afterward.
- Dropped because: it assumes Espressif's **mobile** apps, but our client is
  a **desktop** app — we'd reimplement its DH/AES/GATT framing in Rust for
  no gain. We own both sides, so a **custom BLE characteristic for creds** is
  simpler. Also pulls in the heavier Bluedroid host.
- Note: in the ESP-IDF examples tree, `blufi` sits directly under
  `examples/bluetooth/` (it's Bluedroid-based), whereas the NimBLE
  peripheral examples are nested under `examples/bluetooth/nimble/`.

### Wi-Fi topology: STA now, SoftAP later
- The data-plane code (socket server + protocol) is **identical** either
  way. Only Wi-Fi bring-up and how the app learns the IP differ. Not a
  lock-in decision.
- **STA (join a router)** — chosen first. App keeps internet, everything on
  one LAN, far nicer to develop against. Discovery is nearly free: firmware
  reports its DHCP IP back over the BLE channel we're already building (no
  mDNS needed initially).
- **SoftAP (monocle hosts the AP)** — deferred. No router needed / portable,
  but the **Mac loses its own Wi-Fi/internet** while connected (rough for
  dev and for users). Add later as a "no known Wi-Fi" fallback — a
  self-contained firmware change on top of the same data layer.

---

## 5. Flashing & the build/flash loop

### What the commands do
- `idf.py build` — compiles; writes `build/*.bin` + `compile_commands.json`.
- `idf.py flash` — builds if needed, resets the chip into ROM download mode,
  writes three binaries to fixed flash offsets (bootloader @ 0x0, partition
  table @ 0x8000, app @ 0x10000), resets → runs `app_main()`. Writes to
  **persistent** flash; runs on every power-up.
- `idf.py monitor` — reads the serial log only (doesn't change the chip).
  `idf.py flash monitor` — flash then watch boot. Exit monitor: Ctrl+].
- `idf.py app-flash` — flash only the app partition (faster iteration when
  only your code changed).
- `idf.py set-target esp32s3` — **switch chip**. Rewrites `sdkconfig`,
  reconfigures the whole build. Resets sdkconfig to defaults.

### Errors hit this session (and fixes)
- `undefined reference to 'app_main'` — no `.c` defining `app_main()`. Add
  the source file.
- `This chip is ESP32-S3, not ESP32. Wrong chip argument?` at flash — project
  target was `esp32`, hardware is S3. Run `idf.py set-target esp32s3`,
  rebuild, reflash. (Also fixed the clangd header issue, since the compiler
  then matches the query-driver glob.)
- `Detected size(8192k) larger than ... header(2048k)` — XIAO S3 has **8 MB**
  flash but build defaulted to 2 MB. Boots fine but only sees 2 MB. Set
  `menuconfig → Serial flasher config → Flash size → 8 MB` before building
  the real partition table (OTA/Wi-Fi/audio will want the space).

### Reading a "failed" build
- `ninja: build stopped: subcommand failed` at the end is not the error —
  it's the summary. The real cause is earlier; grep the logs in
  `build/log/` for `FAILED:` / `error:` / `undefined reference`.

### Normal-but-alarming log lines
- `Returned from app_main()` — **not** a crash. NimBLE runs in its own
  FreeRTOS task; `app_main` kicks it off and returns.
- Advertised address printed reversed vs the BT MAC — just little-endian
  display of the same address.

---

## 6. Testing a beacon / BLE device

- **`idf.py monitor`** — device-side proof it's advertising (isolates
  firmware from scanner problems). First check.
- **nRF Connect** (mobile, iOS/Android) — gold standard for beacons; decodes
  iBeacon/Eddystone payloads fully. Use this for the definitive check.
- **macOS scanners** (Bluetility, LightBlue) and **our own btleplug app** —
  will show the device, but **won't decode iBeacon data**: Apple's
  CoreBluetooth filters iBeacon manufacturer data (company ID 0x004C
  reserved for CoreLocation). Not a firmware bug. Eddystone + named
  connectable peripherals are unaffected — so this quirk disappears once we
  move to `bleprph`.

---

## 7. Example-reading roadmap (ESP-IDF v6.x, NimBLE)

Match examples to **v6.x** (we're on v6.0.2, not the v5 the plan mentions).
The NimBLE examples live under `examples/bluetooth/nimble/` — one level
deeper than `blufi`, which sits at the top of `examples/bluetooth/`. (Exact
folder contents shift between IDF versions — list your own install to
confirm names.)

1. **`bleprph`** — read first. Connectable peripheral + GATT service +
   characteristics + security. The monocle's actual skeleton; where the
   desktop app can finally connect (`ble.rs` scan → connect). Characteristics
   = Wi-Fi cred write + IP/status read.
2. **`blehr`** — second. Peripheral pushing data via **notifications** on a
   timer. Ignore the heart-rate specifics; the mechanism is how the monocle
   streams voice/events out to the app.
3. **Later (Wi-Fi data plane):** `wifi/getting_started/station` (join
   network, get IP) + `protocols/sockets/tcp_server` (the pipe). Optionally
   skim `wifi_provisioning/wifi_prov_mgr` to see Espressif's built-in
   BLE-based provisioning before committing to the custom handoff.

Path: **beacon (done)** → **`bleprph`** → **`blehr`** → **Wi-Fi station +
TCP server**. Don't jump ahead to Wi-Fi; `bleprph` is the foundation and the
first real end-to-end test against the minicole app.

---

## Open threads (from this session)

- **pytest language-server errors** (`pytest_embedded_idf` unresolved): the
  package is genuinely **not installed** in the IDF Python env
  (`~/.espressif/python_env/idf6.0_py3.13_env`). `pytest-embedded*` isn't
  part of base IDF. Install via the IDF pytest feature / `requirements.
  test-specific.txt`, then point the Python LSP at that interpreter. Deferred.
- **8 MB flash size** not yet enabled in menuconfig.
- **PSRAM** on the XIAO S3 Sense not yet enabled — turn on before building up
  real firmware (BLE + Wi-Fi + audio will want it).
