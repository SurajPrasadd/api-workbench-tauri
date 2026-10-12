mod workbench;
mod ws;
 
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(ws::WsState::default())
        .invoke_handler(tauri::generate_handler![
            workbench::http_request,
            workbench::state_load,
            workbench::state_save,
            workbench::storage_set,
            workbench::reveal_data_file,
            workbench::save_text_file,
            workbench::clipboard_read,
            workbench::clipboard_write,
            ws::ws_connect,
            ws::ws_send,
            ws::ws_close,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
 