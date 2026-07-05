pub mod ble;
pub mod llama;
pub mod manifest;
pub mod models;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(models::Downloads::default())
        .manage(llama::LlamaState::default())
        .manage(ble::BleState::default())
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
            llama::llama_status,
            ble::ble_status,
            ble::ble_start_scan,
            ble::ble_stop_scan,
            ble::ble_connect,
            ble::ble_disconnect
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
