//! The persisted app settings (`<app-data>/settings.json`): currently just
//! the active model file.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use super::files::validate_file_name;
use super::{app_data_dir, models_dir};

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct AppSettings {
    pub(crate) active_model_file: Option<String>,
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("settings.json"))
}

pub(crate) fn read_settings(app: &AppHandle) -> AppSettings {
    settings_path(app)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub(crate) fn write_settings(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    let dir = app_data_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let raw = serde_json::to_string_pretty(settings).map_err(|err| err.to_string())?;
    std::fs::write(settings_path(app)?, raw).map_err(|err| err.to_string())
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

#[cfg(test)]
#[path = "../../tests/models/settings_test.rs"]
mod tests;
