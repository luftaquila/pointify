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
pub struct CodexUsage {
    pub primary: Option<CodexUsageWindow>,
    pub secondary: Option<CodexUsageWindow>,
}

pub type CodexCache = Arc<Mutex<Option<(Instant, CodexUsage)>>>;

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
pub fn start_watching(app: tauri::AppHandle, cache: CodexCache) {
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
pub fn start_usage_poller(app: tauri::AppHandle, cache: CodexCache, ttl: ClaudeTtl) {
    std::thread::spawn(move || {
        let client = reqwest::Client::new();
        loop {
            let ttl_secs = ttl.0.load(Ordering::Relaxed).max(10);

            let stale = match cache.lock() {
                Ok(c) => c
                    .as_ref()
                    .map(|(t, _)| t.elapsed() >= Duration::from_secs(ttl_secs))
                    .unwrap_or(true),
                Err(_) => false,
            };

            if stale {
                let result: Result<CodexUsage, String> =
                    tauri::async_runtime::block_on(fetch_usage(&client));
                match result {
                    Ok(data) => {
                        if let Ok(mut c) = cache.lock() {
                            *c = Some((Instant::now(), data));
                        }
                        let _ = app.emit("codex-usage-updated", ());
                    }
                    Err(e) => eprintln!("Codex usage poll failed: {}", e),
                }
            }

            std::thread::sleep(Duration::from_secs(5));
        }
    });
}
