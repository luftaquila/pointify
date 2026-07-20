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
    /// Window length in seconds from the payload (18000 = 5h, 604800 = 7d).
    /// None on older payloads that don't report it.
    pub limit_window_seconds: Option<i64>,
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
    /// Raw `plan_type` from the wham/usage payload (e.g. "team"). Server-fresh,
    /// unlike the JWT claim which only updates on token refresh.
    pub plan_type: Option<String>,
}

impl CodexUsage {
    /// Pick the window for a gauge bucket by actual duration, not slot:
    /// Team plans report their weekly credit window as `primary_window` with
    /// no short window at all, so codex_5h/codex_7d can't trust
    /// primary/secondary positions. Windows under 24h count as the short
    /// bucket, the rest as weekly. Falls back to the positional slot for
    /// payloads that don't report `limit_window_seconds`.
    pub fn window_for(&self, nominal_minutes: f64) -> Option<&CodexUsageWindow> {
        let want_weekly = nominal_minutes >= 24.0 * 60.0;
        for w in [self.primary.as_ref(), self.secondary.as_ref()]
            .into_iter()
            .flatten()
        {
            if let Some(secs) = w.limit_window_seconds {
                if (secs >= 86_400) == want_weekly {
                    return Some(w);
                }
            }
        }
        let positional = if want_weekly {
            self.secondary.as_ref()
        } else {
            self.primary.as_ref()
        };
        // Only positionally match windows of unknown duration — a window whose
        // duration is known but didn't match above belongs to the other bucket.
        positional.filter(|w| w.limit_window_seconds.is_none())
    }
}

/// One account/workspace from wham/accounts/check.
#[derive(Serialize, Clone, PartialEq, Debug)]
pub struct CodexAccount {
    pub id: String,
    /// Workspace name; None for the personal account.
    pub name: Option<String>,
    /// "workspace" or "personal".
    pub structure: Option<String>,
    /// "account-owner" or "standard-user".
    pub role: Option<String>,
    /// Human-readable plan label (already through plan_label()).
    pub plan: Option<String>,
    /// Matches the response's default_account_id.
    pub is_default: bool,
    /// Matches tokens.account_id in auth.json — the account Codex CLI (and
    /// our wham/usage poll) is currently operating as.
    pub is_active: bool,
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

/// The workspace Codex CLI is currently operating as. Sent as the
/// ChatGPT-Account-Id header so wham responses are scoped to it.
fn read_account_id() -> Option<String> {
    let path = auth_path()?;
    let file = fs::File::open(path).ok()?;
    let v: serde_json::Value = serde_json::from_reader(file).ok()?;
    v.get("tokens")?
        .get("account_id")?
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
fn read_identity() -> Option<CodexIdentity> {
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

/// Identity for display. Name/email come from the JWT; plan prefers the
/// server-fresh `plan_type` from the last wham/usage fetch because the JWT
/// claim goes stale (it only updates when Codex CLI refreshes tokens, which
/// can lag a plan change by 8+ days). Falls back to the JWT claim while no
/// usage fetch has succeeded yet.
pub fn read_identity_with_cache(cache: &CodexCache) -> Option<CodexIdentity> {
    let mut id = read_identity()?;
    let server_plan = cache
        .lock()
        .ok()
        .and_then(|c| c.as_ref().and_then(|(_, u)| u.plan_type.clone()));
    if server_plan.is_some() {
        id.plan_type = server_plan;
    }
    id.plan_type = id.plan_type.map(|p| plan_label(&p));
    Some(id)
}

/// Map a raw `plan_type` (JWT claim or wham/usage payload) to its
/// human-readable label. Values and labels mirror Codex CLI's
/// `KnownPlan::display_name()` (codex-rs/protocol/src/auth.rs) plus the
/// backend wire enum (rate_limit_status_payload.rs); unknown values pass
/// through unchanged so new plans OpenAI ships still render.
fn plan_label(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "guest" => "Guest",
        "free" => "Free",
        "go" => "Go",
        "plus" => "Plus",
        "pro" => "Pro",
        "prolite" => "Pro Lite",
        "free_workspace" => "Free Workspace",
        "team" => "Team",
        "self_serve_business_usage_based" => "Self Serve Business Usage Based",
        "business" => "Business",
        "enterprise_cbp_usage_based" => "Enterprise CBP Usage Based",
        "enterprise" | "hc" => "Enterprise",
        "education" | "edu" => "Edu",
        "quorum" => "Quorum",
        "k12" => "K12",
        _ => return raw.to_string(),
    }
    .to_string()
}

pub async fn fetch_usage(client: &reqwest::Client) -> Result<CodexUsage, String> {
    let token = read_access_token().ok_or_else(|| "codex auth.json not found".to_string())?;
    let mut req = client
        .get("https://chatgpt.com/backend-api/wham/usage")
        .bearer_auth(token);
    // Pin the response to the workspace Codex CLI is using, like the CLI does.
    if let Some(account_id) = read_account_id() {
        req = req.header("ChatGPT-Account-Id", account_id);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;

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
            limit_window_seconds: w.get("limit_window_seconds").and_then(|v| v.as_i64()),
        })
    };

    Ok(CodexUsage {
        primary: parse_window("primary_window"),
        secondary: parse_window("secondary_window"),
        plan_type: body
            .get("plan_type")
            .and_then(|v| v.as_str())
            .map(String::from),
    })
}

/// Enumerate every account/workspace on this login via wham/accounts/check.
/// Fetched on demand (credentials modal), not polled.
pub async fn fetch_accounts(client: &reqwest::Client) -> Result<Vec<CodexAccount>, String> {
    let token = read_access_token().ok_or_else(|| "codex auth.json not found".to_string())?;
    let resp = client
        .get("https://chatgpt.com/backend-api/wham/accounts/check")
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(parse_accounts(&body, read_account_id().as_deref()))
}

fn parse_accounts(body: &serde_json::Value, active_id: Option<&str>) -> Vec<CodexAccount> {
    let default_id = body.get("default_account_id").and_then(|v| v.as_str());
    body.get("accounts")
        .and_then(|v| v.as_array())
        .map(|accounts| {
            accounts
                .iter()
                .filter_map(|a| {
                    let id = a.get("id")?.as_str()?.to_string();
                    Some(CodexAccount {
                        is_default: default_id == Some(id.as_str()),
                        is_active: active_id == Some(id.as_str()),
                        name: a.get("name").and_then(|v| v.as_str()).map(String::from),
                        structure: a
                            .get("structure")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                        role: a
                            .get("account_user_role")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                        plan: a
                            .get("plan_type")
                            .and_then(|v| v.as_str())
                            .map(plan_label),
                        id,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::{parse_accounts, plan_label, CodexAccount, CodexUsage, CodexUsageWindow};

    fn win(limit_window_seconds: Option<i64>) -> CodexUsageWindow {
        CodexUsageWindow {
            used_percent: 50.0,
            reset_at: 0,
            limit_window_seconds,
        }
    }

    #[test]
    fn window_for_matches_by_duration_on_team_shape() {
        // Team: weekly credit window arrives as primary, no short window.
        let usage = CodexUsage {
            primary: Some(win(Some(604_800))),
            secondary: None,
            plan_type: Some("team".into()),
        };
        assert!(usage.window_for(5.0 * 60.0).is_none());
        assert_eq!(
            usage.window_for(7.0 * 24.0 * 60.0).unwrap().limit_window_seconds,
            Some(604_800)
        );
    }

    #[test]
    fn window_for_matches_by_duration_on_plus_shape() {
        // Plus/Pro: 5h primary + 7d secondary.
        let usage = CodexUsage {
            primary: Some(win(Some(18_000))),
            secondary: Some(win(Some(604_800))),
            plan_type: Some("plus".into()),
        };
        assert_eq!(
            usage.window_for(5.0 * 60.0).unwrap().limit_window_seconds,
            Some(18_000)
        );
        assert_eq!(
            usage.window_for(7.0 * 24.0 * 60.0).unwrap().limit_window_seconds,
            Some(604_800)
        );
    }

    #[test]
    fn window_for_falls_back_to_slots_without_durations() {
        let usage = CodexUsage {
            primary: Some(win(None)),
            secondary: Some(win(None)),
            plan_type: None,
        };
        assert!(usage.window_for(5.0 * 60.0).is_some());
        assert!(usage.window_for(7.0 * 24.0 * 60.0).is_some());
    }

    #[test]
    fn parses_accounts_check_payload() {
        // Shape observed live from wham/accounts/check (2026-07).
        let body = serde_json::json!({
            "accounts": [
                {
                    "id": "ws-1",
                    "account_user_role": "standard-user",
                    "structure": "workspace",
                    "plan_type": "team",
                    "name": "RTst",
                },
                {
                    "id": "personal-1",
                    "account_user_role": "account-owner",
                    "structure": "personal",
                    "plan_type": "prolite",
                    "name": null,
                }
            ],
            "default_account_id": "ws-1",
        });
        assert_eq!(
            parse_accounts(&body, Some("personal-1")),
            vec![
                CodexAccount {
                    id: "ws-1".into(),
                    name: Some("RTst".into()),
                    structure: Some("workspace".into()),
                    role: Some("standard-user".into()),
                    plan: Some("Team".into()),
                    is_default: true,
                    is_active: false,
                },
                CodexAccount {
                    id: "personal-1".into(),
                    name: None,
                    structure: Some("personal".into()),
                    role: Some("account-owner".into()),
                    plan: Some("Pro Lite".into()),
                    is_default: false,
                    is_active: true,
                },
            ]
        );
    }

    #[test]
    fn parses_empty_or_malformed_accounts_payload() {
        assert!(parse_accounts(&serde_json::json!({}), None).is_empty());
        assert!(parse_accounts(&serde_json::json!({"accounts": "nope"}), None).is_empty());
    }

    #[test]
    fn maps_known_plan_types() {
        for (raw, label) in [
            ("guest", "Guest"),
            ("free", "Free"),
            ("go", "Go"),
            ("plus", "Plus"),
            ("pro", "Pro"),
            ("prolite", "Pro Lite"),
            ("free_workspace", "Free Workspace"),
            ("team", "Team"),
            (
                "self_serve_business_usage_based",
                "Self Serve Business Usage Based",
            ),
            ("business", "Business"),
            ("enterprise_cbp_usage_based", "Enterprise CBP Usage Based"),
            ("enterprise", "Enterprise"),
            ("hc", "Enterprise"),
            ("education", "Edu"),
            ("edu", "Edu"),
            ("quorum", "Quorum"),
            ("k12", "K12"),
        ] {
            assert_eq!(plan_label(raw), label);
        }
    }

    #[test]
    fn matches_case_insensitively() {
        assert_eq!(plan_label("TEAM"), "Team");
    }

    #[test]
    fn passes_unknown_plan_types_through() {
        assert_eq!(plan_label("some_future_plan"), "some_future_plan");
    }
}
