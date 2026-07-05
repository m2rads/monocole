use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::AsyncWriteExt;

use crate::manifest::{self, ModelEntry};

pub const DOWNLOAD_EVENT: &str = "model-download";
const PROGRESS_EMIT_STEP: u64 = 4 * 1024 * 1024;

/// Cancellation flags for in-flight downloads, keyed by manifest model id.
#[derive(Default)]
pub struct Downloads(Mutex<HashMap<String, Arc<AtomicBool>>>);

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModel {
    pub file: String,
    pub size_bytes: u64,
    /// true when only a resumable .part file exists.
    pub partial: bool,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct AppSettings {
    active_model_file: Option<String>,
}

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|err| err.to_string())
}

fn models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app_data_dir(app)?.join("models");
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    Ok(dir)
}

/// Model files are always addressed by bare file name inside models_dir.
pub(crate) fn validate_file_name(file: &str) -> Result<(), String> {
    if file.is_empty()
        || file.starts_with('.')
        || file.contains('/')
        || file.contains('\\')
        || !file.ends_with(".gguf")
    {
        return Err(format!("invalid model file name: {file}"));
    }
    Ok(())
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("settings.json"))
}

fn read_settings(app: &AppHandle) -> AppSettings {
    settings_path(app)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_settings(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    let dir = app_data_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let raw = serde_json::to_string_pretty(settings).map_err(|err| err.to_string())?;
    std::fs::write(settings_path(app)?, raw).map_err(|err| err.to_string())
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

/// Absolute path to the active model file, if one is set and still on disk.
pub fn active_model_path(app: &AppHandle) -> Result<Option<PathBuf>, String> {
    let settings = read_settings(app);
    let Some(file) = settings.active_model_file else {
        return Ok(None);
    };
    let path = models_dir(app)?.join(&file);
    Ok(path.is_file().then_some(path))
}

#[tauri::command]
pub fn list_local_models(app: AppHandle) -> Result<Vec<LocalModel>, String> {
    scan_models_dir(&models_dir(&app)?)
}

/// Lists final (.gguf) and resumable partial (.gguf.part) model files.
/// A final file always wins over a partial with the same name.
pub fn scan_models_dir(dir: &std::path::Path) -> Result<Vec<LocalModel>, String> {
    let mut by_file: HashMap<String, LocalModel> = HashMap::new();

    for entry in std::fs::read_dir(dir).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let meta = entry.metadata().map_err(|err| err.to_string())?;
        if !meta.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".gguf") {
            by_file.insert(
                name.clone(),
                LocalModel {
                    file: name,
                    size_bytes: meta.len(),
                    partial: false,
                },
            );
        } else if let Some(final_name) = name.strip_suffix(".part") {
            if final_name.ends_with(".gguf") && !by_file.contains_key(final_name) {
                by_file.insert(
                    final_name.to_string(),
                    LocalModel {
                        file: final_name.to_string(),
                        size_bytes: meta.len(),
                        partial: true,
                    },
                );
            }
        }
    }

    let mut models: Vec<LocalModel> = by_file.into_values().collect();
    models.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(models)
}

#[tauri::command]
pub fn get_active_model(app: AppHandle) -> Result<Option<String>, String> {
    let settings = read_settings(&app);
    let Some(file) = settings.active_model_file else {
        return Ok(None);
    };
    // The file may have been deleted out from under us.
    if models_dir(&app)?.join(&file).is_file() {
        Ok(Some(file))
    } else {
        Ok(None)
    }
}

#[tauri::command]
pub fn set_active_model(app: AppHandle, file: Option<String>) -> Result<(), String> {
    if let Some(name) = &file {
        validate_file_name(name)?;
        if !models_dir(&app)?.join(name).is_file() {
            return Err(format!("model file not found: {name}"));
        }
    }
    let mut settings = read_settings(&app);
    settings.active_model_file = file;
    write_settings(&app, &settings)
}

#[tauri::command]
pub fn delete_model(
    app: AppHandle,
    state: State<'_, Downloads>,
    file: String,
) -> Result<(), String> {
    validate_file_name(&file)?;

    // Refuse while a download for this file is in flight; cancel first.
    if let Ok(manifest) = manifest::bundled() {
        if let Some(entry) = manifest.models.iter().find(|m| m.file == file) {
            if state.0.lock().unwrap().contains_key(&entry.id) {
                return Err("download in progress — cancel it first".into());
            }
        }
    }

    let dir = models_dir(&app)?;
    let final_path = dir.join(&file);
    let part_path = dir.join(format!("{file}.part"));
    for path in [final_path, part_path] {
        if path.is_file() {
            std::fs::remove_file(&path).map_err(|err| err.to_string())?;
        }
    }

    let mut settings = read_settings(&app);
    if settings.active_model_file.as_deref() == Some(file.as_str()) {
        settings.active_model_file = None;
        write_settings(&app, &settings)?;
    }
    Ok(())
}

#[tauri::command]
pub fn cancel_download(state: State<'_, Downloads>, id: String) -> Result<(), String> {
    match state.0.lock().unwrap().get(&id) {
        Some(flag) => {
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
        if active.contains_key(&id) {
            return Err("download already in progress".into());
        }
        active.insert(id.clone(), cancel.clone());
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
#[path = "../tests/models_test.rs"]
mod tests;
