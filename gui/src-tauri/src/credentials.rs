use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeOrg {
    pub uuid: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub plan: Option<String>,
}

/// One stored credential (one login) that may have access to multiple orgs.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeAccount {
    pub id: String,
    pub session_key: String,
    pub cf_clearance: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    /// All organizations the credential has access to. Populated/refreshed by
    /// the poller from /api/account. Each gauge picks one of these.
    #[serde(default)]
    pub orgs: Vec<ClaudeOrg>,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct CredentialsStore {
    #[serde(default)]
    pub claude: Vec<ClaudeAccount>,
}

pub type SharedCredentials = Arc<Mutex<CredentialsStore>>;

fn store_path(config_dir: &Path) -> PathBuf {
    config_dir.join("credentials.json")
}

/// Parse credentials.json, tolerating the legacy single-org schema
/// (`{orgId, orgName, label}`) alongside the current multi-org shape.
pub fn load_store(config_dir: &Path) -> CredentialsStore {
    let path = store_path(config_dir);
    let Ok(text) = fs::read_to_string(&path) else {
        return CredentialsStore::default();
    };
    let Ok(raw) = serde_json::from_str::<serde_json::Value>(&text) else {
        return CredentialsStore::default();
    };
    let mut out = CredentialsStore::default();
    let Some(arr) = raw.get("claude").and_then(|v| v.as_array()) else {
        return out;
    };
    for a in arr {
        let Some(id) = a.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let orgs: Vec<ClaudeOrg> =
            if let Some(orgs_arr) = a.get("orgs").and_then(|v| v.as_array()) {
                orgs_arr
                    .iter()
                    .filter_map(|o| {
                        Some(ClaudeOrg {
                            uuid: o.get("uuid")?.as_str()?.to_string(),
                            name: o
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            plan: o
                                .get("plan")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                        })
                    })
                    .collect()
            } else {
                // Legacy single-org schema from earlier builds
                let uuid = a.get("orgId").and_then(|v| v.as_str()).unwrap_or("");
                if uuid.is_empty() {
                    Vec::new()
                } else {
                    vec![ClaudeOrg {
                        uuid: uuid.to_string(),
                        name: a
                            .get("orgName")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        plan: None,
                    }]
                }
            };
        out.claude.push(ClaudeAccount {
            id: id.to_string(),
            session_key: a
                .get("sessionKey")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            cf_clearance: a
                .get("cfClearance")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            email: a
                .get("email")
                .and_then(|v| v.as_str())
                .map(String::from),
            display_name: a
                .get("displayName")
                .and_then(|v| v.as_str())
                .map(String::from),
            orgs,
        });
    }
    out
}

pub fn save_store(config_dir: &Path, store: &CredentialsStore) -> Result<(), String> {
    fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;
    let path = store_path(config_dir);
    let json = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Migrate legacy `.claude.env` to credentials.json. Seeds one account with the
/// env's ORG_ID as a single placeholder org; the poller's first `/api/account`
/// fetch replaces it with the full membership list and fills identity.
pub fn migrate_from_env(config_dir: &Path) {
    let creds_path = store_path(config_dir);
    let env_path = config_dir.join(".claude.env");
    if creds_path.exists() || !env_path.exists() {
        return;
    }
    let Ok(text) = fs::read_to_string(&env_path) else {
        return;
    };
    let (mut session_key, mut cf_clearance, mut org_id) =
        (String::new(), String::new(), String::new());
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "SESSION_KEY" => session_key = v.trim().to_string(),
                "CF_CLEARANCE" => cf_clearance = v.trim().to_string(),
                "ORG_ID" => org_id = v.trim().to_string(),
                _ => {}
            }
        }
    }
    if session_key.is_empty() && cf_clearance.is_empty() && org_id.is_empty() {
        let _ = fs::remove_file(&env_path);
        return;
    }
    let id = format!(
        "acct-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let orgs = if org_id.is_empty() {
        Vec::new()
    } else {
        vec![ClaudeOrg {
            uuid: org_id,
            name: String::new(),
            plan: None,
        }]
    };
    let store = CredentialsStore {
        claude: vec![ClaudeAccount {
            id,
            session_key,
            cf_clearance,
            email: None,
            display_name: None,
            orgs,
        }],
    };
    if save_store(config_dir, &store).is_ok() {
        let _ = fs::remove_file(&env_path);
    }
}

pub fn upsert_account(store: &mut CredentialsStore, account: ClaudeAccount) -> ClaudeAccount {
    if let Some(existing) = store.claude.iter_mut().find(|a| a.id == account.id) {
        *existing = account.clone();
        return account;
    }
    store.claude.push(account.clone());
    account
}

pub fn remove_account(store: &mut CredentialsStore, id: &str) -> bool {
    let before = store.claude.len();
    store.claude.retain(|a| a.id != id);
    store.claude.len() != before
}
