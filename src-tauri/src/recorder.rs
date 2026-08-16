//! Recording from the Mac's own microphone.
//!
//! Exists so the speech model can be exercised without the monocle: when a
//! spoken turn comes back blank, this says whether the problem is whisper or
//! everything upstream of it.
//!
//! Capture lives here rather than in the webview because **WKWebView does not
//! expose `navigator.mediaDevices`** to embedded content on macOS — the API is
//! not merely permission-gated, it is absent, so the browser route cannot work
//! inside a Tauri window however the privacy settings are configured.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tauri::{AppHandle, State};

use crate::{voice, whisper};

/// What whisper wants, and what the monocle sends.
const TARGET_RATE: u32 = 16_000;

/// Guards against a runaway recording holding the microphone open — a "record"
/// the user forgot to stop is otherwise indefinite.
const MAX_SECONDS: usize = 120;

#[derive(Default)]
pub struct RecorderState(Mutex<Option<Recording>>);

struct Recording {
    stop: Arc<AtomicBool>,
    /// Returns the captured mono samples and the rate they were captured at.
    worker: JoinHandle<Result<(Vec<f32>, u32), String>>,
}

/// Captures until told to stop, on its own thread.
///
/// A thread rather than a stream stored in state: cpal's `Stream` is not
/// `Send` on macOS, so it cannot live in Tauri's shared state at all. Keeping
/// it owned by one thread side-steps that entirely.
fn capture(stop: Arc<AtomicBool>) -> Result<(Vec<f32>, u32), String> {
    let device = cpal::default_host()
        .default_input_device()
        .ok_or("no microphone is available")?;
    let config = device
        .default_input_config()
        .map_err(|err| format!("could not read the microphone's format: {err}"))?;

    let rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let limit = rate as usize * MAX_SECONDS;

    let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
    let sink = samples.clone();

    // Downmix to mono as the frames arrive: everything downstream is mono, and
    // averaging here avoids carrying two channels through the resampler.
    let mix = move |frame: &[f32]| frame.iter().sum::<f32>() / frame.len() as f32;
    let on_error = |err| eprintln!("recorder: stream error: {err}");

    let stream_config = config.clone().into();
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &stream_config,
            move |data: &[f32], _: &_| {
                let mut buffer = sink.lock().unwrap();
                if buffer.len() < limit {
                    buffer.extend(data.chunks(channels).map(&mix));
                }
            },
            on_error,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &stream_config,
            move |data: &[i16], _: &_| {
                let mut buffer = sink.lock().unwrap();
                if buffer.len() < limit {
                    buffer.extend(
                        data.chunks(channels)
                            .map(|frame| {
                                let mono: f32 = frame
                                    .iter()
                                    .map(|s| *s as f32 / i16::MAX as f32)
                                    .sum();
                                mono / frame.len() as f32
                            }),
                    );
                }
            },
            on_error,
            None,
        ),
        other => return Err(format!("unsupported microphone sample format: {other}")),
    }
    .map_err(|err| format!("could not open the microphone: {err}"))?;

    stream
        .play()
        .map_err(|err| format!("could not start recording: {err}"))?;

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    drop(stream); // releases the device, and the OS indicator with it
    let captured = samples.lock().unwrap().clone();
    Ok((captured, rate))
}

/// Resamples to 16 kHz by linear interpolation.
///
/// Good enough for speech: whisper resamples internally anyway, and the point
/// of doing it here is to hand it the same format the monocle produces so the
/// two paths are comparable.
fn resample(samples: &[f32], from: u32) -> Vec<i16> {
    if samples.is_empty() {
        return Vec::new();
    }
    if from == TARGET_RATE {
        return samples.iter().map(|s| to_i16(*s)).collect();
    }

    let ratio = from as f64 / TARGET_RATE as f64;
    let out_len = (samples.len() as f64 / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);

    for i in 0..out_len {
        let position = i as f64 * ratio;
        let index = position.floor() as usize;
        let fraction = (position - index as f64) as f32;
        let a = samples[index];
        let b = *samples.get(index + 1).unwrap_or(&a);
        out.push(to_i16(a + (b - a) * fraction));
    }

    out
}

fn to_i16(sample: f32) -> i16 {
    // Clamp before scaling: values outside [-1, 1] would wrap and turn a loud
    // passage into noise.
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

#[tauri::command]
pub fn start_recording(state: State<'_, RecorderState>) -> Result<(), String> {
    let mut guard = state.0.lock().unwrap();
    if guard.is_some() {
        return Err("already recording".into());
    }

    let stop = Arc::new(AtomicBool::new(false));
    let worker = std::thread::spawn({
        let stop = stop.clone();
        move || capture(stop)
    });

    *guard = Some(Recording { stop, worker });
    Ok(())
}

/// Stops recording and returns what whisper made of it.
///
/// An empty string means whisper heard no speech — it labels silence rather
/// than failing, and `whisper::transcribe` flattens those labels to nothing.
/// The caller says so in its own words.
#[tauri::command]
pub async fn stop_recording(
    app: AppHandle,
    state: State<'_, RecorderState>,
) -> Result<String, String> {
    let recording = state
        .0
        .lock()
        .unwrap()
        .take()
        .ok_or("not recording")?;

    recording.stop.store(true, Ordering::Relaxed);
    let (samples, rate) = recording
        .worker
        .join()
        .map_err(|_| "the recording thread panicked".to_string())??;

    let pcm = resample(&samples, rate);
    let seconds = pcm.len() as f32 / TARGET_RATE as f32;
    let level = rms(&pcm);
    println!("recorder: {seconds:.1}s at {rate} Hz, rms {level:.0}");

    if pcm.is_empty() {
        return Err("nothing was recorded".into());
    }

    let wav = voice::wav_from_pcm(&pcm, TARGET_RATE);
    whisper::transcribe(&app, wav).await
}

/// Loudness with any DC offset removed — the same measure the monocle path
/// logs, so a recording here can be compared against one from the device.
fn rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mean = samples.iter().map(|s| *s as f64).sum::<f64>() / samples.len() as f64;
    let squares: f64 = samples
        .iter()
        .map(|s| {
            let centred = *s as f64 - mean;
            centred * centred
        })
        .sum();
    (squares / samples.len() as f64).sqrt() as f32
}

#[cfg(test)]
#[path = "../tests/recorder_test.rs"]
mod tests;
