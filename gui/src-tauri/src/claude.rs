use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::BufRead;
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
pub type SharedClaudeCodeStats = Arc<Mutex<Option<ClaudeCodeStats>>>;

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

// ── Claude Code JSONL usage (ccusage approach) ──

#[derive(Deserialize)]
struct JsonlEntry {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    message: Option<JsonlMessage>,
    #[serde(default, rename = "costUSD")]
    cost_usd: Option<f64>,
}

#[derive(Deserialize)]
struct JsonlMessage {
    #[serde(default)]
    usage: Option<JsonlUsage>,
    #[serde(default)]
    id: Option<String>,
}

#[derive(Deserialize)]
struct JsonlUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

#[derive(Serialize, Clone)]
pub struct ClaudeCodeStats {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
    pub total_cost: f64,
}

/// Return directories that may contain Claude Code JSONL conversation logs.
fn jsonl_project_dirs() -> Vec<PathBuf> {
    let mut result = Vec::new();
    if let Some(home) = dirs::home_dir() {
        result.push(home.join(".config").join("claude").join("projects"));
        result.push(home.join(".claude").join("projects"));
    }
    if let Ok(custom) = std::env::var("CLAUDE_CONFIG_DIR") {
        for dir in custom.split(',') {
            let path = PathBuf::from(dir.trim()).join("projects");
            if !result.contains(&path) {
                result.push(path);
            }
        }
    }
    result
}

/// Check whether an ISO-8601 timestamp falls on today (local time).
fn is_today(timestamp: &str) -> bool {
    let today = chrono::Local::now().date_naive();
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(timestamp) {
        return dt.with_timezone(&chrono::Local).date_naive() == today;
    }
    // Fallback: compare date prefix
    let today_str = today.format("%Y-%m-%d").to_string();
    timestamp.starts_with(&today_str)
}

fn read_today_stats() -> ClaudeCodeStats {
    let today_naive = chrono::Local::now().date_naive();

    let mut stats = ClaudeCodeStats {
        input_tokens: 0,
        output_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: 0,
        total_cost: 0.0,
    };

    let mut seen = HashSet::new();

    for dir in jsonl_project_dirs() {
        let pattern = match dir.join("**/*.jsonl").to_str() {
            Some(p) => p.to_string(),
            None => continue,
        };

        let entries = match glob::glob(&pattern) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for path in entries.flatten() {
            // Skip files not modified today
            if let Ok(meta) = path.metadata() {
                if let Ok(modified) = meta.modified() {
                    let modified: chrono::DateTime<chrono::Local> = modified.into();
                    if modified.date_naive() < today_naive {
                        continue;
                    }
                }
            }

            let file = match fs::File::open(&path) {
                Ok(f) => f,
                Err(_) => continue,
            };

            for line in std::io::BufReader::new(file).lines() {
                let line = match line {
                    Ok(l) => l,
                    Err(_) => continue,
                };
                if line.trim().is_empty() {
                    continue;
                }

                let entry: JsonlEntry = match serde_json::from_str(&line) {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                // Filter by today
                match entry.timestamp.as_deref() {
                    Some(ts) if is_today(ts) => {}
                    _ => continue,
                }

                // Deduplicate by message id
                if let Some(ref msg) = entry.message {
                    if let Some(ref id) = msg.id {
                        if !seen.insert(id.clone()) {
                            continue;
                        }
                    }

                    if let Some(ref usage) = msg.usage {
                        stats.input_tokens += usage.input_tokens;
                        stats.output_tokens += usage.output_tokens;
                        stats.cache_creation_tokens += usage.cache_creation_input_tokens;
                        stats.cache_read_tokens += usage.cache_read_input_tokens;
                    }
                }

                if let Some(cost) = entry.cost_usd {
                    stats.total_cost += cost;
                }
            }
        }
    }

    stats.total_tokens =
        stats.input_tokens + stats.output_tokens + stats.cache_creation_tokens + stats.cache_read_tokens;

    stats
}

/// Watch Claude Code JSONL directories for changes and emit stats to the frontend.
/// Also re-emits periodically (every 30s) to handle frontend reloads.
pub fn start_watching_stats(app: tauri::AppHandle, shared_stats: SharedClaudeCodeStats) {
    std::thread::spawn(move || {
        let dirs = jsonl_project_dirs();

        let (tx, rx) = std::sync::mpsc::channel::<notify::Result<Event>>();

        let mut watcher = match notify::recommended_watcher(tx) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("Failed to create stats watcher: {}", e);
                return;
            }
        };

        for dir in &dirs {
            if dir.exists() {
                let _ = watcher.watch(dir, RecursiveMode::Recursive);
            }
        }

        let poll_interval = std::time::Duration::from_secs(30);
        let debounce = std::time::Duration::from_secs(2);

        loop {
            // Emit current stats
            let stats = read_today_stats();
            if let Ok(mut lock) = shared_stats.lock() {
                *lock = Some(stats.clone());
            }
            let _ = app.emit("claude-code-stats-changed", stats);

            // Wait for file change or timeout for periodic refresh
            match rx.recv_timeout(poll_interval) {
                Ok(_) => {
                    // File changed — drain burst then debounce
                    std::thread::sleep(debounce);
                    while rx.try_recv().is_ok() {}
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
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
