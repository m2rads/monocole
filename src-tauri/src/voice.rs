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

    /// Loudness, with any DC offset removed.
    ///
    /// Worth having because whisper does not fail on audio without speech in
    /// it — it *invents* text, confidently, usually song lyrics. So "did the
    /// microphone actually hear a voice" is a question that has to be
    /// answered before transcription, not after. Measured on this hardware:
    /// a quiet room is ~120, room tone with movement ~330, someone speaking
    /// at the board ~700.
    pub fn rms(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let mean = self.samples.iter().map(|s| *s as f64).sum::<f64>()
            / self.samples.len() as f64;
        let sum_squares: f64 = self
            .samples
            .iter()
            .map(|s| {
                let centred = *s as f64 - mean;
                centred * centred
            })
            .sum();
        (sum_squares / self.samples.len() as f64).sqrt() as f32
    }

    /// Encodes the utterance as a 16-bit mono WAV.
    pub fn to_wav(&self) -> Vec<u8> {
        wav_from_pcm(&self.samples, SAMPLE_RATE)
    }
}

/// Wraps 16-bit mono PCM in a WAV container.
///
/// A container rather than raw PCM because that is what whisper.cpp's server
/// accepts, and because a file with a header is one someone can double-click
/// when a transcript comes back wrong. Shared with the app's own recorder, so
/// both routes to whisper hand it byte-identical input.
pub fn wav_from_pcm(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);

    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // PCM chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM, uncompressed
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());

    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }

    wav
}

// ---------------------------------------------------------------------------
// Session: status events and frames in, transcripts out
// ---------------------------------------------------------------------------

/// The event name the frontend listens on.
pub const VOICE_EVENT: &str = "voice-session";

/// status events, mirroring `enum monocle_status_event` in the firmware's
/// bleprph.h and docs/protocol.md.
const STATUS_VOICE_STARTED: u8 = 1;
const STATUS_VOICE_ENDED: u8 = 2;
const STATUS_PANEL_GEOMETRY: u8 = 3;

/// Utterances shorter than this are not sent for transcription.
///
/// A wake word followed immediately by silence is someone testing the device,
/// or a false trigger. Transcribing it wastes a second and, worse, files a
/// session containing whatever whisper hallucinates from noise.
const MIN_UTTERANCE_SECS: f32 = 0.4;

/// What the app tells the frontend about a voice session.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceEvent {
    /// "listening" | "transcribing" | "transcript" | "ended" | "error"
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn emit(app: &tauri::AppHandle, kind: &'static str, text: Option<String>, message: Option<String>) {
    use tauri::Emitter;
    let _ = app.emit(
        VOICE_EVENT,
        VoiceEvent {
            kind,
            text,
            message,
        },
    );
}

/// Assembles one utterance at a time from the notification stream.
///
/// Deliberately a plain state machine rather than a task: it is driven by the
/// notification pump in `ble.rs` and owns no I/O, so the interesting
/// transitions are testable without Bluetooth.
pub struct Session {
    current: Option<Utterance>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Session { current: None }
    }

    /// Whether an utterance is being captured right now.
    pub fn is_capturing(&self) -> bool {
        self.current.is_some()
    }

    /// Handles one status notification.
    pub fn on_status(&mut self, app: &tauri::AppHandle, payload: &[u8]) {
        let Some((&event, extra)) = payload.split_first() else {
            return;
        };

        match event {
            STATUS_VOICE_STARTED => {
                println!("voice: utterance started");
                self.current = Some(Utterance::new());
                emit(app, "listening", None, None);
            }
            STATUS_VOICE_ENDED => {
                let reason = extra.first().copied().unwrap_or(0);
                if let Some(utterance) = self.current.as_ref() {
                    println!(
                        "voice: utterance ended (reason {reason}), {:.1}s, rms {:.0}{}",
                        utterance.duration_secs(),
                        utterance.rms(),
                        if utterance.rms() < 400.0 {
                            " — quiet; whisper invents text when it hears no speech"
                        } else {
                            ""
                        }
                    );
                }
                self.finish(app, reason);
            }
            STATUS_PANEL_GEOMETRY => {
                // Not used yet: wrapping and pagination both live in the
                // firmware. Recorded because the panel is a stand-in for a
                // micro-LED with different dimensions. See docs/protocol.md.
                if let [cols, rows] = extra {
                    println!("ble: panel is {cols}x{rows} characters");
                }
            }
            _ => {}
        }
    }

    /// Handles one voice frame. Frames outside an utterance are ignored —
    /// they would be audio nobody asked for.
    pub fn on_frame(&mut self, payload: &[u8]) {
        let Some(utterance) = self.current.as_mut() else {
            return;
        };
        match decode_frame(payload) {
            Ok(frame) => utterance.push(frame),
            // One bad frame is a hole, not a reason to lose the sentence.
            Err(err) => eprintln!("ble: dropped a voice frame: {err}"),
        }
    }

    /// Drops a half-captured utterance, e.g. because the link went away.
    pub fn abandon(&mut self, app: &tauri::AppHandle) {
        if self.current.take().is_some() {
            emit(app, "ended", None, None);
        }
    }

    /// Ends the utterance and starts transcription.
    fn finish(&mut self, app: &tauri::AppHandle, reason: u8) {
        let Some(utterance) = self.current.take() else {
            return;
        };

        // 2 is the firmware's "capture error" — the audio is not trustworthy.
        if reason == 2 {
            emit(app, "error", None, Some("The monocle's microphone failed mid-sentence.".into()));
            return;
        }

        if utterance.is_empty() || utterance.duration_secs() < MIN_UTTERANCE_SECS {
            emit(app, "ended", None, None);
            return;
        }

        if utterance.dropped() > 0 {
            eprintln!(
                "ble: {} of {:.1}s of audio never arrived",
                utterance.dropped(),
                utterance.duration_secs()
            );
        }

        emit(app, "transcribing", None, None);

        let wav = utterance.to_wav();

        // Debug builds keep a copy of exactly what whisper was given. When a
        // transcript comes back empty or wrong, the only way to tell a bad
        // recording from a bad transcription is to listen to the audio — and
        // it is otherwise never written down anywhere.
        #[cfg(debug_assertions)]
        {
            let path = std::env::temp_dir().join(format!(
                "minicole-utterance-{}.wav",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            ));
            match std::fs::write(&path, &wav) {
                Ok(()) => println!("voice: wrote {} for inspection", path.display()),
                Err(err) => eprintln!("voice: could not save the utterance: {err}"),
            }
        }
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            match crate::whisper::transcribe(&app, wav).await {
                Ok(text) if text.is_empty() => {
                    // Whisper heard nothing it could turn into words. Ending
                    // quietly beats filing an empty session — but say so on
                    // the console, because from the outside this is
                    // indistinguishable from the app ignoring you.
                    println!("voice: whisper returned an empty transcript");
                    emit(&app, "ended", None, None);
                }
                Ok(text) => {
                    println!("voice: transcript {text:?}");
                    emit(&app, "transcript", Some(text), None)
                }
                Err(err) => {
                    // Also to the console: this is the step most likely to
                    // fail (missing model, sidecar not built) and the message
                    // says exactly which.
                    eprintln!("voice: transcription failed: {err}");
                    emit(&app, "error", None, Some(err))
                }
            }
        });
    }
}

#[cfg(test)]
#[path = "../tests/voice_test.rs"]
mod tests;
