# Monocle protocol (v1 draft)

The contract between the monocle firmware (`firmware/`) and the desktop app
(`src-tauri/`). Supersedes the old `ble-protocol.md`, which covered only the
BLE half and assumed media flowed over it.

Status: **partly frozen.** The Wi-Fi plane is implemented and verified on
hardware — the three characteristic UUIDs, port 3333, and the socket framing
are settled; change them in all three implementations or not at all. The BLE
characteristics for control, voice, tokens, and status are still unassigned.
Update this file first when the protocol changes.

## Two planes

The monocle uses both radios, split by traffic shape rather than by role:

| | BLE (always on) | Wi-Fi (on demand) |
|---|---|---|
| Carries | control, status, tokens, **voice** | **JPEG stills** |
| Rate | ~64 kbps sustained | ~100 KB bursts at ~1.5 Mbps (measured) |
| Duty cycle | continuous while session active | seconds per capture, then radio down |
| App role | BLE central (`ble.rs`) | TCP client |
| Device role | BLE peripheral | TCP server |

**Why voice stays on BLE.** ADPCM voice is ~64 kbps — comfortably inside BLE's
~200 kbps real-world budget — but it runs continuously while the user speaks.
Carrying it over Wi-Fi would hold a ~100–200 mA radio up for the whole
utterance to move a trickle. BLE's radio is already up for control anyway.

**Why images go to Wi-Fi.** A JPEG still is 30–100 KB. Over BLE that is 1–3 s
per capture; over Wi-Fi it is ~600 ms, after which the radio powers back down.
Images are the only traffic that justifies the second radio, and they are
bursty enough that its duty cycle stays near ~1%.

### Measured throughput

Taken 2026-08-07 on the XIAO ESP32-S3 with BLE connected throughout, by
`test/test_data_plane.py` against the synthetic bulk endpoint:

| Transfer | Time | Rate |
|---|---|---|
| 16 KB | 118 ms | 1.11 Mbps |
| 100 KB | 529–613 ms | 1.34–1.55 Mbps |
| 512 KB | 2374 ms | 1.77 Mbps |

**This is an order of magnitude slower than the "tens of milliseconds" this
document claimed before anyone measured it.** The split still pays — ~600 ms
against BLE's 1–3 s — but the margin is 2–5×, not 30×, and that is the number
to design against.

The ceiling is **BLE/Wi-Fi coexistence**, not modem sleep. One 2.4 GHz antenna
is shared between the two radios, and the driver says so directly when power
save is disabled for a burst: `Coexist!!! Wi-Fi station would only keep waked
when available`. Disabling modem sleep during a transfer (which the firmware
does, per client) bought only ~20%.

Expect this to get *worse* once voice actually streams over BLE rather than
sitting idle — these numbers are the optimistic case.

**Video is explicitly out of scope.** The consumer of imagery is a multimodal
LLM that takes still frames. A stream would cost battery and firmware
complexity for something that gets downsampled to one frame anyway. If it is
ever wanted, it is additive on the same socket.

## Plane 1 — BLE control plane

The desktop app is the **central**; the monocle is the **peripheral**. Device
advertises as `minicole-monocle`, and the app filters scans by the service UUID
(`ScanFilter` in `ble.rs`) rather than listing every nearby device.

The **UUID rides in the advertisement and the name in the scan response**, not
the other way round: a 128-bit UUID costs 18 of the 31 available bytes so the
two do not both fit, and only the advertisement can be filtered on. An active
scan asks for the scan response anyway, so the name still reaches the device
list. The hardware test suite matches the same way — macOS caches a bonded
peripheral's name and would otherwise keep reporting a stale one.

One custom 128-bit service, `83486508-636c-4260-9119-c0ccc2004219`:

| Characteristic | UUID | Direction | Ops | Status |
|---|---|---|---|---|
| wifi_creds | `2c9b4a45-…-1f5f2e98db3c` | app → device | write (enc) | **implemented** |
| wifi_state | `1ad1e743-…-68b4d695ac8b` | device → app | notify | **implemented** |
| wifi_control | `e4782756-…-8c546a9134f1` | app → device | write (enc) | **implemented** |
| display | `e474939e-…-4f365b6fe723` | app → device | write (enc) | **implemented** |
| voice | `adea8e3b-…-cccf43c555af` | device → app | notify | **UUID frozen**, not built |
| status | `d4f52189-…-00cc0dd7dd57` | device → app | notify | **UUID frozen**, not built |
| control | *unassigned* | app → device | write | planned |

"UUID frozen, not built" means the identifier and payload below are settled and
present in all three implementations, while the firmware that produces the data
is milestone 3 work. They are written down first so that the app, the firmware
and the test suite cannot each invent their own.

Implemented UUIDs are frozen: they appear in the firmware's `gatt_svr.c` (as
`BLE_UUID128_INIT`, byte-reversed) and in `src-tauri/src/ble.rs`. Change them
in all three places or not at all.

**Adding a characteristic means bumping `MONOCLE_GATT_VERSION` in the
firmware's `bleprph.h`.** A bonded central caches the attribute table
indefinitely — on macOS a new characteristic is invisible until the user
forgets the device, which looks exactly like a bug in whatever was just added.
The firmware records the version it last announced to each bonded peer and
sends a Service Changed indication when they differ, which is what prompts
rediscovery.

`wifi_creds` / `wifi_state` replace BluFi — evaluated and dropped, see
[firmware-learning-notes.md](firmware-learning-notes.md) §4. Credentials are
written over an encrypted, bonded link (see Security below), never in the
clear.

### wifi_creds payload

```
[ssid_len: u8][ssid bytes][pass_len: u8][pass bytes]
```

SSID is 1–32 bytes, passphrase 0–63 (empty means an open network, which the
firmware maps to `WIFI_AUTH_OPEN`). Worst case is 97 bytes, so it always fits
a single ATT write — no reassembly on either side.

Lengths are **byte** counts, not character counts, and the firmware validates
every offset against the length actually received before indexing.

A successful ATT write means the credentials were **accepted**, not that the
network was joined — the firmware hands them to a worker task, because a join
may have to power the radio up and the BLE host task must not block. The
outcome always arrives on `wifi_state`.

### wifi_state payload

```
[state: u8][extra ...]

0 idle        no credentials yet
1 connecting  join in progress
2 connected   + 4 bytes, IPv4 in octet order (a.b.c.d)
3 failed      + 1 byte, raw 802.11 disconnect reason (0 = local failure)
```

Credentials are persisted to NVS only after a join succeeds, so a reboot
reconnects without the app. Malformed notifications are dropped by the app
rather than surfaced as a state.

Reason `0` is not a real 802.11 disconnect reason — those start at 1 — so it
is used for a join that failed on the device before reaching the air (the
radio would not start, or the request could not be queued). Anything else is
the chip's own reason code, passed through untouched.

### wifi_control payload

A single byte: `0` powers the data plane down, `1` brings it back up using
stored credentials. Encrypted for the same reason as the credentials — an
unauthenticated peer should not be able to flatten the battery by cycling the
radio.

The device also powers itself down on its own idle timer; this characteristic
is the way back up, and the way to end a burst early.

### display payload

Everything the monocle shows on its panel arrives here. Generated tokens are
one *use* of this characteristic, not its definition — status lines and errors
come the same way, which is why it is not called `tokens`.

```
[op: u8][utf-8 text ...]

0 clear   show nothing
1 set     replace the screen with this text
2 append  add to what is already there
```

`set` covers everything the app does today. `append` exists because streaming
tokens will arrive a few at a time, and it costs one byte now instead of a
protocol change later.

One write is one message, and the limit is **252 bytes of text** — the
firmware's buffer, not what the link can carry. An ATT write request holds
`MTU - 3` bytes of value, so the negotiated MTU of 512 leaves room for 509; the
252 dates from NimBLE's old default MTU of 256 and stayed put when the MTU was
raised (see Link parameters), which leaves headroom rather than a bug. Raising
it means changing `DISPLAY_TEXT_MAX` in the firmware's `display.h` and in
`ble.rs` together. The firmware rejects
anything longer rather than reassembling, and the app splits rather than
truncating, since a cut in the middle of a UTF-8 character loses exactly what
the wearer was meant to read. There is no acknowledgement beyond the ATT write
response — the panel is advisory, and a dropped line is not worth a
retransmission protocol.

Encrypted like the other writes: an unauthenticated peer should not be able to
put text in front of the wearer's eye.

**Wrapping is the firmware's job for now.** It wraps at its own character width
so that a long string degrades rather than truncating, and the app sends short
strings. Once `status` can report panel geometry, wrapping moves to the app —
see Future work.

**Character set: ASCII plus what French needs** — the accented letters in both
cases, `œ`/`Œ`, guillemets, curly quotes, dashes, `…`, `°`, `€`, and U+00A0,
which French typography puts before `! ? :` and `»`. The firmware decodes
UTF-8 properly, so a character is one cell however many bytes it occupies.
Anything outside that set draws as `?` — visibly wrong rather than silently
missing. Adding a language means adding glyphs to `EXTRA_GLYPHS` in
`display.c`, nothing more.

Note the size limit is in **bytes**, so accented text fits fewer characters per
write. The app splits on character boundaries, never mid-sequence.

### What starts a voice session

**The device decides, and tells the app.** Whatever wakes the monocle —
a button today, a wake word later — is internal to the firmware. What crosses
the wire is only the *event*: a notification that a voice session has started,
and another when it ends. Keeping the trigger out of the protocol is what makes
it swappable without touching the app.

The app's job on "started" is to open a new session, the same as if the user
had begun typing.

Decided: **a stock WakeNet wake word from the start** — "Hi ESP" or "Alexa";
the choice does not matter yet. No button stage. Speaking to a monocle is the
product; a button on a device worn on the face is not, and building the button
first would mean designing around an interaction nobody will ever use.

A custom word like "Jarvis" is a model Espressif trains commercially, so it is
a later swap. The wake word itself stays out of the protocol either way.

This brings work forward rather than adding it: ESP-SR as a managed component,
and a partition table with room for its models — today's
`partitions_singleapp_large.csv` has none. It also means the audio pipeline is
shaped by ESP-SR's front end from the beginning: I2S feeds the AFE, and both
the detector and the encoder read what the AFE produces, rather than the
encoder reading raw I2S.

Consequences accepted up front: the mic and a small neural net run
continuously, which is tens of milliamps on a head-worn device and the opposite
of the duty cycling the Wi-Fi plane was built around. An always-listening
microphone on someone's face is also a product decision, not only a technical
one.

The risk of doing it first is that a silent pipeline has two possible causes —
the detector or the capture path. Mitigation: bring capture up on its own and
confirm recorded audio is audible **before** wiring the detector to it, so the
two are never unproven at the same time.

### voice payload

```
[seq: u16 LE][predictor: i16 LE][step_index: u8][adpcm payload ...]
```

16 kHz mono source, IMA ADPCM at 4 bits per sample, ~64 kbps. The payload is
**256 bytes — 512 samples, 32 ms of audio** — making the frame 261 bytes on the
wire.

That size is chosen against the link rather than the codec. At the negotiated
MTU of 512 a notification carries 509 bytes, so a frame is under half of one;
at a 30 ms connection interval, one frame per interval keeps up with real time
with room to spare for a retry or for Wi-Fi stealing airtime. Both figures are
measured — see Link parameters.

**Every frame restates the decoder state it starts from, and the encoder
resets to match.** IMA ADPCM normally carries its predictor and step index from
one sample to the next, forever, which means a single lost notification does
not cost you 32 ms of audio — it turns everything after it into noise. Five
bytes per frame buys that back. The app tolerates gaps rather than asking for
retransmission, since stale audio is worthless, so frames have to be
individually decodable for that tolerance to mean anything.

`seq` increments per frame within one utterance and restarts at zero on the
next. A gap tells the app how much audio is missing; it inserts that much
silence rather than discarding the utterance, because losing a sentence to one
dropped packet is worse than a small hole in it.

### status payload

```
[event: u8][extra ...]

1 voice_started    no extra — the wake word fired, voice frames follow
2 voice_ended      + 1 byte reason: 0 vad, 1 capped, 2 error
3 panel_geometry   + 2 bytes: cols, rows
```

**The trigger stays off the wire.** What wakes the monocle — a wake word today,
something else later — is internal to the firmware; only the event crosses.
That is what lets the trigger change without the app knowing. See "What starts
a voice session".

`voice_ended` reasons distinguish a normal end (the VAD heard silence) from the
hard cap on utterance length and from a capture failure, because the app should
transcribe the first two and report the third.

`panel_geometry` is not voice work. It answers the long-standing question of
how the app learns the character grid instead of assuming it, and it costs one
event in a characteristic being added anyway. Nothing needs it while wrapping
and pagination both live in the firmware — it is here for whatever lays out
text once the panel is not a 128×64 stand-in.

### Link parameters

Measured against macOS, 2026-08-12, over 35 connections:

| | Asked for | Granted |
|---|---|---|
| MTU | 512 | **512** |
| PHY | 2M | **2M**, every connection |
| Connection interval | 15–30 ms | **30 ms**, every connection |

**MTU was 256 because of us, not macOS.** `CONFIG_BT_NIMBLE_ATT_PREFERRED_MTU`
defaults to 256 in NimBLE and the negotiated value is the lower of the two
ends. Raising it doubled the payload per notification to ~509 bytes.

**30 ms appears to be the floor with macOS as central.** Apple's accessory
rules require a minimum of at least 15 ms *and* a maximum at least 15 ms above
it, so the narrowest legal request containing 15 ms is 15–30 ms — and macOS
picks the top of the range. A request outside those rules is simply refused,
which is what the first attempt at 15–20 ms hit.

**Ask for one thing at a time.** The link layer runs a single control procedure
at a time. Requesting the PHY change and the interval change together on
connect made roughly half of each fail — connection updates with HCI 0x2A
("different transaction collision"), PHY updates with 0x23 — and made the
hardware test suite flaky, with errors landing on different tests each run. The
fix: set the preferred PHY once at init via
`ble_gap_set_prefered_default_le_phy` so no per-connection procedure runs, and
request the interval after encryption completes rather than at connect, so it
does not compete with the central's own setup.

Budget: ~200 kbps sustained. Voice uses roughly a third. At 30 ms, ADPCM needs
~240 bytes per interval, which is under half of one notification — so the
interval is unlikely to be the constraint. Confirmed by measurement in
milestone 3.

### Security

Bonding is required — the app must reconnect silently rather than re-pair each
session, and `wifi_creds` must never cross an unencrypted link. NimBLE config:
`sm_our_key_dist |= BLE_SM_PAIR_KEY_DIST_ENC` (distribute the LTK), with keys
persisted in NVS, which is why `main/CMakeLists.txt` needs `nvs_flash` in
`PRIV_REQUIRES`.

## Plane 2 — Wi-Fi data plane

### Topology: STA first

The monocle joins an existing network (station mode). The data-plane code is
identical under SoftAP, so this is not a lock-in: only Wi-Fi bring-up and IP
discovery differ. STA is chosen first because the Mac keeps its own internet
and everything lands on one LAN. SoftAP is deferred as a "no known network"
fallback — under it the Mac loses its own Wi-Fi, which is rough for both
development and users.

### Handoff sequence

```
1. app connects over BLE, bonds
2. app writes SSID + password  -> wifi_creds
3. firmware joins, gets DHCP lease
4. firmware notifies { state: "ready", ip, port } -> wifi_state
5. app opens TCP socket to ip:port
6. ... image bursts ...
7. idle timeout -> firmware closes socket, powers Wi-Fi down,
   notifies { state: "down" }
```

BLE stays connected throughout. No mDNS is needed: the device reports its own
lease over a channel that is already open. Step 7 is what makes the power model
work — **the socket is not held open between captures.**

### Transport: raw TCP, length-prefixed

The device listens on **port 3333**; the app connects. Raw TCP rather than
WebSocket — WebSocket exists for browser clients and hostile intermediaries,
and we own both ends. Its handshake and client-side masking are pure cost on
the ESP32.

The port is a fixed constant rather than negotiated: `wifi_state` already
delivers the address, and a negotiated port would add a failure mode for no
benefit.

Every message:

```
[len: u32 BE][type: u8][payload ...]
```

`len` counts the type byte plus the payload, so it is always >= 1. Payloads are
capped at 8192 bytes, which also bounds what a confused peer can make either
side allocate from a 5-byte header.

**Implemented frame types:**

| Type | Name | Direction | Payload |
|---|---|---|---|
| 1 | ECHO_REQ | app → device | arbitrary bytes |
| 2 | ECHO_RESP | device → app | the same bytes |
| 3 | BULK_REQ | app → device | `u32` byte count |
| 4 | BULK_DATA | device → app | synthetic chunk |
| 5 | BULK_END | device → app | `u32` bytes sent |

Echo proves the pipe. Bulk exists to measure throughput before the camera
lands — the receiver checks the BULK_END count against what actually arrived,
so a short transfer is an error rather than a fast-looking result.

**Planned**, once the camera exists — image capture as a sequence:

```
START  { total_size, width, height }
DATA   { seq, bytes }        (repeated)
END    { crc32 }
```

The app reassembles, verifies CRC32, and requests a retake on mismatch. Capture
is triggered over BLE `control`; only the bytes come back here.

### Power model

Wi-Fi is off until it is asked for, and goes down after an idle timeout —
currently 30 s with no client connected or no traffic on a connected one.
Teardown closes the listener, disconnects the station, calls `esp_wifi_stop()`,
and notifies `idle` over BLE. `wifi_control` brings it back.

Sustained-connection Wi-Fi would dominate the power budget and defeat the split
this protocol is built around. Firmware treats "Wi-Fi is up" as a short-lived
state, not a session property.

## Future work — from "Connected" to streaming tokens

The first use of `display` is a greeting the app writes on connect. Turning
that into the real output path needs the following, none of which changes the
payload format above:

- ~~Coalesce tokens in the app~~ — **done.** `src-tauri/src/monocle.rs` batches
  tokens on a 150 ms timer and sends one write per batch: `set` for the first
  (replacing a `...` thinking indicator), `append` thereafter. Writing per
  token would saturate the link and outrun the panel's ~25 ms redraw.
- ~~Decide what a long answer does~~ — **paginated, in the firmware.** The panel
  holds ~168 characters and a reply runs 500–2000, so `display.c` keeps the
  whole response and walks the reader through it a page at a time. The dwell is
  proportional to how much is on the page (2 s plus 40 ms per character, capped
  at 8 s) because two lines do not need the time eight do.

  Pagination has to live in the firmware: a page boundary is a line boundary,
  and only the renderer knows where lines break. Doing it app-side would mean a
  second copy of the wrapping algorithm that has to agree forever.

  **The reader cannot go back** — there is no input on the device — so the
  dwell errs long. A gesture or button to hold and step is the eventual answer.
  Constraining generation so answers are monocle-shaped in the first place is
  still worth doing, and is a system-prompt change rather than a rendering one.
- **Panel geometry over `status`**, so the app knows the character grid instead
  of assuming it. Less urgent now that wrapping *and* pagination are both
  firmware-side, but still wanted for anything that needs to lay out text —
  this 128×64 OLED is a stand-in for a micro-LED with different dimensions.
- ~~Say what is happening between question and answer~~ — **done.** The app
  puts `...` up when a generation starts, replaced by the first batch of
  tokens. Inference takes seconds, and a panel still showing the last answer is
  indistinguishable from a frozen one.
- **Errors belong here too** — no model loaded, generation failed, monocle
  disconnected. The panel is the only surface the wearer can see.

## Open questions

- Exact UUIDs — the four implemented ones are frozen above; `control`, `voice`
  and `status` still need generating, and committing here and in firmware
  constants at the same time.
- TCP port number, and whether the app should accept a device-chosen port from
  `wifi_state` (currently assumed yes) or pin a constant.
- ADPCM frame size vs. connection-event packing — tune in milestone 3.
- Wi-Fi idle timeout before teardown. Too short thrashes the radio on
  multi-shot bursts; too long wastes the savings.
- Status payload schema (battery, thermal, error codes).
- Whether `control` needs a response characteristic, or write-with-response is
  enough.
- Behaviour when Wi-Fi is unavailable (no known network, join failure). Fall
  back to slow JPEG-over-BLE, or surface the failure and refuse captures?
