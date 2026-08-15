# Voice plan

From a wake word on the monocle to a transcribed turn in the app's session
history. Covers milestone 3 of [firmware-plan.md](firmware-plan.md) and the
app-side session work that has no milestone yet.

Written 2026-08-13, against the state committed at 9592da0 plus the
advertising/link-tuning work on disk. The wire additions this plan makes belong
in [protocol.md](protocol.md) the moment they are assigned, not after they
work.

## The loop

```
wake word ("Hi ESP")
  -> firmware: status notify  voice_started
  -> panel: "listening"
  -> mic -> AFE -> IMA ADPCM -> voice notify, ~32 ms per frame
  -> VAD says the utterance ended
  -> firmware: status notify  voice_ended
  -> app: decode ADPCM -> WAV -> whisper-server -> transcript
  -> app: transcript becomes the user turn in a session, visible in history
  -> llama.cpp replies, streaming to the window and to the panel as it does today
```

Everything after the transcript already exists. The new work is the four stages
before it, plus teaching sessions to start from something other than the
composer.

## Decisions

**Wake word: `wn9_hiesp` ("Hi ESP").** Free, English, ships with esp-sr.
`Alexa` is equally available and equally free, and is rejected only because a
room containing an Echo is a room full of false triggers, and because leaning
on another product's trigger phrase is a thing to undo later rather than keep.

The component turns out to carry about twenty English phrases outright,
including **Jarvis** (`wn9_jarvis_tts`), Computer, Mycroft and Hey Willow. An
earlier draft of this plan said Jarvis was a commercial model Espressif trains
to order; that was wrong. Swapping is one line in `sdkconfig.defaults` and
costs a reflash, because the wake word never crosses the wire and nothing
outside the firmware knows which one is loaded.

**The session starts on the first utterance, not on connect.** Connecting arms
a voice session; the row appears in history the moment someone speaks, titled
from what they said, exactly as typing behaves today. Creating the row on
connect would have been a more literal reading, but BLE links drop — auto
reconnect is still a TODO in `ble.rs` — and every drop would leave an empty
session behind. Disconnecting ends the session; the next connect begins a new
one.

**The device decides when an utterance ends, and says so.** AFE's VAD, after
~800 ms of silence, with a hard cap at 15 s so a noisy room cannot stream
forever. The app never guesses from a gap in frames.

**ADPCM frames are self-contained.** IMA ADPCM carries a predictor and a step
index from sample to sample, so a single dropped notification corrupts
everything after it. Each frame therefore restates its own starting state, in a
5-byte header. The alternative is an utterance that turns to noise halfway
through because one packet was lost, on a link where the app already tolerates
gaps rather than asking for retransmission.

**The encoder keeps running across frames rather than resetting** — the header
records where each frame began, it does not force a fresh start. An earlier
draft said "resets per frame", which is equally decodable and sounds worse: the
predictor climbs from zero at the top of all 31 frames a second, which is an
audible buzz. Caught by a codec test rather than by ear, which is the argument
for having written the codec tests before the firmware.

**STT is `whisper-server`, built from source.** whisper.cpp publishes no macOS
release binary — only Ubuntu, Windows, and an xcframework — so
`scripts/fetch-whisper-server.sh` clones a pinned tag and builds with Metal,
then fetches `ggml-base.en.bin`. It then mirrors `llama.rs` exactly: spawn on
demand, keep the model resident, POST per utterance. Rejected: `whisper-cli`
per utterance, which reloads the model every time someone speaks and puts
0.5–1 s in front of every reply; and Homebrew, which is an unpinned dependency
and a second install instruction.

**Sessions stay owned by the frontend.** `use-sessions.tsx` remains the single
source of truth; Rust emits voice events and the provider turns them into the
same `Session` objects the composer produces. Moving sessions into Rust would
fix persistence too, but it is a separate piece of work and this plan should
not smuggle it in.

## Phase 0 — land what is on disk

Thirteen files are uncommitted and verified on hardware. Commit them before
starting, or the first voice commit drags an unrelated firmware rename with it.

While in those files, close the drift the rename left behind:

- `firmware/test/README.md` still says the default device name is
  `nimble-bleprph` and to check that the name matches. The suite finds the
  board by service UUID now, and `firmware/README.md` sends readers here.
- protocol.md contradicts itself on MTU — the display section still explains
  253 bytes via "the MTU of 256 macOS currently negotiates", while the new Link
  parameters table above it says 512. Same stale reasoning in `ble.rs:40`,
  `display.h:36`, `test/display.py:22`, `test_display.py:52`.
- firmware-plan.md's "Throughput essentials" still prescribes MTU 517 and a
  ~15 ms interval, both superseded by measurement.
- `ble.rs:276` says a device without the wifi_state characteristic "connects
  normally" — with the scan filtered by service UUID it never appears at all.
- protocol.md line 68 and the first Open question both describe filtering scans
  by service UUID as future work. It is done.

Optional while there: `DISPLAY_TEXT_MAX` can go from 252 to 509 now that the
MTU is 512, halving the writes per token batch. Four places plus the firmware
buffer.

## Phase 1 — de-risk, before anything depends on it

Two spikes, independent, both cheap.

**1a. Does esp-sr build on ESP-IDF v6.0.2?** — ~~the largest unknown in the
plan~~ **answered, on hardware, 2026-08-13. It works.**

esp-sr **2.5.0** resolves, compiles, links and runs on v6.0.2. Measured on the
board:

| | |
|---|---|
| App binary | 1.03 MB → **2.29 MB** with WakeNet linked |
| Model partition | 284 KB packed (`wn9_hiesp`), in a 512 K partition |
| AFE instance | **22 KB internal, 374 KB PSRAM** |
| Free with AFE up | 124 KB internal, **7814 KB PSRAM** |
| Feed chunk | **512 samples = 32 ms**, 1 channel, 16 kHz |
| Pipeline | `[input] -> |VAD(WebRTC)| -> |WakeNet(wn9_hiesp)| -> [output]` |

Three things worth keeping from that:

- **The app more than doubled.** WakeNet's code would not have fitted the old
  1500 K app partition, so `partitions.csv` was not optional bookkeeping — it
  was load-bearing before a single line of voice code existed.
- **`nvs` keeps its old offset and size on purpose**, so changing the table did
  not re-pair the bonded Mac or forget the network. Verified: the board came up
  saying `using stored credentials`.
- **PSRAM is a non-issue.** 374 KB of nearly 8 MB. The constraint on this
  device is internal RAM and flash, not PSRAM.

The escape hatch that is no longer needed: parking the wake word behind an
app-driven trigger on `control` while esp-sr got sorted.

**1b. Sustained BLE notification throughput** (task 6 from the handoff). A
firmware task pushing synthetic 261-byte frames at 31.25 Hz, and a pytest that
measures achieved kbps and counts sequence gaps over 60 s — once with Wi-Fi
idle, once with a bulk transfer running, because one antenna serves both.

Gate: ≥64 kbps sustained with no gaps, Wi-Fi idle. The coexistence number is
information rather than a gate, but if voice collapses during a bulk transfer,
then voice and stills must not overlap, and that is a protocol rule worth
knowing before the encoder exists.

## Phase 2 — freeze the protocol additions

Assign the `voice` and `status` UUIDs, and write them into protocol.md,
`gatt_svr.c`, `ble.rs`, and `test/protocol.py` in the same change.

**Bump `MONOCLE_GATT_VERSION` once, for both.** Two characteristics land
together; one bump covers them. Skipping it means a bonded Mac keeps its cached
table and both are simply invisible, which reads as a bug in the new code.

**voice** — device → app, notify:

```
[seq: u16][predictor: i16][step_index: u8][adpcm payload]
```

256 payload bytes = 512 samples = 32 ms of 16 kHz mono audio, 261 bytes on the
wire. That is ~65 kbps, one notification per 30 ms connection interval, and
half of the 509-byte ATT payload the MTU of 512 allows — so there is room for a
retry or a Wi-Fi burst stealing airtime.

**status** — device → app, notify: `[event: u8][extra ...]`

```
1 voice_started    wake word fired, audio follows
2 voice_ended      + 1 byte reason: 0 vad, 1 capped, 2 error
3 panel_geometry   + 2 bytes: cols, rows
```

`panel_geometry` is not voice work, but it has been an open item since the
display landed and it costs one byte in a characteristic being added anyway.

Update `test/protocol.py` with an independent Python IMA decoder — deliberately
not shared with the firmware, so drift on either side fails a test instead of
being mirrored into it, which is how the existing wire tests are built.

## Phase 3 — capture, proven audible on its own — **done, 2026-08-14**

`main/mic.c`: PDM RX on GPIO42 (CLK) and GPIO41 (DATA) — the XIAO Sense's
onboard mic — 16 kHz mono 16-bit, read through the chip's PDM-to-PCM filter so
samples come back as ordinary signed 16-bit. No detector, no encoder.

**The audio was confirmed intelligible by ear** before anything was wired to
it. The dump goes over the serial console rather than the Wi-Fi socket as
originally planned — the board could not reach a network, and the console
needs neither Wi-Fi nor the app. `test/capture_mic.py` collects it into a WAV.

Measured, three seconds each:

| | quiet room | speaking at the board |
|---|---|---|
| peak (DC removed) | 357 | **4536** |
| rms | 118 | **707** |
| distinct values | 508 | **4857** |

Useful beyond the gate: **the noise floor sits around rms 118 and speech around
700**, roughly a 6× separation. That is the number to hold against the VAD
threshold in phase 4 — and it is why `capture_mic.py` calls anything under 500
inconclusive rather than passing it.

This ordering was the whole mitigation for doing the wake word early: a silent
pipeline otherwise has two possible causes, and debugging a detector against a
mic that was never producing sound is a day nobody gets back.

Two things the dump path cost, both worth knowing before writing another
console-heavy debug tool:

- **`vTaskDelay(pdMS_TO_TICKS(2))` truncates to zero ticks** at a 100 Hz tick,
  so it yields nothing. Blocking in `uart_tx_char` without yielding starves the
  idle task, trips the task watchdog, and prints a backtrace into the middle of
  the stream it is complaining about. Use `vTaskDelay(1)`.
- **Only one process can read the port.** An open `idf.py monitor` swallows the
  dump and the script sees a port that reports data and returns none.

## Phase 4 — AFE and the wake word

I2S feeds AFE; the detector and, later, the encoder both read what AFE
produces, rather than the encoder reading raw I2S. **AFE's feed chunk is 512
samples — 32 ms at 16 kHz, one channel** (measured, phase 1a), which is exactly
one voice frame. One chunk in, one notification out, no regrouping anywhere.

On detection: `status` voice_started, and the panel shows `listening`. On
~800 ms of VAD silence or the 15 s cap: `status` voice_ended with a reason.

Gate: sit in a normal room for 30 minutes and count false triggers. An
always-listening mic that fires at the television is worse than a button.

## Phase 5 — encode and stream

`main/adpcm.c`, IMA encoder, state reset per frame. Notifications go out from a
task downstream of AFE fetch, never from a callback.

Gate: a pytest subscribes, a human speaks a sentence, the Python decoder from
phase 2 turns the captured frames into a WAV, and it is intelligible. Assert no
sequence gaps at ≥64 kbps in the same run.

## Phase 6 — app side: receive and transcribe

**Refactor `ble.rs` to one notification pump.** `watch_wifi_state` currently
opens its own `peripheral.notifications()` stream; three subscribers doing that
is three streams filtering the same traffic. One task, dispatching by
characteristic UUID.

`src-tauri/src/voice.rs` — decodes frames to PCM and accumulates an utterance.
A sequence gap inserts silence rather than failing: stale audio is worthless,
but so is throwing away a sentence over one lost packet.

`src-tauri/src/whisper.rs` — mirrors `llama.rs`: `ensure_running` spawns the
sidecar, the model stays resident, each utterance is a POST. Separate the
transcription call from the process lifecycle, the way `stream_completion` is
separated, so it is testable without the binary.

Rust then emits a `voice-session` event:

```
{ kind: "armed" | "listening" | "transcribing" | "transcript" | "ended" | "error",
  utteranceId, text?, message? }
```

`armed` on BLE connect, `ended` on disconnect. Rust owns the BLE connection
already, so the frontend needs one new listener rather than a second view of
connection state.

## Phase 7 — app side: sessions and what the user sees

Extract the turn-taking core out of `sendMessage` in `use-sessions.tsx` so the
composer and voice share one path, then:

- `armed` — remember that voice is live. No session row yet.
- `listening` — create the session, with a user message in a listening state.
  This is the first utterance, so this is where lazy creation fires. Provisional
  title until the transcript arrives.
- `transcribing` — the same bubble, transcribing.
- `transcript` — fill the bubble, retitle the session from it, and start the
  generation exactly as a typed message would. The existing AI titling then
  runs unchanged after the first exchange.
- `ended` — close the boundary; the next `listening` starts a new session.

Voice sessions appear in `nav-history` for free, because they are the same
`Session` objects. A small mic glyph distinguishes them.

The panel mirrors the same states through the existing `monocle.rs` path:
`listening`, then the transcript — so the wearer can see they were heard
correctly — then the reply.

An utterance arriving while a generation is streaming is refused, with a `busy`
notice on the panel. `sendMessage` already guards this way for typed input, and
two concurrent generations against one llama-server is not a thing to invent
here.

**This inherits the in-memory session hole.** History is still lost on restart,
and voice makes that considerably more annoying than typing did. Worth fixing;
not in this plan.

## Phase 8 — close the loop

Speak, and read the reply on the panel. Then the tests:

- Rust: the ADPCM decoder against known vectors, and the session state machine
  against a fake transcriber — the same sink-injection trick `monocle.rs` uses
  to stay testable without hardware.
- Frontend: vitest over `voice-session` events via `emitTauriEvent`, asserting
  a session appears on `listening` and not on `armed`.
- pytest: the wire, per phases 2 and 5.
- Manual, because nothing else can judge it: audio quality, false triggers per
  hour, and whether the latency from finishing a sentence to seeing a transcript
  feels acceptable.

## What to measure

Wake-to-first-frame latency. Utterance-end-to-transcript latency. Sustained
voice kbps and dropped frames. False triggers per hour. Free heap and PSRAM
with BLE, Wi-Fi, AFE and the panel all live.

## Risks

1. **esp-sr on ESP-IDF v6.** Front-loaded as phase 1a for exactly this reason.
2. **Always-on mic and neural net.** Tens of milliamps, continuously, on a
   head-worn device — the opposite of the duty cycling the Wi-Fi plane was
   designed around. Accepted to get the interaction right; measure it, and
   expect to gate listening on connection state later.
3. **Coexistence.** 1.5 Mbps was measured with BLE idle. Voice streaming will
   tighten it, and phase 1b says by how much before anything depends on it.
4. **Memory.** AFE, WakeNet, NimBLE, Wi-Fi and the display framebuffer at once.
   PSRAM is enabled and octal; phase 1a produces the number.
5. **Two models resident on the Mac.** whisper and llama-server both holding
   RAM, with whisper running at exactly the moment the user is waiting.

## Open questions

- Does voice need to stop while a still is transferring, or can they overlap?
  Phase 1b answers it.
- Should a wake word during generation cancel the generation instead of being
  refused? That needs the cancel path that `llama.rs` still lacks.
- Barge-in — speaking over a reply that is still paging on the panel.
- Whether the wearer should be able to reject a bad transcript, given there is
  no input on the device.
