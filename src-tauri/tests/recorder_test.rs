//! Unit tests for resampling and level measurement.
//!
//! Not covered (by design): opening a real microphone. That needs hardware,
//! a device with an input, and the OS permission dialog — none of which
//! belong in a test run. What is covered is the arithmetic between the
//! microphone and whisper, which is where a silent bug would hide.

use super::*;

#[test]
fn a_matching_rate_passes_straight_through() {
    let samples = vec![0.0, 0.5, -0.5, 1.0];
    let out = resample(&samples, 16_000);
    assert_eq!(out.len(), 4);
    assert_eq!(out[0], 0);
    assert!(out[1] > 16_000 && out[1] < 16_600);
}

#[test]
fn downsampling_48k_gives_a_third_of_the_samples() {
    // The common case: Macs capture at 48 kHz and whisper wants 16 kHz. A
    // wrong ratio here does not fail, it just makes everyone sound wrong.
    let samples: Vec<f32> = (0..4800).map(|i| (i as f32 / 100.0).sin()).collect();
    assert_eq!(resample(&samples, 48_000).len(), 1600);
}

#[test]
fn downsampling_preserves_a_tone_rather_than_scrambling_it() {
    // A 440 Hz tone at 48 kHz must still be 440 Hz at 16 kHz. Counting zero
    // crossings is a cheap way to say so: ~880 per second either way.
    let rate = 48_000;
    let samples: Vec<f32> = (0..rate)
        .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / rate as f32).sin())
        .collect();

    let out = resample(&samples, rate);
    let crossings = out
        .windows(2)
        .filter(|w| (w[0] < 0) != (w[1] < 0))
        .count();

    assert!(
        (860..=900).contains(&crossings),
        "expected ~880 zero crossings for a 440 Hz tone, got {crossings}"
    );
}

#[test]
fn loud_samples_clamp_instead_of_wrapping() {
    // Without the clamp, a value above 1.0 wraps to a large negative number
    // and a loud passage becomes noise.
    let out = resample(&[2.0, -2.0], 16_000);
    assert_eq!(out, vec![i16::MAX, i16::MIN + 1]);
}

#[test]
fn empty_input_produces_empty_output() {
    assert!(resample(&[], 48_000).is_empty());
}

#[test]
fn rms_ignores_a_dc_offset() {
    // A PDM microphone sits on an offset; measuring it as signal would report
    // a silent room as loud.
    let quiet_but_offset: Vec<i16> = vec![5000; 1000];
    assert!(rms(&quiet_but_offset) < 1.0);

    let alternating: Vec<i16> = (0..1000)
        .map(|i| if i % 2 == 0 { 1000 } else { -1000 })
        .collect();
    assert!((rms(&alternating) - 1000.0).abs() < 1.0);
}

#[test]
fn rms_of_nothing_is_zero() {
    assert_eq!(rms(&[]), 0.0);
}
