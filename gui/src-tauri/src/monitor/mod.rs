pub mod gpu;
pub mod system;
pub mod types;

#[cfg(target_os = "macos")]
mod apple_gpu;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use gpu::GpuMonitor;
use system::SystemMonitor;
use types::{GpuMetrics, SystemMetrics};

pub type SharedMetrics = Arc<Mutex<Option<SystemMetrics>>>;
pub type SharedInterval = Arc<AtomicU64>;

/// Metric groups no active gauge reads are still refreshed this often, so the
/// option lists (interfaces, disks, GPUs) stay current and a newly selected
/// gauge starts from recent data.
const SLOW_REFRESH: Duration = Duration::from_secs(10);

/// A group is due on every tick while a gauge reads it, otherwise once per
/// `SLOW_REFRESH`. Never refreshed (`None`) is always due.
fn is_due(last: Option<Instant>, needed: bool) -> bool {
    needed || last.map_or(true, |t| t.elapsed() >= SLOW_REFRESH)
}

/// The costly metric groups the active gauges read. CPU usage and memory are
/// cheap and always refreshed, so they are not listed.
#[derive(Clone, Copy, Default)]
pub struct MetricNeeds {
    pub cpu_temp: bool,
    pub network: bool,
    pub disk_io: bool,
    pub gpu: bool,
}

impl MetricNeeds {
    pub fn from_metric_ids<'a>(ids: impl IntoIterator<Item = &'a str>) -> Self {
        let mut needs = Self::default();
        for id in ids {
            match id {
                "cpu_temp" => needs.cpu_temp = true,
                // On macOS CPU power comes from the GPU monitor's IOReport subscription.
                "cpu_power" => needs.gpu = true,
                "net_rx" | "net_tx" | "net_rxtx" => needs.network = true,
                "disk_read" | "disk_write" | "disk_rw" => needs.disk_io = true,
                _ if id.starts_with("gpu_") => needs.gpu = true,
                _ => {}
            }
        }
        needs
    }
}

pub fn start_monitoring<F>(state: SharedMetrics, interval: SharedInterval, needs: F)
where
    F: Fn() -> MetricNeeds + Send + 'static,
{
    thread::spawn(move || {
        let mut sys_monitor = SystemMonitor::new();
        let mut gpu_monitor = GpuMonitor::new();
        let mut gpu_at: Option<Instant> = None;
        let mut gpu_last: (Vec<GpuMetrics>, Option<f64>) = (Vec::new(), None);

        // First refresh is baseline; wait before collecting
        thread::sleep(Duration::from_millis(500));

        loop {
            let needs = needs();
            let (mut cpu, memory, swap, network, disk) = sys_monitor.collect(needs);
            if is_due(gpu_at, needs.gpu) {
                gpu_last = gpu_monitor.collect();
                gpu_at = Some(Instant::now());
            }
            let (gpus, gpu_cpu_power) = gpu_last.clone();

            // On macOS, CPU power comes from IOReport via GPU monitor
            if cpu.power_watts.is_none() {
                cpu.power_watts = gpu_cpu_power;
            }

            let metrics = SystemMetrics {
                cpu,
                memory,
                swap,
                network,
                disk,
                gpus,
            };

            if let Ok(mut lock) = state.lock() {
                *lock = Some(metrics);
            }

            let ms = interval.load(Ordering::Relaxed);
            thread::sleep(Duration::from_millis(ms));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_follow_gauge_metrics() {
        let needs = MetricNeeds::from_metric_ids(["cpu_usage", "cpu_core_usage", "mem_usage"]);
        assert!(!needs.cpu_temp && !needs.network && !needs.disk_io && !needs.gpu);

        let needs = MetricNeeds::from_metric_ids(["cpu_temp", "net_rxtx", "disk_write", "gpu_clock"]);
        assert!(needs.cpu_temp && needs.network && needs.disk_io && needs.gpu);

        // disk_usage is refreshed on the slow cadence only
        assert!(!MetricNeeds::from_metric_ids(["disk_usage"]).disk_io);
        assert!(MetricNeeds::from_metric_ids(["cpu_power"]).gpu);
    }

    #[test]
    fn unread_groups_refresh_on_slow_cadence() {
        assert!(is_due(None, false));
        assert!(is_due(Some(Instant::now()), true));
        assert!(!is_due(Some(Instant::now()), false));
        assert!(is_due(Some(Instant::now() - SLOW_REFRESH), false));
    }
}
