"""Codec tests for the voice framing. No hardware needed."""

from __future__ import annotations

import math
import struct

import pytest

from adpcm import (
    FRAME_LEN,
    HEADER_LEN,
    MAX_STEP_INDEX,
    PAYLOAD_LEN,
    SAMPLES_PER_FRAME,
    Frame,
    decode_frame,
    encode_frame,
    encode_utterance,
    frames_to_wav,
)


def sine(samples: int, hz: float = 440.0, rate: int = 16000,
         amplitude: int = 12000) -> list[int]:
    return [
        int(amplitude * math.sin(2 * math.pi * hz * i / rate))
        for i in range(samples)
    ]


def snr_db(original: list[int], decoded: list[int]) -> float:
    signal = sum(float(s) ** 2 for s in original)
    noise = sum((float(a) - float(b)) ** 2 for a, b in zip(original, decoded))
    if noise == 0:
        return math.inf
    return 10 * math.log10(signal / noise)


class TestFraming:
    def test_frame_is_one_notification(self):
        # 261 bytes has to fit an ATT notification at the negotiated MTU of
        # 512, which carries 509. Well under half.
        assert FRAME_LEN == 261
        assert FRAME_LEN < 509

    def test_payload_is_32ms_of_audio(self):
        assert SAMPLES_PER_FRAME == 512
        assert SAMPLES_PER_FRAME / 16000 == pytest.approx(0.032)

    def test_header_carries_seq_and_decoder_state(self):
        frame = encode_frame(7, sine(SAMPLES_PER_FRAME), predictor=-1234,
                             step_index=11)
        seq, predictor, step_index = struct.unpack_from("<HhB", frame)
        assert (seq, predictor, step_index) == (7, -1234, 11)
        assert len(frame) == HEADER_LEN + PAYLOAD_LEN

    def test_short_frame_is_refused(self):
        with pytest.raises(ValueError):
            decode_frame(b"\x00\x00")

    def test_impossible_step_index_is_refused(self):
        # Guards against decoding garbage off the air by indexing past the
        # step table, which in a less careful decoder is a crash.
        bad = struct.pack("<HhB", 0, 0, MAX_STEP_INDEX + 1) + b"\x00" * PAYLOAD_LEN
        with pytest.raises(ValueError):
            decode_frame(bad)


class TestRoundTrip:
    # IMA at 4 bits/sample manages ~20 dB SNR on a tone. Well below that means
    # the tables or the nibble order are wrong, which would still sound like
    # something — just not like the speaker.
    MIN_SNR_DB = 20

    # The predictor starts at zero and the step table starts at 7, so the very
    # beginning of an utterance takes a moment to catch up. That is inherent to
    # IMA and only ever happens once, at the start of speech.
    CONVERGENCE = 64

    def test_decoded_audio_resembles_the_original(self):
        original = sine(SAMPLES_PER_FRAME * 4)
        decoded: list[int] = []
        for frame in encode_utterance(original):
            decoded.extend(decode_frame(frame).samples)

        assert len(decoded) == len(original)
        assert snr_db(original[self.CONVERGENCE:],
                      decoded[self.CONVERGENCE:]) > self.MIN_SNR_DB

    def test_every_frame_after_the_first_is_clean(self):
        """No per-frame convergence artefact.

        This is the test that fails if the encoder resets its state per frame:
        each frame then restarts from zero and the first samples of all of them
        are wrong, which reads as a buzz at the frame rate.
        """
        original = sine(SAMPLES_PER_FRAME * 4)
        frames = encode_utterance(original)

        for seq, frame in enumerate(frames[1:], start=1):
            chunk = original[seq * SAMPLES_PER_FRAME:(seq + 1) * SAMPLES_PER_FRAME]
            decoded = decode_frame(frame).samples
            # The opening samples specifically, not the frame as a whole — an
            # average over 512 samples would hide a bad first twenty.
            assert snr_db(chunk[:32], decoded[:32]) > self.MIN_SNR_DB, (
                f"frame {seq} starts badly; encoder state is not continuous"
            )

    def test_silence_stays_silent(self):
        decoded = decode_frame(encode_frame(0, [0] * SAMPLES_PER_FRAME)).samples
        assert max(abs(s) for s in decoded) < 16


class TestFramesAreSelfContained:
    """The property the extra five bytes per frame buy."""

    def test_a_frame_decodes_the_same_in_isolation_as_in_sequence(self):
        frames = encode_utterance(sine(SAMPLES_PER_FRAME * 3))

        # Decoding only the third frame, having never seen the first two, must
        # give exactly what decoding all three in order gives. Without the
        # per-frame state in the header this is precisely what breaks.
        in_sequence = [decode_frame(f).samples for f in frames]
        assert decode_frame(frames[2]).samples == in_sequence[2]

    def test_a_dropped_frame_does_not_corrupt_later_ones(self):
        original = sine(SAMPLES_PER_FRAME * 3)
        frames = encode_utterance(original)

        # Frame 1 never arrives; frame 2 must still be good audio.
        survived = decode_frame(frames[2])
        assert survived.seq == 2
        chunk = original[2 * SAMPLES_PER_FRAME:3 * SAMPLES_PER_FRAME]
        assert snr_db(chunk, survived.samples) > 20


class TestWavAssembly:
    def test_gaps_become_silence_rather_than_shifting_audio(self, tmp_path):
        import wave

        # Frames 0 and 2 arrive; 1 is lost. The result must be three frames
        # long, with the survivors still at their original offsets — a
        # recording that shifts earlier is worse than one with a hole.
        frames = [
            Frame(seq=0, predictor=0, step_index=0, samples=[100] * SAMPLES_PER_FRAME),
            Frame(seq=2, predictor=0, step_index=0, samples=[200] * SAMPLES_PER_FRAME),
        ]
        path = tmp_path / "gap.wav"
        frames_to_wav(str(path), frames)

        with wave.open(str(path)) as f:
            assert f.getnchannels() == 1
            assert f.getframerate() == 16000
            assert f.getnframes() == SAMPLES_PER_FRAME * 3
            pcm = struct.unpack(f"<{f.getnframes()}h", f.readframes(f.getnframes()))

        assert pcm[0] == 100
        assert pcm[SAMPLES_PER_FRAME] == 0            # the missing frame
        assert pcm[SAMPLES_PER_FRAME * 2] == 200      # back in the right place
