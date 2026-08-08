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

**The usual cause is that something else is already connected.** `bleprph`
accepts one central and stops advertising while connected, so nRF Connect or
the minicole app holding the link makes the chip invisible to these tests.
Disconnect there first.

Otherwise check the board is powered and flashed, and that the advertised name
matches (`--device-name`, default `nimble-bleprph`).

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

`protocol.py` and `display.py` are independent implementations of the wire
formats. They are deliberately not shared with the firmware or the Rust app, so
that a drift on either side fails a test instead of being mirrored into it.

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
  longer than 21 characters, text past 8 lines, and an append that overflows
  the 512-byte buffer, where the *oldest* text should scroll off.
- **Coexistence throughput.** BLE and Wi-Fi sharing one antenna under sustained
  load is milestone 3/4 work; there is nothing to measure until voice or the
  socket exists.
