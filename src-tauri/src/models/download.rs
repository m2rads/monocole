//! The download engine: resumable HTTP downloads of .gguf files with
//! progress events and cancellation, fed by the curated manifest or a
//! direct URL.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::io::AsyncWriteExt;

use crate::manifest::{self, ModelEntry};

use super::files::validate_file_name;
use super::models_dir;

pub const DOWNLOAD_EVENT: &str = "model-download";
const PROGRESS_EMIT_STEP: u64 = 4 * 1024 * 1024;

/// In-flight downloads keyed by download id (manifest model id, or the file
/// name for URL downloads): the cancellation flag plus the target file name,
/// so two downloads can never write the same .part file.
#[derive(Default)]
pub struct Downloads(Mutex<HashMap<String, (Arc<AtomicBool>, String)>>);

impl Downloads {
    /// True if any in-flight download targets `file`.
    pub(crate) fn is_downloading_file(&self, file: &str) -> bool {
        self.0.lock().unwrap().values().any(|(_, f)| f == file)
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadEvent {
    id: String,
    /// "downloading" | "completed" | "cancelled" | "failed"
    status: &'static str,
    downloaded: u64,
    total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn emit(app: &AppHandle, id: &str, status: &'static str, downloaded: u64, total: u64, error: Option<String>) {
    let _ = app.emit(
        DOWNLOAD_EVENT,
        DownloadEvent {
            id: id.to_string(),
            status,
            downloaded,
            total,
            error,
        },
    );
}

/// Derives the on-disk file name from a direct-download URL: the last path
/// segment, which must be a valid .gguf file name.
pub(crate) fn file_name_from_url(url: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(url).map_err(|err| format!("invalid URL: {err}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("URL must use http or https".into());
    }
    let file = parsed
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or("");
    if !file.ends_with(".gguf") {
        return Err("URL must point to a .gguf file".into());
    }
    validate_file_name(file)?;
    Ok(file.to_string())
}

#[tauri::command]
pub fn cancel_download(state: State<'_, Downloads>, id: String) -> Result<(), String> {
    match state.0.lock().unwrap().get(&id) {
        Some((flag, _)) => {
            flag.store(true, Ordering::Relaxed);
            Ok(())
        }
        None => Err(format!("no active download for {id}")),
    }
}

#[tauri::command]
pub async fn download_model(
    app: AppHandle,
    state: State<'_, Downloads>,
    id: String,
) -> Result<(), String> {
    let entry = manifest::bundled()
        .map_err(|err| err.to_string())?
        .models
        .into_iter()
        .find(|model| model.id == id)
        .ok_or_else(|| format!("unknown model id: {id}"))?;
    validate_file_name(&entry.file)?;
    execute_download(app, state, id, entry).await
}

/// Downloads a GGUF from an arbitrary direct URL (e.g. a Hugging Face
/// download link). The on-disk name comes from the URL's last path segment;
/// progress events are keyed by that file name instead of a manifest id.
#[tauri::command]
pub async fn download_model_from_url(
    app: AppHandle,
    state: State<'_, Downloads>,
    url: String,
) -> Result<(), String> {
    let file = file_name_from_url(&url)?;
    let entry = ModelEntry {
        id: file.clone(),
        name: file.clone(),
        description: String::new(),
        repo: String::new(),
        file: file.clone(),
        url,
        quant: String::new(),
        // Unknown until the response arrives; progress totals fall back to
        // the Content-Length header.
        size_bytes: 0,
        min_ram_gb: 0,
        sha256: None,
    };
    execute_download(app, state, file, entry).await
}

async fn execute_download(
    app: AppHandle,
    state: State<'_, Downloads>,
    id: String,
    entry: ModelEntry,
) -> Result<(), String> {
    let final_path = models_dir(&app)?.join(&entry.file);
    if final_path.is_file() {
        let size = std::fs::metadata(&final_path)
            .map(|meta| meta.len())
            .unwrap_or(entry.size_bytes);
        emit(&app, &id, "completed", size, size, None);
        return Ok(());
    }

    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut active = state.0.lock().unwrap();
        if active.contains_key(&id) || active.values().any(|(_, f)| f == &entry.file) {
            return Err("download already in progress".into());
        }
        active.insert(id.clone(), (cancel.clone(), entry.file.clone()));
    }

    let outcome = run_download(&entry, &final_path, &cancel, |downloaded, total| {
        emit(&app, &id, "downloading", downloaded, total, None);
    })
    .await;
    state.0.lock().unwrap().remove(&id);

    match outcome {
        Ok(Outcome::Completed { total }) => {
            emit(&app, &id, "completed", total, total, None);
            Ok(())
        }
        Ok(Outcome::Cancelled { downloaded, total }) => {
            emit(&app, &id, "cancelled", downloaded, total, None);
            Ok(())
        }
        Err(err) => {
            emit(&app, &id, "failed", 0, entry.size_bytes, Some(err.clone()));
            Err(err)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed { total: u64 },
    Cancelled { downloaded: u64, total: u64 },
}

/// Streams `entry.url` to `final_path` via a resumable `.part` file.
/// `on_progress(downloaded, total)` fires at start and every few MB; the
/// caller decides how to surface it (the app emits Tauri events).
pub async fn run_download(
    entry: &ModelEntry,
    final_path: &std::path::Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<Outcome, String> {
    let part_path = final_path.with_file_name(format!("{}.part", entry.file));
    let existing = tokio::fs::metadata(&part_path)
        .await
        .map(|meta| meta.len())
        .unwrap_or(0);

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|err| err.to_string())?;
    let mut request = client.get(&entry.url);
    if existing > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
    }
    let response = request.send().await.map_err(|err| err.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("download failed: HTTP {status}"));
    }

    // 206 means the server honored our Range header and we append; anything
    // else restarts from scratch (some hosts ignore Range).
    let resuming = existing > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT;
    let mut downloaded = if resuming { existing } else { 0 };
    let total = response
        .content_length()
        .map(|len| len + downloaded)
        .unwrap_or(entry.size_bytes);

    let mut file = if resuming {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(&part_path)
            .await
    } else {
        tokio::fs::File::create(&part_path).await
    }
    .map_err(|err| err.to_string())?;

    on_progress(downloaded, total);
    let mut last_emitted = downloaded;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            file.flush().await.map_err(|err| err.to_string())?;
            // Keep the .part file so the next attempt resumes.
            return Ok(Outcome::Cancelled { downloaded, total });
        }
        let chunk = chunk.map_err(|err| err.to_string())?;
        file.write_all(&chunk).await.map_err(|err| err.to_string())?;
        downloaded += chunk.len() as u64;
        if downloaded - last_emitted >= PROGRESS_EMIT_STEP {
            on_progress(downloaded, total);
            last_emitted = downloaded;
        }
    }

    file.flush().await.map_err(|err| err.to_string())?;
    drop(file);

    // TODO(config-server): verify entry.sha256 here once the manifest
    // provides checksums; delete the file and fail on mismatch.
    tokio::fs::rename(&part_path, final_path)
        .await
        .map_err(|err| err.to_string())?;
    Ok(Outcome::Completed { total: downloaded })
}

#[cfg(test)]
#[path = "../../tests/models/download_test.rs"]
mod tests;
