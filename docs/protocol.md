# Monocle protocol (v1 draft)

The contract between the monocle firmware (`firmware/`) and the desktop app
(`src-tauri/`). Supersedes the old `ble-protocol.md`, which covered only the
BLE half and assumed media flowed over it.

Status: **draft — UUIDs, ports, and framing not yet frozen.** Update this file
first when the protocol changes.

## Two planes

The monocle uses both radios, split by traffic shape rather than by role:

| | BLE (always on) | Wi-Fi (on demand) |
|---|---|---|
| Carries | control, status, tokens, **voice** | **JPEG stills** |
| Rate | ~64 kbps sustained | ~100 KB bursts |
| Duty cycle | continuous while session active | seconds per capture, then radio down |
| App role | BLE central (`ble.rs`) | TCP client |
| Device role | BLE peripheral | TCP server |

**Why voice stays on BLE.** ADPCM voice is ~64 kbps — comfortably inside BLE's
~200 kbps real-world budget — but it runs continuously while the user speaks.
Carrying it over Wi-Fi would hold a ~100–200 mA radio up for the whole
utterance to move a trickle. BLE's radio is already up for control anyway.

**Why images go to Wi-Fi.** A JPEG still is 30–100 KB. Over BLE that is 1–3 s
per capture; over Wi-Fi it is tens of milliseconds, after which the radio
powers back down. Images are the only traffic that justifies the second radio,
and they are bursty enough that its duty cycle stays near ~1%.

**Video is explicitly out of scope.** The consumer of imagery is a multimodal
LLM that takes still frames. A stream would cost battery and firmware
complexity for something that gets downsampled to one frame anyway. If it is
ever wanted, it is additive on the same socket.

## Plane 1 — BLE control plane

The desktop app is the **central**; the monocle is the **peripheral**. Device
advertises as `minicole-monocle`. Once the service UUID is frozen, the app
should filter scans by it (`ScanFilter` in `ble.rs`) instead of listing every
nearby device.

One custom 128-bit service:

| Characteristic | Direction | Ops | Purpose |
|---|---|---|---|
| control | app → device | write | start/stop mic, capture photo, settings |
| voice | device → app | notify | ADPCM audio frames |
| tokens | app → device | write | generated text for the LED display |
| status | device → app | notify | battery, state, errors |
| wifi_creds | app → device | write | SSID + password for the data plane |
| wifi_state | device → app | notify | join progress, assigned IP, port |

`wifi_creds` / `wifi_state` replace BluFi — evaluated and dropped, see
[firmware-learning-notes.md](firmware-learning-notes.md) §4. Credentials are
written over an encrypted, bonded link (see Security below), never in the
clear.

### Voice framing

`[seq: u16][adpcm payload]` — 16 kHz mono source, IMA ADPCM (4-bit/sample,
~64 kbps). The sequence number detects drops; the app tolerates gaps rather
than requesting retransmission, since stale audio is worthless.

Exact frame size is tuned against the connection interval in milestone 3.

### Link parameters

- MTU: negotiate 517 (macOS typically grants 512-byte payloads).
- PHY: request 2M.
- Connection interval: short (~15 ms) while streaming voice.
- Budget: ~200 kbps sustained. Voice uses roughly a third of it.

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

Raw TCP rather than WebSocket. WebSocket buys nothing here — it exists for
browser clients and hostile intermediaries, and we own both ends. Its handshake
and client-side masking are pure cost on the ESP32.

Every message:

```
[len: u32 BE][type: u8][payload ...]
```

Image capture, as a message sequence:

```
START  { total_size: u32, width: u16, height: u16 }
DATA   { seq: u16, bytes }        (repeated)
END    { crc32: u32 }
```

The app reassembles, verifies CRC32, and requests a retake on mismatch. The
capture is triggered over BLE `control`; only the bytes come back here.

### Power model

Wi-Fi is off until a capture is requested and goes down after an idle timeout.
Sustained-connection Wi-Fi would dominate the power budget and defeat the split
this protocol is built around. Firmware must treat "Wi-Fi is up" as a
short-lived state, not a session property.

## Open questions

- Exact UUIDs — generate once, commit here and in firmware constants.
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
