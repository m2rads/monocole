#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod llama;
mod manifest;
mod models;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(models::Downloads::default())
        .manage(llama::LlamaState::default())
        .invoke_handler(tauri::generate_handler![
            manifest::get_model_manifest,
            models::list_local_models,
            models::download_model,
            models::cancel_download,
            models::delete_model,
            models::get_active_model,
            models::set_active_model,
            llama::chat_stream,
            llama::generate_session_title,
            llama::llama_status
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Never leave an orphaned llama-server holding gigabytes of RAM.
            if let tauri::RunEvent::Exit = event {
                llama::shutdown(app);
            }
        });
}
