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
            let (cpu, memory, swap, network, disk) = sys_monitor.collect();
            let gpus = gpu_monitor.collect();

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
