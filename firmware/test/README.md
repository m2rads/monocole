# Wi-Fi provisioning tests

Host-side tests for the monocle's BLE Wi-Fi provisioning service. They drive
the chip as a real BLE central (via `bleak`), so they exercise the actual GATT
layer rather than a mock.

Two tiers:

| Tier | Marker | Needs |
|---|---|---|
| Codec | *(none)* | nothing — always runs |
| Integration | `hardware` | board powered, flashed, in range |
| Join | `hardware` + `join` | plus real credentials via `--ssid` |

## Setup

```bash
python3 -m venv .venv
.venv/bin/pip install -r requirements.txt
```

## Running

```bash
.venv/bin/python -m pytest -m "not hardware"   # codec only, no board needed
.venv/bin/python -m pytest -m hardware         # against the chip
.venv/bin/python -m pytest --ssid "MyNet" --password "hunter2"   # everything
```

Credentials can come from the environment instead: `MONOCLE_SSID`,
`MONOCLE_PASSWORD`, `MONOCLE_NAME`.

## When the hardware tests all skip

Run the scanner:

```bash
.venv/bin/python scan.py
```

**The usual cause is that something else is already connected.** The firmware
accepts one central and stops advertising while connected, so nRF Connect or
the minicole app holding the link makes the chip invisible to these tests.
Disconnect there first.

Otherwise check the board is powered and flashed. The suite finds the board by
the service UUID it advertises, not by name — macOS caches a bonded
peripheral's name and keeps reporting a stale one, which would silently strand
the whole suite after a rename. `--device-name` is informational only.

## What is covered

- **GATT layout** — service and characteristic UUIDs, and that `wifi_creds` is
  write-only (readable credentials would leak the passphrase) and `wifi_state`
  is notify-only and unreadable.
- **Input validation** — every bounds check in `gatt_svr_chr_access_wifi()`
  has a matching malformed payload in `protocol.MALFORMED_PAYLOADS`: truncated
  fields, length bytes that overrun the buffer, and lengths above the 802.11
  maxima. A separate test replays all of them and then asserts the device is
  still serving GATT, which is how a crash shows up.
- **Full-size payload** — 97 bytes must land in a single ATT write.
- **Provisioning** — `connecting` is reported, ordering before `connected` is
  guaranteed, a real join yields a plausible address, a wrong password and an
  unknown network both end in `failed` *with a reason* rather than silence or
  an endless retry loop, and re-provisioning supersedes an in-flight attempt.
- **Notification hygiene** — the recorder decodes every frame as it arrives, so
  a malformed notification fails the test at the point it is emitted.

- **Display** — the characteristic is write-only and unreadable, full-size
  writes land in one ATT transaction, every undefined op is rejected, and the
  device still serves GATT after being fed all of them. A visible display
  characteristic also proves the Service Changed mechanism works: a bonded
  central with a stale cache would not see it at all.

- **Voice and status** — both characteristics are present and notify-only, and
  status reports the panel geometry as soon as anything subscribes. There is no
  mic pipeline yet, so the voice test asserts that *nothing* arrives: a device
  streaming junk from an uninitialised buffer fails there. It is written so it
  keeps working once frames do arrive.
- **The ADPCM codec** — round-trip SNR, and the property the 5-byte frame
  header exists for: a frame decodes identically alone and in sequence, so a
  dropped notification costs only its own 32 ms. One test specifically catches
  an encoder that resets its state per frame, which is decodable but buzzes at
  the frame rate.

`protocol.py`, `display.py` and `adpcm.py` are independent implementations of
the wire formats. They are deliberately not shared with the firmware or the
Rust app, so that a drift on either side fails a test instead of being mirrored
into it. That matters most for the codec, where a subtle disagreement produces
audio that is recognisable but wrong.

## Not covered here

- **The encryption gate.** `wifi_creds` is `BLE_GATT_CHR_F_WRITE_ENC`, but
  CoreBluetooth pairs transparently on the first refused write, so a host-side
  test cannot observe the refusal. Verify manually: with the bond removed, the
  serial log must show `encryption change event; status=0` *before*
  `credentials received`. Also confirm the log never prints the passphrase.
- **NVS persistence across reboot.** Needs a power cycle. Verify manually: with
  nothing connected, the log should show `using stored credentials for SSID …`
  at boot.
- **What is actually on the panel.** The display characteristic is write-only
  with no read-back, so nothing here can tell legible text from a blank
  screen. Look at it after any change to the renderer — particularly a word
  longer than 21 characters, and text past 8 lines.
- **Pagination.** Same reason, and the timing only means something to a person
  reading it. Worth eyeballing: a two-page answer (does the second page arrive
  before you finish the first?), a new response arriving while an old one is
  still paging (the reader should jump back to the top), and an answer past
  4 KB, where the front of the buffer drops and the reader's place should shift
  with it rather than jumping.
- **Accented glyphs.** Ask the model something in French and read it. The
  lowercase accents have two clear rows to sit in; the capitals are shifted
  down with a one-pixel mark, so `É` and `È` differ only by which columns it
  covers — check they are still tellable apart. `œ` and `Œ` are a squeeze at
  5×7 and are the most likely to look wrong.
- **Coexistence throughput.** BLE and Wi-Fi sharing one antenna under sustained
  load is milestone 3/4 work; there is nothing to measure until voice or the
  socket exists.
