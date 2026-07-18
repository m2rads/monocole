# Monocole 🧐

Desktop companion app for a smart monocle wearable. The monocle sends voice
(and later images) over BLE; the app runs local inference with llama.cpp and
streams tokens back to the monocle's display. Plans and protocol docs live in
[`docs/`](docs/).

## Setup

Prerequisites: [bun](https://bun.sh), Rust (stable), and the
[Tauri v2 system dependencies](https://v2.tauri.app/start/prerequisites/).

```bash
bun install
./scripts/fetch-llama-server.sh   # fetches the llama-server binary (gitignored)
bun tauri dev
```

Without the fetch script, chat will error — the app spawns `llama-server`
from `src-tauri/binaries/llama/`. Models are downloaded in-app under
Settings → Models.

## Commands

```bash
bun tauri dev      # run the app
bun run build      # typecheck + bundle frontend
bun run test       # frontend tests (Vitest — never `bun test`)
bun test:rust      # backend tests
bun run typecheck
```
