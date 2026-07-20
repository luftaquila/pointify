mod claude;
mod codex;
mod credentials;
mod flasher;
mod monitor;
mod serial_loop;

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::WebviewWindow,
    Emitter, Manager, State, WindowEvent,
};

use nusb::MaybeFuture;
use tauri_plugin_autostart::ManagerExt;

use claude::{AccountInfo, ClaudeCache, ClaudeTtl, ClaudeUsageEntry, SharedClaudeCodeStats};
use codex::{CodexAccount, CodexCache, CodexIdentity, CodexStatus, CodexStatusState, CodexUsage};
use credentials::{ClaudeAccount, SharedCredentials};
use monitor::types::SystemMetrics;
use monitor::{SharedInterval, SharedMetrics};
use serial_loop::{SerialConfig, SharedSerialConfig};

type SharedSerial = Arc<Mutex<Option<Box<dyn serialport::SerialPort + Send>>>>;

/// Show window and nudge its size to force macOS WKWebView to re-render.
/// Without this, the WebView can blank out after a hide/show cycle.
fn show_window(window: &WebviewWindow) {
    let _ = window.show();
    let _ = window.set_focus();
    if let Ok(size) = window.inner_size() {
        let _ = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
            width: size.width + 1,
            height: size.height,
        }));
        let _ = window.set_size(tauri::Size::Physical(size));
    }
}

/// Monotonic ms timestamp for heartbeat liveness checks.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Tracks the last time the frontend reported it was alive.
/// Seeded at startup so the watchdog doesn't fire before the page loads.
static LAST_HEARTBEAT_MS: AtomicU64 = AtomicU64::new(0);

/// Reload the WebView when JS stops sending heartbeats.
/// After days of idle on macOS the WKWebView WebContent process can die,
/// leaving a blank window that no amount of forceRepaint can recover —
/// only a full reload brings it back.
fn start_heartbeat_watchdog(app: tauri::AppHandle) {
    LAST_HEARTBEAT_MS.store(now_ms(), Ordering::Relaxed);
    std::thread::spawn(move || {
        // Grace period before first check: let the page load
        std::thread::sleep(Duration::from_secs(30));
        loop {
            std::thread::sleep(Duration::from_secs(15));
            let age = now_ms().saturating_sub(LAST_HEARTBEAT_MS.load(Ordering::Relaxed));
            if age > 60_000 {
                if let Some(window) = app.get_webview_window("main") {
                    eprintln!("Heartbeat stale ({}ms), reloading WebView", age);
                    LAST_HEARTBEAT_MS.store(now_ms(), Ordering::Relaxed);
                    // Native reload via wry dispatcher — works even when the
                    // WebContent process has died, which is exactly the case
                    // where window.eval(...) silently no-ops.
                    let _ = window.reload();
                }
            }
        }
    });
}

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
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    org_id: Option<String>,
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
    #[serde(default = "default_rows")]
    rows: usize,
    #[serde(default = "default_cols")]
    cols: usize,
    gauges: Vec<GaugeConfig>,
    #[serde(default = "default_claude_refresh")]
    claude_refresh_secs: u64,
    #[serde(default)]
    smooth: bool,
    #[serde(default)]
    overshoot: bool,
    theme: String,
}

fn default_voltage() -> String {
    "3".to_string()
}
fn default_claude_refresh() -> u64 {
    120
}
fn default_rows() -> usize {
    1
}
fn default_cols() -> usize {
    3
}

#[tauri::command]
fn get_metrics(state: State<SharedMetrics>) -> Option<SystemMetrics> {
    state.lock().ok().and_then(|lock| lock.clone())
}

#[tauri::command]
fn set_interval(interval: State<SharedInterval>, ms: u64) {
    interval.store(ms, Ordering::Relaxed);
}

#[tauri::command]
fn update_serial_config(
    config: SerialConfig,
    serial_config: State<SharedSerialConfig>,
    interval: State<SharedInterval>,
) {
    interval.store(config.interval_ms, Ordering::Relaxed);
    if let Ok(mut lock) = serial_config.lock() {
        if lock.active && !config.active {
            serial_loop::request_send_zeros();
        }
        *lock = config;
    }
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
                if !(usb.vid == 512 && usb.pid == 731) {
                    return None;
                }
                // On macOS, prefer /dev/cu.* over /dev/tty.* for reliable flush-on-close
                #[cfg(target_os = "macos")]
                if p.port_name.contains("/dev/tty.") {
                    return None;
                }
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
fn get_claude_usage(
    cache: State<'_, ClaudeCache>,
) -> Result<HashMap<String, HashMap<String, ClaudeUsageEntry>>, String> {
    let cached = cache.lock().map_err(|e| e.to_string())?;
    Ok(cached
        .iter()
        .map(|(id, (_, data))| (id.clone(), data.clone()))
        .collect())
}

#[tauri::command]
fn get_codex_usage(cache: State<'_, CodexCache>) -> Result<Option<CodexUsage>, String> {
    let cached = cache.lock().map_err(|e| e.to_string())?;
    Ok(cached.as_ref().map(|(_, data)| data.clone()))
}

#[tauri::command]
fn get_codex_error(status: State<'_, CodexStatusState>) -> Option<String> {
    status.lock().ok().and_then(|s| s.error.clone())
}

#[tauri::command]
fn get_codex_identity(cache: State<'_, CodexCache>) -> Option<CodexIdentity> {
    codex::read_identity_with_cache(&cache)
}

#[tauri::command]
async fn get_codex_accounts() -> Result<Vec<CodexAccount>, String> {
    let client = reqwest::Client::new();
    codex::fetch_accounts(&client).await
}

#[tauri::command]
fn list_claude_accounts(state: State<'_, SharedCredentials>) -> Vec<ClaudeAccount> {
    state
        .lock()
        .map(|s| s.claude.clone())
        .unwrap_or_default()
}

#[tauri::command]
async fn verify_claude_credentials(
    session_key: String,
    cf_clearance: String,
) -> Result<AccountInfo, String> {
    let client = reqwest::Client::new();
    claude::fetch_account_info(&client, &session_key, &cf_clearance).await
}

#[tauri::command]
fn save_claude_account(
    app: tauri::AppHandle,
    state: State<'_, SharedCredentials>,
    cache: State<'_, ClaudeCache>,
    account: ClaudeAccount,
) -> Result<ClaudeAccount, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let saved = {
        let mut store = state.lock().map_err(|e| e.to_string())?;
        let saved = credentials::upsert_account(&mut store, account);
        credentials::save_store(&dir, &store)?;
        saved
    };
    // Force a refetch next tick for this account in case creds changed
    if let Ok(mut c) = cache.lock() {
        c.remove(&saved.id);
    }
    let _ = app.emit("credentials-changed", ());
    Ok(saved)
}

#[tauri::command]
fn delete_claude_account(
    app: tauri::AppHandle,
    state: State<'_, SharedCredentials>,
    cache: State<'_, ClaudeCache>,
    id: String,
) -> Result<(), String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    {
        let mut store = state.lock().map_err(|e| e.to_string())?;
        credentials::remove_account(&mut store, &id);
        credentials::save_store(&dir, &store)?;
    }
    if let Ok(mut c) = cache.lock() {
        c.remove(&id);
    }
    let _ = app.emit("credentials-changed", ());
    Ok(())
}

#[tauri::command]
fn open_serial_port(port: String, serial: State<SharedSerial>) -> Result<(), String> {
    let mut lock = serial.lock().map_err(|e| e.to_string())?;
    if let Some(old) = lock.take() {
        drop(old);
    }
    let p = serialport::new(&port, 115200)
        .timeout(std::time::Duration::from_millis(100))
        .open()
        .map_err(|e| e.to_string())?;
    *lock = Some(p);
    Ok(())
}

#[tauri::command]
fn close_serial_port(serial: State<SharedSerial>) -> Result<(), String> {
    let mut lock = serial.lock().map_err(|e| e.to_string())?;
    if let Some(old) = lock.take() {
        drop(old);
    }
    Ok(())
}

fn reset_gauges(app: &tauri::AppHandle) {
    let serial = app.state::<SharedSerial>();
    let mut lock = match serial.lock() {
        Ok(l) => l,
        Err(_) => return,
    };
    if let Some(port) = lock.as_mut() {
        // Send PWM=0 for gauge indices 0-6
        let bytes: Vec<u8> = (0u16..8).flat_map(|i| (i << 10).to_be_bytes()).collect();
        let _ = port.write_all(&bytes);
        let _ = port.flush();
    }
}

#[tauri::command]
fn send_serial_data(data: Vec<u16>, serial: State<SharedSerial>) -> Result<(), String> {
    let mut lock = serial.lock().map_err(|e| e.to_string())?;
    if let Some(port) = lock.as_mut() {
        let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_be_bytes()).collect();
        port.write_all(&bytes).map_err(|e| e.to_string())?;
        port.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn enter_firmware_update(port: String) -> Result<(), String> {
    // Raw file I/O — serialport crate's termios config prevents data delivery on macOS
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(&port)
        .map_err(|e| format!("open({}): {}", port, e))?;
    file.write_all(&[0x7F, 0xFF])
        .map_err(|e| format!("write: {}", e))?;
    file.flush().map_err(|e| format!("flush: {}", e))?;
    Ok(())
}

#[tauri::command]
fn is_in_bootloader() -> bool {
    // WCH bootloader USB VID/PID: 4348:55e0 or 1a86:55e0
    nusb::list_devices()
        .wait()
        .map(|devices| {
            devices.into_iter().any(|d| {
                (d.vendor_id() == 0x4348 || d.vendor_id() == 0x1a86) && d.product_id() == 0x55e0
            })
        })
        .unwrap_or(false)
}

#[tauri::command]
fn get_metric_options(state: State<SharedMetrics>) -> MetricOptions {
    let lock = state.lock().ok();
    let metrics = lock.as_ref().and_then(|l| l.as_ref());

    match metrics {
        Some(m) => MetricOptions {
            cpu_core_count: m.cpu.cores.len(),
            network_interfaces: {
                let mut names: Vec<String> =
                    m.network.interfaces.iter().map(|i| i.name.clone()).collect();
                names.sort();
                names
            },
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

#[tauri::command]
fn get_version() -> &'static str {
    env!("GIT_VERSION")
}

#[tauri::command]
fn heartbeat() {
    LAST_HEARTBEAT_MS.store(now_ms(), Ordering::Relaxed);
}

#[tauri::command]
fn get_firmware_version() -> Option<String> {
    let devices = nusb::list_devices().wait().ok()?;
    for dev in devices {
        if dev.vendor_id() == 0x0200 && dev.product_id() == 0x02DB {
            let ver = dev.device_version();
            let major = ((ver >> 12) & 0xF) * 10 + ((ver >> 8) & 0xF);
            let minor = ((ver >> 4) & 0xF) * 10 + (ver & 0xF);
            return Some(format!("v{}.{}", major, minor));
        }
    }
    None
}

fn start_watching_serial(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let watch = match nusb::watch_devices() {
            Ok(w) => w,
            Err(_) => return,
        };
        for event in futures_lite::stream::block_on(watch) {
            let dominated = match &event {
                nusb::hotplug::HotplugEvent::Connected(dev) => {
                    (dev.vendor_id() == 0x0200 && dev.product_id() == 0x02DB)
                        || ((dev.vendor_id() == 0x4348 || dev.vendor_id() == 0x1a86)
                            && dev.product_id() == 0x55e0)
                }
                nusb::hotplug::HotplugEvent::Disconnected(_) => true,
            };
            if dominated {
                // Small delay for OS to finish registering the device node
                std::thread::sleep(std::time::Duration::from_millis(500));
                let _ = app.emit("serial-ports-changed", ());
            }
        }
    });
}

pub fn run() {
    let shared_metrics: SharedMetrics = Arc::new(Mutex::new(None));
    let shared_interval: SharedInterval = Arc::new(AtomicU64::new(200));
    // Migrate legacy .claude.env into credentials.json before loading the store
    let config_dir_for_migrate = dirs::config_dir()
        .map(|d| d.join("pointify"))
        .unwrap_or_else(|| PathBuf::from("."));
    credentials::migrate_from_env(&config_dir_for_migrate);

    let claude_cache: ClaudeCache = Arc::new(Mutex::new(HashMap::new()));
    let claude_ttl = ClaudeTtl(Arc::new(AtomicU64::new(120)));
    let codex_cache: CodexCache = Arc::new(Mutex::new(None));
    let codex_status: CodexStatusState = Arc::new(Mutex::new(CodexStatus::default()));
    // Load and immediately rewrite so legacy `{orgId, orgName, label}` shapes
    // get normalized to the current multi-org schema on disk.
    let loaded_credentials = credentials::load_store(&config_dir_for_migrate);
    let _ = credentials::save_store(&config_dir_for_migrate, &loaded_credentials);
    let shared_credentials: SharedCredentials = Arc::new(Mutex::new(loaded_credentials));
    let shared_serial: SharedSerial = Arc::new(Mutex::new(None));
    let shared_claude_code_stats: SharedClaudeCodeStats = Arc::new(Mutex::new(None));
    let shared_serial_config: SharedSerialConfig = Arc::new(Mutex::new(SerialConfig::default()));

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(shared_metrics.clone())
        .manage(shared_interval.clone())
        .manage(claude_cache.clone())
        .manage(claude_ttl.clone())
        .manage(codex_cache.clone())
        .manage(codex_status.clone())
        .manage(shared_credentials.clone())
        .manage(shared_serial.clone())
        .manage(shared_claude_code_stats.clone())
        .manage(shared_serial_config.clone())
        .invoke_handler(tauri::generate_handler![
            get_metrics,
            get_metric_options,
            set_interval,
            update_serial_config,
            list_serial_ports,
            open_serial_port,
            close_serial_port,
            send_serial_data,
            enter_firmware_update,
            is_in_bootloader,
            load_config,
            save_config,
            set_claude_ttl,
            get_claude_usage,
            get_codex_usage,
            get_codex_error,
            get_codex_identity,
            get_codex_accounts,
            list_claude_accounts,
            verify_claude_credentials,
            save_claude_account,
            delete_claude_account,
            get_version,
            get_firmware_version,
            heartbeat,
            flasher::fetch_latest_firmware,
            flasher::download_firmware,
            flasher::flash_firmware,
            flasher::install_wch_driver
        ])
        .setup(move |app| {
            // Build tray menu
            let show = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)?;
            let autostart_manager = app.autolaunch();
            let autostart_enabled = autostart_manager.is_enabled().unwrap_or(false);
            let autostart = CheckMenuItem::with_id(
                app,
                "autostart",
                "Start on Login",
                true,
                autostart_enabled,
                None::<&str>,
            )?;
            let separator = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu =
                Menu::with_items(app, &[&show, &autostart, &separator, &quit])?;

            // Build tray icon
            let icon = Image::from_bytes(include_bytes!("../icons/icon.png"))?;

            TrayIconBuilder::new()
                .icon(icon)
                .menu(&menu)
                .tooltip("Pointify")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            show_window(&window);
                        }
                    }
                    "autostart" => {
                        let manager = app.autolaunch();
                        let enabled = manager.is_enabled().unwrap_or(false);
                        if enabled {
                            let _ = manager.disable();
                        } else {
                            let _ = manager.enable();
                        }
                    }
                    "quit" => {
                        reset_gauges(app);
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
                                show_window(&window);
                            }
                        }
                    }
                })
                .build(app)?;

            // Watch /dev for USB serial device changes
            start_watching_serial(app.handle().clone());

            // Reload WebView if JS stops sending heartbeats (macOS WKWebView
            // WebContent process can die after days of idle)
            start_heartbeat_watchdog(app.handle().clone());

            // Start hardware monitoring
            monitor::start_monitoring(shared_metrics.clone(), shared_interval);

            // Poll Claude usage API in background so the cache stays fresh
            // even when the WebView is frozen (macOS WKWebView blank-screen)
            claude::start_usage_poller(
                app.handle().clone(),
                claude_cache.clone(),
                claude_ttl.clone(),
                shared_credentials.clone(),
            );

            // Start ~/.claude/stats-cache.json file watcher
            claude::start_watching_stats(app.handle().clone(), shared_claude_code_stats.clone());

            // Watch ~/.codex/auth.json so Codex CLI token refreshes invalidate the cache
            codex::start_watching(
                app.handle().clone(),
                codex_cache.clone(),
                codex_status.clone(),
            );

            // Poll Codex wham/usage in background, sharing the Claude refresh TTL
            codex::start_usage_poller(
                app.handle().clone(),
                codex_cache.clone(),
                codex_status.clone(),
                claude_ttl.clone(),
            );

            // Start backend serial loop (independent of WebView throttling)
            serial_loop::start_serial_loop(
                shared_metrics,
                shared_serial,
                shared_serial_config,
                claude_cache,
                shared_claude_code_stats,
                codex_cache,
            );

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
