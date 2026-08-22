pub mod collector;
pub mod history;
pub mod parser;
pub mod platform;

pub use collector::HostMetricsCollector;
pub use platform::Platform;

use serde::{Deserialize, Serialize};

/// The lifecycle of a single metric group.
///
/// The invariant this type exists to enforce: **a metric we do not have is
/// never rendered as a number.** v1 defaulted every field to `0` and drew it,
/// so a monitor that had collected nothing was indistinguishable from an idle
/// host — while the terminal in the same window printed the real values.
///
/// `Default` is deliberately `Pending`, not `Collected`. A freshly constructed
/// `HostMetrics` claims nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[derive(Default)]
pub enum MetricState {
    /// Not sampled yet. Normal for the first tick; not an error.
    #[default]
    Pending,
    /// Sampled successfully. The associated fields are real readings.
    Collected,
    /// Sampling was attempted and failed. Carries the reason in words and how
    /// many consecutive attempts have failed.
    Uncollected { reason: String, attempts: u32 },
    /// The remote platform cannot provide this metric at all. Terminal state —
    /// retrying will not help, so the UI should say so once and stop.
    Unsupported { reason: String },
}


impl MetricState {
    /// True only when the associated numbers are real readings.
    pub fn is_collected(&self) -> bool {
        matches!(self, MetricState::Collected)
    }

    /// True when the UI must render an explanation instead of a value.
    // Part of MetricState's honest-reporting surface; read by callers that
    // render explanations rather than values.
    #[allow(dead_code)]
    pub fn needs_explanation(&self) -> bool {
        matches!(
            self,
            MetricState::Uncollected { .. } | MetricState::Unsupported { .. }
        )
    }

    /// One-line explanation for the UI, in words. Never a number, never a dash.
    ///
    /// ```text
    /// CPU   uncollected · collector timed out 4×
    /// DISK  unsupported · macOS reports no per-device queue depth
    /// ```
    pub fn explain(&self) -> Option<String> {
        match self {
            MetricState::Pending => Some("waiting for first sample".to_string()),
            MetricState::Collected => None,
            MetricState::Uncollected { reason, attempts } => {
                if *attempts > 1 {
                    Some(format!("uncollected · {} {}×", reason, attempts))
                } else {
                    Some(format!("uncollected · {}", reason))
                }
            }
            MetricState::Unsupported { reason } => Some(format!("unsupported · {}", reason)),
        }
    }

    /// Record a failure, incrementing the attempt counter if this group was
    /// already failing for the same reason.
    pub fn fail(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        let attempts = match self {
            MetricState::Uncollected {
                reason: prev,
                attempts,
            } if *prev == reason => *attempts + 1,
            _ => 1,
        };
        *self = MetricState::Uncollected { reason, attempts };
    }

    /// Mark unsupported. Idempotent — does not reset an existing reason.
    pub fn unsupported(&mut self, reason: impl Into<String>) {
        if !matches!(self, MetricState::Unsupported { .. }) {
            *self = MetricState::Unsupported {
                reason: reason.into(),
            };
        }
    }
}

/// Per-group collection state for one host.
///
/// Every group is independent: a host that can report memory but not CPU says
/// exactly that, rather than reporting a zeroed CPU.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CollectionStatus {
    pub platform: Platform,
    pub cpu: MetricState,
    pub mem: MetricState,
    pub load: MetricState,
    pub disk: MetricState,
    pub net: MetricState,
    pub procs: MetricState,
    pub uptime: MetricState,
}

impl CollectionStatus {
    /// Mark every group as failed with the same reason — used when the whole
    /// batch command fails, so one dead channel doesn't look like seven
    /// independent problems.
    pub fn fail_all(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        self.cpu.fail(reason.clone());
        self.mem.fail(reason.clone());
        self.load.fail(reason.clone());
        self.disk.fail(reason.clone());
        self.net.fail(reason.clone());
        self.procs.fail(reason.clone());
        self.uptime.fail(reason);
    }

    /// Mark every group unsupported — used for a platform we have no
    /// collectors for, where retrying will never help.
    pub fn unsupported_all(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        self.cpu.unsupported(reason.clone());
        self.mem.unsupported(reason.clone());
        self.load.unsupported(reason.clone());
        self.disk.unsupported(reason.clone());
        self.net.unsupported(reason.clone());
        self.procs.unsupported(reason.clone());
        self.uptime.unsupported(reason);
    }

    /// Groups currently needing an explanation, as `(label, explanation)`.
    #[allow(dead_code)]
    pub fn problems(&self) -> Vec<(&'static str, String)> {
        let groups: [(&'static str, &MetricState); 7] = [
            ("CPU", &self.cpu),
            ("MEM", &self.mem),
            ("LOAD", &self.load),
            ("DISK", &self.disk),
            ("NET", &self.net),
            ("PROCS", &self.procs),
            ("UPTIME", &self.uptime),
        ];
        groups
            .iter()
            .filter(|(_, st)| st.needs_explanation())
            .filter_map(|(name, st)| st.explain().map(|e| (*name, e)))
            .collect()
    }
}

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
    /// Per-group truth about which of the fields above mean anything.
    pub status: CollectionStatus,
}

impl HostMetrics {
    /// Memory used as a percentage — `None` unless memory was actually
    /// collected and a total is known.
    pub fn mem_percent(&self) -> Option<f64> {
        if !self.status.mem.is_collected() || self.mem_total_kb == 0 {
            return None;
        }
        Some(self.mem_used_kb as f64 / self.mem_total_kb as f64 * 100.0)
    }

    /// CPU percentage — `None` unless collected.
    pub fn cpu_percent_opt(&self) -> Option<f64> {
        self.status.cpu.is_collected().then_some(self.cpu_percent)
    }

    /// Load averages — `None` unless collected.
    #[allow(dead_code)]
    pub fn load_opt(&self) -> Option<(f64, f64, f64)> {
        self.status
            .load
            .is_collected()
            .then_some((self.load_1m, self.load_5m, self.load_15m))
    }

    /// Uptime — `None` unless collected.
    pub fn uptime_opt(&self) -> Option<u64> {
        self.status.uptime.is_collected().then_some(self.uptime_secs)
    }

    /// Network rates — `None` unless collected. Note that the first sample
    /// after connect has no previous counter to difference against, so `net`
    /// stays `Pending` until the second tick rather than reporting `0 B/s`.
    pub fn net_opt(&self) -> Option<(f64, f64)> {
        self.status
            .net
            .is_collected()
            .then_some((self.net_rx_bps, self.net_tx_bps))
    }

    /// Disks worth showing a human, fullest first.
    ///
    /// v1 listed every line `df` emitted — nine macOS system volumes including
    /// `/System/Volumes/xarts` at 12.6 MB. The rule is user data only.
    pub fn user_disks(&self) -> Vec<&DiskInfo> {
        if !self.status.disk.is_collected() {
            return Vec::new();
        }
        let mut kept: Vec<&DiskInfo> = self.disks.iter().filter(|d| d.is_user_visible()).collect();
        kept.sort_by(|a, b| {
            b.use_pct
                .partial_cmp(&a.use_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        kept
    }

    /// How many mounts `user_disks` suppressed, for the "N system volumes
    /// hidden" footnote. Hiding data silently is its own kind of dishonesty.
    pub fn hidden_disk_count(&self) -> usize {
        if !self.status.disk.is_collected() {
            return 0;
        }
        self.disks.iter().filter(|d| !d.is_user_visible()).count()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DiskInfo {
    pub mount: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub use_pct: f64,
}

/// One GiB, the floor below which a mount is assumed to be plumbing.
const SMALL_MOUNT_BYTES: u64 = 1024 * 1024 * 1024;

impl DiskInfo {
    /// Whether this mount carries user data worth a row.
    ///
    /// Skip macOS firmware/system volumes (except `Data`, which is where the
    /// user's files actually live), skip App Translocation scratch mounts, and
    /// skip anything under 1 GiB — unless it is over 90% full, because a small
    /// mount about to fill is exactly the thing you want to be told about.
    pub fn is_user_visible(&self) -> bool {
        let m = self.mount.as_str();

        if m.contains("AppTranslocation") {
            return false;
        }
        if m.starts_with("/System/Volumes/") && m != "/System/Volumes/Data" {
            return false;
        }
        // Linux plumbing that df -Pk still reports on some distros.
        if m.starts_with("/snap/")
            || m.starts_with("/sys/")
            || m.starts_with("/proc/")
            || m == "/dev"
            || m == "/dev/shm"
            || m.starts_with("/run/")
        {
            return false;
        }

        // Container bind mounts. Docker mounts these individually off the
        // host filesystem, so `df` reports each one with the *host's* full
        // capacity — three identical multi-hundred-GB rows for three text
        // files, crowding out the actual root.
        if matches!(m, "/etc/hosts" | "/etc/hostname" | "/etc/resolv.conf") {
            return false;
        }
        if self.total_bytes < SMALL_MOUNT_BYTES && self.use_pct < 90.0 {
            return false;
        }
        true
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_metric_state_claims_nothing() {
        let m = HostMetrics::default();
        assert_eq!(m.status.cpu, MetricState::Pending);
        assert!(!m.status.cpu.is_collected());
        // The whole point: a default HostMetrics reports no readings at all,
        // even though every numeric field is 0.
        assert!(m.cpu_percent_opt().is_none());
        assert!(m.mem_percent().is_none());
        assert!(m.load_opt().is_none());
        assert!(m.uptime_opt().is_none());
        assert!(m.net_opt().is_none());
    }

    #[test]
    fn repeated_failures_accumulate_a_count() {
        let mut st = MetricState::default();
        st.fail("collector timed out");
        assert_eq!(st.explain().unwrap(), "uncollected · collector timed out");
        st.fail("collector timed out");
        st.fail("collector timed out");
        assert_eq!(st.explain().unwrap(), "uncollected · collector timed out 3×");
    }

    #[test]
    fn a_different_reason_restarts_the_count() {
        let mut st = MetricState::default();
        st.fail("collector timed out");
        st.fail("collector timed out");
        st.fail("permission denied");
        assert_eq!(st.explain().unwrap(), "uncollected · permission denied");
    }

    #[test]
    fn unsupported_is_terminal_and_keeps_its_first_reason() {
        let mut st = MetricState::default();
        st.unsupported("macOS reports no per-device queue depth");
        st.unsupported("something else");
        assert_eq!(
            st.explain().unwrap(),
            "unsupported · macOS reports no per-device queue depth"
        );
    }

    #[test]
    fn disk_filter_keeps_user_data_and_drops_system_noise() {
        let d = |mount: &str, total: u64, pct: f64| DiskInfo {
            mount: mount.to_string(),
            total_bytes: total,
            used_bytes: 0,
            use_pct: pct,
        };
        let gb = 1024 * 1024 * 1024;

        assert!(d("/", 200 * gb, 42.0).is_user_visible());
        assert!(d("/System/Volumes/Data", 200 * gb, 42.0).is_user_visible());
        // The exact noise from the v1 screenshot.
        assert!(!d("/System/Volumes/xarts", 12_600_000, 1.0).is_user_visible());
        assert!(!d("/System/Volumes/Preboot", 5 * gb, 10.0).is_user_visible());
        assert!(!d(
            "/private/var/folders/6c/T/AppTranslocation/foo",
            200 * gb,
            42.0
        )
        .is_user_visible());
        // Small but nearly full still earns its row.
        assert!(d("/boot", 500 * 1024 * 1024, 94.0).is_user_visible());
        assert!(!d("/boot", 500 * 1024 * 1024, 30.0).is_user_visible());
    }

    #[test]
    fn user_disks_are_empty_until_collected_and_sorted_fullest_first() {
        let gb = 1024 * 1024 * 1024;
        let mut m = HostMetrics {
            disks: vec![
                DiskInfo {
                    mount: "/".into(),
                    total_bytes: 200 * gb,
                    used_bytes: 0,
                    use_pct: 9.0,
                },
                DiskInfo {
                    mount: "/System/Volumes/Data".into(),
                    total_bytes: 200 * gb,
                    used_bytes: 0,
                    use_pct: 42.0,
                },
                DiskInfo {
                    mount: "/System/Volumes/xarts".into(),
                    total_bytes: 12_600_000,
                    used_bytes: 0,
                    use_pct: 1.0,
                },
            ],
            ..Default::default()
        };
        // Not collected yet: show nothing, not three rows of stale truth.
        assert!(m.user_disks().is_empty());

        m.status.disk = MetricState::Collected;
        let shown = m.user_disks();
        assert_eq!(shown.len(), 2);
        assert_eq!(shown[0].mount, "/System/Volumes/Data"); // fullest first
        assert_eq!(shown[1].mount, "/");
        assert_eq!(m.hidden_disk_count(), 1);
    }

    #[test]
    fn fail_all_reports_one_problem_per_group_with_a_shared_reason() {
        let mut st = CollectionStatus::default();
        st.fail_all("ssh channel closed");
        let problems = st.problems();
        assert_eq!(problems.len(), 7);
        assert!(problems
            .iter()
            .all(|(_, e)| e == "uncollected · ssh channel closed"));
    }
}
