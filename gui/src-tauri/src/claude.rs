use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
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
///   - `membership.seat_tier == "team_standard"` → Team Standard seat
///   - `membership.seat_tier == "team_tier_1"` → Team Premium seat
///   - `organization.rate_limit_tier == "default_claude_max_20x"` → Max 20x
///   - org `capabilities` includes `claude_pro` → Pro
///   - org `capabilities == ["chat"]` with no seat → Free
fn infer_plan(org: &serde_json::Value, seat_tier: Option<&str>) -> Option<String> {
    if let Some(label) = seat_tier.and_then(plan_label_from_seat_tier) {
        return Some(label.to_string());
    }

    if let Some(label) = org_rate_limit_tier(org).and_then(plan_label_from_rate_limit_tier) {
        return Some(label.to_string());
    }

    let caps = org_capabilities(org);
    if caps.contains(&"claude_max") {
        return Some("Max".to_string());
    }
    if caps.contains(&"claude_pro") {
        return Some("Pro".to_string());
    }
    if seat_tier.is_none() && caps.len() == 1 && caps[0] == "chat" {
        return Some("Free".to_string());
    }
    None
}

fn plan_label_from_seat_tier(seat_tier: &str) -> Option<&'static str> {
    match seat_tier {
        "team_standard" => Some("Team Standard"),
        "team_tier_1" => Some("Team Premium"),
        _ => None,
    }
}

fn plan_label_from_rate_limit_tier(rate_limit_tier: &str) -> Option<&'static str> {
    match rate_limit_tier {
        "default_claude_max_20x" => Some("Max 20x"),
        _ => None,
    }
}

fn org_rate_limit_tier(org: &serde_json::Value) -> Option<&str> {
    org.get("rate_limit_tier").and_then(|v| v.as_str())
}

fn org_capabilities(org: &serde_json::Value) -> Vec<&str> {
    org.get("capabilities")
        .and_then(|c| c.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{infer_plan, plan_label_from_rate_limit_tier, plan_label_from_seat_tier};
    use serde_json::json;

    #[test]
    fn maps_known_account_api_seat_tiers() {
        let cases = [
            ("team_standard", "Team Standard"),
            ("team_tier_1", "Team Premium"),
        ];

        for (seat_tier, label) in cases {
            assert_eq!(plan_label_from_seat_tier(seat_tier), Some(label));
        }
    }

    #[test]
    fn ignores_unknown_account_api_seat_tiers() {
        assert_eq!(plan_label_from_seat_tier("enterprise_unknown"), None);
    }

    #[test]
    fn maps_known_account_api_rate_limit_tiers() {
        assert_eq!(
            plan_label_from_rate_limit_tier("default_claude_max_20x"),
            Some("Max 20x")
        );
    }

    #[test]
    fn infers_plan_from_seat_tier_before_capabilities() {
        let org = json!({ "capabilities": ["chat", "claude_pro"] });
        assert_eq!(
            infer_plan(&org, Some("team_standard")).as_deref(),
            Some("Team Standard")
        );
    }

    #[test]
    fn infers_max_20x_from_rate_limit_tier() {
        let org = json!({
            "capabilities": ["claude_max", "chat"],
            "rate_limit_tier": "default_claude_max_20x",
        });
        assert_eq!(infer_plan(&org, None).as_deref(), Some("Max 20x"));
    }

    #[test]
    fn infers_pro_from_claude_pro_capability() {
        let org = json!({ "capabilities": ["chat", "claude_pro"] });
        assert_eq!(infer_plan(&org, None).as_deref(), Some("Pro"));
    }
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

#[derive(Serialize, Clone, Default)]
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

fn is_on_day(timestamp: &str, day: chrono::NaiveDate) -> bool {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(timestamp) {
        return dt.with_timezone(&chrono::Local).date_naive() == day;
    }
    let day_str = day.format("%Y-%m-%d").to_string();
    timestamp.starts_with(&day_str)
}

/// Today's Claude Code usage, kept current by reading only what was appended to
/// each JSONL file since the previous pass. Claude Code only ever appends to
/// these files, so a file that shrank was rewritten and everything is recounted.
struct StatsTracker {
    day: chrono::NaiveDate,
    /// Bytes of each file (canonical path) already counted, always ending on a
    /// line boundary. Canonical so a file reached through a glob and through a
    /// watcher event, or through two symlinked project dirs, is read once.
    offsets: HashMap<PathBuf, u64>,
    /// Message ids already counted today; one message is logged several times.
    seen: HashSet<String>,
    stats: ClaudeCodeStats,
}

impl StatsTracker {
    fn new(day: chrono::NaiveDate) -> Self {
        Self {
            day,
            offsets: HashMap::new(),
            seen: HashSet::new(),
            stats: ClaudeCodeStats::default(),
        }
    }

    /// Bring the totals up to date. `changed` lists the files to read;
    /// `None` scans every project directory.
    fn refresh(&mut self, changed: Option<&HashSet<PathBuf>>) {
        let today = chrono::Local::now().date_naive();
        let consistent = if today != self.day {
            *self = Self::new(today);
            self.scan_all()
        } else {
            match changed {
                Some(paths) => paths
                    .iter()
                    .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
                    .all(|p| self.read_new(p)),
                None => self.scan_all(),
            }
        };
        if !consistent {
            *self = Self::new(today);
            self.scan_all();
        }
    }

    /// Read every JSONL file modified today. Returns false if a file shrank.
    fn scan_all(&mut self) -> bool {
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
                        if modified.date_naive() < self.day {
                            continue;
                        }
                    }
                }
                if !self.read_new(&path) {
                    return false;
                }
            }
        }
        true
    }

    /// Count the lines appended to `path` since the last read. Returns false
    /// if the file shrank, i.e. it was rewritten and the totals are stale.
    /// A file that can't be read (e.g. deleted) is skipped.
    fn read_new(&mut self, path: &Path) -> bool {
        let Ok(path) = fs::canonicalize(path) else {
            return true;
        };
        let Ok(mut file) = fs::File::open(&path) else {
            return true;
        };
        let Ok(len) = file.metadata().map(|m| m.len()) else {
            return true;
        };
        let offset = self.offsets.get(&path).copied().unwrap_or(0);
        if len < offset {
            return false;
        }
        if len == offset {
            return true;
        }

        let mut buf = Vec::new();
        if file.seek(SeekFrom::Start(offset)).is_err()
            || file.take(len - offset).read_to_end(&mut buf).is_err()
        {
            return true;
        }

        // Stop at the last newline: the line after it may still be being
        // written. Take it anyway if it already parses, so a final line
        // without a trailing newline is not lost.
        let mut consumed = buf.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        for line in buf[..consumed].split(|&b| b == b'\n') {
            self.count_line(line);
        }
        let tail = &buf[consumed..];
        if !tail.is_empty() && serde_json::from_slice::<JsonlEntry>(tail).is_ok() {
            self.count_line(tail);
            consumed = buf.len();
        }

        self.offsets.insert(path, offset + consumed as u64);
        true
    }

    fn count_line(&mut self, line: &[u8]) {
        let Ok(line) = std::str::from_utf8(line) else {
            return;
        };
        if line.trim().is_empty() {
            return;
        }
        let entry: JsonlEntry = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => return,
        };
        match entry.timestamp.as_deref() {
            Some(ts) if is_on_day(ts, self.day) => {}
            _ => return,
        }
        let stats = &mut self.stats;
        if let Some(ref msg) = entry.message {
            if let Some(ref id) = msg.id {
                if !self.seen.insert(id.clone()) {
                    return;
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
        stats.total_tokens = stats.input_tokens
            + stats.output_tokens
            + stats.cache_creation_tokens
            + stats.cache_read_tokens;
    }
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
        let mut tracker = StatsTracker::new(chrono::Local::now().date_naive());
        // Files reported by the watcher since the last pass; None = scan everything
        let mut changed: Option<HashSet<PathBuf>> = None;
        loop {
            tracker.refresh(changed.as_ref());
            let stats = tracker.stats.clone();
            if let Ok(mut lock) = shared_stats.lock() {
                *lock = Some(stats.clone());
            }
            let _ = app.emit("claude-code-stats-changed", stats);
            changed = match rx.recv_timeout(poll_interval) {
                Ok(first) => {
                    std::thread::sleep(debounce);
                    let mut paths = HashSet::new();
                    let mut rescan = false;
                    for res in std::iter::once(first).chain(rx.try_iter()) {
                        match res {
                            Ok(event) if !event.need_rescan() => paths.extend(event.paths),
                            // Dropped events or a watcher error: paths are incomplete
                            _ => rescan = true,
                        }
                    }
                    if rescan {
                        None
                    } else {
                        Some(paths)
                    }
                }
                // Quiet period: full scan in case an event was missed
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };
        }
    });
}

#[cfg(test)]
mod stats_tests {
    use super::*;
    use std::io::Write;

    fn entry(id: &str, day: chrono::NaiveDate, input: u64, output: u64) -> String {
        format!(
            r#"{{"timestamp":"{}T12:00:00","message":{{"id":"{}","usage":{{"input_tokens":{},"output_tokens":{}}}}}}}"#,
            day.format("%Y-%m-%d"),
            id,
            input,
            output
        )
    }

    fn append(path: &Path, text: &str) {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn reads_only_appended_complete_lines() {
        // Timestamps without an offset are matched by date prefix, so the
        // test doesn't depend on the local timezone
        let day = chrono::NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let dir = std::env::temp_dir().join(format!("pointify-stats-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let _ = fs::remove_file(&path);

        let mut t = StatsTracker::new(day);
        let other_day = day.pred_opt().unwrap();
        append(
            &path,
            &format!(
                "{}\n{}\n{}\n",
                entry("a", day, 10, 1),
                entry("a", day, 10, 1), // same message logged twice
                entry("old", other_day, 1000, 1000)
            ),
        );
        assert!(t.read_new(&path));
        assert_eq!((t.stats.input_tokens, t.stats.output_tokens), (10, 1));

        // A line still being written is left for the next pass
        let line = entry("b", day, 5, 2);
        let (head, rest) = line.split_at(20);
        append(&path, head);
        assert!(t.read_new(&path));
        assert_eq!(t.stats.input_tokens, 10);
        append(&path, &format!("{}\n", rest));
        assert!(t.read_new(&path));
        assert_eq!((t.stats.input_tokens, t.stats.output_tokens), (15, 3));
        assert_eq!(t.stats.total_tokens, 18);

        // Nothing new: nothing counted twice
        assert!(t.read_new(&path));
        assert_eq!(t.stats.input_tokens, 15);

        // A final line without a newline is still counted
        append(&path, &entry("c", day, 1, 1));
        assert!(t.read_new(&path));
        assert_eq!(t.stats.input_tokens, 16);

        // A rewritten (shorter) file is reported so the caller recounts
        fs::write(&path, format!("{}\n", entry("d", day, 1, 1))).unwrap();
        assert!(!t.read_new(&path));

        let _ = fs::remove_dir_all(&dir);
    }
}
