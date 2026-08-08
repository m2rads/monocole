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

One custom 128-bit service, `83486508-636c-4260-9119-c0ccc2004219`:

| Characteristic | UUID | Direction | Ops | Status |
|---|---|---|---|---|
| wifi_creds | `2c9b4a45-…-1f5f2e98db3c` | app → device | write (enc) | **implemented** |
| wifi_state | `1ad1e743-…-68b4d695ac8b` | device → app | notify | **implemented** |
| wifi_control | `e4782756-…-8c546a9134f1` | app → device | write (enc) | **implemented** |
| control | *unassigned* | app → device | write | planned |
| voice | *unassigned* | device → app | notify | planned |
| tokens | *unassigned* | app → device | write | planned |
| status | *unassigned* | device → app | notify | planned |

Implemented UUIDs are frozen: they appear in the firmware's `gatt_svr.c` (as
`BLE_UUID128_INIT`, byte-reversed) and in `src-tauri/src/ble.rs`. Change them
in all three places or not at all.

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
