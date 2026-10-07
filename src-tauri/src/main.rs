#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;
use tauri::{Manager, State};

// ---------------------------------------------------------------------------
// App state: configurable data file path
// ---------------------------------------------------------------------------

struct AppState {
    data_file: Mutex<PathBuf>,
}

fn default_data_file() -> PathBuf {
    dirs::data_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("API Workbench")
        .join("workbench-data.json")
}

// ---------------------------------------------------------------------------
// HTTP payload / result (mirrors the VS Code extension interface)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MultipartPart {
    name: String,
    value: Option<String>,
    filename: Option<String>,
    content_type: Option<String>,
    data_base64: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HttpPayload {
    method: String,
    url: String,
    headers: Option<std::collections::HashMap<String, String>>,
    body: Option<String>,
    body_base64: Option<String>,
    multipart: Option<Vec<MultipartPart>>,
    timeout_ms: Option<u64>,
    /// Accept self-signed / invalid TLS certificates (local HTTPS only).
    allow_insecure_tls: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HttpResult {
    status: u16,
    status_text: String,
    headers: std::collections::HashMap<String, String>,
    body_text: String,
    size: usize,
    time: u64,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
async fn http_request(payload: HttpPayload) -> Result<HttpResult, String> {
    let method = payload.method.to_uppercase();
    let url = payload.url.clone();
    let timeout = std::time::Duration::from_millis(payload.timeout_ms.unwrap_or(60_000));
    let insecure = payload.allow_insecure_tls.unwrap_or(false);

    let client = reqwest::Client::builder()
        .timeout(timeout)
        .danger_accept_invalid_certs(insecure)
        .build()
        .map_err(|e| e.to_string())?;

    let mut req = match method.as_str() {
        "GET" => client.get(&url),
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "PATCH" => client.patch(&url),
        "DELETE" => client.delete(&url),
        "HEAD" => client.head(&url),
        "OPTIONS" => client.request(reqwest::Method::OPTIONS, &url),
        other => client.request(
            reqwest::Method::from_bytes(other.as_bytes()).map_err(|e| e.to_string())?,
            &url,
        ),
    };

    if let Some(headers) = &payload.headers {
        for (k, v) in headers {
            // Skip content-type when we build multipart ourselves
            if payload.multipart.is_some() && k.eq_ignore_ascii_case("content-type") {
                continue;
            }
            if k.eq_ignore_ascii_case("content-length") {
                continue;
            }
            req = req.header(k.as_str(), v.as_str());
        }
    }

    let has_body = method != "GET" && method != "HEAD";
    if has_body {
        if let Some(parts) = &payload.multipart {
            let mut form = reqwest::multipart::Form::new();
            for part in parts {
                if let Some(b64) = &part.data_base64 {
                    let bytes = B64.decode(b64).map_err(|e| format!("base64 decode: {e}"))?;
                    let filename = part.filename.clone().unwrap_or_else(|| "file".into());
                    let mut part_builder = reqwest::multipart::Part::bytes(bytes).file_name(filename);
                    if let Some(ct) = &part.content_type {
                        part_builder = part_builder
                            .mime_str(ct)
                            .map_err(|e| format!("mime: {e}"))?;
                    }
                    form = form.part(part.name.clone(), part_builder);
                } else {
                    form = form.text(part.name.clone(), part.value.clone().unwrap_or_default());
                }
            }
            req = req.multipart(form);
        } else if let Some(b64) = &payload.body_base64 {
            let bytes = B64.decode(b64).map_err(|e| format!("base64 decode: {e}"))?;
            req = req.body(bytes);
        } else if let Some(body) = &payload.body {
            req = req.body(body.clone());
        }
    }

    let started = Instant::now();
    let res = req.send().await.map_err(|e| describe_reqwest(e))?;
    let status = res.status().as_u16();
    let status_text = res
        .status()
        .canonical_reason()
        .unwrap_or("")
        .to_string();

    let mut headers_map = std::collections::HashMap::new();
    for (k, v) in res.headers().iter() {
        headers_map.insert(
            k.as_str().to_string(),
            v.to_str().unwrap_or("").to_string(),
        );
    }

    let bytes = res.bytes().await.map_err(|e| e.to_string())?;
    let size = bytes.len();
    let body_text = String::from_utf8_lossy(&bytes).into_owned();
    let time = started.elapsed().as_millis() as u64;

    Ok(HttpResult {
        status,
        status_text,
        headers: headers_map,
        body_text,
        size,
        time,
    })
}

fn describe_reqwest(e: reqwest::Error) -> String {
    if e.is_timeout() {
        return "Request timed out".into();
    }
    if e.is_connect() {
        return format!("Connection failed: {e}");
    }
    e.to_string()
}

// ---------------------------------------------------------------------------
// State file I/O
// ---------------------------------------------------------------------------

#[tauri::command]
fn state_load(state: State<'_, AppState>) -> Result<StateLoadResult, String> {
    let file = state.data_file.lock().map_err(|e| e.to_string())?.clone();
    if !file.exists() {
        return Ok(StateLoadResult {
            data: None,
            warning: None,
            file: file.to_string_lossy().into_owned(),
        });
    }
    let text = fs::read_to_string(&file).map_err(|e| e.to_string())?;
    if text.trim().is_empty() {
        return Ok(StateLoadResult {
            data: None,
            warning: None,
            file: file.to_string_lossy().into_owned(),
        });
    }
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(data) => Ok(StateLoadResult {
            data: Some(data),
            warning: None,
            file: file.to_string_lossy().into_owned(),
        }),
        Err(_) => {
            let backup = format!(
                "{}.corrupt-{}",
                file.display(),
                chrono_like_ts()
            );
            let _ = fs::copy(&file, &backup);
            Ok(StateLoadResult {
                data: None,
                warning: Some(format!(
                    "Data file was not valid JSON. A copy was kept at {backup}"
                )),
                file: file.to_string_lossy().into_owned(),
            })
        }
    }
}

fn chrono_like_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StateLoadResult {
    data: Option<serde_json::Value>,
    warning: Option<String>,
    file: String,
}

#[tauri::command]
fn state_save(state: State<'_, AppState>, data: serde_json::Value) -> Result<StateSaveResult, String> {
    let file = state.data_file.lock().map_err(|e| e.to_string())?.clone();
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = file.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &file).map_err(|e| e.to_string())?;
    Ok(StateSaveResult {
        file: file.to_string_lossy().into_owned(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StateSaveResult {
    file: String,
}

#[tauri::command]
fn reveal_data_file(state: State<'_, AppState>) -> Result<(), String> {
    let file = state.data_file.lock().map_err(|e| e.to_string())?.clone();
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).ok();
        open::that(parent).map_err(|e| e.to_string())?;
    } else {
        open::that(&file).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StorageSetArgs {
    reset: Option<bool>,
    data: Option<serde_json::Value>,
    /// Absolute path chosen by the frontend (folder or file).
    path: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageSetResult {
    file: String,
    existed: bool,
    cancelled: bool,
}

#[tauri::command]
fn storage_set(
    state: State<'_, AppState>,
    args: StorageSetArgs,
) -> Result<StorageSetResult, String> {
    if args.reset.unwrap_or(false) {
        let def = default_data_file();
        if let Some(parent) = def.parent() {
            fs::create_dir_all(parent).ok();
        }
        // Copy current data if default is empty
        let current = state.data_file.lock().map_err(|e| e.to_string())?.clone();
        let existed = def.exists();
        if !existed {
            if let Some(data) = args.data {
                let json = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
                fs::write(&def, json).map_err(|e| e.to_string())?;
            } else if current.exists() && current != def {
                fs::copy(&current, &def).ok();
            }
        }
        *state.data_file.lock().map_err(|e| e.to_string())? = def.clone();
        return Ok(StorageSetResult {
            file: def.to_string_lossy().into_owned(),
            existed,
            cancelled: false,
        });
    }

    let path_str = args.path.ok_or_else(|| "No path provided".to_string())?;
    let path = PathBuf::from(&path_str);
    let file = if path.is_dir() || path_str.ends_with('/') || path_str.ends_with('\\') {
        path.join("workbench-data.json")
    } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
        path
    } else {
        // Treat as folder path even if it doesn't exist yet
        path.join("workbench-data.json")
    };

    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let existed = file.exists();
    if !existed {
        if let Some(data) = args.data {
            let json = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
            fs::write(&file, json).map_err(|e| e.to_string())?;
        }
    }

    *state.data_file.lock().map_err(|e| e.to_string())? = file.clone();
    Ok(StorageSetResult {
        file: file.to_string_lossy().into_owned(),
        existed,
        cancelled: false,
    })
}

#[tauri::command]
fn get_data_file(state: State<'_, AppState>) -> Result<String, String> {
    let file = state.data_file.lock().map_err(|e| e.to_string())?;
    Ok(file.to_string_lossy().into_owned())
}

#[tauri::command]
fn pick_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri::api::dialog::blocking::FileDialogBuilder;
    let folder = FileDialogBuilder::new()
        .set_title("Choose data folder")
        .pick_folder();
    Ok(folder.map(|p| p.to_string_lossy().into_owned()))
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let data_file = default_data_file();
    if let Some(parent) = data_file.parent() {
        let _ = fs::create_dir_all(parent);
    }

    tauri::Builder::default()
        .manage(AppState {
            data_file: Mutex::new(data_file),
        })
        .invoke_handler(tauri::generate_handler![
            http_request,
            state_load,
            state_save,
            reveal_data_file,
            storage_set,
            get_data_file,
            pick_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running API Workbench");
}
