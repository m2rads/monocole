//! Turning voice notifications back into audio.
//!
//! The monocle streams IMA ADPCM in 261-byte frames — see docs/protocol.md.
//! This module decodes them, reassembles an utterance, and hands out WAV bytes
//! for the transcriber.
//!
//! Decoding is deliberately separated from BLE and from whisper, in the same
//! spirit as `llama::stream_completion` and `monocle::run`: the interesting
//! logic is then testable without hardware or a sidecar.

/// Bytes of header before the payload: seq, predictor, step index.
const HEADER_LEN: usize = 5;

/// Samples one frame carries — 512, which is 32 ms at 16 kHz.
pub const FRAME_SAMPLES: usize = 512;

/// The source sample rate. Whisper resamples internally; this is what the
/// WAV header has to declare so the audio plays at the right speed.
pub const SAMPLE_RATE: u32 = 16_000;

/// A frame that failed to decode. Kept as a type rather than a bare string so
/// callers can tell "not for us" from "the firmware is misbehaving".
#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    TooShort(usize),
    BadStepIndex(u8),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::TooShort(n) => {
                write!(f, "voice frame is {n} bytes, shorter than its header")
            }
            FrameError::BadStepIndex(i) => {
                write!(f, "voice frame has step index {i}, outside 0..=88")
            }
        }
    }
}

/// Standard IMA tables.
const INDEX_TABLE: [i8; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

#[rustfmt::skip]
const STEP_TABLE: [i16; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37,
    41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173,
    190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658,
    724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066,
    2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894,
    6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289,
    16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

const MAX_STEP_INDEX: u8 = (STEP_TABLE.len() - 1) as u8;

/// One decoded frame.
#[derive(Debug, PartialEq, Eq)]
pub struct Frame {
    pub seq: u16,
    pub samples: Vec<i16>,
}

/// Decodes one notification.
///
/// Needs nothing but the frame itself: the predictor and step index in the
/// header are exactly what make that true, so a frame lost on the air costs
/// only the audio it carried rather than corrupting everything after it.
pub fn decode_frame(data: &[u8]) -> Result<Frame, FrameError> {
    if data.len() < HEADER_LEN {
        return Err(FrameError::TooShort(data.len()));
    }

    let seq = u16::from_le_bytes([data[0], data[1]]);
    let mut predictor = i16::from_le_bytes([data[2], data[3]]) as i32;
    let mut step_index = data[4];

    if step_index > MAX_STEP_INDEX {
        return Err(FrameError::BadStepIndex(step_index));
    }

    let mut samples = Vec::with_capacity((data.len() - HEADER_LEN) * 2);
    for byte in &data[HEADER_LEN..] {
        // Low nibble first, which is what IMA specifies.
        for nibble in [byte & 0x0f, byte >> 4] {
            let step = STEP_TABLE[step_index as usize] as i32;

            let mut diff = step >> 3;
            if nibble & 4 != 0 {
                diff += step;
            }
            if nibble & 2 != 0 {
                diff += step >> 1;
            }
            if nibble & 1 != 0 {
                diff += step >> 2;
            }
            if nibble & 8 != 0 {
                diff = -diff;
            }

            predictor = (predictor + diff).clamp(i16::MIN as i32, i16::MAX as i32);
            step_index = step_index
                .saturating_add_signed(INDEX_TABLE[nibble as usize])
                .min(MAX_STEP_INDEX);

            samples.push(predictor as i16);
        }
    }

    Ok(Frame { seq, samples })
}

/// Collects frames into one utterance.
#[derive(Default)]
pub struct Utterance {
    samples: Vec<i16>,
    next_seq: Option<u16>,
    dropped: usize,
}

impl Utterance {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a frame, filling anything missing before it with silence.
    ///
    /// A gap is a hole rather than a discard: losing a whole sentence to one
    /// dropped notification would be worse than a moment of quiet in it, and
    /// padding keeps the rest of the audio at the right offset instead of
    /// shifting it earlier.
    pub fn push(&mut self, frame: Frame) {
        if let Some(expected) = self.next_seq {
            let missing = frame.seq.wrapping_sub(expected);
            // A large value here is a frame arriving out of order or a
            // restarted utterance, not a gap worth padding for.
            if (missing as usize) < 64 {
                self.dropped += missing as usize;
                self.samples
                    .extend(std::iter::repeat_n(0, missing as usize * FRAME_SAMPLES));
            }
        }

        self.next_seq = Some(frame.seq.wrapping_add(1));
        self.samples.extend_from_slice(&frame.samples);
    }

    /// How many frames never arrived.
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Length in seconds, for deciding whether there is anything worth
    /// transcribing.
    pub fn duration_secs(&self) -> f32 {
        self.samples.len() as f32 / SAMPLE_RATE as f32
    }

    /// Encodes the utterance as a 16-bit mono WAV.
    ///
    /// A container rather than raw PCM because that is what whisper.cpp's
    /// server accepts, and because a file with a header is one someone can
    /// double-click when a transcript comes back wrong.
    pub fn to_wav(&self) -> Vec<u8> {
        let data_len = (self.samples.len() * 2) as u32;
        let mut wav = Vec::with_capacity(44 + data_len as usize);

        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // PCM chunk size
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM, uncompressed
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
        wav.extend_from_slice(&2u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());

        for sample in &self.samples {
            wav.extend_from_slice(&sample.to_le_bytes());
        }

        wav
    }
}

#[cfg(test)]
#[path = "../tests/voice_test.rs"]
mod tests;
