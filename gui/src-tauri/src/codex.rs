use std::fs;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::Emitter;

use crate::claude::ClaudeTtl;

#[derive(Serialize, Clone)]
pub struct CodexUsageWindow {
    pub used_percent: f64,
    pub reset_at: i64,
}

#[derive(Serialize, Clone, Default)]
pub struct CodexIdentity {
    pub email: Option<String>,
    pub name: Option<String>,
    pub plan_type: Option<String>,
}

#[derive(Serialize, Clone, Default)]
pub struct CodexUsage {
    pub primary: Option<CodexUsageWindow>,
    pub secondary: Option<CodexUsageWindow>,
}

pub type CodexCache = Arc<Mutex<Option<(Instant, CodexUsage)>>>;

#[derive(Default)]
pub struct CodexStatus {
    /// When the poller last attempted a fetch (success or failure).
    /// Used to honor TTL on failures so we don't spam the endpoint.
    pub last_attempt: Option<Instant>,
    /// Most recent fetch error, cleared on success.
    pub error: Option<String>,
}

pub type CodexStatusState = Arc<Mutex<CodexStatus>>;

fn auth_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codex").join("auth.json"))
}

fn read_access_token() -> Option<String> {
    let path = auth_path()?;
    let file = fs::File::open(path).ok()?;
    let v: serde_json::Value = serde_json::from_reader(file).ok()?;
    v.get("tokens")?
        .get("access_token")?
        .as_str()
        .map(|s| s.to_string())
}

fn read_id_token() -> Option<String> {
    let path = auth_path()?;
    let file = fs::File::open(path).ok()?;
    let v: serde_json::Value = serde_json::from_reader(file).ok()?;
    v.get("tokens")?
        .get("id_token")?
        .as_str()
        .map(|s| s.to_string())
}

fn decode_jwt_claims(token: &str) -> Option<serde_json::Value> {
    use base64::Engine;
    let payload_b64 = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

/// Decode the ID token from ~/.codex/auth.json and return identity fields.
/// Returns None when auth.json is absent or malformed; ID token validity
/// (exp) is intentionally not checked — identity claims remain correct
/// even after the short-lived id_token expires.
pub fn read_identity() -> Option<CodexIdentity> {
    let token = read_id_token()?;
    let claims = decode_jwt_claims(&token)?;
    Some(CodexIdentity {
        email: claims.get("email").and_then(|v| v.as_str()).map(String::from),
        name: claims.get("name").and_then(|v| v.as_str()).map(String::from),
        plan_type: claims
            .get("https://api.openai.com/auth")
            .and_then(|v| v.get("chatgpt_plan_type"))
            .and_then(|v| v.as_str())
            .map(String::from),
    })
}

pub async fn fetch_usage(client: &reqwest::Client) -> Result<CodexUsage, String> {
    let token = read_access_token().ok_or_else(|| "codex auth.json not found".to_string())?;
    let resp = client
        .get("https://chatgpt.com/backend-api/wham/usage")
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let rl = body.get("rate_limit");

    let parse_window = |key: &str| -> Option<CodexUsageWindow> {
        let w = rl?.get(key)?;
        Some(CodexUsageWindow {
            used_percent: w.get("used_percent")?.as_f64()?,
            reset_at: w.get("reset_at")?.as_i64()?,
        })
    };

    Ok(CodexUsage {
        primary: parse_window("primary_window"),
        secondary: parse_window("secondary_window"),
    })
}

/// Watch ~/.codex/auth.json for changes (Codex CLI rewrites it on token refresh).
/// On modification, clear the cache and emit an event so the WebView drops stale data.
pub fn start_watching(app: tauri::AppHandle, cache: CodexCache, status: CodexStatusState) {
    let path = match auth_path() {
        Some(p) => p,
        None => return,
    };
    let dir = match path.parent() {
        Some(d) => d.to_path_buf(),
        None => return,
    };

    std::thread::spawn(move || {
        // Wait until the ~/.codex directory exists so notify::watch() doesn't fail permanently.
        while !dir.exists() {
            std::thread::sleep(Duration::from_secs(5));
        }

        let (tx, rx) = std::sync::mpsc::channel::<notify::Result<Event>>();

        let mut watcher = match notify::recommended_watcher(tx) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("Failed to create codex watcher: {}", e);
                return;
            }
        };

        if let Err(e) = watcher.watch(dir.as_path(), RecursiveMode::NonRecursive) {
            eprintln!("Failed to watch ~/.codex dir: {}", e);
            return;
        }

        for result in rx {
            match result {
                Ok(event) => {
                    let dominated =
                        matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_));
                    if dominated && event.paths.iter().any(|p| p == &path) {
                        if let Ok(mut c) = cache.lock() {
                            *c = None;
                        }
                        // Force the poller to retry immediately on the next
                        // tick instead of waiting out the previous TTL.
                        if let Ok(mut s) = status.lock() {
                            s.last_attempt = None;
                            s.error = None;
                        }
                        let _ = app.emit("codex-auth-changed", ());
                    }
                }
                Err(e) => eprintln!("Codex watch error: {}", e),
            }
        }
    });
}

/// Poll wham/usage in a background thread. Shares the Claude refresh TTL so
/// both integrations honor the same "Refresh" dropdown.
///
/// Failures honor the same TTL as successes so an expired access_token
/// doesn't cause us to hammer chatgpt.com every 5 seconds.
pub fn start_usage_poller(
    app: tauri::AppHandle,
    cache: CodexCache,
    status: CodexStatusState,
    ttl: ClaudeTtl,
) {
    std::thread::spawn(move || {
        let client = reqwest::Client::new();
        let mut last_logged_error: Option<String> = None;
        loop {
            let ttl_secs = ttl.0.load(Ordering::Relaxed).max(10);

            let due = match status.lock() {
                Ok(s) => s
                    .last_attempt
                    .map(|t| t.elapsed() >= Duration::from_secs(ttl_secs))
                    .unwrap_or(true),
                Err(_) => false,
            };

            if due {
                if let Ok(mut s) = status.lock() {
                    s.last_attempt = Some(Instant::now());
                }
                let result: Result<CodexUsage, String> =
                    tauri::async_runtime::block_on(fetch_usage(&client));
                match result {
                    Ok(data) => {
                        if let Ok(mut c) = cache.lock() {
                            *c = Some((Instant::now(), data));
                        }
                        if let Ok(mut s) = status.lock() {
                            s.error = None;
                        }
                        if last_logged_error.is_some() {
                            eprintln!("Codex usage poll recovered");
                            last_logged_error = None;
                        }
                        let _ = app.emit("codex-usage-updated", ());
                    }
                    Err(e) => {
                        // Only log when the error message changes — avoids
                        // the per-tick "401 Unauthorized" spam loop.
                        if last_logged_error.as_deref() != Some(e.as_str()) {
                            eprintln!("Codex usage poll failed: {}", e);
                            last_logged_error = Some(e.clone());
                        }
                        if let Ok(mut s) = status.lock() {
                            s.error = Some(e);
                        }
                        let _ = app.emit("codex-usage-updated", ());
                    }
                }
            }

            std::thread::sleep(Duration::from_secs(5));
        }
    });
}
