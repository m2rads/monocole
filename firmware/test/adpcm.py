"""IMA ADPCM, and the framing the voice characteristic carries it in.

An independent implementation, like ``protocol.py`` and ``display.py``: it is
deliberately not shared with the firmware, so that a drift on either side fails
a test instead of being mirrored into it. That matters more here than
elsewhere, because a codec that disagrees subtly produces audio that is
recognisable but wrong, which is exactly the kind of bug a shared
implementation would hide.

See ``docs/protocol.md`` for the authoritative description.
"""

from __future__ import annotations

import struct
import wave
from dataclasses import dataclass

# The wire format: [seq u16][predictor i16][step_index u8][adpcm payload].
HEADER = "<HhB"
HEADER_LEN = struct.calcsize(HEADER)

# 256 payload bytes at 4 bits per sample is 512 samples, which at 16 kHz is
# 32 ms — one AFE feed chunk, and one BLE notification.
PAYLOAD_LEN = 256
SAMPLES_PER_FRAME = PAYLOAD_LEN * 2
FRAME_LEN = HEADER_LEN + PAYLOAD_LEN

SAMPLE_RATE = 16000

# Standard IMA tables. The step index walks STEP_TABLE; INDEX_TABLE says how
# far each nibble moves it, which is how the codec adapts to loud and quiet
# passages.
INDEX_TABLE = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8]

STEP_TABLE = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37,
    41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173,
    190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658,
    724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066,
    2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894,
    6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289,
    16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
]

MAX_STEP_INDEX = len(STEP_TABLE) - 1


def _clamp_sample(value: int) -> int:
    return max(-32768, min(32767, value))


def _clamp_index(value: int) -> int:
    return max(0, min(MAX_STEP_INDEX, value))


def decode_nibble(nibble: int, predictor: int, index: int) -> tuple[int, int]:
    """Decodes one 4-bit code, returning (predictor, step_index).

    The new predictor *is* the decoded sample — IMA reconstructs by
    accumulating differences, so there is no separate output value.
    """
    step = STEP_TABLE[index]

    diff = step >> 3
    if nibble & 4:
        diff += step
    if nibble & 2:
        diff += step >> 1
    if nibble & 1:
        diff += step >> 2
    if nibble & 8:
        diff = -diff

    predictor = _clamp_sample(predictor + diff)
    index = _clamp_index(index + INDEX_TABLE[nibble])
    return predictor, index


def encode_sample(sample: int, predictor: int, index: int) -> tuple[int, int, int]:
    """Encodes one PCM sample, returning (nibble, predictor, step_index).

    Present so tests can round-trip without the firmware. The device does the
    real encoding; this only has to agree with it.
    """
    step = STEP_TABLE[index]
    diff = sample - predictor

    nibble = 0
    if diff < 0:
        nibble = 8
        diff = -diff

    if diff >= step:
        nibble |= 4
        diff -= step
    step >>= 1
    if diff >= step:
        nibble |= 2
        diff -= step
    step >>= 1
    if diff >= step:
        nibble |= 1

    # The encoder must track the decoder's reconstruction, not the true
    # sample, or the two drift apart within a few dozen samples.
    predictor, index = decode_nibble(nibble, predictor, index)
    return nibble, predictor, index


@dataclass
class Frame:
    """One decoded voice notification."""

    seq: int
    predictor: int
    step_index: int
    samples: list[int]


def decode_frame(data: bytes) -> Frame:
    """Decodes one notification into 16-bit PCM.

    Needs nothing but the frame itself — that is the whole point of the
    predictor and step index being in the header. A frame lost on the air
    costs exactly the audio it carried, rather than corrupting everything
    that follows it.
    """
    if len(data) < HEADER_LEN:
        raise ValueError(f"frame is {len(data)} bytes, shorter than its header")

    seq, predictor, step_index = struct.unpack_from(HEADER, data)
    if not 0 <= step_index <= MAX_STEP_INDEX:
        raise ValueError(f"step index {step_index} is outside 0-{MAX_STEP_INDEX}")

    payload = data[HEADER_LEN:]
    samples: list[int] = []
    for byte in payload:
        # Low nibble first, which is what IMA specifies and what every other
        # decoder will assume when someone opens the WAV.
        for nibble in (byte & 0x0F, byte >> 4):
            predictor, step_index = decode_nibble(nibble, predictor, step_index)
            samples.append(predictor)

    return Frame(seq=seq, predictor=predictor, step_index=step_index, samples=samples)


def encode_frame_chained(seq: int, samples: list[int], predictor: int,
                         step_index: int) -> tuple[bytes, int, int]:
    """Encodes one frame and returns it with the state it ended on.

    The header snapshots the state the frame *starts* from. That is what makes
    a frame self-contained without costing anything: the encoder keeps running
    continuously, so the predictor never has to slew back up to the signal, and
    a decoder that missed everything before this frame still starts from
    exactly the right place.

    Resetting the encoder per frame would also be decodable, and is what an
    earlier draft of docs/protocol.md specified — but the predictor then climbs
    from zero at the start of every frame, and the step table starts at 7, so
    the first dozen-odd samples of all 31 frames per second are wrong. That is
    a buzz, not a subtlety.
    """
    if len(samples) % 2 != 0:
        raise ValueError("a frame holds two samples per byte, so it needs an even count")

    header = struct.pack(HEADER, seq, predictor, step_index)

    payload = bytearray()
    for i in range(0, len(samples), 2):
        low, predictor, step_index = encode_sample(samples[i], predictor, step_index)
        high, predictor, step_index = encode_sample(samples[i + 1], predictor, step_index)
        payload.append((high << 4) | low)

    return header + bytes(payload), predictor, step_index


def encode_frame(seq: int, samples: list[int], predictor: int = 0,
                 step_index: int = 0) -> bytes:
    """Encodes one frame from a given starting state."""
    frame, _, _ = encode_frame_chained(seq, samples, predictor, step_index)
    return frame


def encode_utterance(samples: list[int]) -> list[bytes]:
    """Encodes PCM as the device would: continuous state, one frame per chunk.

    The last chunk is padded with silence, because a frame is a fixed number
    of samples on the wire.
    """
    frames: list[bytes] = []
    predictor, step_index = 0, 0

    for seq, start in enumerate(range(0, len(samples), SAMPLES_PER_FRAME)):
        chunk = samples[start:start + SAMPLES_PER_FRAME]
        chunk = chunk + [0] * (SAMPLES_PER_FRAME - len(chunk))
        frame, predictor, step_index = encode_frame_chained(
            seq, chunk, predictor, step_index
        )
        frames.append(frame)

    return frames


def frames_to_wav(path: str, frames: list[Frame]) -> None:
    """Writes decoded frames to a WAV, inserting silence for missing ones.

    Gaps are filled rather than skipped so the recording stays in time with
    what was said: a dropped notification should sound like a hole, not shift
    everything after it earlier.
    """
    pcm = bytearray()
    expected = frames[0].seq if frames else 0

    for frame in frames:
        missing = frame.seq - expected
        if missing > 0:
            pcm.extend(b"\x00\x00" * SAMPLES_PER_FRAME * missing)
        pcm.extend(struct.pack(f"<{len(frame.samples)}h", *frame.samples))
        expected = frame.seq + 1

    with wave.open(path, "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(SAMPLE_RATE)
        out.writeframes(bytes(pcm))
