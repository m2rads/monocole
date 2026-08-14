# Minicole

Desktop companion app for a smart monocle wearable. The monocle (currently a
Seeed XIAO ESP32-S3 Sense, board may change) sends voice — and later images —
over Bluetooth Low Energy; the app runs local inference with llama.cpp and
streams generated tokens back to the monocle's micro-LED display. The app is
essentially a llama.cpp wrapper plus model management plus monocle
connection/session management. Full plans live in `docs/`.

## Commands

- `bun tauri dev` — run the app (`bun run dev` is Vite only, no Tauri shell).
- `bun run test` — frontend (Vitest). **Never `bun test`** — that invokes
  Bun's own runner, which bypasses vitest.config/setup and fails everything.
- `bun run lint` has pre-existing noise (scans src-tauri/target artifacts,
  shadcn react-refresh warnings) — not introduced by current work.

## Gotchas

- llama.cpp `llama-server` runs as a spawned child process. The binary is
  fetched by `scripts/fetch-llama-server.sh` into `src-tauri/binaries/llama/`
  and is gitignored — **run that script on fresh clones or the chat will
  error.**
- Tauri capabilities are pruned to `core:default` only
  (`capabilities/default.json`): everything IPC-heavy is done in Rust, so the
  webview needs no plugin permissions. Only plugin: shell.
- `ble.rs` is deliberately generic (no service UUIDs) until firmware exists.
- **Adding a BLE characteristic? Bump `MONOCLE_GATT_VERSION` in firmware
  `bleprph.h`.** A bonded macOS caches the GATT table forever, so a new
  characteristic is simply absent from `peripheral.characteristics()` until
  the device is forgotten — it presents as the app's write doing nothing.
  Bumping the version makes the firmware send Service Changed on the next
  connect. Symptom to recognise: `ble: no display characteristic on this
  device — discovered [...]` listing the *previous* build's UUIDs.
  The Service Changed handle is resolved in `gatt_svr_on_sync()`, **not** in
  `gatt_svr_init()` — handles do not exist until `ble_gatts_start()` runs on
  the way to the sync callback, and looking it up in init silently disabled the
  whole mechanism until 2026-08-13. Boot log must say `service changed
  characteristic at handle N`; the warning form means bumping the version does
  nothing.
- The theme provider globally disables CSS transitions during a light/dark
  switch — the animated toggle icons opt out with `!important` utilities.

## Tests

- Not covered by design: spawning a real llama-server (needs binary + model)
  and live BLE (needs hardware) — those are manual tests.
- Frontend: Tauri IPC is mocked globally in `tests/setup.ts`; drive it with
  `tests/tauri-mocks.ts` (`invokeMock`, `emitTauriEvent`).
- Rust test layout is non-standard — see `src-tauri/AGENTS.md`.

## Deferred / known holes (decided, not forgotten)

- **Config server**: model manifest should become remote > cached > bundled
  with ETag + sha256 verification + signing — see TODO(config-server) in
  manifest.rs/models.rs. Catalog `sizeBytes` are approximate and `sha256` is
  currently null.
- **Packaging the sidecar**: current llama-server resolution is dev-only
  (CARGO_MANIFEST_DIR); production needs bundled resources or externalBin +
  static build, and code-signing survives — TODO(packaging) in llama.rs and
  scripts/fetch-llama-server.sh.
- **Session persistence**: sessions/messages are in-memory only and are lost
  on restart.
- **Generation cancel**: no stop button; TODO in llama.rs stream_completion.
- **Monocle protocol**: the wire contract lives in `docs/protocol.md`. The
  Wi-Fi plane (`wifi_creds` write, `wifi_state` notify, `wifi_control` write,
  and the port-3333 socket framing) is implemented end to end and **frozen**
  in three places — firmware `gatt_svr.c`, `src-tauri/src/ble.rs` +
  `socket.rs`, and protocol.md. control/voice/tokens/status are still
  unassigned. TODO(monocle-protocol) and TODO(auto-reconnect) in ble.rs.
  Nothing in `src/` calls the Wi-Fi or socket commands yet — they are driven
  only from the pytest suite.
- **Display**: a 128×64 SSD1306 OLED on I2C (SDA GPIO5, SCL GPIO6), driven by
  the `display` characteristic — `[op][utf-8]`, op is clear/set/append. Both
  sides are built: `main/display.c` renders on its own task (never render in
  the BLE callback — I2C is ~25 ms), and `ble.rs` writes a greeting on
  connect. Text goes over BLE, never the socket, because waking Wi-Fi costs
  ~1.5 s regardless of payload size. Generated tokens are mirrored to the panel
  by `monocle.rs`, which batches them on a 150 ms timer — never write per
  token, it outruns both the link and the ~25 ms redraw. A reply longer than
  the panel is **paginated in the firmware** — `display.c` holds the whole
  response and advances a page at a time, dwelling 2 s + 40 ms/char (max 8 s).
  Pagination and wrapping both belong there because a page break is a line
  break, and only the renderer knows where lines fall; panel geometry is 21×8
  and nothing outside `display.c` should assume it. **Still open**: geometry
  over `status`, and no way for the wearer to page back. See Future work in
  protocol.md.
- **Firmware**: lives in `firmware/`, forked from ESP-IDF's
  `bleprph_wifi_coex` and rewritten since; advertises as `minicole-monocle`.
  Milestones 1, 2, 4 and 5 are done and verified on hardware: bonded pairing
  with keys in NVS, provisioning, join, IP over BLE, the TCP data plane with
  idle teardown, and the OLED. Voice and camera are not started. Decisions:
  C++ on ESP-IDF v6.0.2, NimBLE, and a
  two-plane transport — BLE always-on for control/status/tokens/voice (ADPCM
  ~64kbps), Wi-Fi on-demand for JPEG stills only (LLM consumes stills, not
  video). See docs/firmware-plan.md for rejected alternatives.
- **Wi-Fi throughput is ~1.5 Mbps, not "tens of milliseconds" per still** —
  BLE/Wi-Fi coexistence on one antenna is the ceiling, and it will tighten
  when voice streams. Measured figures in docs/protocol.md.
- **STT**: voice → text stage (likely whisper.cpp as a second sidecar) not
  started; chat is text-only today.
- **Model acquisition UX**: curated list + direct .gguf URL download are
  implemented; "browse a Hugging Face repo" and "import local GGUF file"
  remain planned. Resuming a cancelled URL download requires re-pasting the
  URL (source URLs are not persisted).
