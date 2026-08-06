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
- The theme provider globally disables CSS transitions during a light/dark
  switch — the animated toggle icons opt out with `!important` utilities.

## Tests

- Not covered by design: spawning a real llama-server (needs binary + model)
  and live BLE (needs hardware) — those are manual tests.
- Frontend: Tauri IPC is mocked globally in `tests/setup.ts`; drive it with
  `tests/tauri-mocks.ts` (`invokeMock`, `emitTauriEvent`).
- Rust test layout is non-standard — see `src-tauri/CLAUDE.md`.

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
- **Monocle protocol**: the wire contract (both planes) is a draft in
  `docs/protocol.md`. The Wi-Fi provisioning pair (`wifi_creds` write,
  `wifi_state` notify) is implemented and its UUIDs are **frozen** in three
  places — firmware `gatt_svr.c`, `src-tauri/src/ble.rs`, and protocol.md.
  control/voice/tokens/status are still unassigned. No TCP client exists yet.
  TODO(monocle-protocol) and TODO(auto-reconnect) in ble.rs.
- **Firmware**: `firmware/` not started. Milestone 1 (advertise + connect) is
  done via the stock `bleprph` example in `ble-examples/`, which the app
  connects to unmodified. Decisions: C++ on ESP-IDF v6.0.2, NimBLE, and a
  two-plane transport — BLE always-on for control/status/tokens/voice (ADPCM
  ~64kbps), Wi-Fi on-demand for JPEG stills only (LLM consumes stills, not
  video). See docs/firmware-plan.md for rejected alternatives.
- **STT**: voice → text stage (likely whisper.cpp as a second sidecar) not
  started; chat is text-only today.
- **Model acquisition UX**: curated list + direct .gguf URL download are
  implemented; "browse a Hugging Face repo" and "import local GGUF file"
  remain planned. Resuming a cancelled URL download requires re-pasting the
  URL (source URLs are not persisted).
