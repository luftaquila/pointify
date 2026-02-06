pub mod gpu;
pub mod system;
pub mod types;

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use gpu::GpuMonitor;
use system::SystemMonitor;
use types::SystemMetrics;

pub type SharedMetrics = Arc<Mutex<Option<SystemMetrics>>>;

pub fn start_monitoring(state: SharedMetrics) {
    thread::spawn(move || {
        let mut sys_monitor = SystemMonitor::new();
        let gpu_monitor = GpuMonitor::new();

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

            thread::sleep(Duration::from_secs(1));
        }
    });
}
