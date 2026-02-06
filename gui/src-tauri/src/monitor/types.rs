use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct CpuMetrics {
    pub name: String,
    pub usage: f32,
    pub cores: Vec<CoreMetrics>,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoreMetrics {
    pub usage: f32,
    pub frequency: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryMetrics {
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub usage_percent: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SwapMetrics {
    pub total: u64,
    pub used: u64,
    pub usage_percent: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkInterfaceMetrics {
    pub name: String,
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkMetrics {
    pub interfaces: Vec<NetworkInterfaceMetrics>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskInfo {
    pub name: String,
    pub mount_point: String,
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub usage_percent: f32,
    pub read_bytes_per_sec: f64,
    pub write_bytes_per_sec: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskMetrics {
    pub disks: Vec<DiskInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuMetrics {
    pub name: String,
    pub utilization: Option<u32>,
    pub temperature: Option<u32>,
    pub memory_total: Option<u64>,
    pub memory_used: Option<u64>,
    pub clock_mhz: Option<u32>,
    pub power_watts: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemMetrics {
    pub cpu: CpuMetrics,
    pub memory: MemoryMetrics,
    pub swap: SwapMetrics,
    pub network: NetworkMetrics,
    pub disk: DiskMetrics,
    pub gpus: Vec<GpuMetrics>,
}
