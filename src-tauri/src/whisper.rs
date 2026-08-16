//! The speech-to-text sidecar.
//!
//! Deliberately shaped like `llama.rs`: a child process spawned on demand,
//! kept alive with its model resident, and addressed over HTTP on a loopback
//! port. Keeping the model loaded is the whole point — reloading ~148 MB per
//! utterance would put a second between finishing a sentence and seeing it
//! transcribed, on top of the inference that follows.
//!
//! The transcription call is separated from the process lifecycle, in the same
//! spirit as `llama::stream_completion`, so it can be exercised against any
//! HTTP server in tests rather than needing the real binary.

use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

use crate::{manifest, models};

/// Whisper loads far faster than an LLM, but a cold Metal shader compile on
/// first run is not instant.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct WhisperState(Mutex<Option<WhisperProcess>>);

pub struct WhisperProcess {
    child: Child,
    port: u16,
}

fn server_binary(app: &AppHandle) -> Result<PathBuf, String> {
    // TODO(packaging): identical to the hole in llama.rs — a packaged build
    // resolves from resource_dir(), but tauri.conf.json bundles nothing, so
    // neither sidecar is present in a distributed .app. Fix both together.
    let path = if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries/whisper/whisper-server")
    } else {
        app.path()
            .resource_dir()
            .map_err(|err| err.to_string())?
            .join("binaries/whisper/whisper-server")
    };
    if !path.is_file() {
        return Err(format!(
            "whisper-server not found at {} — run scripts/fetch-whisper-server.sh",
            path.display()
        ));
    }
    Ok(path)
}

/// Where the speech model lives once downloaded.
///
/// It goes in the same directory as the chat models and through the same
/// downloader, so it gets progress, resume and a place in Settings → Models
/// for free. The manifest is what says which file that is.
pub fn model_path(app: &AppHandle) -> Result<PathBuf, String> {
    let catalog = manifest::bundled().map_err(|err| err.to_string())?;
    let entry = catalog
        .stt_model()
        .ok_or("the model catalog lists no speech-to-text model")?;
    Ok(models::models_dir(app)?.join(&entry.file))
}

fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|err| err.to_string())
}

/// Kill the sidecar synchronously; called from the app exit handler.
pub fn shutdown(app: &AppHandle) {
    if let Some(state) = app.try_state::<WhisperState>() {
        if let Ok(mut guard) = state.0.try_lock() {
            if let Some(mut proc) = guard.take() {
                let _ = proc.child.kill();
                let _ = proc.child.wait();
            }
        }
    }
}

/// Returns the port of a healthy whisper-server, spawning it if needed.
async fn ensure_running(app: &AppHandle, state: &WhisperState) -> Result<u16, String> {
    let model = model_path(app)?;
    if !model.is_file() {
        // Worth naming precisely: the fix is a download, not a reinstall.
        return Err(
            "The speech model has not been downloaded yet. Open Settings → Models to fetch it."
                .into(),
        );
    }

    let mut guard = state.0.lock().await;

    if let Some(proc) = guard.as_mut() {
        if matches!(proc.child.try_wait(), Ok(None)) {
            return Ok(proc.port);
        }
        // Exited on its own — reap it and start again rather than handing back
        // a port nothing is listening on.
        let _ = proc.child.wait();
        *guard = None;
    }

    let binary = server_binary(app)?;
    let port = free_port()?;
    let mut child = Command::new(&binary)
        .args([
            "--model",
            model.to_str().ok_or("model path is not valid UTF-8")?,
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("failed to spawn whisper-server: {err}"))?;

    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/");
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("whisper-server exited during startup ({status})"));
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("whisper-server did not become ready in time".into());
        }
        // Any answer means the listener is up; whisper-server has no dedicated
        // health endpoint, so a served response is the signal.
        let up = client
            .get(&url)
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .is_ok();
        if up {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    *guard = Some(WhisperProcess { child, port });
    Ok(port)
}

/// Transcribes one utterance, given a WAV.
///
/// Separated from the sidecar lifecycle so tests can point it at any HTTP
/// server. Returns the text with surrounding whitespace removed — whisper
/// habitually prefixes a space, which would show up in the chat bubble.
pub async fn transcribe_wav(port: u16, wav: Vec<u8>) -> Result<String, String> {
    let form = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(wav)
                .file_name("utterance.wav")
                .mime_str("audio/wav")
                .map_err(|err| err.to_string())?,
        )
        .text("response_format", "json")
        // Greedy decoding: this is a short utterance from a head-worn mic, and
        // sampling would only add latency and variance to a transcript the
        // user is waiting on.
        .text("temperature", "0.0");

    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/inference"))
        .multipart(form)
        .send()
        .await
        .map_err(|err| err.to_string())?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("whisper-server returned {status}: {body}"));
    }

    let value: serde_json::Value = response.json().await.map_err(|err| err.to_string())?;
    let text = value["text"]
        .as_str()
        .ok_or("whisper-server returned no text field")?;

    Ok(text.trim().to_string())
}

/// Transcribes an utterance, bringing the sidecar up if it is not running.
pub async fn transcribe(app: &AppHandle, wav: Vec<u8>) -> Result<String, String> {
    let state = app
        .try_state::<WhisperState>()
        .ok_or("whisper state is not registered")?;
    let port = ensure_running(app, &state).await?;
    transcribe_wav(port, wav).await
}

#[cfg(test)]
#[path = "../tests/whisper_test.rs"]
mod tests;
