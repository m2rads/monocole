//! Unit tests for voice decoding and utterance assembly.
//!
//! No BLE and no hardware: frames are built here in the shape docs/protocol.md
//! describes. What these cannot check is whether the audio *sounds* right —
//! that needs firmware/test/capture_voice.py and a pair of ears.

use super::*;

/// Builds a frame the way the firmware does: a header snapshotting the
/// starting state, then the payload.
fn frame(seq: u16, predictor: i16, step_index: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&predictor.to_le_bytes());
    out.push(step_index);
    out.extend_from_slice(payload);
    out
}

/// A full-size frame of a constant nibble, which is enough to exercise
/// decoding without hand-computing IMA output.
fn full_frame(seq: u16, nibble_pair: u8) -> Vec<u8> {
    frame(seq, 0, 0, &vec![nibble_pair; FRAME_SAMPLES / 2])
}

mod decoding {
    use super::*;

    #[test]
    fn decodes_a_full_frame_to_512_samples() {
        let decoded = decode_frame(&full_frame(7, 0x00)).unwrap();
        assert_eq!(decoded.seq, 7);
        assert_eq!(decoded.samples.len(), FRAME_SAMPLES);
    }

    #[test]
    fn silence_stays_near_zero() {
        // Nibble 0 is the smallest positive step, so a frame of them barely
        // moves the predictor. Anything large here means the tables are wrong.
        let decoded = decode_frame(&full_frame(0, 0x00)).unwrap();
        assert!(
            decoded.samples.iter().all(|s| s.abs() < 512),
            "expected near-silence, got peak {}",
            decoded.samples.iter().map(|s| s.abs()).max().unwrap()
        );
    }

    #[test]
    fn the_header_sets_the_starting_point() {
        // The predictor in the header is where decoding begins, which is what
        // lets a frame stand alone. A frame starting at 10000 must decode to
        // samples near 10000, not near zero.
        let decoded = decode_frame(&frame(0, 10_000, 0, &[0x00; 8])).unwrap();
        assert!(decoded.samples[0] > 9_000);
    }

    #[test]
    fn a_frame_shorter_than_its_header_is_refused() {
        assert_eq!(decode_frame(&[0, 0]), Err(FrameError::TooShort(2)));
    }

    #[test]
    fn an_impossible_step_index_is_refused() {
        // Guards against indexing past the step table on garbage off the air,
        // which in a less careful decoder is a panic.
        assert_eq!(
            decode_frame(&frame(0, 0, 89, &[0x00; 8])),
            Err(FrameError::BadStepIndex(89))
        );
    }

    #[test]
    fn a_header_only_frame_decodes_to_nothing() {
        let decoded = decode_frame(&frame(3, 0, 0, &[])).unwrap();
        assert_eq!(decoded.seq, 3);
        assert!(decoded.samples.is_empty());
    }
}

mod assembly {
    use super::*;

    #[test]
    fn frames_concatenate_in_order() {
        let mut utterance = Utterance::new();
        for seq in 0..3 {
            utterance.push(decode_frame(&full_frame(seq, 0x22)).unwrap());
        }
        assert_eq!(utterance.dropped(), 0);
        assert_eq!(utterance.duration_secs(), 3.0 * 512.0 / 16_000.0);
    }

    #[test]
    fn a_gap_becomes_silence_and_keeps_later_audio_in_place() {
        let mut utterance = Utterance::new();
        utterance.push(decode_frame(&full_frame(0, 0x77)).unwrap());
        // Frame 1 never arrives.
        utterance.push(decode_frame(&full_frame(2, 0x77)).unwrap());

        assert_eq!(utterance.dropped(), 1);

        let wav = utterance.to_wav();
        let samples = (wav.len() - 44) / 2;
        assert_eq!(samples, 3 * FRAME_SAMPLES, "the hole must occupy real time");
    }

    #[test]
    fn a_wild_sequence_jump_is_not_padded() {
        // A restarted utterance or a reordered frame must not make us
        // allocate thousands of frames of silence.
        let mut utterance = Utterance::new();
        utterance.push(decode_frame(&full_frame(0, 0x11)).unwrap());
        utterance.push(decode_frame(&full_frame(40_000, 0x11)).unwrap());

        assert_eq!(utterance.dropped(), 0);
        assert_eq!((utterance.to_wav().len() - 44) / 2, 2 * FRAME_SAMPLES);
    }

    #[test]
    fn an_empty_utterance_is_empty() {
        let utterance = Utterance::new();
        assert!(utterance.is_empty());
        assert_eq!(utterance.duration_secs(), 0.0);
    }
}

/// Cross-checks this decoder against the *other* implementation.
///
/// The fixture was produced by `firmware/test/adpcm.py`, which is written
/// independently of this module and is the decoder the hardware capture
/// already validates end to end. If the two ever disagree, the audio reaching
/// the transcriber is subtly wrong in a way that still sounds like speech —
/// exactly the failure a second implementation exists to catch.
///
/// Regenerate with:
///   python -c "import math,sys; sys.path.insert(0,'firmware/test'); \
///     from adpcm import encode_utterance, SAMPLES_PER_FRAME; \
///     open('src-tauri/tests/fixtures/voice_sine_440.bin','wb').write( \
///       b''.join(encode_utterance([int(12000*math.sin(2*math.pi*440*i/16000)) \
///         for i in range(SAMPLES_PER_FRAME*2)])))"
mod cross_implementation {
    use super::*;

    const SINE_440: &[u8] = include_bytes!("fixtures/voice_sine_440.bin");
    const FRAME_LEN: usize = 261;

    fn expected_sine(count: usize) -> Vec<f64> {
        (0..count)
            .map(|i| 12000.0 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / 16000.0).sin())
            .collect()
    }

    fn snr_db(original: &[f64], decoded: &[i16]) -> f64 {
        let signal: f64 = original.iter().map(|s| s * s).sum();
        let noise: f64 = original
            .iter()
            .zip(decoded)
            .map(|(a, b)| (a - *b as f64).powi(2))
            .sum();
        10.0 * (signal / noise).log10()
    }

    #[test]
    fn decodes_what_the_python_encoder_produced() {
        let mut utterance = Utterance::new();
        for chunk in SINE_440.chunks(FRAME_LEN) {
            utterance.push(decode_frame(chunk).unwrap());
        }
        assert_eq!(utterance.dropped(), 0);

        let wav = utterance.to_wav();
        let decoded: Vec<i16> = wav[44..]
            .chunks(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();

        assert_eq!(decoded.len(), 2 * FRAME_SAMPLES);

        // IMA manages ~20 dB on a tone. The predictor starts at zero and the
        // step table at 7, so skip the once-per-utterance convergence.
        let original = expected_sine(decoded.len());
        let snr = snr_db(&original[64..], &decoded[64..]);
        assert!(snr > 20.0, "SNR {snr:.1} dB — the two codecs disagree");
    }

    #[test]
    fn the_second_frame_carries_continuous_encoder_state() {
        // Not a reset to (0, 0). If a future firmware reset per frame this
        // would catch it, and the audio would buzz at the frame rate.
        let second = &SINE_440[FRAME_LEN..];
        let predictor = i16::from_le_bytes([second[2], second[3]]);
        let step_index = second[4];

        assert_ne!((predictor, step_index), (0, 0));
        assert!(step_index > 0 && step_index <= 88);
    }

    #[test]
    fn a_frame_decodes_identically_alone_and_in_sequence() {
        // The property the 5-byte header buys: a dropped notification costs
        // only its own 32 ms.
        let in_sequence: Vec<i16> = SINE_440
            .chunks(FRAME_LEN)
            .flat_map(|c| decode_frame(c).unwrap().samples)
            .collect();
        let alone = decode_frame(&SINE_440[FRAME_LEN..]).unwrap().samples;

        assert_eq!(alone, in_sequence[FRAME_SAMPLES..]);
    }
}

mod wav {
    use super::*;

    fn utterance_of(frames: u16) -> Utterance {
        let mut utterance = Utterance::new();
        for seq in 0..frames {
            utterance.push(decode_frame(&full_frame(seq, 0x33)).unwrap());
        }
        utterance
    }

    #[test]
    fn header_declares_16khz_mono_16_bit() {
        let wav = utterance_of(1).to_wav();

        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1, "mono");
        assert_eq!(
            u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
            16_000
        );
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16, "bits per sample");
    }

    #[test]
    fn declared_lengths_match_the_payload() {
        // A WAV whose header disagrees with its body plays as noise or gets
        // truncated, and whisper would be blamed for it.
        let wav = utterance_of(4).to_wav();
        let riff_len = u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]) as usize;
        let data_len = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]) as usize;

        assert_eq!(riff_len, wav.len() - 8);
        assert_eq!(data_len, wav.len() - 44);
        assert_eq!(data_len, 4 * FRAME_SAMPLES * 2);
    }
}
