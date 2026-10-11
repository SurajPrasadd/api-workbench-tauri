//! Native backend for API Workbench (Windows, macOS, Linux) - Tauri v2.
//! HTTP is sent from Rust, so there is no CORS. State lives in JSON files.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    error::Error as _,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;

const DATA_FILE: &str = "workbench-data.json";
const RESP_FILE: &str = "workbench-data.responses.json";
const CONFIG_FILE: &str = "workbench-config.json";

/* ------------------------------ HTTP ------------------------------ */

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpPayload {
    method: String,
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    body: Option<String>,
    body_base64: Option<String>,
    multipart: Option<Vec<Part>>,
    timeout_ms: Option<u64>,
    #[serde(default)]
    insecure_tls: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Part {
    name: String,
    filename: Option<String>,
    content_type: Option<String>,
    data_base64: Option<String>,
    value: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpResult {
    status: u16,
    status_text: String,
    headers: HashMap<String, String>,
    body_text: String,
    time: u128,
    size: usize,
}

fn describe(e: &reqwest::Error) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(c) = src {
        s.push_str(": ");
        s.push_str(&c.to_string());
        src = c.source();
    }
    s
}

#[tauri::command]
pub async fn http_request(payload: HttpPayload) -> Result<HttpResult, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(payload.timeout_ms.unwrap_or(60_000)))
        .danger_accept_invalid_certs(payload.insecure_tls)
        .build()
        .map_err(|e| e.to_string())?;

    let method = reqwest::Method::from_bytes(payload.method.to_uppercase().as_bytes())
        .map_err(|e| format!("Invalid method: {e}"))?;
    let mut req = client.request(method.clone(), &payload.url);

    let mut has_ua = false;
    for (k, v) in &payload.headers {
        has_ua |= k.eq_ignore_ascii_case("user-agent");
        req = req.header(k.as_str(), v.as_str());
    }
    if !has_ua {
        req = req.header(
            "User-Agent",
            concat!("APIworkbench/", env!("CARGO_PKG_VERSION")),
        );
    }

    if !matches!(method, reqwest::Method::GET | reqwest::Method::HEAD) {
        if let Some(parts) = payload.multipart.filter(|p| !p.is_empty()) {
            let mut form = reqwest::multipart::Form::new();
            for p in parts {
                if let Some(b64) = p.data_base64 {
                    let bytes = B64.decode(b64).map_err(|e| e.to_string())?;
                    let mut part = reqwest::multipart::Part::bytes(bytes)
                        .file_name(p.filename.unwrap_or_else(|| "file".into()));
                    if let Some(ct) = p.content_type {
                        part = part.mime_str(&ct).map_err(|e| e.to_string())?;
                    }
                    form = form.part(p.name, part);
                } else {
                    form = form.text(p.name, p.value.unwrap_or_default());
                }
            }
            req = req.multipart(form);
        } else if let Some(b64) = payload.body_base64 {
            req = req.body(B64.decode(b64).map_err(|e| e.to_string())?);
        } else if let Some(b) = payload.body {
            req = req.body(b);
        }
    }

    let t0 = Instant::now();
    let res = req.send().await.map_err(|e| describe(&e))?;
    let status = res.status();
    let mut headers: HashMap<String, String> = HashMap::new();
    for (k, v) in res.headers() {
        let v = String::from_utf8_lossy(v.as_bytes()).into_owned();
        headers
            .entry(k.as_str().to_string())
            .and_modify(|e| {
                e.push_str(", ");
                e.push_str(&v);
            })
            .or_insert(v);
    }
    let bytes = res.bytes().await.map_err(|e| describe(&e))?;

    Ok(HttpResult {
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or("").to_string(),
        headers,
        body_text: String::from_utf8_lossy(&bytes).into_owned(),
        time: t0.elapsed().as_millis(),
        size: bytes.len(),
    })
}

/* ------------------------- State storage -------------------------- */

fn default_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|e| e.to_string())
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join(CONFIG_FILE))
}

/// Active data folder (+ a warning if a custom folder is configured but missing).
fn data_dir(app: &AppHandle) -> Result<(PathBuf, Option<String>), String> {
    if let Ok(raw) = fs::read_to_string(config_path(app)?) {
        if let Some(d) = serde_json::from_str::<Value>(&raw)
            .ok()
            .and_then(|v| v.get("dataDir").and_then(|d| d.as_str().map(String::from)))
        {
            let p = PathBuf::from(&d);
            return if p.is_dir() {
                Ok((p, None))
            } else {
                Ok((
                    default_dir(app)?,
                    Some(format!(
                        "Data folder not found: {d}. Using the default location."
                    )),
                ))
            };
        }
    }
    Ok((default_dir(app)?, None))
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn load_from(dir: &Path) -> Result<Option<Value>, String> {
    let main = dir.join(DATA_FILE);
    if !main.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&main).map_err(|e| e.to_string())?;
    let mut v: Value = serde_json::from_str(&raw)
        .map_err(|e| format!("{} is not valid JSON: {e}", main.display()))?;
    if let Some(r) = fs::read_to_string(dir.join(RESP_FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    {
        if let Some(o) = v.as_object_mut() {
            o.insert("savedResponses".into(), r);
        }
    }
    Ok(Some(v))
}

fn save_to(dir: &Path, mut data: Value) -> Result<(), String> {
    let responses = data
        .as_object_mut()
        .and_then(|o| o.remove("savedResponses"))
        .unwrap_or_else(|| json!([]));
    write_atomic(&dir.join(RESP_FILE), &responses.to_string())?;
    write_atomic(
        &dir.join(DATA_FILE),
        &serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?,
    )
}

#[tauri::command]
pub fn state_load(app: AppHandle) -> Result<Value, String> {
    let (dir, warning) = data_dir(&app)?;
    let data = load_from(&dir)?;
    Ok(json!({
        "data": data,
        "file": dir.join(DATA_FILE).to_string_lossy(),
        "warning": warning
    }))
}

#[tauri::command]
pub fn state_save(app: AppHandle, data: Value) -> Result<Value, String> {
    let (dir, _) = data_dir(&app)?;
    save_to(&dir, data)?;
    Ok(json!({ "file": dir.join(DATA_FILE).to_string_lossy() }))
}

/// "Change folder…" (reset = false) or "Use default location" (reset = true).
/// Uses a non-async command so the blocking folder dialog cannot deadlock the
/// async runtime.
#[tauri::command]
pub fn storage_set(app: AppHandle, reset: bool, data: Value) -> Result<Value, String> {
    let target = if reset {
        default_dir(&app)?
    } else {
        match app.dialog().file().blocking_pick_folder() {
            Some(fp) => fp.into_path().map_err(|e| e.to_string())?,
            None => return Ok(json!({ "cancelled": true })),
        }
    };
    // Ensure the chosen folder exists (pick_folder returns an existing dir,
    // but default_dir might not have been created yet).
    fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let existed = target.join(DATA_FILE).exists();
    if !existed {
        save_to(&target, data)?;
    }
    let cfg = if reset {
        json!({})
    } else {
        json!({ "dataDir": target.to_string_lossy() })
    };
    write_atomic(&config_path(&app)?, &cfg.to_string())?;
    Ok(json!({
        "cancelled": false,
        "existed": existed,
        "file": target.join(DATA_FILE).to_string_lossy()
    }))
}

#[tauri::command]
pub fn reveal_data_file(app: AppHandle) -> Result<(), String> {
    let (dir, _) = data_dir(&app)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(DATA_FILE);
    #[cfg(target_os = "windows")]
    {
        let arg = if file.exists() {
            format!("/select,{}", file.display())
        } else {
            dir.display().to_string()
        };
        std::process::Command::new("explorer")
            .arg(arg)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        let mut c = std::process::Command::new("open");
        if file.exists() {
            c.arg("-R").arg(&file);
        } else {
            c.arg(&dir);
        }
        c.spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/* ------------------------ Native "Save as" ------------------------ */

/// Non-async so the blocking Save dialog cannot deadlock the async runtime.
#[tauri::command]
pub fn save_text_file(
    app: AppHandle,
    filename: String,
    contents: String,
) -> Result<Option<String>, String> {
    let Some(fp) = app
        .dialog()
        .file()
        .set_file_name(&filename)
        .add_filter("JSON", &["json"])
        .add_filter("All files", &["*"])
        .blocking_save_file()
    else {
        return Ok(None); // user cancelled
    };
    let path = fp.into_path().map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(&path, contents).map_err(|e| e.to_string())?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/* --------------------------- Clipboard ---------------------------- */

#[tauri::command]
pub fn clipboard_write(app: AppHandle, text: String) -> Result<(), String> {
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clipboard_read(app: AppHandle) -> Result<String, String> {
    app.clipboard().read_text().map_err(|e| e.to_string())
}