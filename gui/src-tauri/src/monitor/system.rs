use std::time::Instant;

use sysinfo::{Components, Disks, Networks, System};

use super::types::*;

pub struct SystemMonitor {
    sys: System,
    components: Components,
    disks: Disks,
    networks: Networks,
    last_net_rx: Vec<(String, u64)>,
    last_net_tx: Vec<(String, u64)>,
    last_disk_read: Vec<(String, u64)>,
    last_disk_write: Vec<(String, u64)>,
    last_update: Instant,
}

impl SystemMonitor {
    pub fn new() -> Self {
        let mut sys = System::new_all();
        sys.refresh_all();

        let components = Components::new_with_refreshed_list();
        let disks = Disks::new_with_refreshed_list();
        let networks = Networks::new_with_refreshed_list();

        let last_net_rx = networks
            .iter()
            .map(|(name, data)| (name.to_string(), data.total_received()))
            .collect();
        let last_net_tx = networks
            .iter()
            .map(|(name, data)| (name.to_string(), data.total_transmitted()))
            .collect();

        let last_disk_read = Vec::new();
        let last_disk_write = Vec::new();

        Self {
            sys,
            components,
            disks,
            networks,
            last_net_rx,
            last_net_tx,
            last_disk_read,
            last_disk_write,
            last_update: Instant::now(),
        }
    }

    pub fn collect(&mut self) -> (CpuMetrics, MemoryMetrics, SwapMetrics, NetworkMetrics, DiskMetrics) {
        let elapsed = self.last_update.elapsed().as_secs_f64();
        self.last_update = Instant::now();

        self.sys.refresh_all();
        self.components.refresh(true);
        self.disks.refresh(true);
        self.networks.refresh(true);

        let cpu = self.collect_cpu();
        let memory = self.collect_memory();
        let swap = self.collect_swap();
        let network = self.collect_network(elapsed);
        let disk = self.collect_disk(elapsed);

        (cpu, memory, swap, network, disk)
    }

    fn collect_cpu(&self) -> CpuMetrics {
        let cpus = self.sys.cpus();
        let name = if cpus.is_empty() {
            "Unknown".to_string()
        } else {
            cpus[0].brand().to_string()
        };

        let usage = self.sys.global_cpu_usage();
        let cores: Vec<CoreMetrics> = cpus
            .iter()
            .map(|cpu| CoreMetrics {
                usage: cpu.cpu_usage(),
                frequency: cpu.frequency(),
            })
            .collect();

        let temperature = self.find_cpu_temperature();

        CpuMetrics {
            name,
            usage,
            cores,
            temperature,
        }
    }

    fn find_cpu_temperature(&self) -> Option<f32> {
        let labels = ["cpu", "coretemp", "k10temp", "zenpower", "soc_thermal"];
        for component in self.components.iter() {
            let label = component.label().to_lowercase();
            if labels.iter().any(|l| label.contains(l)) {
                return component.temperature();
            }
        }
        // Fallback: try first component with a temperature
        for component in self.components.iter() {
            if let Some(temp) = component.temperature() {
                return Some(temp);
            }
        }
        None
    }

    fn collect_memory(&self) -> MemoryMetrics {
        let total = self.sys.total_memory();
        let used = self.sys.used_memory();
        let available = self.sys.available_memory();
        let usage_percent = if total > 0 {
            (used as f32 / total as f32) * 100.0
        } else {
            0.0
        };
        MemoryMetrics {
            total,
            used,
            available,
            usage_percent,
        }
    }

    fn collect_swap(&self) -> SwapMetrics {
        let total = self.sys.total_swap();
        let used = self.sys.used_swap();
        let usage_percent = if total > 0 {
            (used as f32 / total as f32) * 100.0
        } else {
            0.0
        };
        SwapMetrics {
            total,
            used,
            usage_percent,
        }
    }

    fn collect_network(&mut self, elapsed: f64) -> NetworkMetrics {
        let mut interfaces = Vec::new();
        let mut new_rx = Vec::new();
        let mut new_tx = Vec::new();

        for (name, data) in self.networks.iter() {
            let current_rx = data.total_received();
            let current_tx = data.total_transmitted();

            let prev_rx = self
                .last_net_rx
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| *v)
                .unwrap_or(current_rx);
            let prev_tx = self
                .last_net_tx
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| *v)
                .unwrap_or(current_tx);

            let rx_bytes_per_sec = if elapsed > 0.0 {
                (current_rx.saturating_sub(prev_rx)) as f64 / elapsed
            } else {
                0.0
            };
            let tx_bytes_per_sec = if elapsed > 0.0 {
                (current_tx.saturating_sub(prev_tx)) as f64 / elapsed
            } else {
                0.0
            };

            new_rx.push((name.to_string(), current_rx));
            new_tx.push((name.to_string(), current_tx));

            interfaces.push(NetworkInterfaceMetrics {
                name: name.to_string(),
                rx_bytes_per_sec,
                tx_bytes_per_sec,
            });
        }

        self.last_net_rx = new_rx;
        self.last_net_tx = new_tx;

        NetworkMetrics { interfaces }
    }

    fn collect_disk(&mut self, elapsed: f64) -> DiskMetrics {
        let mut disk_infos = Vec::new();
        let mut new_read = Vec::new();
        let mut new_write = Vec::new();

        for disk in self.disks.iter() {
            let name = disk.name().to_string_lossy().to_string();
            let mount_point = disk.mount_point().to_string_lossy().to_string();
            let total = disk.total_space();
            let available = disk.available_space();
            let used = total.saturating_sub(available);
            let usage_percent = if total > 0 {
                (used as f32 / total as f32) * 100.0
            } else {
                0.0
            };

            let usage = disk.usage();
            let current_read = usage.read_bytes;
            let current_write = usage.written_bytes;

            let key = format!("{}:{}", name, mount_point);

            let prev_read = self
                .last_disk_read
                .iter()
                .find(|(n, _)| n == &key)
                .map(|(_, v)| *v)
                .unwrap_or(current_read);
            let prev_write = self
                .last_disk_write
                .iter()
                .find(|(n, _)| n == &key)
                .map(|(_, v)| *v)
                .unwrap_or(current_write);

            let read_bytes_per_sec = if elapsed > 0.0 {
                (current_read.saturating_sub(prev_read)) as f64 / elapsed
            } else {
                0.0
            };
            let write_bytes_per_sec = if elapsed > 0.0 {
                (current_write.saturating_sub(prev_write)) as f64 / elapsed
            } else {
                0.0
            };

            new_read.push((key.clone(), current_read));
            new_write.push((key, current_write));

            disk_infos.push(DiskInfo {
                name,
                mount_point,
                total,
                used,
                available,
                usage_percent,
                read_bytes_per_sec,
                write_bytes_per_sec,
            });
        }

        self.last_disk_read = new_read;
        self.last_disk_write = new_write;

        DiskMetrics { disks: disk_infos }
    }
}
