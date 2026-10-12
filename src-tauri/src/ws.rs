//! WebSocket support for API Workbench (Tauri v2).
//! Connections live in Rust (no CORS, custom handshake headers, optional insecure TLS).
//! Events are sent to the webview as a "ws-event" event: { connId, event, ... }.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use futures_util::{SinkExt, StreamExt};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
    DigitallySignedStruct, SignatureScheme,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::{
    connect_async_tls_with_config,
    tungstenite::{
        client::IntoClientRequest,
        http::{HeaderName, HeaderValue},
        Error as WsError, Message,
    },
    Connector,
};

enum WsCmd {
    Send(String),
    Close,
}

type Senders = Arc<Mutex<HashMap<String, UnboundedSender<WsCmd>>>>;

#[derive(Default, Clone)]
pub struct WsState(Senders);

fn emit(app: &AppHandle, id: &str, event: &str, extra: Value) {
    let mut p = json!({ "connId": id, "event": event });
    if let (Some(o), Some(e)) = (p.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            o.insert(k.clone(), v.clone());
        }
    }
    // Visible in the terminal running `tauri dev` - helps tell Rust-side
    // problems apart from webview-side (permission / listener) problems.
    eprintln!("[ws] {id} -> {event}");
    if let Err(e) = app.emit("ws-event", p) {
        eprintln!("[ws] emit failed: {e}");
    }
}

/* ------------------------------ TLS ------------------------------- */

#[derive(Debug)]
struct NoVerify(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Built explicitly with the `ring` provider so no process-wide default is needed.
/// Returns an error instead of panicking, so a failure is reported to the UI
/// instead of silently killing the connection task.
fn tls_config(insecure: bool) -> Result<Arc<rustls::ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?;
    let cfg = if insecure {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
            .with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        builder.with_root_certificates(roots).with_no_client_auth()
    };
    Ok(Arc::new(cfg))
}

/* ---------------------------- Session ----------------------------- */

/// Runs one connection. Emits open / message / error events itself and
/// returns the (code, reason) to report in the final "close" event.
async fn session(
    app: &AppHandle,
    id: &str,
    url: String,
    headers: HashMap<String, String>,
    protocols: Vec<String>,
    insecure: bool,
    rx: &mut UnboundedReceiver<WsCmd>,
) -> (u16, String) {
    let fail = |msg: String| {
        emit(app, id, "error", json!({ "message": msg }));
        (1006u16, String::new())
    };

    let mut req = match url.as_str().into_client_request() {
        Ok(r) => r,
        Err(e) => return fail(e.to_string()),
    };
    for (k, v) in &headers {
        match (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v)) {
            (Ok(n), Ok(val)) => {
                req.headers_mut().insert(n, val);
            }
            _ => return fail(format!("Invalid header: {k}")),
        }
    }
    if !protocols.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&protocols.join(", ")) {
            req.headers_mut().insert("sec-websocket-protocol", v);
        }
    }

    let connector = match tls_config(insecure) {
        Ok(c) => Connector::Rustls(c),
        Err(e) => return fail(format!("TLS setup failed: {e}")),
    };
    let connected = tokio::time::timeout(
        Duration::from_secs(30),
        connect_async_tls_with_config(req, None, false, Some(connector)),
    )
    .await;
    let (ws, resp) = match connected {
        Err(_) => return fail("Connection timed out".into()),
        Ok(Err(WsError::Http(r))) => {
            return fail(format!("Handshake rejected: HTTP {}", r.status()));
        }
        Ok(Err(e)) => return fail(e.to_string()),
        Ok(Ok(v)) => v,
    };

    // Handshake response headers -> shown in Response > Headers
    let mut hdrs: HashMap<String, String> = HashMap::new();
    for (k, v) in resp.headers() {
        let v = String::from_utf8_lossy(v.as_bytes()).into_owned();
        hdrs.entry(k.as_str().to_string())
            .and_modify(|e| {
                e.push_str(", ");
                e.push_str(&v);
            })
            .or_insert(v);
    }
    let protocol = hdrs.get("sec-websocket-protocol").cloned().unwrap_or_default();
    emit(
        app,
        id,
        "open",
        json!({
            "protocol": protocol,
            "status": resp.status().as_u16(),
            "statusText": resp.status().canonical_reason().unwrap_or(""),
            "headers": hdrs
        }),
    );

    let (mut write, mut read) = ws.split();
    loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(WsCmd::Send(s)) => {
                    if let Err(e) = write.send(Message::text(s)).await {
                        emit(app, id, "error", json!({ "message": e.to_string() }));
                    }
                }
                Some(WsCmd::Close) | None => {
                    let _ = write.send(Message::Close(None)).await;
                    // give the server a moment to answer with its own Close frame
                    let _ = tokio::time::timeout(Duration::from_secs(3), async {
                        while let Some(m) = read.next().await {
                            if matches!(m, Ok(Message::Close(_)) | Err(_)) { break; }
                        }
                    }).await;
                    return (1000, String::new());
                }
            },
            msg = read.next() => match msg {
                Some(Ok(Message::Text(t))) => emit(
                    app, id, "message",
                    json!({ "binary": false, "size": t.len(), "data": t.to_string() }),
                ),
                Some(Ok(Message::Binary(b))) => emit(
                    app, id, "message",
                    json!({ "binary": true, "size": b.len(), "data": B64.encode(&b) }),
                ),
                Some(Ok(Message::Close(frame))) => {
                    return frame
                        .map(|f| (u16::from(f.code), f.reason.to_string()))
                        .unwrap_or((1005, String::new()));
                }
                Some(Ok(_)) => {} // ping / pong are answered by the library
                Some(Err(e)) => {
                    emit(app, id, "error", json!({ "message": e.to_string() }));
                    return (1006, String::new());
                }
                None => return (1006, String::new()),
            },
        }
    }
}

async fn run(
    app: AppHandle,
    map: Senders,
    id: String,
    url: String,
    headers: HashMap<String, String>,
    protocols: Vec<String>,
    insecure: bool,
    mut rx: UnboundedReceiver<WsCmd>,
) {
    let (code, reason) = session(&app, &id, url, headers, protocols, insecure, &mut rx).await;
    // Only remove our own entry: a reconnect with the same id may have replaced it.
    if let Ok(mut g) = map.lock() {
        g.remove(&id);
    }
    emit(&app, &id, "close", json!({ "code": code, "reason": reason }));
}

/* ----------------------------- Commands --------------------------- */

#[tauri::command]
pub fn ws_connect(
    app: AppHandle,
    state: State<'_, WsState>,
    conn_id: String,
    url: String,
    headers: HashMap<String, String>,
    protocols: Vec<String>,
    insecure_tls: bool,
) -> Result<(), String> {
    eprintln!("[ws] connect {conn_id} {url}");
    let (tx, rx) = unbounded_channel();
    let map = state.0.clone();
    if let Some(old) = map.lock().map_err(|e| e.to_string())?.insert(conn_id.clone(), tx) {
        let _ = old.send(WsCmd::Close);
    }
    tauri::async_runtime::spawn(run(
        app, map, conn_id, url, headers, protocols, insecure_tls, rx,
    ));
    Ok(())
}

#[tauri::command]
pub fn ws_send(state: State<'_, WsState>, conn_id: String, data: String) -> Result<(), String> {
    let g = state.0.lock().map_err(|e| e.to_string())?;
    g.get(&conn_id)
        .ok_or_else(|| "Not connected".to_string())?
        .send(WsCmd::Send(data))
        .map_err(|_| "Not connected".to_string())
}

#[tauri::command]
pub fn ws_close(state: State<'_, WsState>, conn_id: String) -> Result<(), String> {
    if let Ok(g) = state.0.lock() {
        if let Some(tx) = g.get(&conn_id) {
            let _ = tx.send(WsCmd::Close);
        }
    }
    Ok(())
}