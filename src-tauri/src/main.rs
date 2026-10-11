// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod workbench;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            workbench::http_request,
            workbench::state_load,
            workbench::state_save,
            workbench::storage_set,
            workbench::reveal_data_file,
            workbench::save_text_file,
            workbench::clipboard_read,
            workbench::clipboard_write,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}