"""Wire format for the monocle's display characteristic.

Mirrors the firmware (``main/display.h``, ``main/gatt_svr.c``) and the app
(``src-tauri/src/ble.rs``). Kept deliberately independent of both, like
``protocol.py``, so a drift on either side fails a test rather than being
mirrored into it.

See ``docs/protocol.md`` for the authoritative description.
"""

from __future__ import annotations

DISPLAY_UUID = "e474939e-3010-4284-b280-4f365b6fe723"

# Ops the characteristic understands.
OP_CLEAR = 0
OP_SET = 1
OP_APPEND = 2

OP_NAMES = {OP_CLEAR: "clear", OP_SET: "set", OP_APPEND: "append"}

# An ATT write request carries MTU-3 bytes of value; at the 256 macOS
# negotiates that is 253, and the op byte is one of them.
ATT_VALUE_MAX = 253
TEXT_MAX = ATT_VALUE_MAX - 1


def encode(op: int, text: str = "") -> bytes:
    """Builds ``[op][utf-8 text]``.

    The limit counts *bytes*, not characters — a payload measured in
    characters would overrun a single ATT write as soon as the text stopped
    being ASCII.
    """
    if op not in OP_NAMES:
        raise ValueError(f"unknown op {op}")

    encoded = text.encode()
    if len(encoded) > TEXT_MAX:
        raise ValueError(f"text must be 0-{TEXT_MAX} bytes, got {len(encoded)}")

    return bytes([op]) + encoded


# Payloads the firmware must reject without falling over. Each targets a check
# in gatt_svr_chr_access_display(); an accepted write or a crash is a defect.
MALFORMED_PAYLOADS: list[tuple[str, bytes]] = [
    ("empty, not even an op byte", b""),
    ("op 3 is not defined", b"\x03hello"),
    ("op 255 is not defined", b"\xffhello"),
    ("an unknown op with no text", b"\x7f"),
]
