use serde::{Deserialize, Serialize};

/// The remote operating system, as reported by `uname -s`.
///
/// v1 had no such concept: it issued `cat /proc/stat` and friends at every
/// host and rendered whatever came back — which on macOS is nothing at all.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Platform {
    /// Not probed yet.
    #[default]
    Undetected,
    Linux,
    MacOS,
    /// A uname we recognise the name of but have no collectors for.
    Other(String),
}

impl Platform {
    pub fn from_uname(raw: &str) -> Self {
        match raw.trim() {
            "Linux" => Platform::Linux,
            "Darwin" => Platform::MacOS,
            "" => Platform::Undetected,
            other => Platform::Other(other.to_string()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Platform::Undetected => "unknown",
            Platform::Linux => "Linux",
            Platform::MacOS => "macOS",
            Platform::Other(s) => s,
        }
    }

    /// Whether we have collectors for this platform at all.
    pub fn is_supported(&self) -> bool {
        matches!(self, Platform::Linux | Platform::MacOS)
    }

    /// The batch command for one metrics sweep, or `None` for a platform we
    /// cannot collect from.
    ///
    /// Both variants emit the same `===SECTION===` envelope so the splitter and
    /// the collector do not need to care which one ran.
    pub fn metrics_command(&self, proc_limit: usize) -> Option<String> {
        match self {
            Platform::Linux => Some(linux_command()),
            Platform::MacOS => Some(macos_command(proc_limit)),
            _ => None,
        }
    }
}

fn linux_command() -> String {
    // `df -Pk` rather than `df -P`: POSIX mode alone leaves the block size
    // implementation-defined, and macOS answers in 512-byte blocks. v1
    // multiplied every figure by 1024 regardless, so macOS disks read 2× too
    // large. `-k` pins it to 1024 everywhere.
    concat!(
        "echo '===UNAME==='; uname -s; ",
        "echo '===CPUSTAT==='; cat /proc/stat; ",
        "echo '===MEMINFO==='; cat /proc/meminfo; ",
        "echo '===LOADAVG==='; cat /proc/loadavg; ",
        "echo '===DF==='; df -Pk; ",
        "echo '===NETDEV==='; cat /proc/net/dev; ",
        "echo '===UPTIME==='; cat /proc/uptime; ",
        "echo '===PS==='; ps aux --sort=-%cpu 2>/dev/null || ps aux; ",
        "echo '===END==='"
    )
    .to_string()
}

/// macOS has no `/proc`, so every group needs a different source.
///
/// The CPU figure is the awkward one. There is no cheap cumulative counter
/// reachable over a plain exec channel — `kern.cp_time` is FreeBSD, and
/// `ps`'s `%cpu` on Darwin is a decaying per-process average, not a system
/// instantaneous rate. `iostat -c 2 -w 1` does the honest thing: it samples
/// twice a second apart and reports the interval. That costs ~1s of wall time
/// per sweep, which is why `top -l 2` is only the fallback — it costs the same
/// but parses worse.
fn macos_command(proc_limit: usize) -> String {
    format!(
        concat!(
            "echo '===UNAME==='; uname -s; ",
            "echo '===CPUSTAT==='; (iostat -c 2 -w 1 2>/dev/null || top -l 2 -n 0 -s 1 2>/dev/null); ",
            "echo '===VMSTAT==='; vm_stat; ",
            "echo '===MEMSIZE==='; sysctl -n hw.memsize; ",
            "echo '===SWAP==='; sysctl -n vm.swapusage; ",
            "echo '===LOADAVG==='; sysctl -n vm.loadavg; ",
            "echo '===NCPU==='; sysctl -n hw.ncpu; ",
            "echo '===DF==='; df -Pk; ",
            "echo '===NETDEV==='; netstat -ib; ",
            "echo '===BOOTTIME==='; sysctl -n kern.boottime; ",
            "echo '===NOW==='; date +%s; ",
            "echo '===PS==='; ps axo user,pid,pcpu,pmem,vsz,rss,tty,state,start,time,command -r 2>/dev/null | head -n {}; ",
            "echo '===END==='"
        ),
        proc_limit + 1 // +1 for the header row
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uname_maps_to_platforms() {
        assert_eq!(Platform::from_uname("Linux\n"), Platform::Linux);
        assert_eq!(Platform::from_uname("Darwin\n"), Platform::MacOS);
        assert_eq!(Platform::from_uname(""), Platform::Undetected);
        assert_eq!(
            Platform::from_uname("FreeBSD\n"),
            Platform::Other("FreeBSD".to_string())
        );
    }

    #[test]
    fn unsupported_platforms_have_no_command() {
        assert!(Platform::Other("FreeBSD".into())
            .metrics_command(6)
            .is_none());
        assert!(Platform::Undetected.metrics_command(6).is_none());
        assert!(!Platform::Other("FreeBSD".into()).is_supported());
    }

    #[test]
    fn both_platforms_pin_df_to_1k_blocks() {
        // The macOS 2× disk-size bug came from an unpinned block size.
        for p in [Platform::Linux, Platform::MacOS] {
            let cmd = p.metrics_command(6).unwrap();
            assert!(cmd.contains("df -Pk"), "{} must pin df blocks", p.label());
        }
    }

    #[test]
    fn both_platforms_emit_the_same_envelope() {
        for p in [Platform::Linux, Platform::MacOS] {
            let cmd = p.metrics_command(6).unwrap();
            for section in ["UNAME", "CPUSTAT", "LOADAVG", "DF", "NETDEV", "PS", "END"] {
                assert!(
                    cmd.contains(&format!("==={}===", section)),
                    "{} missing {}",
                    p.label(),
                    section
                );
            }
        }
    }
}
