#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod manifest;
mod models;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(models::Downloads::default())
        .invoke_handler(tauri::generate_handler![
            manifest::get_model_manifest,
            models::list_local_models,
            models::download_model,
            models::cancel_download,
            models::delete_model,
            models::get_active_model,
            models::set_active_model
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
