mod claude;
mod monitor;

use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, State, WindowEvent,
};

use claude::{ClaudeCache, ClaudeUsageEntry};
use monitor::types::SystemMetrics;
use monitor::{SharedInterval, SharedMetrics};

struct ClaudeTtl(AtomicU64);

#[derive(Serialize)]
struct MetricOptions {
    cpu_core_count: usize,
    network_interfaces: Vec<String>,
    disk_names: Vec<String>,
    gpu_count: usize,
    gpu_names: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GaugeConfig {
    metric_id: String,
    sub_index: String,
    max_value: Option<f64>,
    max_unit: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Config {
    active: bool,
    #[serde(default = "default_voltage")]
    voltage: String,
    interval_ms: u64,
    gauge_count: usize,
    gauges: Vec<GaugeConfig>,
    #[serde(default = "default_claude_refresh")]
    claude_refresh_secs: u64,
    theme: String,
}

fn default_voltage() -> String { "3".to_string() }
fn default_claude_refresh() -> u64 { 120 }

#[tauri::command]
fn get_metrics(state: State<SharedMetrics>) -> Option<SystemMetrics> {
    state.lock().ok().and_then(|lock| lock.clone())
}

#[tauri::command]
fn set_interval(interval: State<SharedInterval>, ms: u64) {
    interval.store(ms, Ordering::Relaxed);
}

#[tauri::command]
fn load_config(app: tauri::AppHandle) -> Option<Config> {
    let path = app.path().app_config_dir().ok()?.join("config.json");
    let data = fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

#[tauri::command]
fn save_config(app: tauri::AppHandle, config: Config) {
    if let Ok(dir) = app.path().app_config_dir() {
        let _ = fs::create_dir_all(&dir);
        if let Ok(json) = serde_json::to_string_pretty(&config) {
            let _ = fs::write(dir.join("config.json"), json);
        }
    }
}

#[derive(Serialize, Clone)]
struct SerialPortInfo {
    port: String,
    product: String,
}

#[tauri::command]
fn list_serial_ports() -> Vec<SerialPortInfo> {
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| {
            if let serialport::SerialPortType::UsbPort(usb) = &p.port_type {
                if !(usb.vid == 512 && usb.pid == 731) { return None; }
                return Some(SerialPortInfo {
                    port: p.port_name,
                    product: usb.product.clone().unwrap_or_default(),
                });
            }
            None
        })
        .collect()
}

#[tauri::command]
fn set_claude_ttl(ttl: State<'_, ClaudeTtl>, secs: u64) {
    ttl.0.store(secs, Ordering::Relaxed);
}

#[tauri::command]
fn open_claude_env(app: tauri::AppHandle) -> Result<(), String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let path = claude::ensure_env_file(&dir);
    opener::open(&path).map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_claude_usage(
    cache: State<'_, ClaudeCache>,
    ttl: State<'_, ClaudeTtl>,
    app: tauri::AppHandle,
) -> Result<Option<HashMap<String, ClaudeUsageEntry>>, String> {
    let cache = cache.inner().clone();
    let ttl_secs = ttl.0.load(Ordering::Relaxed);

    // Return cached data if fresh
    {
        let cached = cache.lock().map_err(|e| e.to_string())?;
        if let Some((time, data)) = cached.as_ref() {
            if time.elapsed() < std::time::Duration::from_secs(ttl_secs) {
                return Ok(Some(data.clone()));
            }
        }
    }

    // Load credentials
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let mut creds = match claude::load_credentials(&dir) {
        Some(c) if !c.session_key.is_empty() && !c.cf_clearance.is_empty() => c,
        _ => return Ok(None),
    };

    let client = reqwest::Client::new();

    // Auto-fetch org_id if empty
    if creds.org_id.is_empty() {
        creds.org_id = claude::fetch_org_id(&client, &creds).await?;
        claude::save_credentials(&dir, &creds);
    }

    let data = claude::fetch_usage(&client, &creds, &creds.org_id).await?;

    // Update cache
    {
        let mut cached = cache.lock().map_err(|e| e.to_string())?;
        *cached = Some((Instant::now(), data.clone()));
    }

    Ok(Some(data))
}

#[tauri::command]
fn get_metric_options(state: State<SharedMetrics>) -> MetricOptions {
    let lock = state.lock().ok();
    let metrics = lock.as_ref().and_then(|l| l.as_ref());

    match metrics {
        Some(m) => MetricOptions {
            cpu_core_count: m.cpu.cores.len(),
            network_interfaces: m.network.interfaces.iter().map(|i| i.name.clone()).collect(),
            disk_names: m
                .disk
                .disks
                .iter()
                .map(|d| format!("{} ({})", d.name, d.mount_point))
                .collect(),
            gpu_count: m.gpus.len(),
            gpu_names: m.gpus.iter().map(|g| g.name.clone()).collect(),
        },
        None => MetricOptions {
            cpu_core_count: 0,
            network_interfaces: vec![],
            disk_names: vec![],
            gpu_count: 0,
            gpu_names: vec![],
        },
    }
}

pub fn run() {
    let shared_metrics: SharedMetrics = Arc::new(Mutex::new(None));
    let shared_interval: SharedInterval = Arc::new(AtomicU64::new(200));
    let claude_cache: ClaudeCache = Arc::new(Mutex::new(None));
    let claude_ttl = ClaudeTtl(AtomicU64::new(120));

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .manage(shared_metrics.clone())
        .manage(shared_interval.clone())
        .manage(claude_cache)
        .manage(claude_ttl)
        .invoke_handler(tauri::generate_handler![
            get_metrics, get_metric_options, set_interval, list_serial_ports,
            load_config, save_config,
            open_claude_env, set_claude_ttl, get_claude_usage
        ])
        .setup(move |app| {
            // Build tray menu
            let show = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)?;
            let hide = MenuItem::with_id(app, "hide", "Hide Window", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &hide, &quit])?;

            // Build tray icon
            let icon = Image::from_bytes(include_bytes!("../icons/icon.png"))?;

            TrayIconBuilder::new()
                .icon(icon)
                .menu(&menu)
                .tooltip("Pointify")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "hide" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.hide();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            // Start hardware monitoring
            monitor::start_monitoring(shared_metrics, shared_interval);

            // Start .claude.env file watcher
            let claude_cache: ClaudeCache = app.state::<ClaudeCache>().inner().clone();
            claude::start_watching(app.handle().clone(), claude_cache);

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
