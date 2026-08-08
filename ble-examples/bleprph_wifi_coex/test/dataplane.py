"""Wi-Fi data plane client.

Independent implementation of the framing in the firmware's ``tcp_server.c``
and the app's ``src-tauri/src/socket.rs``, for the same reason as protocol.py:
a third implementation catches drift instead of mirroring it.
"""

from __future__ import annotations

import socket
import struct
import time
from dataclasses import dataclass

TCP_PORT = 3333
HEADER_LEN = 5
MAX_PAYLOAD = 8192

FRAME_ECHO_REQ = 1
FRAME_ECHO_RESP = 2
FRAME_BULK_REQ = 3
FRAME_BULK_DATA = 4
FRAME_BULK_END = 5


def encode_frame(frame_type: int, payload: bytes = b"") -> bytes:
    """``[len: u32 BE][type: u8][payload]``, where len covers the type byte."""
    return struct.pack(">IB", len(payload) + 1, frame_type) + payload


@dataclass
class Transfer:
    total_bytes: int
    elapsed_s: float
    frames: int

    @property
    def kbps(self) -> float:
        return (self.total_bytes * 8) / self.elapsed_s / 1000 if self.elapsed_s else 0.0

    @property
    def mbps(self) -> float:
        return self.kbps / 1000


class DataPlane:
    """Blocking client. Use as a context manager."""

    def __init__(self, ip: str, port: int = TCP_PORT, timeout: float = 15.0):
        self.address = (ip, port)
        self.timeout = timeout
        self._sock: socket.socket | None = None

    def __enter__(self) -> DataPlane:
        self._sock = socket.create_connection(self.address, timeout=self.timeout)
        self._sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        return self

    def __exit__(self, *exc) -> None:
        if self._sock:
            self._sock.close()
            self._sock = None

    def _read_exact(self, n: int) -> bytes:
        assert self._sock is not None, "not connected"
        chunks = []
        remaining = n
        while remaining:
            chunk = self._sock.recv(remaining)
            if not chunk:
                raise ConnectionError(f"peer closed with {remaining} of {n} bytes left")
            chunks.append(chunk)
            remaining -= len(chunk)
        return b"".join(chunks)

    def _send(self, frame_type: int, payload: bytes = b"") -> None:
        assert self._sock is not None, "not connected"
        self._sock.sendall(encode_frame(frame_type, payload))

    def _recv(self) -> tuple[int, bytes]:
        length, frame_type = struct.unpack(">IB", self._read_exact(HEADER_LEN))
        if length == 0:
            raise ValueError("frame length must include the type byte")
        payload_len = length - 1
        if payload_len > MAX_PAYLOAD:
            raise ValueError(f"payload {payload_len} exceeds maximum {MAX_PAYLOAD}")
        return frame_type, self._read_exact(payload_len)

    def echo(self, payload: bytes) -> bytes:
        self._send(FRAME_ECHO_REQ, payload)
        frame_type, echoed = self._recv()
        if frame_type != FRAME_ECHO_RESP:
            raise AssertionError(f"expected echo reply, got frame type {frame_type}")
        return echoed

    def bulk(self, total: int) -> Transfer:
        """Requests `total` synthetic bytes and times the transfer."""
        self._send(FRAME_BULK_REQ, struct.pack(">I", total))

        started = time.perf_counter()
        received = 0
        frames = 0

        while True:
            frame_type, payload = self._recv()
            if frame_type == FRAME_BULK_DATA:
                received += len(payload)
                frames += 1
            elif frame_type == FRAME_BULK_END:
                claimed = struct.unpack(">I", payload)[0]
                if claimed != received:
                    raise AssertionError(
                        f"device claims {claimed} bytes sent, {received} arrived"
                    )
                break
            else:
                raise AssertionError(f"unexpected frame type {frame_type}")

        return Transfer(received, time.perf_counter() - started, frames)
