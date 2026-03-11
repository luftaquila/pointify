use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::claude::{ClaudeCache, ClaudeCodeStats, ClaudeUsageEntry, SharedClaudeCodeStats};
use crate::monitor::types::SystemMetrics;
use crate::monitor::SharedMetrics;
use crate::SharedSerial;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SerialConfig {
    pub active: bool,
    pub gauge_count: usize,
    pub voltage: String,
    pub smooth: bool,
    pub overshoot: bool,
    pub interval_ms: u64,
    pub gauges: Vec<GaugeEntry>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GaugeEntry {
    pub metric_id: String,
    pub sub_index: String,
    pub max_value: Option<f64>,
    pub max_unit: Option<String>,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            active: false,
            gauge_count: 3,
            voltage: "3".to_string(),
            smooth: false,
            overshoot: false,
            interval_ms: 200,
            gauges: Vec::new(),
        }
    }
}

pub type SharedSerialConfig = Arc<Mutex<SerialConfig>>;

// Flag to signal the serial loop to send zeros once when deactivated
static SEND_ZEROS: AtomicBool = AtomicBool::new(false);

pub fn request_send_zeros() {
    SEND_ZEROS.store(true, Ordering::Relaxed);
}

fn unit_multiplier(unit: &str) -> f64 {
    match unit {
        "KB/s" => 1024.0,
        "MB/s" => 1_048_576.0,
        "GB/s" => 1_073_741_824.0,
        "Kbps" => 125.0,
        "Mbps" => 125_000.0,
        "Gbps" => 125_000_000.0,
        "GHz" => 1000.0,
        "K" => 1000.0,
        "M" => 1_000_000.0,
        _ => 1.0,
    }
}

fn is_percentage_metric(id: &str) -> bool {
    matches!(
        id,
        "cpu_usage"
            | "cpu_core_usage"
            | "mem_usage"
            | "swap_usage"
            | "gpu_util"
            | "gpu_vram"
            | "claude_5h"
            | "claude_5h_reset"
            | "claude_7d"
            | "claude_7d_reset"
            | "claude_7d_sonnet"
            | "disk_usage"
    )
}

fn effective_max(gauge: &GaugeEntry) -> f64 {
    if is_percentage_metric(&gauge.metric_id) {
        return 100.0;
    }
    match (gauge.max_value, gauge.max_unit.as_deref()) {
        (Some(v), Some(u)) if v > 0.0 => v * unit_multiplier(u),
        (Some(v), None) if v > 0.0 => v,
        _ => 1.0,
    }
}

fn claude_reset_pct(resets_at: &str, total_minutes: f64) -> Option<f64> {
    let reset_time = chrono::DateTime::parse_from_rfc3339(resets_at)
        .ok()?
        .with_timezone(&chrono::Utc);
    let remaining = (reset_time - chrono::Utc::now()).num_seconds() as f64 / 60.0;
    let pct: f64 = (1.0 - remaining / total_minutes) * 100.0;
    Some(pct.clamp(0.0, 100.0))
}

fn extract_value(
    metrics: Option<&SystemMetrics>,
    claude_usage: Option<&HashMap<String, ClaudeUsageEntry>>,
    claude_code_stats: Option<&ClaudeCodeStats>,
    metric_id: &str,
    sub_index: &str,
) -> Option<f64> {
    // Claude API metrics
    match metric_id {
        "claude_5h" => {
            return claude_usage
                .and_then(|u| u.get("five_hour"))
                .map(|e| e.utilization);
        }
        "claude_5h_reset" => {
            return claude_usage
                .and_then(|u| u.get("five_hour"))
                .and_then(|e| e.resets_at.as_deref())
                .and_then(|r| claude_reset_pct(r, 5.0 * 60.0));
        }
        "claude_7d" => {
            return claude_usage
                .and_then(|u| u.get("seven_day"))
                .map(|e| e.utilization);
        }
        "claude_7d_reset" => {
            return claude_usage
                .and_then(|u| u.get("seven_day"))
                .and_then(|e| e.resets_at.as_deref())
                .and_then(|r| claude_reset_pct(r, 7.0 * 24.0 * 60.0));
        }
        "claude_7d_sonnet" => {
            return claude_usage
                .and_then(|u| u.get("seven_day_sonnet"))
                .map(|e| e.utilization);
        }
        "claude_code_tokens" => return claude_code_stats.map(|s| s.total_tokens as f64),
        "claude_code_io_tokens" => {
            return claude_code_stats.map(|s| (s.input_tokens + s.output_tokens) as f64)
        }
        "claude_code_input_tokens" => return claude_code_stats.map(|s| s.input_tokens as f64),
        "claude_code_output_tokens" => return claude_code_stats.map(|s| s.output_tokens as f64),
        "claude_code_cache_creation_tokens" => {
            return claude_code_stats.map(|s| s.cache_creation_tokens as f64)
        }
        "claude_code_cache_read_tokens" => {
            return claude_code_stats.map(|s| s.cache_read_tokens as f64)
        }
        "claude_code_cost" => return claude_code_stats.map(|s| s.total_cost),
        _ => {}
    }

    // System metrics
    let m = metrics?;
    let core_idx: Option<usize> = sub_index
        .strip_prefix("Core ")
        .and_then(|s| s.parse().ok());

    match metric_id {
        "cpu_usage" => Some(m.cpu.usage as f64),
        "cpu_core_usage" => core_idx.and_then(|i| m.cpu.cores.get(i).map(|c| c.usage as f64)),
        "cpu_temp" => m.cpu.temperature.map(|t| t as f64),
        "cpu_core_freq" => core_idx.and_then(|i| m.cpu.cores.get(i).map(|c| c.frequency as f64)),
        "cpu_power" => m.cpu.power_watts,
        "mem_usage" => Some(m.memory.usage_percent as f64),
        "swap_usage" => Some(m.swap.usage_percent as f64),
        "net_rx" => {
            if sub_index == "Total" {
                Some(m.network.interfaces.iter().map(|i| i.rx_bytes_per_sec).sum())
            } else {
                m.network
                    .interfaces
                    .iter()
                    .find(|i| i.name == sub_index)
                    .map(|i| i.rx_bytes_per_sec)
            }
        }
        "net_tx" => {
            if sub_index == "Total" {
                Some(m.network.interfaces.iter().map(|i| i.tx_bytes_per_sec).sum())
            } else {
                m.network
                    .interfaces
                    .iter()
                    .find(|i| i.name == sub_index)
                    .map(|i| i.tx_bytes_per_sec)
            }
        }
        "net_rxtx" => {
            if sub_index == "Total" {
                Some(
                    m.network
                        .interfaces
                        .iter()
                        .map(|i| i.rx_bytes_per_sec + i.tx_bytes_per_sec)
                        .sum(),
                )
            } else {
                m.network
                    .interfaces
                    .iter()
                    .find(|i| i.name == sub_index)
                    .map(|i| i.rx_bytes_per_sec + i.tx_bytes_per_sec)
            }
        }
        "disk_usage" => {
            let key = sub_index;
            m.disk
                .disks
                .iter()
                .find(|d| format!("{} ({})", d.name, d.mount_point) == key)
                .map(|d| d.usage_percent as f64)
        }
        "disk_read" => {
            if sub_index == "Total" {
                Some(m.disk.disks.iter().map(|d| d.read_bytes_per_sec).sum())
            } else {
                m.disk
                    .disks
                    .iter()
                    .find(|d| format!("{} ({})", d.name, d.mount_point) == sub_index)
                    .map(|d| d.read_bytes_per_sec)
            }
        }
        "disk_write" => {
            if sub_index == "Total" {
                Some(m.disk.disks.iter().map(|d| d.write_bytes_per_sec).sum())
            } else {
                m.disk
                    .disks
                    .iter()
                    .find(|d| format!("{} ({})", d.name, d.mount_point) == sub_index)
                    .map(|d| d.write_bytes_per_sec)
            }
        }
        "disk_rw" => {
            if sub_index == "Total" {
                Some(
                    m.disk
                        .disks
                        .iter()
                        .map(|d| d.read_bytes_per_sec + d.write_bytes_per_sec)
                        .sum(),
                )
            } else {
                m.disk
                    .disks
                    .iter()
                    .find(|d| format!("{} ({})", d.name, d.mount_point) == sub_index)
                    .map(|d| d.read_bytes_per_sec + d.write_bytes_per_sec)
            }
        }
        "gpu_util" => m
            .gpus
            .iter()
            .find(|g| g.name == sub_index)
            .and_then(|g| g.utilization.map(|v| v as f64)),
        "gpu_temp" => m
            .gpus
            .iter()
            .find(|g| g.name == sub_index)
            .and_then(|g| g.temperature.map(|v| v as f64)),
        "gpu_vram" => m
            .gpus
            .iter()
            .find(|g| g.name == sub_index)
            .and_then(|g| match (g.memory_total, g.memory_used) {
                (Some(total), Some(used)) if total > 0 => {
                    Some(used as f64 / total as f64 * 100.0)
                }
                _ => None,
            }),
        "gpu_clock" => m
            .gpus
            .iter()
            .find(|g| g.name == sub_index)
            .and_then(|g| g.clock_mhz.map(|v| v as f64)),
        "gpu_power" => m
            .gpus
            .iter()
            .find(|g| g.name == sub_index)
            .and_then(|g| g.power_watts),
        _ => None,
    }
}

fn send_serial_raw(serial: &SharedSerial, pcts: &[f64], voltage: &str, gauge_count: usize, overshoot: bool) {
    let mut lock = match serial.lock() {
        Ok(l) => l,
        Err(_) => return,
    };
    let port = match lock.as_mut() {
        Some(p) => p,
        None => return,
    };
    // Overshoot + 3V: tell firmware it's 5V (skip 3/5 scaling), and we scale pct by 3/5 ourselves
    let overshoot_3v = overshoot && voltage == "3";
    let v_bit: u16 = if voltage == "5" || overshoot_3v { 1 } else { 0 };
    let bytes: Vec<u8> = (0..gauge_count)
        .flat_map(|i| {
            let pct = pcts.get(i).copied().unwrap_or(0.0);
            let scaled = if overshoot_3v { pct * 3.0 / 5.0 } else { pct };
            let pwm = (scaled * 1023.0).round() as u16;
            let pwm = pwm.min(1023);
            ((v_bit << 15) | (((i as u16) & 0x1f) << 10) | pwm).to_be_bytes()
        })
        .collect();
    let _ = port.write_all(&bytes);
    let _ = port.flush();
}

pub fn start_serial_loop(
    metrics: SharedMetrics,
    serial: SharedSerial,
    config: SharedSerialConfig,
    claude_cache: ClaudeCache,
    claude_code_stats: SharedClaudeCodeStats,
) {
    thread::spawn(move || {
        let mut current_pct = [0.0f64; 7];
        let mut target_pct = [0.0f64; 7];
        let alpha = 0.51;

        let mut last_target_update = Instant::now();
        let mut was_active = false;

        loop {
            let cfg = match config.lock() {
                Ok(l) => l.clone(),
                Err(_) => {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };

            // Send zeros once when transitioning to inactive
            if !cfg.active {
                if was_active || SEND_ZEROS.swap(false, Ordering::Relaxed) {
                    target_pct.fill(0.0);
                    current_pct.fill(0.0);
                    send_serial_raw(&serial, &current_pct, &cfg.voltage, cfg.gauge_count, cfg.overshoot);
                    was_active = false;
                }
                thread::sleep(Duration::from_millis(100));
                continue;
            }
            was_active = true;

            // Determine tick interval
            let tick_ms = if cfg.smooth && cfg.interval_ms > 100 {
                100
            } else {
                cfg.interval_ms
            };

            // Update targets from fresh metrics every interval_ms
            let now = Instant::now();
            if now.duration_since(last_target_update) >= Duration::from_millis(cfg.interval_ms) {
                last_target_update = now;

                let sys_metrics = metrics.lock().ok().and_then(|l| l.clone());

                let claude_data: Option<HashMap<String, ClaudeUsageEntry>> = claude_cache
                    .lock()
                    .ok()
                    .and_then(|l| l.as_ref().map(|(_, data)| data.clone()));

                let code_stats: Option<ClaudeCodeStats> =
                    claude_code_stats.lock().ok().and_then(|l| l.clone());

                for i in 0..cfg.gauge_count {
                    if let Some(gauge) = cfg.gauges.get(i) {
                        let value = extract_value(
                            sys_metrics.as_ref(),
                            claude_data.as_ref(),
                            code_stats.as_ref(),
                            &gauge.metric_id,
                            &gauge.sub_index,
                        );
                        let max = effective_max(gauge);
                        let upper = if cfg.overshoot {
                            if cfg.voltage == "3" { 3.5 / 3.0 } else { f64::MAX }
                        } else {
                            1.0
                        };
                        target_pct[i] = value.map(|v| (v / max).clamp(0.0, upper)).unwrap_or(0.0);
                    } else {
                        target_pct[i] = 0.0;
                    }
                }
            }

            // Apply EMA smoothing or direct assignment
            if cfg.smooth {
                for i in 0..cfg.gauge_count {
                    current_pct[i] += alpha * (target_pct[i] - current_pct[i]);
                }
            } else {
                for i in 0..cfg.gauge_count {
                    current_pct[i] = target_pct[i];
                }
            }

            send_serial_raw(&serial, &current_pct, &cfg.voltage, cfg.gauge_count, cfg.overshoot);

            thread::sleep(Duration::from_millis(tick_ms));
        }
    });
}
