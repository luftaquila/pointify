use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{Event, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

use crate::credentials::{ClaudeAccount, ClaudeOrg, SharedCredentials};

#[derive(Serialize, Deserialize, Clone)]
pub struct ClaudeUsageEntry {
    #[serde(default)]
    pub utilization: f64,
    #[serde(default)]
    pub resets_at: Option<String>,
}

/// Per (account, org) cache: map `"{account_id}:{org_id}"` → (fetch time, bucket→entry).
/// A key absent from the map means that pair hasn't been fetched yet.
pub type ClaudeCache = Arc<Mutex<HashMap<String, (Instant, HashMap<String, ClaudeUsageEntry>)>>>;

/// Compose the cache key used everywhere (poller, Tauri command, serial_loop).
pub fn cache_key(account_id: &str, org_id: &str) -> String {
    format!("{}:{}", account_id, org_id)
}
pub type SharedClaudeCodeStats = Arc<Mutex<Option<ClaudeCodeStats>>>;

/// Newtype wrapper so Tauri's managed-state registry can distinguish this
/// from other `Arc<AtomicU64>` values (e.g. `SharedInterval`).
#[derive(Clone)]
pub struct ClaudeTtl(pub Arc<AtomicU64>);

const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:147.0) Gecko/20100101 Firefox/147.0";

fn build_cookie(session_key: &str, cf_clearance: &str) -> String {
    format!("sessionKey={}; cf_clearance={}", session_key, cf_clearance)
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

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OrgMembership {
    pub uuid: String,
    pub name: String,
    pub plan: Option<String>,
}

/// Infer a Claude plan label from a membership payload. Only covers cases
/// we've verified against live `/api/account` responses; anything else
/// returns None so the UI stays honest instead of guessing.
///
/// Verified signals:
///   - `membership.seat_tier == "team_tier_1"` → Team Premium seat
///   - org `capabilities` includes `claude_pro` → Pro
///   - org `capabilities == ["chat"]` with no seat → Free
fn infer_plan(org: &serde_json::Value, seat_tier: Option<&str>) -> Option<String> {
    if seat_tier == Some("team_tier_1") {
        return Some("Team Premium".to_string());
    }
    let caps: Vec<&str> = org
        .get("capabilities")
        .and_then(|c| c.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    if caps.contains(&"claude_pro") {
        return Some("Pro".to_string());
    }
    if seat_tier.is_none() && caps.len() == 1 && caps[0] == "chat" {
        return Some("Free".to_string());
    }
    None
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub memberships: Vec<OrgMembership>,
}

/// Fetch `/api/account` — primary identity + organization memberships.
pub async fn fetch_account_info(
    client: &reqwest::Client,
    session_key: &str,
    cf_clearance: &str,
) -> Result<AccountInfo, String> {
    let cookie = build_cookie(session_key, cf_clearance);
    let resp = request_builder(client, "https://claude.ai/api/account", &cookie)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let email = v
        .get("email_address")
        .and_then(|x| x.as_str())
        .map(String::from);
    let display_name = v
        .get("display_name")
        .and_then(|x| x.as_str())
        .map(String::from)
        .or_else(|| {
            v.get("full_name")
                .and_then(|x| x.as_str())
                .map(String::from)
        });
    let memberships = v
        .get("memberships")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let org = m.get("organization")?;
                    // Only keep orgs with the `chat` capability — these are the
                    // claude.ai workspaces the web UI surfaces. API-only orgs
                    // (`capabilities: ["api"]`, console.anthropic.com) appear
                    // in memberships but the /organizations/{id}/usage endpoint
                    // rejects them with 403.
                    let has_chat = org
                        .get("capabilities")
                        .and_then(|c| c.as_array())
                        .map(|a| a.iter().any(|c| c.as_str() == Some("chat")))
                        .unwrap_or(false);
                    if !has_chat {
                        return None;
                    }
                    let seat_tier = m.get("seat_tier").and_then(|v| v.as_str());
                    Some(OrgMembership {
                        uuid: org.get("uuid")?.as_str()?.to_string(),
                        name: org
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string(),
                        plan: infer_plan(org, seat_tier),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(AccountInfo {
        email,
        display_name,
        memberships,
    })
}

pub async fn fetch_usage(
    client: &reqwest::Client,
    session_key: &str,
    cf_clearance: &str,
    org_id: &str,
) -> Result<HashMap<String, ClaudeUsageEntry>, String> {
    let cookie = build_cookie(session_key, cf_clearance);
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
        if key == "extra_usage" {
            if let Ok(eu) = serde_json::from_value::<ExtraUsageRaw>(value) {
                if eu.is_enabled && eu.monthly_limit > 0.0 {
                    let util = eu
                        .utilization
                        .unwrap_or(eu.used_credits / eu.monthly_limit * 100.0);
                    result.insert(
                        key,
                        ClaudeUsageEntry {
                            utilization: util,
                            resets_at: None,
                        },
                    );
                }
            }
            continue;
        }
        if let Ok(entry) = serde_json::from_value::<ClaudeUsageEntry>(value) {
            result.insert(key, entry);
        }
    }
    Ok(result)
}

#[derive(Deserialize)]
struct ExtraUsageRaw {
    #[serde(default)]
    is_enabled: bool,
    #[serde(default)]
    monthly_limit: f64,
    #[serde(default)]
    used_credits: f64,
    #[serde(default)]
    utilization: Option<f64>,
}

/// Poll Claude usage for every (account, org) pair. Per-pair failures keep
/// the previous cache entry; identity and org list are filled lazily from
/// `/api/account` whenever an account has missing info.
pub fn start_usage_poller(
    app: tauri::AppHandle,
    cache: ClaudeCache,
    ttl: ClaudeTtl,
    credentials: SharedCredentials,
) {
    std::thread::spawn(move || {
        let client = reqwest::Client::new();
        // Force a fresh /api/account call for every account on the first loop
        // iteration so newly-added memberships (or legacy single-org accounts
        // migrated from .claude.env) discover all orgs immediately on startup.
        let mut force_identity_refresh = true;
        loop {
            let ttl_secs = ttl.0.load(Ordering::Relaxed).max(10);

            // Snapshot the current account list
            let mut accounts: Vec<ClaudeAccount> = credentials
                .lock()
                .map(|s| s.claude.clone())
                .unwrap_or_default();

            // Lazy identity/org fill (runs once per tick for accounts that need it).
            // Mutates both our working snapshot AND the shared store on disk.
            for acct in accounts.iter_mut() {
                if acct.session_key.is_empty() || acct.cf_clearance.is_empty() {
                    continue;
                }
                let needs_identity = force_identity_refresh
                    || acct.email.is_none()
                    || acct.display_name.is_none()
                    || acct.orgs.is_empty()
                    || acct.orgs.iter().any(|o| o.name.is_empty());
                if !needs_identity {
                    continue;
                }
                let info = tauri::async_runtime::block_on(fetch_account_info(
                    &client,
                    &acct.session_key,
                    &acct.cf_clearance,
                ));
                let Ok(info) = info else { continue };
                acct.email = info.email.clone();
                acct.display_name = info.display_name.clone();
                acct.orgs = info
                    .memberships
                    .iter()
                    .map(|m| ClaudeOrg {
                        uuid: m.uuid.clone(),
                        name: m.name.clone(),
                        plan: m.plan.clone(),
                    })
                    .collect();
                if let (Ok(dir), Ok(mut store)) = (app.path().app_config_dir(), credentials.lock()) {
                    if let Some(stored) = store.claude.iter_mut().find(|a| a.id == acct.id) {
                        stored.email = acct.email.clone();
                        stored.display_name = acct.display_name.clone();
                        stored.orgs = acct.orgs.clone();
                        let snapshot = store.clone();
                        drop(store);
                        let _ = crate::credentials::save_store(&dir, &snapshot);
                        let _ = app.emit("credentials-changed", ());
                    }
                }
            }

            // Prune cache entries for pairs that no longer exist
            let live_keys: HashSet<String> = accounts
                .iter()
                .flat_map(|a| {
                    let aid = a.id.clone();
                    a.orgs.iter().map(move |o| cache_key(&aid, &o.uuid))
                })
                .collect();
            if let Ok(mut c) = cache.lock() {
                c.retain(|k, _| live_keys.contains(k));
            }

            for acct in &accounts {
                if acct.session_key.is_empty() || acct.cf_clearance.is_empty() {
                    continue;
                }
                for org in &acct.orgs {
                    if org.uuid.is_empty() {
                        continue;
                    }
                    let key = cache_key(&acct.id, &org.uuid);
                    let stale = cache
                        .lock()
                        .ok()
                        .and_then(|c| {
                            c.get(&key)
                                .map(|(t, _)| t.elapsed() >= Duration::from_secs(ttl_secs))
                        })
                        .unwrap_or(true);
                    if !stale {
                        continue;
                    }

                    let result: Result<HashMap<String, ClaudeUsageEntry>, String> =
                        tauri::async_runtime::block_on(fetch_usage(
                            &client,
                            &acct.session_key,
                            &acct.cf_clearance,
                            &org.uuid,
                        ));
                    match result {
                        Ok(data) => {
                            if let Ok(mut c) = cache.lock() {
                                c.insert(key, (Instant::now(), data));
                            }
                        }
                        Err(e) => eprintln!(
                            "Claude usage poll failed for {}/{}: {}",
                            acct.id, org.uuid, e
                        ),
                    }
                }
            }

            force_identity_refresh = false;
            std::thread::sleep(Duration::from_secs(5));
        }
    });
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

fn is_today(timestamp: &str) -> bool {
    let today = chrono::Local::now().date_naive();
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(timestamp) {
        return dt.with_timezone(&chrono::Local).date_naive() == today;
    }
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
                match entry.timestamp.as_deref() {
                    Some(ts) if is_today(ts) => {}
                    _ => continue,
                }
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
            let stats = read_today_stats();
            if let Ok(mut lock) = shared_stats.lock() {
                *lock = Some(stats.clone());
            }
            let _ = app.emit("claude-code-stats-changed", stats);
            match rx.recv_timeout(poll_interval) {
                Ok(_) => {
                    std::thread::sleep(debounce);
                    while rx.try_recv().is_ok() {}
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
}
