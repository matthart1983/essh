//! Benchmarks, per §9's *"Performance should be benchmarked and published."*
//!
//! The spec's own targets are mostly unfalsifiable — "startup under 100 ms"
//! is free in Rust and "low input latency" cannot be failed. These measure
//! things that can actually come out badly, and each prints the number rather
//! than a pass/fail, so a regression is visible even when it stays inside a
//! threshold.
//!
//! What is deliberately *not* measured here is stated in the output: input
//! latency and throughput against a real host need a real host, and inventing
//! a number for them locally would be worse than admitting the gap.

use std::time::{Duration, Instant};

pub struct BenchResult {
    pub name: String,
    pub detail: String,
    /// The headline measurement.
    pub value: f64,
    pub unit: &'static str,
    /// The target from §9, when there is one.
    pub target: Option<f64>,
}

impl BenchResult {
    pub fn within_target(&self) -> Option<bool> {
        self.target.map(|t| self.value <= t)
    }
}

fn time_it<F: FnMut()>(iterations: usize, mut f: F) -> Duration {
    // One warm-up pass so the first allocation is not counted as the cost.
    f();
    let start = Instant::now();
    for _ in 0..iterations {
        f();
    }
    start.elapsed() / iterations as u32
}

/// Parsing the user's ssh_config, which happens on every launch.
pub fn bench_ssh_config(text: &str) -> BenchResult {
    let elapsed = time_it(200, || {
        let cfg = crate::sshconfig::SshConfig::parse_str(text);
        std::hint::black_box(cfg.aliases());
    });
    BenchResult {
        name: "ssh_config parse".into(),
        detail: format!("{} bytes", text.len()),
        value: elapsed.as_secs_f64() * 1000.0,
        unit: "ms",
        target: Some(10.0),
    }
}

/// Ranking the launcher, which runs on every keystroke.
///
/// This is the one with a real user-facing deadline: it happens between a
/// key going down and the list redrawing, so it has to stay well inside a
/// frame.
pub fn bench_launcher(hosts: usize) -> BenchResult {
    let candidates: Vec<crate::launcher::Candidate> = (0..hosts)
        .map(|i| crate::launcher::Candidate {
            alias: format!("prod-api-{:03}", i),
            hostname: format!("10.0.{}.{}", i / 256, i % 256),
            port: 22,
            user: Some("deploy".into()),
            tags: vec![("role".into(), "web".into()), ("env".into(), "prod".into())],
            last_used_secs: Some(i as u64 * 60),
            source: crate::launcher::Source::SshConfig,
            delegated: None,
        })
        .collect();

    let elapsed = time_it(200, || {
        std::hint::black_box(crate::launcher::search(&candidates, "pa1"));
    });
    BenchResult {
        name: "launcher search".into(),
        detail: format!("{} hosts, per keystroke", hosts),
        value: elapsed.as_secs_f64() * 1000.0,
        unit: "ms",
        target: Some(16.0), // one 60fps frame
    }
}

/// Feeding bytes through the virtual terminal — the path every byte of
/// remote output takes.
pub fn bench_vt_throughput() -> BenchResult {
    // A realistic mix: text, newlines and SGR colour changes, which is what
    // build output and log tails actually look like.
    let mut chunk = String::new();
    for i in 0..200 {
        chunk.push_str(&format!(
            "\x1b[3{}mline {:04}\x1b[0m  some output text here\r\n",
            i % 8,
            i
        ));
    }
    let bytes = chunk.as_bytes();
    let total = bytes.len();

    let mut parser = vt100::Parser::new(40, 120, 0);
    let start = Instant::now();
    let rounds = 200;
    for _ in 0..rounds {
        parser.process(bytes);
    }
    let elapsed = start.elapsed();
    let mib = (total * rounds) as f64 / (1024.0 * 1024.0);

    BenchResult {
        name: "VT parse throughput".into(),
        detail: "coloured log output through vt100".into(),
        value: mib / elapsed.as_secs_f64(),
        unit: "MiB/s",
        target: None,
    }
}

/// Building one Host Monitor frame through ratatui.
///
/// This exists to be compared against the same screen in `essh-renderer`,
/// which is the only way the renderer question gets an honest answer. Both
/// measure the same thing — data in, drawable output out, no I/O — so the
/// difference between them is attributable to the rendering approach rather
/// than to what is being drawn.
///
/// Note what the comparison does *not* prove: ratatui is not slow here, and
/// nobody should adopt a GPU renderer for frame cost. The number matters only
/// as the price of admission — if owning the renderer cost frame time, the
/// design arguments for it would not survive.
pub fn bench_tui_frame() -> BenchResult {
    use crate::monitor::history::MetricHistory;
    use crate::monitor::{DiskInfo, HostMetrics, MetricState, ProcessInfo};
    use crate::tui::host_monitor::{self, PeerContext, ProcessSort};
    use ratatui::{backend::TestBackend, Terminal};

    // 1060×642 at the design's cell metrics is about 132×36 cells, so both
    // sides of the comparison draw the same screen at the same size.
    let mut term = Terminal::new(TestBackend::new(132, 36)).expect("test backend");

    let mut metrics = HostMetrics {
        cpu_percent: 22.4,
        cpu_per_core: vec![18.0, 31.0, 12.0, 44.0, 9.0, 27.0, 15.0, 33.0],
        mem_total_kb: 16 * 1024 * 1024,
        mem_used_kb: 15 * 1024 * 1024,
        mem_available_kb: 1024 * 1024,
        load_1m: 1.37,
        load_5m: 1.12,
        load_15m: 0.94,
        disks: vec![
            DiskInfo {
                mount: "/System/Volumes/Data".into(),
                total_bytes: 500 * 1024 * 1024 * 1024,
                used_bytes: 183 * 1024 * 1024 * 1024,
                use_pct: 42.0,
            },
            DiskInfo {
                mount: "/".into(),
                total_bytes: 500 * 1024 * 1024 * 1024,
                used_bytes: 24 * 1024 * 1024 * 1024,
                use_pct: 9.0,
            },
        ],
        net_rx_bps: 111_000.0,
        net_tx_bps: 3_000.0,
        uptime_secs: 1200,
        os_info: "Linux web-01 6.1.0-15-amd64".into(),
        ..Default::default()
    };
    metrics.top_procs_cpu = (0..12)
        .map(|i| ProcessInfo {
            pid: 1000 + i,
            name: format!("/usr/libexec/worker-{i} --serve"),
            cpu_pct: 12.0 - i as f64 * 0.8,
            mem_pct: 2.0,
            mem_rss_kb: 118_800,
            state: "S".into(),
        })
        .collect();
    metrics.top_procs_mem = metrics.top_procs_cpu.clone();
    // Everything drawn, so the bench measures a full frame rather than a
    // screen full of "unavailable" explanations.
    for slot in [
        &mut metrics.status.cpu,
        &mut metrics.status.mem,
        &mut metrics.status.load,
        &mut metrics.status.disk,
        &mut metrics.status.net,
        &mut metrics.status.procs,
        &mut metrics.status.uptime,
    ] {
        *slot = MetricState::Collected;
    }

    let hist = || {
        let mut h = MetricHistory::new(120);
        for i in 0..120 {
            h.push((20.0 + ((i as f64) * 0.31).sin() * 18.0) as u64);
        }
        h
    };
    let (cpu, mem, rx, tx) = (hist(), hist(), hist(), hist());
    let sort = ProcessSort::Cpu;
    let peers = PeerContext {
        cpu_median_pct: Some(31.0),
        mem_median_gb: Some(11.2),
        peers: 39,
    };
    let theme = crate::theme::dark();

    let elapsed = time_it(200, || {
        term.draw(|f| {
            let area = f.area();
            host_monitor::render(
                f, area, &metrics, &cpu, &mem, &rx, &tx, &sort, 0, &peers, &theme,
            );
        })
        .expect("draw");
    });

    BenchResult {
        name: "TUI frame (monitor)".into(),
        detail: "132×36 cells, ratatui".into(),
        value: elapsed.as_secs_f64() * 1000.0,
        unit: "ms",
        target: Some(16.67),
    }
}

/// Comparing a host against its peers, which runs on every facet refresh.
pub fn bench_divergence(hosts: usize) -> BenchResult {
    use crate::divergence::*;
    use std::collections::HashMap;

    let mut all = HashMap::new();
    let mut names = Vec::new();
    for i in 0..hosts {
        let name = format!("web-{:03}", i);
        let mut f = HostFacts::new(&name);
        f.facets.insert(
            FacetKey::Kernel,
            FacetValue::Text(if i == 7 { "6.1.0-15".into() } else { "6.1.0-18".into() }),
        );
        f.facets
            .insert(FacetKey::OsRelease, FacetValue::Text("debian 12".into()));
        f.facets.insert(
            FacetKey::DiskRootPct,
            FacetValue::Number(40.0 + (i % 6) as f64),
        );
        f.facets.insert(
            FacetKey::FileHash("/etc/nginx/nginx.conf".into()),
            FacetValue::Text("aaaa1111".into()),
        );
        all.insert(name.clone(), f);
        names.push(name);
    }
    let set = PeerSet {
        selector: ("role".into(), "web".into()),
        hosts: names,
    };

    let elapsed = time_it(50, || {
        std::hint::black_box(consensus(&set, &all));
    });

    BenchResult {
        name: "divergence consensus".into(),
        detail: format!("{} hosts × 4 facets, full recompute", hosts),
        value: elapsed.as_secs_f64() * 1000.0,
        unit: "ms",
        target: Some(50.0),
    }
}

/// Run everything and print a table.
pub fn run_all() {
    let sample_config = {
        let mut s = String::new();
        for i in 0..200 {
            s.push_str(&format!(
                "Host prod-api-{:03}\n  HostName 10.0.{}.{}\n  User deploy\n  Port 22\n\n",
                i,
                i / 256,
                i % 256
            ));
        }
        s.push_str("Host *\n  ServerAliveInterval 30\n");
        s
    };

    let results = vec![
        bench_ssh_config(&sample_config),
        bench_launcher(500),
        bench_vt_throughput(),
        bench_divergence(40),
        bench_tui_frame(),
    ];

    println!("ESSH benchmarks  ({}, {})", std::env::consts::OS, std::env::consts::ARCH);
    println!();
    for r in &results {
        let verdict = match r.within_target() {
            Some(true) => "  ✓",
            Some(false) => "  ✗ over target",
            None => "",
        };
        let target = match r.target {
            Some(t) => format!("  (target {:.0}{})", t, r.unit),
            None => String::new(),
        };
        println!(
            "  {:<22} {:>9.3} {:<7} {}{}",
            r.name, r.value, r.unit, r.detail, verdict
        );
        if !target.is_empty() {
            println!("  {:<22} {}", "", target.trim());
        }
    }

    println!();
    println!("Not measured here, because it needs a real host:");
    println!("  · added keystroke→echo latency versus plain `ssh`");
    println!("  · sustained output throughput over a live SSH channel");
    println!("  · memory per idle session at n=30");
    println!();
    println!("Not measurable in process at all:");
    println!("  · input-to-photon latency, here or in any other terminal.");
    println!("    It spans the window server before the event and the");
    println!("    compositor after present, so comparing against Ghostty or");
    println!("    plain ssh needs a high-speed camera or a photodiode.");
    println!("    `essh-renderer window` reports the part we can see.");
    println!();
    println!("Those are the numbers §9 actually cares about. Measuring them");
    println!("locally would produce a figure that is precise and meaningless.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_config_parsing_stays_fast_enough_for_startup() {
        let mut cfg = String::new();
        for i in 0..200 {
            cfg.push_str(&format!("Host h{}\n  HostName 10.0.0.{}\n", i, i % 256));
        }
        let r = bench_ssh_config(&cfg);
        assert_eq!(r.within_target(), Some(true), "took {:.2}ms", r.value);
    }

    #[test]
    fn launcher_search_fits_inside_a_frame_at_fleet_scale() {
        // 500 hosts is a large fleet; this runs on every keystroke, so
        // exceeding a frame would be visible as input lag.
        let r = bench_launcher(500);
        assert_eq!(r.within_target(), Some(true), "took {:.2}ms", r.value);
    }

    #[test]
    fn divergence_recompute_is_not_a_stall() {
        let r = bench_divergence(40);
        assert_eq!(r.within_target(), Some(true), "took {:.2}ms", r.value);
    }

    #[test]
    fn vt_throughput_is_reported_without_a_pass_fail_claim() {
        // No target: there is no defensible threshold without a baseline to
        // compare against, and inventing one would make the check theatre.
        let r = bench_vt_throughput();
        assert!(r.target.is_none());
        assert!(r.value > 0.0, "throughput must be measurable");
    }
}
