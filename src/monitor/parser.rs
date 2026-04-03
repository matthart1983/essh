use super::{DiskInfo, ProcessInfo};

/// Parse `/proc/stat` output. Returns (aggregate_cpu_pct, per_core_pcts).
/// Takes current and previous raw output to compute deltas.
pub fn parse_cpu(current: &str, previous: &str) -> (f64, Vec<f64>) {
    let curr_cpus = parse_cpu_lines(current);
    let prev_cpus = parse_cpu_lines(previous);

    let mut per_core = Vec::new();
    let mut aggregate = 0.0;

    for (i, (curr, prev)) in curr_cpus.iter().zip(prev_cpus.iter()).enumerate() {
        let idle_delta = curr.idle.saturating_sub(prev.idle) as f64;
        let total_delta = curr.total.saturating_sub(prev.total) as f64;
        if total_delta > 0.0 {
            let usage = (1.0 - idle_delta / total_delta) * 100.0;
            if i == 0 {
                aggregate = usage;
            } else {
                per_core.push(usage);
            }
        }
    }
    (aggregate, per_core)
}

struct CpuTimes {
    idle: u64,
    total: u64,
}

fn parse_cpu_lines(raw: &str) -> Vec<CpuTimes> {
    let mut result = Vec::new();
    for line in raw.lines() {
        if !line.starts_with("cpu") {
            continue;
        }
        let parts: Vec<u64> = line
            .split_whitespace()
            .skip(1) // skip "cpu" or "cpuN"
            .filter_map(|s| s.parse().ok())
            .collect();
        if parts.len() >= 4 {
            let idle = parts[3] + parts.get(4).copied().unwrap_or(0); // idle + iowait
            let total: u64 = parts.iter().sum();
            result.push(CpuTimes { idle, total });
        }
    }
    result
}

/// Parse `/proc/meminfo` output.
/// Returns (total_kb, used_kb, available_kb, swap_total_kb, swap_used_kb)
pub fn parse_meminfo(raw: &str) -> (u64, u64, u64, u64, u64) {
    let mut total = 0u64;
    let mut free = 0u64;
    let mut available = 0u64;
    let mut buffers = 0u64;
    let mut cached = 0u64;
    let mut swap_total = 0u64;
    let mut swap_free = 0u64;

    for line in raw.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let val: u64 = parts[1].parse().unwrap_or(0);
        match parts[0] {
            "MemTotal:" => total = val,
            "MemFree:" => free = val,
            "MemAvailable:" => available = val,
            "Buffers:" => buffers = val,
            "Cached:" => cached = val,
            "SwapTotal:" => swap_total = val,
            "SwapFree:" => swap_free = val,
            _ => {}
        }
    }

    // If MemAvailable not present, estimate it
    if available == 0 {
        available = free + buffers + cached;
    }
    let used = total.saturating_sub(available);
    let swap_used = swap_total.saturating_sub(swap_free);

    (total, used, available, swap_total, swap_used)
}

/// Parse `/proc/loadavg` output.
/// Returns (load_1m, load_5m, load_15m)
pub fn parse_loadavg(raw: &str) -> (f64, f64, f64) {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    let l1 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let l5 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let l15 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    (l1, l5, l15)
}

/// Parse `df -P` output into DiskInfo entries.
///
/// `df -P` guarantees one-line-per-entry POSIX format, but filesystem names
/// and mount points can contain spaces (e.g. macOS AppTranslocation paths,
/// `map auto_home`).  We locate the numeric columns by scanning from the right
/// side of each line: the mount point is the last field (may contain spaces),
/// preceded by Capacity%, Available, Used, and 1K-blocks.
pub fn parse_df(raw: &str) -> Vec<DiskInfo> {
    let mut disks = Vec::new();
    for line in raw.lines().skip(1) {
        // skip header
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Strategy: split from the right.  The POSIX format is:
        //   Filesystem  1K-blocks  Used  Available  Capacity  Mounted-on
        // "Mounted-on" may contain spaces, so we find Capacity% first by
        // scanning right-to-left for a token matching \d+%.
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 6 {
            continue;
        }

        // Find the Capacity% column index (right-to-left, looks like "42%" or "100%")
        let cap_idx = match parts
            .iter()
            .rposition(|p| p.ends_with('%') && p.trim_end_matches('%').parse::<u64>().is_ok())
        {
            Some(i) if i >= 3 => i, // need at least fs, blocks, used, avail before it
            _ => continue,
        };

        // Fields before Capacity%
        let avail_idx = cap_idx - 1;
        let used_idx = cap_idx - 2;
        let blocks_idx = cap_idx - 3;

        // Filesystem is everything before blocks_idx (may contain spaces)
        let fs_name: String = parts[..blocks_idx].join(" ");

        // Mount point is everything after cap_idx (may contain spaces)
        let mount = if cap_idx + 1 < parts.len() {
            parts[cap_idx + 1..].join(" ")
        } else {
            continue;
        };

        // Skip pseudo-filesystems with no real storage
        if fs_name == "none"
            || fs_name == "udev"
            || fs_name == "overlay"
            || fs_name == "devfs"
            || fs_name.starts_with("map ")
        {
            continue;
        }

        let total: u64 = parts[blocks_idx].parse().unwrap_or(0) * 1024;
        let used: u64 = parts[used_idx].parse().unwrap_or(0) * 1024;
        let _avail: u64 = parts[avail_idx].parse().unwrap_or(0) * 1024;
        let use_pct_str = parts[cap_idx].trim_end_matches('%');
        let use_pct: f64 = use_pct_str.parse().unwrap_or(0.0);

        // Skip entries with zero total (virtual mounts)
        if total == 0 {
            continue;
        }

        disks.push(DiskInfo {
            mount,
            total_bytes: total,
            used_bytes: used,
            use_pct,
        });
    }
    disks
}

/// Parse `ps aux --sort=-%cpu` or similar output into ProcessInfo.
/// Expects format: USER PID %CPU %MEM VSZ RSS TTY STAT START TIME COMMAND
pub fn parse_ps(raw: &str, limit: usize) -> Vec<ProcessInfo> {
    let mut procs = Vec::new();
    for line in raw.lines().skip(1) {
        // skip header
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 11 {
            continue;
        }
        let pid: u32 = parts[1].parse().unwrap_or(0);
        let cpu_pct: f64 = parts[2].parse().unwrap_or(0.0);
        let mem_pct: f64 = parts[3].parse().unwrap_or(0.0);
        let rss: u64 = parts[5].parse().unwrap_or(0); // RSS in KB
        let state = parts[7].to_string();
        let name = parts[10..].join(" ");
        // Skip kernel threads (names in brackets)
        if name.starts_with('[') {
            continue;
        }
        procs.push(ProcessInfo {
            pid,
            name,
            cpu_pct,
            mem_pct,
            mem_rss_kb: rss,
            state,
        });
        if procs.len() >= limit {
            break;
        }
    }
    procs
}

/// Parse `/proc/uptime` output. Returns uptime in seconds.
pub fn parse_uptime(raw: &str) -> u64 {
    raw.split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|f| f as u64)
        .unwrap_or(0)
}

/// Parse `/proc/net/dev` output.
/// Takes current and previous raw output plus elapsed seconds.
/// Returns (rx_bps, tx_bps).
pub fn parse_net_dev(current: &str, previous: &str, elapsed_secs: f64) -> (f64, f64) {
    let curr = sum_net_bytes(current);
    let prev = sum_net_bytes(previous);

    let rx_delta = curr.0.saturating_sub(prev.0) as f64;
    let tx_delta = curr.1.saturating_sub(prev.1) as f64;

    if elapsed_secs > 0.0 {
        (rx_delta / elapsed_secs, tx_delta / elapsed_secs)
    } else {
        (0.0, 0.0)
    }
}

fn sum_net_bytes(raw: &str) -> (u64, u64) {
    let mut rx_total = 0u64;
    let mut tx_total = 0u64;
    for line in raw.lines() {
        let line = line.trim();
        if !line.contains(':') || line.starts_with("Inter") || line.starts_with("face") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 10 {
            continue;
        }
        let iface = parts[0].trim_end_matches(':');
        if iface == "lo" {
            continue;
        } // skip loopback
        let rx: u64 = parts[1].parse().unwrap_or(0);
        let tx: u64 = parts[9].parse().unwrap_or(0);
        rx_total += rx;
        tx_total += tx;
    }
    (rx_total, tx_total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_meminfo() {
        let raw = "MemTotal:       16384000 kB\n\
                    MemFree:         2048000 kB\n\
                    MemAvailable:    8192000 kB\n\
                    Buffers:          512000 kB\n\
                    Cached:          4096000 kB\n\
                    SwapTotal:       4096000 kB\n\
                    SwapFree:        3072000 kB\n";
        let (total, used, avail, swap_total, swap_used) = parse_meminfo(raw);
        assert_eq!(total, 16384000);
        assert_eq!(avail, 8192000);
        assert_eq!(used, 16384000 - 8192000);
        assert_eq!(swap_total, 4096000);
        assert_eq!(swap_used, 1024000);
    }

    #[test]
    fn test_parse_loadavg() {
        let (l1, l5, l15) = parse_loadavg("0.42 0.38 0.35 1/234 5678\n");
        assert!((l1 - 0.42).abs() < 0.001);
        assert!((l5 - 0.38).abs() < 0.001);
        assert!((l15 - 0.35).abs() < 0.001);
    }

    #[test]
    fn test_parse_uptime() {
        assert_eq!(parse_uptime("3641234.56 7282469.12\n"), 3641234);
    }

    #[test]
    fn test_parse_df() {
        let raw = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n\
                    /dev/sda1       102400000  24576000  77824000      24% /\n\
                    tmpfs             8192000         0   8192000       0% /dev/shm\n\
                    none              8192000         0   8192000       0% /run/lock\n\
                    /dev/sdb1       512000000 419430400  92569600      82% /data\n";
        let disks = parse_df(raw);
        assert_eq!(disks.len(), 3); // 'none' skipped, tmpfs kept
        assert_eq!(disks[0].mount, "/");
        assert!((disks[0].use_pct - 24.0).abs() < 0.1);
        assert_eq!(disks[1].mount, "/dev/shm");
        assert_eq!(disks[2].mount, "/data");
    }

    #[test]
    fn test_parse_df_macos_spaces() {
        let raw = "Filesystem                               512-blocks      Used Available Capacity  Mounted on\n\
                    /dev/disk3s1s1                            478724992  24017264 256100400     9%    /\n\
                    devfs                                           405       405         0   100%    /dev\n\
                    /dev/disk3s5                              478724992 179601016 256100400    42%    /System/Volumes/Data\n\
                    map auto_home                                     0         0         0   100%    /System/Volumes/Data/home\n\
                    /Users/mattbot/Downloads/Geekbench 6.app  478724992 179130792 256570640    42%    /private/var/folders/6c/T/AppTranslocation/foo\n";
        let disks = parse_df(raw);
        // devfs, map auto_home skipped; Geekbench 6.app (space in fs) should parse OK
        assert_eq!(disks.len(), 3);
        assert_eq!(disks[0].mount, "/");
        assert!((disks[0].use_pct - 9.0).abs() < 0.1);
        assert_eq!(disks[1].mount, "/System/Volumes/Data");
        assert!((disks[1].use_pct - 42.0).abs() < 0.1);
        // Filesystem with space in name
        assert_eq!(
            disks[2].mount,
            "/private/var/folders/6c/T/AppTranslocation/foo"
        );
        assert!((disks[2].use_pct - 42.0).abs() < 0.1);
    }

    #[test]
    fn test_parse_ps() {
        let raw = "USER       PID %CPU %MEM    VSZ   RSS TTY      STAT START   TIME COMMAND\n\
                    postgres  1842 28.3 12.1 500000 196608 ?       Ss   Jan01 100:00 /usr/lib/postgresql/14/bin/postgres\n\
                    node      2103 14.7  8.4 400000 134217 ?       Sl   Jan01  50:00 node /app/server.js\n\
                    root       891  3.2  0.8  50000  12800 ?       Ss   Jan01  10:00 nginx: master process\n";
        let procs = parse_ps(raw, 10);
        assert_eq!(procs.len(), 3);
        assert_eq!(procs[0].pid, 1842);
        assert!((procs[0].cpu_pct - 28.3).abs() < 0.1);
        assert!(procs[0].name.contains("postgres"));
    }
}
