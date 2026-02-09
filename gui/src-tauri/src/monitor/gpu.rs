use super::types::GpuMetrics;

#[cfg(any(target_os = "windows", target_os = "linux"))]
mod nvidia {
    use super::GpuMetrics;
    use nvml_wrapper::Nvml;

    pub struct NvidiaMonitor {
        nvml: Nvml,
        device_count: u32,
    }

    impl NvidiaMonitor {
        pub fn try_new() -> Option<Self> {
            let nvml = Nvml::init().ok()?;
            let device_count = nvml.device_count().ok()?;
            Some(Self { nvml, device_count })
        }

        pub fn collect(&self) -> Vec<GpuMetrics> {
            let mut gpus = Vec::new();
            for i in 0..self.device_count {
                if let Ok(device) = self.nvml.device_by_index(i) {
                    let name = device.name().unwrap_or_else(|_| "NVIDIA GPU".to_string());
                    let utilization = device.utilization_rates().ok().map(|u| u.gpu);
                    let temperature = device
                        .temperature(nvml_wrapper::enum_wrappers::device::TemperatureSensor::Gpu)
                        .ok();
                    let memory = device.memory_info().ok();
                    let memory_total = memory.as_ref().map(|m| m.total);
                    let memory_used = memory.as_ref().map(|m| m.used);
                    let clock_mhz = device
                        .clock_info(nvml_wrapper::enum_wrappers::device::Clock::Graphics)
                        .ok();
                    let power_watts = device.power_usage().ok().map(|mw| mw as f64 / 1000.0);

                    gpus.push(GpuMetrics {
                        name,
                        utilization,
                        temperature,
                        memory_total,
                        memory_used,
                        clock_mhz,
                        power_watts,
                    });
                }
            }
            gpus
        }
    }
}

#[cfg(target_os = "macos")]
use super::apple_gpu;

pub struct GpuMonitor {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    nvidia: Option<nvidia::NvidiaMonitor>,
    #[cfg(target_os = "macos")]
    apple: Option<apple_gpu::AppleGpuMonitor>,
}

impl GpuMonitor {
    pub fn new() -> Self {
        Self {
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            nvidia: nvidia::NvidiaMonitor::try_new(),
            #[cfg(target_os = "macos")]
            apple: apple_gpu::AppleGpuMonitor::try_new(),
        }
    }

    pub fn collect(&mut self) -> Vec<GpuMetrics> {
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            if let Some(nvidia) = &self.nvidia {
                return nvidia.collect();
            }
        }

        #[cfg(target_os = "macos")]
        {
            if let Some(apple) = &mut self.apple {
                return apple.collect();
            }
        }

        Vec::new()
    }
}
