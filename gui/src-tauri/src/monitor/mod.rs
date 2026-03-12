pub mod gpu;
pub mod system;
pub mod types;

#[cfg(target_os = "macos")]
mod apple_gpu;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use gpu::GpuMonitor;
use system::SystemMonitor;
use types::SystemMetrics;

pub type SharedMetrics = Arc<Mutex<Option<SystemMetrics>>>;
pub type SharedInterval = Arc<AtomicU64>;

pub fn start_monitoring(state: SharedMetrics, interval: SharedInterval) {
    thread::spawn(move || {
        let mut sys_monitor = SystemMonitor::new();
        let mut gpu_monitor = GpuMonitor::new();

        // First refresh is baseline; wait before collecting
        thread::sleep(Duration::from_millis(500));

        loop {
            let (mut cpu, memory, swap, network, disk, process_count) = sys_monitor.collect();
            let (gpus, gpu_cpu_power) = gpu_monitor.collect();

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
                process_count,
            };

            if let Ok(mut lock) = state.lock() {
                *lock = Some(metrics);
            }

            let ms = interval.load(Ordering::Relaxed);
            thread::sleep(Duration::from_millis(ms));
        }
    });
}
