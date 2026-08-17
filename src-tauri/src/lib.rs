pub mod ble;
pub mod llama;
pub mod manifest;
pub mod models;
pub mod recorder;
pub mod monocle;
pub mod socket;
pub mod voice;
pub mod whisper;

#[cfg(test)]
#[path = "../tests/helpers.rs"]
mod test_helpers;

pub fn run() {
    // Load .env if there is one. Absent is the normal case — it carries
    // troubleshooting switches, not configuration the app needs to work.
    let _ = dotenvy::dotenv();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(models::Downloads::default())
        .manage(llama::LlamaState::default())
        .manage(ble::BleState::default())
        .manage(whisper::WhisperState::default())
        .manage(recorder::RecorderState::default())
        .invoke_handler(tauri::generate_handler![
            manifest::get_model_manifest,
            models::files::list_local_models,
            models::files::delete_model,
            models::download::download_model,
            models::download::download_model_from_url,
            models::download::cancel_download,
            models::settings::get_active_model,
            models::settings::set_active_model,
            llama::chat_stream,
            llama::generate_session_title,
            llama::llama_status,
            ble::ble_status,
            ble::ble_start_scan,
            ble::ble_stop_scan,
            ble::ble_connect,
            ble::ble_disconnect,
            ble::ble_set_wifi_credentials,
            ble::ble_set_wifi_power,
            ble::ble_display_text,
            socket::socket_echo,
            socket::socket_benchmark,
            recorder::start_recording,
            recorder::stop_recording
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Never leave an orphaned sidecar holding gigabytes of RAM.
            if let tauri::RunEvent::Exit = event {
                llama::shutdown(app);
                whisper::shutdown(app);
            }
        });
}
