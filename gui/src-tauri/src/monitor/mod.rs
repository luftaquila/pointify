pub mod gpu;
pub mod system;
pub mod types;

use std::thread;
use std::time::Duration;

use system::SystemMonitor;
use gpu::GpuMonitor;
use types::SystemMetrics;

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.2} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

fn format_speed(bytes_per_sec: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    if bytes_per_sec >= MB {
        format!("{:.2} MB/s", bytes_per_sec / MB)
    } else if bytes_per_sec >= KB {
        format!("{:.2} KB/s", bytes_per_sec / KB)
    } else {
        format!("{:.0} B/s", bytes_per_sec)
    }
}

fn print_metrics(metrics: &SystemMetrics) {
    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║                    POINTIFY MONITOR                     ║");
    println!("╠══════════════════════════════════════════════════════════╣");

    // CPU
    println!("║ CPU: {} ({} cores)", metrics.cpu.name, metrics.cpu.cores.len());
    println!(
        "║   Usage: {:.1}%  Temp: {}",
        metrics.cpu.usage,
        metrics.cpu.temperature
            .map(|t| format!("{:.0}°C", t))
            .unwrap_or_else(|| "N/A".to_string())
    );

    // Memory
    println!(
        "║ Memory: {}/{} ({:.1}%)",
        format_bytes(metrics.memory.used),
        format_bytes(metrics.memory.total),
        metrics.memory.usage_percent
    );

    // Swap
    if metrics.swap.total > 0 {
        println!(
            "║ Swap: {}/{} ({:.1}%)",
            format_bytes(metrics.swap.used),
            format_bytes(metrics.swap.total),
            metrics.swap.usage_percent
        );
    }

    // Disks
    println!("║ Disks:");
    for disk in &metrics.disk.disks {
        println!(
            "║   {} ({}): {}/{} ({:.1}%)  R:{} W:{}",
            disk.name,
            disk.mount_point,
            format_bytes(disk.used),
            format_bytes(disk.total),
            disk.usage_percent,
            format_speed(disk.read_bytes_per_sec),
            format_speed(disk.write_bytes_per_sec)
        );
    }

    // Network
    let active_interfaces: Vec<_> = metrics
        .network
        .interfaces
        .iter()
        .filter(|i| i.rx_bytes_per_sec > 0.0 || i.tx_bytes_per_sec > 0.0)
        .collect();
    if !active_interfaces.is_empty() {
        println!("║ Network:");
        for iface in active_interfaces {
            println!(
                "║   {}: ↓{} ↑{}",
                iface.name,
                format_speed(iface.rx_bytes_per_sec),
                format_speed(iface.tx_bytes_per_sec)
            );
        }
    }

    // GPU
    for gpu in &metrics.gpus {
        println!("║ GPU: {}", gpu.name);
        if let Some(util) = gpu.utilization {
            print!("║   Usage: {}%", util);
        }
        if let Some(temp) = gpu.temperature {
            print!("  Temp: {}°C", temp);
        }
        println!();
        if let (Some(used), Some(total)) = (gpu.memory_used, gpu.memory_total) {
            println!("║   VRAM: {}/{}", format_bytes(used), format_bytes(total));
        }
        if let Some(clock) = gpu.clock_mhz {
            print!("║   Clock: {} MHz", clock);
        }
        if let Some(power) = gpu.power_watts {
            print!("  Power: {:.1}W", power);
        }
        println!();
    }

    println!("╚══════════════════════════════════════════════════════════╝");
    println!();
}

pub fn start_monitoring() {
    thread::spawn(|| {
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

            print_metrics(&metrics);

            thread::sleep(Duration::from_secs(1));
        }
    });
}
