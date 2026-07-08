//! Model management, split by concern: `files` (naming rules, scanning,
//! deletion), `download` (resumable download engine), and `settings`
//! (persisted active model). Shared path helpers live here.

pub mod download;
pub mod files;
pub mod settings;

pub use download::Downloads;
pub use settings::active_model_path;

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

pub(crate) fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|err| err.to_string())
}

pub(crate) fn models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app_data_dir(app)?.join("models");
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    Ok(dir)
}
