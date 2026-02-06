mod monitor;

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, State, WindowEvent,
};

use monitor::types::SystemMetrics;
use monitor::{SharedInterval, SharedMetrics};

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
    sub_index: usize,
    max_value: Option<f64>,
    max_unit: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Config {
    active: bool,
    interval_ms: u64,
    gauge_count: usize,
    gauges: Vec<GaugeConfig>,
    theme: String,
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

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(shared_metrics.clone())
        .manage(shared_interval.clone())
        .invoke_handler(tauri::generate_handler![get_metrics, get_metric_options, set_interval, load_config, save_config])
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
