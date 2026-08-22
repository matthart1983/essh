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

/// Parse `df -Pk` output into DiskInfo entries.
///
/// The `-k` matters: POSIX mode alone leaves the block size implementation-
/// defined and macOS answers in 512-byte blocks, so multiplying by 1024
/// unconditionally — as v1 did — reported every macOS filesystem at twice its
/// real size. With `-k` the 1024 multiplier below is correct on both platforms.
///
/// Filesystem names and mount points can contain spaces (macOS
/// AppTranslocation paths, `map auto_home`), so we locate the numeric columns
/// by scanning from the right: the mount point is the last field (may contain
/// spaces), preceded by Capacity%, Available, Used, and 1K-blocks.
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

        // Skip pseudo-filesystems with no real storage.
        //
        // `overlay` is deliberately NOT in this list. It is the root
        // filesystem of every container, with real capacity behind it —
        // skipping it made `/` vanish from the monitor on any containerised
        // host, leaving a bind-mounted `/etc/hosts` as the only "disk".
        if fs_name == "none"
            || fs_name == "udev"
            || fs_name == "devfs"
            || fs_name == "shm"
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
        // Skip our own collector. The sweep runs `ps` as its last step, so
        // ESSH's batch command and the `ps` inside it both appear in the
        // output they generate — two of the top rows on an idle host were
        // ESSH looking at itself.
        if is_own_collector(&name) {
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

/// Whether a process line is ESSH's own metrics sweep.
///
/// Matched on the section markers the batch command emits, which nothing else
/// on a normal host has any reason to contain.
fn is_own_collector(name: &str) -> bool {
    name.contains("===CPUSTAT===")
        || name.contains("===UPTIME===")
        || name.contains("===PS===")
        || name.contains("===END===")
        || name.trim() == "ps aux --sort=-%cpu"
}

/// Parse `/proc/uptime` output. Returns uptime in seconds.
pub fn parse_uptime(raw: &str) -> u64 {
    raw.split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|f| f as u64)
        .unwrap_or(0)
}

// ─────────────────────────────────────────────────────────────────────────
//  macOS collectors
//
//  Darwin has no /proc, so every group below reads a different source. These
//  are the parsers v1's SPEC promised ("sysctl/vm_stat on macOS") and never
//  shipped — which is why the monitor rendered zeros on a Mac while the
//  terminal beside it printed the real figures.
// ─────────────────────────────────────────────────────────────────────────

/// Parse the CPU figure out of `iostat -c 2 -w 1`.
///
/// ```text
///               disk0               cpu     load average
///     KB/t  tps  MB/s  us sy id   1m   5m   15m
///    26.63   14  0.36   4  3 93  1.37 1.97 2.98
///    12.00    1  0.01   2  1 97  1.37 1.97 2.98
/// ```
///
/// The first row is a since-boot average and the second is the one-second
/// interval we asked for, so we take the *last* data row. The disk columns on
/// the left vary with how many disks are attached, so we index from the right:
/// the final six fields are always `us sy id 1m 5m 15m`.
///
/// Returns `None` rather than `0.0` when the output cannot be read.
pub fn parse_iostat_cpu(raw: &str) -> Option<f64> {
    let last = raw.lines().rfind(|l| {
        let t = l.trim();
        !t.is_empty() && t.chars().next().is_some_and(|c| c.is_ascii_digit())
    })?;

    let parts: Vec<&str> = last.split_whitespace().collect();
    if parts.len() < 6 {
        return None;
    }
    let idle: f64 = parts[parts.len() - 4].parse().ok()?;
    Some((100.0 - idle).clamp(0.0, 100.0))
}

/// Fallback CPU parser for `top -l 2 -n 0 -s 1`.
///
/// ```text
/// CPU usage: 4.76% user, 9.52% sys, 85.71% idle
/// ```
///
/// Same reasoning as above: two samples are printed and the second is the
/// interval one, so take the last.
pub fn parse_top_cpu(raw: &str) -> Option<f64> {
    let line = raw.lines().rfind(|l| l.contains("CPU usage"))?;
    let idle_tok = line.split(',').find(|s| s.contains("idle"))?;
    let idle: f64 = idle_tok
        .trim()
        .trim_end_matches(" idle")
        .trim_end_matches('%')
        .parse()
        .ok()?;
    Some((100.0 - idle).clamp(0.0, 100.0))
}

/// Try `iostat` first, then `top`. Returns `None` if neither produced a figure.
pub fn parse_macos_cpu(raw: &str) -> Option<f64> {
    parse_iostat_cpu(raw).or_else(|| parse_top_cpu(raw))
}

/// Parse `vm_stat` plus `sysctl -n hw.memsize`.
///
/// Returns `(total_kb, used_kb, available_kb)`, or `None` if `vm_stat` could
/// not be read.
///
/// `used` is active + wired + compressor-occupied, matching what Activity
/// Monitor calls memory used. `available` is free + inactive + speculative +
/// purgeable. Total comes from `hw.memsize`, which is authoritative — summing
/// page buckets drifts because some pages are counted twice.
pub fn parse_vm_stat(raw: &str, memsize_raw: &str) -> Option<(u64, u64, u64)> {
    let page_size = raw
        .lines()
        .next()
        .and_then(|l| l.split("page size of ").nth(1))
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(4096);

    let pages = |label: &str| -> u64 {
        raw.lines()
            .find(|l| l.trim_start().starts_with(label))
            .and_then(|l| l.split(':').nth(1))
            .map(|v| v.trim().trim_end_matches('.'))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
    };

    let free = pages("Pages free");
    let active = pages("Pages active");
    let inactive = pages("Pages inactive");
    let speculative = pages("Pages speculative");
    let wired = pages("Pages wired down");
    let purgeable = pages("Pages purgeable");
    let compressed = pages("Pages occupied by compressor");

    // No recognisable page counters at all — vm_stat did not run.
    if free == 0 && active == 0 && wired == 0 {
        return None;
    }

    let total_kb = memsize_raw
        .trim()
        .parse::<u64>()
        .ok()
        .map(|b| b / 1024)
        .unwrap_or_else(|| {
            (free + active + inactive + speculative + wired + compressed) * page_size / 1024
        });

    let used_kb = (active + wired + compressed) * page_size / 1024;
    let avail_kb = (free + inactive + speculative + purgeable) * page_size / 1024;

    Some((total_kb, used_kb.min(total_kb), avail_kb))
}

/// Parse `sysctl -n vm.swapusage`.
///
/// ```text
/// total = 2048.00M  used = 1024.50M  free = 1023.50M  (encrypted)
/// ```
///
/// Returns `(total_kb, used_kb)`. A Mac with swap disabled legitimately
/// reports zeros here, which is a real reading rather than a missing one.
pub fn parse_swapusage(raw: &str) -> Option<(u64, u64)> {
    let field = |name: &str| -> Option<u64> {
        let after = raw.split(&format!("{} = ", name)).nth(1)?;
        let tok = after.split_whitespace().next()?;
        let (num, mult) = match tok.chars().last()? {
            'K' => (&tok[..tok.len() - 1], 1.0),
            'M' => (&tok[..tok.len() - 1], 1024.0),
            'G' => (&tok[..tok.len() - 1], 1024.0 * 1024.0),
            _ => (tok, 1.0 / 1024.0), // bare bytes
        };
        num.parse::<f64>().ok().map(|v| (v * mult) as u64)
    };
    Some((field("total")?, field("used")?))
}

/// Parse `sysctl -n vm.loadavg` — `{ 1.37 1.97 2.98 }`.
pub fn parse_sysctl_loadavg(raw: &str) -> Option<(f64, f64, f64)> {
    let inner = raw.trim().trim_start_matches('{').trim_end_matches('}');
    let vals: Vec<f64> = inner
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    if vals.len() < 3 {
        return None;
    }
    Some((vals[0], vals[1], vals[2]))
}

/// Parse `netstat -ib` into cumulative `(rx_bytes, tx_bytes)`.
///
/// macOS prints one row per interface *per address family*, all carrying the
/// same counters, so naively summing double-counts every interface. Only the
/// `<Link#N>` row holds the interface totals, so that is the only row we take.
///
/// Column positions shift between rows — an interface with a MAC address has
/// an extra field — so after the `<Link#N>` token we take the numeric fields
/// in order: `Ipkts Ierrs Ibytes Opkts Oerrs Obytes`. A MAC address never
/// parses as an integer, so it drops out on its own.
pub fn parse_netstat_ib(raw: &str) -> Option<(u64, u64)> {
    let mut rx_total = 0u64;
    let mut tx_total = 0u64;
    let mut saw_link_row = false;

    for line in raw.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 6 {
            continue;
        }
        let link_idx = match parts.iter().position(|p| p.starts_with("<Link#")) {
            Some(i) => i,
            None => continue,
        };
        // Skip loopback, consistent with the Linux path.
        if parts[0].trim_end_matches('*') == "lo0" {
            continue;
        }
        let nums: Vec<u64> = parts[link_idx + 1..]
            .iter()
            .filter_map(|s| s.parse::<u64>().ok())
            .collect();
        if nums.len() < 6 {
            continue;
        }
        saw_link_row = true;
        rx_total += nums[2]; // Ibytes
        tx_total += nums[5]; // Obytes
    }

    saw_link_row.then_some((rx_total, tx_total))
}

/// Compute uptime from `sysctl -n kern.boottime` and the remote `date +%s`.
///
/// ```text
/// { sec = 1740000000, usec = 123456 } Tue Mar  3 06:36:17 2026
/// ```
///
/// The remote clock is used for "now" rather than ours — a host with a skewed
/// clock should report its own uptime, not one biased by our disagreement.
pub fn parse_boottime(boottime_raw: &str, now_raw: &str) -> Option<u64> {
    let sec: i64 = boottime_raw
        .split("sec = ")
        .nth(1)?
        .split(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty())?
        .parse()
        .ok()?;
    let now: i64 = now_raw.trim().parse().ok()?;
    let up = now - sec;
    (up >= 0).then_some(up as u64)
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

    // ─────────────────────────────────────────────────────────────────────
    //  macOS golden fixtures
    //
    //  Every string below is literal output captured from a running macOS
    //  15 host, not hand-written. Finding 1 (the monitor rendering zeros)
    //  survived because no test ever fed the collector real Darwin output.
    // ─────────────────────────────────────────────────────────────────────

    const IOSTAT_GOLDEN: &str = "\
              disk0       cpu    load average
    KB/t  tps  MB/s  us sy id   1m   5m   15m
   22.72  170  3.77   5  4 91  0.75 1.47 1.65
   24.00    2  0.05   2  1 98  0.75 1.47 1.65
";

    const VM_STAT_GOLDEN: &str = "\
Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                               17878.
Pages active:                            363979.
Pages inactive:                          330134.
Pages speculative:                        32689.
Pages throttled:                              0.
Pages wired down:                        103518.
Pages purgeable:                          22167.
\"Translation faults\":                  51949863.
Pages copy-on-write:                    2832235.
Pages zero filled:                     29310903.
Pages reactivated:                      2827850.
Pages stored in compressor:              673522.
Pages occupied by compressor:            277642.
";

    const NETSTAT_IB_GOLDEN: &str = "\
Name       Mtu   Network       Address            Ipkts Ierrs     Ibytes    Opkts Oerrs     Obytes  Coll
lo0        16384 <Link#1>                        113504     0  764081721   113504     0  764081721     0
lo0        16384 127           localhost         113504     -  764081721   113504     -  764081721     -
lo0        16384 localhost   ::1                 113504     -  764081721   113504     -  764081721     -
gif0*      1280  <Link#2>                             0     0          0        0     0          0     0
en0        1500  <Link#14>   4e:ac:1c:ae:e2:ff  2932778     0 3899988586   841279     0  772923171     0
en0        1500  192.168.0   192.168.0.7        2932778     - 3899988586   841279     -  772923171     -
";

    #[test]
    fn container_root_is_not_dropped_as_a_pseudo_filesystem() {
        // Real `df -Pk` from a Debian container. `overlay` is the root
        // filesystem with 511 GiB behind it; skipping it left a bind-mounted
        // /etc/hosts as the only "disk" the monitor showed.
        let raw = "Filesystem     1024-blocks    Used Available Capacity Mounted on\n\
                   overlay          535884848 6187032 529697816       2% /\n\
                   tmpfs                65536       0     65536       0% /dev\n\
                   shm                4600832       0   4600832       0% /dev/shm\n\
                   /dev/vdb1        535884848 6187032 529697816       2% /etc/hosts\n\
                   tmpfs              4601476       0   4601476       0% /sys/firmware\n";
        let disks = parse_df(raw);
        assert!(
            disks.iter().any(|d| d.mount == "/"),
            "the container root must survive parsing"
        );

        let visible: Vec<&DiskInfo> = disks.iter().filter(|d| d.is_user_visible()).collect();
        assert_eq!(visible.len(), 1, "only / carries user data here");
        assert_eq!(visible[0].mount, "/");
        // The bind mount reports the host's full capacity and must not appear.
        assert!(!visible.iter().any(|d| d.mount == "/etc/hosts"));
    }

    #[test]
    fn the_collector_does_not_report_itself_as_a_process() {
        // Both of these are ESSH's own sweep, seen in its own `ps` output.
        let raw = "USER PID %CPU %MEM VSZ RSS TTY STAT START TIME COMMAND\n\
                   root 162 0.0 0.0 2800 2700 ?  Ss 16:11 0:00 sh -c cat /proc/net/dev; echo '===UPTIME==='; cat /proc/uptime; echo '===PS==='\n\
                   root 170 0.0 0.0 4000 4000 ?  R  16:11 0:00 ps aux --sort=-%cpu\n\
                   root 1   0.0 0.0 2700 2700 ?  Ss 16:00 0:00 /usr/sbin/sshd -D -e\n";
        let procs = parse_ps(raw, 10);
        assert_eq!(procs.len(), 1, "only the real process should remain");
        assert!(procs[0].name.contains("sshd"));
    }

    #[test]
    fn macos_cpu_uses_the_interval_sample_not_the_since_boot_one() {
        // Row 1 is since-boot (91% idle), row 2 is the 1s interval (98% idle).
        // Taking the wrong row is a subtle, permanent overstatement.
        let cpu = parse_iostat_cpu(IOSTAT_GOLDEN).expect("iostat parses");
        assert!((cpu - 2.0).abs() < 0.01, "got {}", cpu);
    }

    #[test]
    fn macos_cpu_falls_back_to_top() {
        let raw = "Processes: 602 total, 2 running\n\
                   CPU usage: 12.50% user, 7.50% sys, 80.00% idle\n\
                   Processes: 602 total, 2 running\n\
                   CPU usage: 4.76% user, 9.52% sys, 85.71% idle\n";
        let cpu = parse_top_cpu(raw).expect("top parses");
        assert!((cpu - 14.29).abs() < 0.01, "got {}", cpu);
        // parse_macos_cpu prefers iostat but must handle top-only output.
        assert!(parse_macos_cpu(raw).is_some());
        assert!(parse_macos_cpu(IOSTAT_GOLDEN).is_some());
    }

    #[test]
    fn macos_cpu_returns_none_rather_than_zero_on_garbage() {
        // The whole failure mode of v1 in one assertion: no output must never
        // become a confident 0.0%.
        assert!(parse_macos_cpu("").is_none());
        assert!(parse_macos_cpu("sh: iostat: command not found").is_none());
    }

    #[test]
    fn macos_memory_matches_the_host_it_was_captured_from() {
        let (total_kb, used_kb, avail_kb) =
            parse_vm_stat(VM_STAT_GOLDEN, "19327352832\n").expect("vm_stat parses");

        // hw.memsize is authoritative: 18 GiB exactly.
        assert_eq!(total_kb, 19327352832 / 1024);
        // used = (active + wired + compressor) * 16 KiB
        assert_eq!(used_kb, (363979 + 103518 + 277642) * 16);
        // available = (free + inactive + speculative + purgeable) * 16 KiB
        assert_eq!(avail_kb, (17878 + 330134 + 32689 + 22167) * 16);
        // Sanity: used must land inside the machine's real memory.
        assert!(used_kb < total_kb);
        let pct = used_kb as f64 / total_kb as f64 * 100.0;
        assert!((50.0..75.0).contains(&pct), "implausible {}%", pct);
    }

    #[test]
    fn macos_memory_picks_occupied_compressor_not_stored() {
        // Both lines start with "Pages ", and "stored" appears first and is
        // more than twice as large. Matching the wrong one inflates used
        // memory by ~6 GiB on this fixture.
        let (_, used_kb, _) = parse_vm_stat(VM_STAT_GOLDEN, "19327352832").unwrap();
        let with_stored = (363979 + 103518 + 673522) * 16;
        assert_ne!(used_kb, with_stored);
    }

    #[test]
    fn macos_memory_returns_none_when_vm_stat_did_not_run() {
        assert!(parse_vm_stat("", "19327352832").is_none());
        assert!(parse_vm_stat("sh: vm_stat: not found", "").is_none());
    }

    #[test]
    fn macos_memory_falls_back_to_page_sum_without_memsize() {
        let (total_kb, _, _) = parse_vm_stat(VM_STAT_GOLDEN, "").expect("still parses");
        assert!(total_kb > 0);
    }

    #[test]
    fn macos_swap_disabled_is_a_real_zero_not_a_missing_reading() {
        let (total, used) =
            parse_swapusage("total = 0.00M  used = 0.00M  free = 0.00M  (encrypted)\n").unwrap();
        assert_eq!((total, used), (0, 0));

        let (total, used) =
            parse_swapusage("total = 2048.00M  used = 1024.50M  free = 1023.50M  (encrypted)")
                .unwrap();
        assert_eq!(total, 2048 * 1024);
        assert_eq!(used, 1024 * 1024 + 512);

        assert!(parse_swapusage("").is_none());
    }

    #[test]
    fn macos_loadavg_parses_the_sysctl_braces() {
        let (l1, l5, l15) = parse_sysctl_loadavg("{ 0.75 1.47 1.65 }\n").unwrap();
        assert!((l1 - 0.75).abs() < 0.001);
        assert!((l5 - 1.47).abs() < 0.001);
        assert!((l15 - 1.65).abs() < 0.001);
        assert!(parse_sysctl_loadavg("").is_none());
    }

    #[test]
    fn macos_netstat_counts_each_interface_once() {
        // en0 appears twice (Link row + IPv4 row) carrying identical counters.
        // Summing every row double-counts the machine's entire traffic.
        let (rx, tx) = parse_netstat_ib(NETSTAT_IB_GOLDEN).expect("netstat parses");
        assert_eq!(rx, 3_899_988_586, "en0 rx counted once");
        assert_eq!(tx, 772_923_171, "en0 tx counted once");
    }

    #[test]
    fn macos_netstat_skips_loopback_like_the_linux_path() {
        // lo0 carries 764 MB in this fixture; including it would swamp the
        // real interface and disagree with what /proc/net/dev reports.
        let (rx, _) = parse_netstat_ib(NETSTAT_IB_GOLDEN).unwrap();
        assert!(rx < 764_081_721 + 3_899_988_586);
    }

    #[test]
    fn macos_netstat_tolerates_interfaces_with_and_without_a_mac() {
        // gif0* has no Address column; en0 has a MAC. Both must parse without
        // the column shift throwing the byte counters off.
        let raw =
            "Name  Mtu  Network   Address        Ipkts Ierrs Ibytes Opkts Oerrs Obytes Coll\n\
                   gif0* 1280 <Link#2>                    0     0      0     0     0      0    0\n\
                   en0   1500 <Link#14> 4e:ac:1c:ae:e2:ff 10    0    500    20     0    900    0\n";
        assert_eq!(parse_netstat_ib(raw).unwrap(), (500, 900));
        assert!(parse_netstat_ib("").is_none());
    }

    #[test]
    fn macos_uptime_uses_the_remote_clock() {
        let boot = "{ sec = 1786755015, usec = 888275 } Sat Aug 15 10:50:15 2026\n";
        assert_eq!(parse_boottime(boot, "1786764850\n").unwrap(), 9835);
        // A remote clock behind its own boot time is nonsense, not a negative.
        assert!(parse_boottime(boot, "1786755000").is_none());
        assert!(parse_boottime("", "1786764850").is_none());
    }

    #[test]
    fn macos_df_reports_true_sizes_with_k_blocks() {
        // Captured from `df -Pk`. Under v1's unpinned `df -P`, macOS answered
        // in 512-byte blocks and every size came out doubled.
        let raw = "Filesystem     1024-blocks      Used Available Capacity  Mounted on\n\
                   /dev/disk3s1s1   971350180  17380616 562007844     3%    /\n\
                   devfs                  350       350         0   100%    /dev\n\
                   /dev/disk3s6     971350180        20 562007844     1%    /System/Volumes/VM\n";
        let disks = parse_df(raw);
        assert_eq!(disks.len(), 2); // devfs skipped
        assert_eq!(disks[0].mount, "/");
        // 971350180 KiB ≈ 926 GiB, i.e. a 1 TB disk — not 1.8 TB.
        let gib = disks[0].total_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
        assert!((900.0..950.0).contains(&gib), "got {} GiB", gib);
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
