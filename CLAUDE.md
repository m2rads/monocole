# Minicole

Desktop companion app for a smart monocle wearable. The monocle (currently a
Seeed XIAO ESP32-S3 Sense, board may change) sends voice — and later images —
over Bluetooth Low Energy; the app runs local inference with llama.cpp and
streams generated tokens back to the monocle's micro-LED display. The app is
essentially a llama.cpp wrapper plus model management plus monocle
connection/session management. Full plans live in `docs/`.

## Stack

- Tauri v2 (migrated from v1) + React 19 + Vite 8 + Tailwind v4 + shadcn
  (Base UI variant, `@base-ui/react`). Package manager: **bun**.
- Rust backend in `src-tauri/` (lib `app_lib`, thin `main.rs` shim).
- llama.cpp `llama-server` runs as a spawned child process (Metal build,
  fetched by `scripts/fetch-llama-server.sh` into `src-tauri/binaries/llama/`,
  gitignored — run that script on fresh clones or the chat will error).

## Commands

- `bun tauri dev` — run the app. `bun run build` — typecheck + bundle frontend.
- `bun run test` — frontend (Vitest). **Never `bun test`** — that invokes
  Bun's own runner, which bypasses vitest.config/setup and fails everything.
- `bun test:rust` (or `cargo test` in src-tauri) — backend suite.
- `bun run typecheck` — includes `tests/` since tsconfig.app.json covers it.
- `bun run lint` has pre-existing noise (scans src-tauri/target artifacts,
  shadcn react-refresh warnings) — not introduced by current work.

## Current features

Frontend (src/):
- Chat UI (`components/chat-view.tsx`): centered welcome state on a fresh
  session; streaming assistant replies (typing dot, blinking cursor), inline
  errors, Enter-to-send, input locked while generating.
- Sessions (`hooks/use-sessions.tsx`): provider owning all session state;
  session created on first message; AI-generated titles after the first
  exchange (provisional truncated title until then). **In-memory only — lost
  on restart; persistence is an open TODO.**
- Sidebar (`components/app-sidebar.tsx`, `nav-main.tsx`, `nav-history.tsx`):
  New Session, Settings → Models / Connection, History capped at 7 items
  with More/Less overflow (MAX_VISIBLE const).
- Models page (`components/models-view.tsx`, `hooks/use-models.ts`): curated
  catalog cards with download/progress/cancel/resume/delete and active-model
  selection; "Add from URL" form for direct .gguf links (backend derives the
  file name from the URL, events keyed by it) with a Custom models section
  for non-catalog local files.
- Connection page (`components/connection-view.tsx`, `hooks/use-ble.ts`):
  BLE scan (30s TTL) with discovered-device list, unnamed devices hidden by
  default, connect/disconnect, bluetooth-off detection.
- Navigation is a simple view switch (`hooks/use-view.tsx`: chat | models |
  connection) — no router.
- Theme: light/dark via "d" key or header toggle (`mode-toggle.tsx`); theme
  provider globally disables CSS transitions during switch — the animated
  toggle icons opt out with `!important` utilities.

Backend (src-tauri/src/):
- `manifest.rs` — curated model catalog compiled in from
  `src-tauri/manifest/models.json` (4 models; sizeBytes approximate,
  sha256 currently null).
- `models/` — model management, one submodule per concern (shared path
  helpers in mod.rs): `files.rs` (naming rules, dir scanning, delete),
  `download.rs` (resumable engine: .part + HTTP Range, progress events every
  4MB, cancel, direct-URL entry point), `settings.rs` (active-model persisted
  in `<app-data>/settings.json`); models stored in `<app-data>/models/`
  (macOS: `~/Library/Application Support/com.atelier.minicole/`).
- `llama.rs` — sidecar lifecycle: spawns llama-server on a random free port
  with the active model, /health polling (120s), auto-restart on model
  switch, killed on app exit; chat via OpenAI-compatible SSE re-emitted as
  `chat-stream` events; `generate_session_title` one-shot completion with
  `<think>`-block stripping.
- `ble.rs` — BLE central via btleplug: scan (TTL + generation counter),
  connect/disconnect, adapter power-state checks, events to the frontend.
  Deliberately generic (no service UUIDs) until firmware exists.
- Capabilities pruned to `core:default` only (capabilities/default.json);
  everything IPC-heavy is done in Rust, so the webview needs no plugin
  permissions. Only plugin: shell. macOS Bluetooth permission via
  `src-tauri/Info.plist`.

## Tests

- Rust: 29 tests, all files in `src-tauri/tests/` but compiled as in-crate
  modules via `#[path]` hooks at the bottom of each src file (private access;
  `autotests = false` in Cargo.toml — top-level tests/*.rs are NOT separate
  crates). One file per domain, mirroring src: llama_test.rs,
  manifest_test.rs, ble_test.rs, models/{files,download,settings}_test.rs +
  shared `tests/helpers.rs` (crate::test_helpers). Download/chat tests run
  against real local tiny_http servers.
- Frontend: 27 Vitest tests in `tests/` (project root): use-sessions,
  use-models, use-ble, chat-view, nav-history. Tauri IPC mocked globally in
  `tests/setup.ts`; drive it with `tests/tauri-mocks.ts` (`invokeMock`,
  `emitTauriEvent`).
- Not covered by design: spawning a real llama-server (needs binary + model)
  and live BLE (needs hardware) — manual tests.

## Deferred / known holes (decided, not forgotten)

- **Config server**: model manifest should become remote > cached > bundled
  with ETag + sha256 verification + signing — see TODO(config-server) in
  manifest.rs/models.rs.
- **Packaging the sidecar**: current llama-server resolution is dev-only
  (CARGO_MANIFEST_DIR); production needs bundled resources or externalBin +
  static build, and code-signing survives — TODO(packaging) in llama.rs and
  scripts/fetch-llama-server.sh.
- **Session persistence**: sessions/messages are not saved to disk.
- **Generation cancel**: no stop button; TODO in llama.rs stream_completion.
- **BLE protocol**: GATT service/characteristics are a draft in
  `docs/ble-protocol.md` — UUIDs unfrozen until the firmware throughput test
  (milestone 2 in docs/firmware-plan.md). TODO(monocle-protocol) and
  TODO(auto-reconnect) in ble.rs.
- **Firmware**: not started. Decisions made: C++ on ESP-IDF v5, BLE-only v1
  (voice ADPCM ~64kbps, on-demand JPEG stills — LLM consumes stills, not
  video), Wi-Fi media plane deferred to v2. Planned home: `firmware/`.
- **STT**: voice → text stage (likely whisper.cpp as a second sidecar) not
  started; chat is text-only today.
- **Model acquisition UX**: curated list + direct .gguf URL download are
  implemented; "browse a Hugging Face repo" and "import local GGUF file"
  remain planned. Resuming a cancelled URL download requires re-pasting the
  URL (source URLs are not persisted).
