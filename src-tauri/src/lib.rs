pub mod ble;
pub mod llama;
pub mod manifest;
pub mod models;
pub mod socket;

#[cfg(test)]
#[path = "../tests/helpers.rs"]
mod test_helpers;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(models::Downloads::default())
        .manage(llama::LlamaState::default())
        .manage(ble::BleState::default())
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
            socket::socket_echo,
            socket::socket_benchmark
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
