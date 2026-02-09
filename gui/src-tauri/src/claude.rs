use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use notify::{Event, EventKind, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

#[derive(Clone, Default)]
pub struct ClaudeCredentials {
    pub session_key: String,
    pub cf_clearance: String,
    pub org_id: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ClaudeUsageEntry {
    #[serde(default)]
    pub utilization: f64,
    #[serde(default)]
    pub resets_at: Option<String>,
}

pub type ClaudeCache = Arc<Mutex<Option<(Instant, HashMap<String, ClaudeUsageEntry>)>>>;

const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:147.0) Gecko/20100101 Firefox/147.0";

fn env_path(config_dir: &Path) -> PathBuf {
    config_dir.join(".claude.env")
}

const ENV_TEMPLATE: &str = "\
SESSION_KEY=
CF_CLEARANCE=
ORG_ID=
";

pub fn load_credentials(config_dir: &Path) -> Option<ClaudeCredentials> {
    let content = fs::read_to_string(env_path(config_dir)).ok()?;
    let mut creds = ClaudeCredentials::default();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            match key {
                "SESSION_KEY" => creds.session_key = value.to_string(),
                "CF_CLEARANCE" => creds.cf_clearance = value.to_string(),
                "ORG_ID" => creds.org_id = value.to_string(),
                _ => {}
            }
        }
    }
    Some(creds)
}

pub fn save_credentials(config_dir: &Path, creds: &ClaudeCredentials) {
    let _ = fs::create_dir_all(config_dir);
    let content = format!(
        "SESSION_KEY={}\nCF_CLEARANCE={}\nORG_ID={}\n",
        creds.session_key, creds.cf_clearance, creds.org_id
    );
    let _ = fs::write(env_path(config_dir), content);
}

/// Ensure .claude.env exists (create with template if missing) and return its path.
pub fn ensure_env_file(config_dir: &Path) -> PathBuf {
    let path = env_path(config_dir);
    if !path.exists() {
        let _ = fs::create_dir_all(config_dir);
        let _ = fs::write(&path, ENV_TEMPLATE);
    }
    path
}

/// Watch .claude.env for changes. On modification, clear the cache and emit an event.
pub fn start_watching(app: tauri::AppHandle, cache: ClaudeCache) {
    let config_dir = match app.path().app_config_dir() {
        Ok(d) => d,
        Err(_) => return,
    };

    let env_file = ensure_env_file(&config_dir);

    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel::<notify::Result<Event>>();

        let mut watcher = match notify::recommended_watcher(tx) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("Failed to create file watcher: {}", e);
                return;
            }
        };

        // Watch the config directory (covers file creation/replacement)
        if let Err(e) = watcher.watch(config_dir.as_path(), RecursiveMode::NonRecursive) {
            eprintln!("Failed to watch config dir: {}", e);
            return;
        }

        for result in rx {
            match result {
                Ok(event) => {
                    let dominated =
                        matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_));
                    if dominated && event.paths.iter().any(|p| p == &env_file) {
                        if let Ok(mut c) = cache.lock() {
                            *c = None;
                        }
                        let _ = app.emit("claude-env-changed", ());
                    }
                }
                Err(e) => eprintln!("File watch error: {}", e),
            }
        }
    });
}

fn build_cookie(creds: &ClaudeCredentials) -> String {
    format!(
        "sessionKey={}; cf_clearance={}",
        creds.session_key, creds.cf_clearance
    )
}

fn request_builder(client: &reqwest::Client, url: &str, cookie: &str) -> reqwest::RequestBuilder {
    client
        .get(url)
        .header("User-Agent", USER_AGENT)
        .header("Cookie", cookie)
        .header("Accept", "*/*")
        .header("content-type", "application/json")
        .header("Referer", "https://claude.ai/settings/usage")
}

pub async fn fetch_org_id(
    client: &reqwest::Client,
    creds: &ClaudeCredentials,
) -> Result<String, String> {
    let cookie = build_cookie(creds);
    let resp = request_builder(client, "https://claude.ai/api/organizations", &cookie)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let orgs: Vec<serde_json::Value> = resp.json().await.map_err(|e| e.to_string())?;
    orgs.first()
        .and_then(|o| o.get("uuid"))
        .and_then(|u| u.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "No organization found".to_string())
}

pub async fn fetch_usage(
    client: &reqwest::Client,
    creds: &ClaudeCredentials,
    org_id: &str,
) -> Result<HashMap<String, ClaudeUsageEntry>, String> {
    let cookie = build_cookie(creds);
    let url = format!("https://claude.ai/api/organizations/{}/usage", org_id);
    let resp = request_builder(client, &url, &cookie)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let data: HashMap<String, serde_json::Value> = resp.json().await.map_err(|e| e.to_string())?;

    let mut result = HashMap::new();
    for (key, value) in data {
        if let Ok(entry) = serde_json::from_value::<ClaudeUsageEntry>(value) {
            result.insert(key, entry);
        }
    }
    Ok(result)
}
