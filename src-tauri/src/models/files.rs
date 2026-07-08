//! Local model files: naming rules, directory scanning, and deletion.

use std::collections::HashMap;

use serde::Serialize;
use tauri::{AppHandle, State};

use super::download::Downloads;
use super::{models_dir, settings};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModel {
    pub file: String,
    pub size_bytes: u64,
    /// true when only a resumable .part file exists.
    pub partial: bool,
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
pub fn delete_model(
    app: AppHandle,
    state: State<'_, Downloads>,
    file: String,
) -> Result<(), String> {
    validate_file_name(&file)?;

    // Refuse while a download for this file is in flight; cancel first.
    if state.is_downloading_file(&file) {
        return Err("download in progress — cancel it first".into());
    }

    let dir = models_dir(&app)?;
    let final_path = dir.join(&file);
    let part_path = dir.join(format!("{file}.part"));
    for path in [final_path, part_path] {
        if path.is_file() {
            std::fs::remove_file(&path).map_err(|err| err.to_string())?;
        }
    }

    let mut settings = settings::read_settings(&app);
    if settings.active_model_file.as_deref() == Some(file.as_str()) {
        settings.active_model_file = None;
        settings::write_settings(&app, &settings)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/models/files_test.rs"]
mod tests;
