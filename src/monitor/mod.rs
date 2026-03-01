pub mod collector;
pub mod history;
pub mod parser;

pub use collector::HostMetricsCollector;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HostMetrics {
    pub cpu_percent: f64,
    pub cpu_per_core: Vec<f64>,
    pub mem_total_kb: u64,
    pub mem_used_kb: u64,
    pub mem_available_kb: u64,
    pub mem_swap_total_kb: u64,
    pub mem_swap_used_kb: u64,
    pub load_1m: f64,
    pub load_5m: f64,
    pub load_15m: f64,
    pub disks: Vec<DiskInfo>,
    pub disk_read_bps: f64,
    pub disk_write_bps: f64,
    pub net_rx_bps: f64,
    pub net_tx_bps: f64,
    pub top_procs_cpu: Vec<ProcessInfo>,
    pub top_procs_mem: Vec<ProcessInfo>,
    pub uptime_secs: u64,
    pub os_info: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DiskInfo {
    pub mount: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub use_pct: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_pct: f64,
    pub mem_pct: f64,
    pub mem_rss_kb: u64,
    pub state: String,
}
