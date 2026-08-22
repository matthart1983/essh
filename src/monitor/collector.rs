use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use russh::client::Handle;
use tokio::sync::RwLock;

use super::{
    history::MetricHistory, parser, platform::Platform, CollectionStatus, HostMetrics, MetricState,
};

#[derive(Clone)]
pub struct HostMetricsCollector {
    metrics: Arc<RwLock<HostMetrics>>,
    pub cpu_history: Arc<RwLock<MetricHistory>>,
    pub mem_history: Arc<RwLock<MetricHistory>>,
    pub net_rx_history: Arc<RwLock<MetricHistory>>,
    pub net_tx_history: Arc<RwLock<MetricHistory>>,
    prev_cpu_raw: Arc<RwLock<String>>,
    prev_net_counters: Arc<RwLock<Option<(u64, u64)>>>,
    last_net_time: Arc<RwLock<Instant>>,
    /// Detected once per session and reused; `uname` does not change under us.
    platform: Arc<RwLock<Platform>>,
    process_count: usize,
}

impl HostMetricsCollector {
    pub fn new(history_samples: usize, process_count: usize) -> Self {
        Self {
            metrics: Arc::new(RwLock::new(HostMetrics::default())),
            cpu_history: Arc::new(RwLock::new(MetricHistory::new(history_samples))),
            mem_history: Arc::new(RwLock::new(MetricHistory::new(history_samples))),
            net_rx_history: Arc::new(RwLock::new(MetricHistory::new(history_samples))),
            net_tx_history: Arc::new(RwLock::new(MetricHistory::new(history_samples))),
            prev_cpu_raw: Arc::new(RwLock::new(String::new())),
            prev_net_counters: Arc::new(RwLock::new(None)),
            last_net_time: Arc::new(RwLock::new(Instant::now())),
            platform: Arc::new(RwLock::new(Platform::Undetected)),
            process_count,
        }
    }

    pub fn metrics(&self) -> Arc<RwLock<HostMetrics>> {
        Arc::clone(&self.metrics)
    }

    pub fn cpu_history(&self) -> Arc<RwLock<MetricHistory>> {
        Arc::clone(&self.cpu_history)
    }

    pub fn mem_history(&self) -> Arc<RwLock<MetricHistory>> {
        Arc::clone(&self.mem_history)
    }

    pub fn net_rx_history(&self) -> Arc<RwLock<MetricHistory>> {
        Arc::clone(&self.net_rx_history)
    }

    pub fn net_tx_history(&self) -> Arc<RwLock<MetricHistory>> {
        Arc::clone(&self.net_tx_history)
    }

    pub async fn platform(&self) -> Platform {
        self.platform.read().await.clone()
    }

    /// Execute a single command on the remote host and return its stdout.
    async fn exec_remote<H: russh::client::Handler>(
        handle: &Handle<H>,
        command: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let mut channel = handle.channel_open_session().await?;
        channel.exec(true, command.as_bytes()).await?;
        let mut output = Vec::new();
        while let Some(msg) = channel.wait().await {
            match msg {
                russh::ChannelMsg::Data { data } => output.extend_from_slice(&data),
                russh::ChannelMsg::Eof | russh::ChannelMsg::Close => break,
                russh::ChannelMsg::ExitStatus { .. } => break,
                _ => {}
            }
        }
        Ok(String::from_utf8_lossy(&output).to_string())
    }

    /// Detect the remote platform, if we have not already.
    async fn ensure_platform<H: russh::client::Handler>(&self, handle: &Handle<H>) -> Platform {
        {
            let p = self.platform.read().await;
            if *p != Platform::Undetected {
                return p.clone();
            }
        }
        let detected = match Self::exec_remote(handle, "uname -s").await {
            Ok(raw) => Platform::from_uname(&raw),
            Err(_) => Platform::Undetected,
        };
        *self.platform.write().await = detected.clone();
        detected
    }

    /// Collect all metrics from the remote host in a single batch.
    ///
    /// Every group ends the sweep in exactly one of `Collected`,
    /// `Uncollected` or `Unsupported` — there is no path that leaves a stale
    /// or defaulted number looking like a reading.
    pub async fn collect<H: russh::client::Handler>(
        &self,
        handle: &Handle<H>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let platform = self.ensure_platform(handle).await;

        let command = match platform.metrics_command(self.process_count) {
            Some(c) => c,
            None => {
                // A platform we have no collectors for is a terminal state:
                // say so once, in words, and stop pretending to sample it.
                let mut m = self.metrics.write().await;
                m.status.platform = platform.clone();
                let reason = match &platform {
                    Platform::Undetected => "could not run `uname -s` on this host".to_string(),
                    other => format!("no collectors for {}", other.label()),
                };
                m.status.unsupported_all(reason);
                return Ok(());
            }
        };

        let raw = match Self::exec_remote(handle, &command).await {
            Ok(raw) => raw,
            Err(e) => {
                let mut m = self.metrics.write().await;
                m.status.platform = platform;
                m.status.fail_all(describe_exec_error(e.as_ref()));
                return Ok(());
            }
        };

        let sections = split_sections(&raw);

        // A response with no envelope at all means the command never ran —
        // a restricted shell, a forced command, or an exec channel denied.
        if !sections.contains_key("UNAME") {
            let mut m = self.metrics.write().await;
            m.status.platform = platform;
            m.status.fail_all("host returned no metrics output");
            return Ok(());
        }

        let mut status = CollectionStatus {
            platform: platform.clone(),
            ..Default::default()
        };
        let get = |k: &str| sections.get(k).cloned().unwrap_or_default();

        match platform {
            Platform::Linux => {
                self.collect_linux(&get, &mut status).await;
            }
            Platform::MacOS => {
                self.collect_macos(&get, &mut status).await;
            }
            _ => unreachable!("metrics_command returned None for this platform"),
        }

        Ok(())
    }

    /// Linux: everything comes from `/proc`.
    async fn collect_linux(&self, get: &impl Fn(&str) -> String, status: &mut CollectionStatus) {
        let cpu_raw = get("CPUSTAT");
        let prev_cpu = self.prev_cpu_raw.read().await.clone();

        let cpu = if cpu_raw.trim().is_empty() {
            status.cpu.fail("/proc/stat unreadable");
            None
        } else if prev_cpu.is_empty() {
            // First sweep has nothing to difference against. That is a real
            // "not yet", not 0% — v1 published the 0.
            status.cpu = MetricState::Pending;
            None
        } else {
            let (pct, per_core) = parser::parse_cpu(&cpu_raw, &prev_cpu);
            status.cpu = MetricState::Collected;
            Some((pct, per_core))
        };
        *self.prev_cpu_raw.write().await = cpu_raw;

        let meminfo = get("MEMINFO");
        let mem = if meminfo.trim().is_empty() {
            status.mem.fail("/proc/meminfo unreadable");
            None
        } else {
            let parsed = parser::parse_meminfo(&meminfo);
            if parsed.0 == 0 {
                status.mem.fail("/proc/meminfo reported no MemTotal");
                None
            } else {
                status.mem = MetricState::Collected;
                Some(parsed)
            }
        };

        let load_raw = get("LOADAVG");
        let load = if load_raw.trim().is_empty() {
            status.load.fail("/proc/loadavg unreadable");
            None
        } else {
            status.load = MetricState::Collected;
            Some(parser::parse_loadavg(&load_raw))
        };

        let uptime_raw = get("UPTIME");
        let uptime = if uptime_raw.trim().is_empty() {
            status.uptime.fail("/proc/uptime unreadable");
            None
        } else {
            status.uptime = MetricState::Collected;
            Some(parser::parse_uptime(&uptime_raw))
        };

        let net_raw = get("NETDEV");
        let net_counters = if net_raw.trim().is_empty() {
            status.net.fail("/proc/net/dev unreadable");
            None
        } else {
            Some(sum_linux_net(&net_raw))
        };

        self.finish(get, status, cpu, mem, load, uptime, net_counters)
            .await;
    }

    /// macOS: seven different sources, each of which can fail on its own.
    async fn collect_macos(&self, get: &impl Fn(&str) -> String, status: &mut CollectionStatus) {
        let cpu = match parser::parse_macos_cpu(&get("CPUSTAT")) {
            Some(pct) => {
                status.cpu = MetricState::Collected;
                // Darwin gives us an aggregate only; per-core would need a
                // Mach call we cannot make over an exec channel.
                Some((pct, Vec::new()))
            }
            None => {
                status
                    .cpu
                    .fail("neither `iostat` nor `top` returned a CPU sample");
                None
            }
        };

        let mem = match parser::parse_vm_stat(&get("VMSTAT"), &get("MEMSIZE")) {
            Some((total, used, avail)) => {
                status.mem = MetricState::Collected;
                let (swap_total, swap_used) =
                    parser::parse_swapusage(&get("SWAP")).unwrap_or((0, 0));
                Some((total, used, avail, swap_total, swap_used))
            }
            None => {
                status.mem.fail("`vm_stat` returned no page counters");
                None
            }
        };

        let load = match parser::parse_sysctl_loadavg(&get("LOADAVG")) {
            Some(l) => {
                status.load = MetricState::Collected;
                Some(l)
            }
            None => {
                status.load.fail("`sysctl vm.loadavg` unreadable");
                None
            }
        };

        let uptime = match parser::parse_boottime(&get("BOOTTIME"), &get("NOW")) {
            Some(u) => {
                status.uptime = MetricState::Collected;
                Some(u)
            }
            None => {
                status.uptime.fail("`sysctl kern.boottime` unreadable");
                None
            }
        };

        let net_counters = match parser::parse_netstat_ib(&get("NETDEV")) {
            Some(c) => Some(c),
            None => {
                status.net.fail("`netstat -ib` returned no interfaces");
                None
            }
        };

        self.finish(get, status, cpu, mem, load, uptime, net_counters)
            .await;
    }

    /// Shared tail: disks, processes, network rate, history, commit.
    #[allow(clippy::too_many_arguments)]
    async fn finish(
        &self,
        get: &impl Fn(&str) -> String,
        status: &mut CollectionStatus,
        cpu: Option<(f64, Vec<f64>)>,
        mem: Option<(u64, u64, u64, u64, u64)>,
        load: Option<(f64, f64, f64)>,
        uptime: Option<u64>,
        net_counters: Option<(u64, u64)>,
    ) {
        let df_raw = get("DF");
        let disks = if df_raw.trim().is_empty() {
            status.disk.fail("`df` returned nothing");
            Vec::new()
        } else {
            let parsed = parser::parse_df(&df_raw);
            if parsed.is_empty() {
                status.disk.fail("`df` output had no usable filesystems");
            } else {
                status.disk = MetricState::Collected;
            }
            parsed
        };

        let ps_raw = get("PS");
        let (top_cpu, top_mem) = if ps_raw.trim().is_empty() {
            status.procs.fail("`ps` returned nothing");
            (Vec::new(), Vec::new())
        } else {
            let top_cpu = parser::parse_ps(&ps_raw, self.process_count);
            if top_cpu.is_empty() {
                status.procs.fail("`ps` output could not be parsed");
                (Vec::new(), Vec::new())
            } else {
                status.procs = MetricState::Collected;
                let mut top_mem = top_cpu.clone();
                top_mem.sort_by(|a, b| {
                    b.mem_pct
                        .partial_cmp(&a.mem_pct)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                top_mem.truncate(self.process_count);
                (top_cpu, top_mem)
            }
        };

        // Network is a rate, so it needs two samples. Until the second sweep
        // there is genuinely no reading — not 0 B/s.
        let now = Instant::now();
        let prev = *self.prev_net_counters.read().await;
        let elapsed = now
            .duration_since(*self.last_net_time.read().await)
            .as_secs_f64();

        let net = match (net_counters, prev) {
            (Some(curr), Some(prev)) if elapsed > 0.0 => {
                let rx = curr.0.saturating_sub(prev.0) as f64 / elapsed;
                let tx = curr.1.saturating_sub(prev.1) as f64 / elapsed;
                status.net = MetricState::Collected;
                Some((rx, tx))
            }
            (Some(_), None) => {
                status.net = MetricState::Pending;
                None
            }
            _ => None,
        };
        if let Some(curr) = net_counters {
            *self.prev_net_counters.write().await = Some(curr);
            *self.last_net_time.write().await = now;
        }

        {
            let mut m = self.metrics.write().await;

            if let Some((pct, per_core)) = &cpu {
                m.cpu_percent = *pct;
                m.cpu_per_core = per_core.clone();
            }
            if let Some((total, used, avail, swap_total, swap_used)) = mem {
                m.mem_total_kb = total;
                m.mem_used_kb = used;
                m.mem_available_kb = avail;
                m.mem_swap_total_kb = swap_total;
                m.mem_swap_used_kb = swap_used;
            }
            if let Some((l1, l5, l15)) = load {
                m.load_1m = l1;
                m.load_5m = l5;
                m.load_15m = l15;
            }
            if let Some(u) = uptime {
                m.uptime_secs = u;
            }
            if let Some((rx, tx)) = net {
                m.net_rx_bps = rx;
                m.net_tx_bps = tx;
            }
            if status.disk.is_collected() {
                m.disks = disks;
            }
            if status.procs.is_collected() {
                m.top_procs_cpu = top_cpu;
                m.top_procs_mem = top_mem;
            }
            m.os_info = status.platform.label().to_string();
            m.status = status.clone();
        }

        // Histories only ever receive real readings. Pushing a placeholder is
        // how v1 ended up drawing flat sparklines for data it never had.
        if let Some((pct, _)) = cpu {
            self.cpu_history.write().await.push(pct as u64);
        }
        if let Some((total, used, _, _, _)) = mem {
            if total > 0 {
                self.mem_history
                    .write()
                    .await
                    .push((used as f64 / total as f64 * 100.0) as u64);
            }
        }
        if let Some((rx, tx)) = net {
            self.net_rx_history.write().await.push((rx / 1024.0) as u64);
            self.net_tx_history.write().await.push((tx / 1024.0) as u64);
        }
    }
}

/// Turn a transport error into something a human can act on.
fn describe_exec_error(e: &(dyn std::error::Error + Send + Sync)) -> String {
    let text = e.to_string();
    let lowered = text.to_lowercase();
    if lowered.contains("timed out") || lowered.contains("timeout") {
        "collector timed out".to_string()
    } else if lowered.contains("channel") {
        "host refused an exec channel".to_string()
    } else {
        format!("collector failed: {}", text)
    }
}

/// Sum `/proc/net/dev` byte counters across non-loopback interfaces.
fn sum_linux_net(raw: &str) -> (u64, u64) {
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
        if parts[0].trim_end_matches(':') == "lo" {
            continue;
        }
        rx_total += parts[1].parse::<u64>().unwrap_or(0);
        tx_total += parts[9].parse::<u64>().unwrap_or(0);
    }
    (rx_total, tx_total)
}

pub fn split_sections(raw: &str) -> HashMap<String, String> {
    let mut sections = HashMap::new();
    let mut current_key = String::new();
    let mut current_content = String::new();

    for line in raw.lines() {
        if line.starts_with("===") && line.ends_with("===") && line.len() > 6 {
            if !current_key.is_empty() {
                sections.insert(current_key.clone(), current_content.trim().to_string());
            }
            current_key = line.trim_matches('=').to_string();
            current_content = String::new();
        } else if current_key != "END" {
            current_content.push_str(line);
            current_content.push('\n');
        }
    }
    if !current_key.is_empty() && current_key != "END" {
        sections.insert(current_key, current_content.trim().to_string());
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_sections_handles_both_envelopes() {
        let raw = "===UNAME===\nDarwin\n===LOADAVG===\n{ 0.75 1.47 1.65 }\n===END===\n";
        let s = split_sections(raw);
        assert_eq!(s.get("UNAME").unwrap(), "Darwin");
        assert_eq!(s.get("LOADAVG").unwrap(), "{ 0.75 1.47 1.65 }");
        assert!(!s.contains_key("END"));
    }

    #[test]
    fn split_sections_ignores_terminal_rules_in_output() {
        // A remote command printing a row of '=' must not be read as a marker.
        let raw = "===UNAME===\nLinux\n======\n===END===\n";
        let s = split_sections(raw);
        assert_eq!(s.get("UNAME").unwrap().trim(), "Linux\n======".trim());
    }

    #[test]
    fn linux_net_sum_skips_loopback() {
        let raw = "Inter-|   Receive                    |  Transmit\n\
                    face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets\n\
                       lo: 1000 10 0 0 0 0 0 0 1000 10 0 0 0 0 0 0\n\
                     eth0: 5000 50 0 0 0 0 0 0 7000 70 0 0 0 0 0 0\n";
        assert_eq!(sum_linux_net(raw), (5000, 7000));
    }

    /// Run the platform's real command set through a local shell and assert we
    /// get plausible readings out the other end.
    ///
    /// This is the test whose absence let finding 1 ship: every previous test
    /// fed the parsers hand-written Linux strings, so nobody noticed the
    /// collector had no Darwin path at all. Running the actual command means
    /// this also fails if a future macOS renames a field.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_command_set_produces_real_readings_on_this_host() {
        use std::process::Command;

        let cmd = Platform::MacOS.metrics_command(6).expect("macOS command");
        let out = Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .output()
            .expect("run metrics command");
        let raw = String::from_utf8_lossy(&out.stdout);
        let s = split_sections(&raw);

        assert_eq!(s.get("UNAME").map(|v| v.trim()), Some("Darwin"));

        // CPU: a real percentage, and specifically not the 0.0 v1 rendered.
        let cpu = parser::parse_macos_cpu(s.get("CPUSTAT").expect("CPUSTAT"))
            .expect("CPU must be readable on macOS");
        assert!((0.0..=100.0).contains(&cpu), "cpu {}", cpu);

        // Memory: must agree with hw.memsize to within a page or two.
        let (total_kb, used_kb, _avail) = parser::parse_vm_stat(
            s.get("VMSTAT").expect("VMSTAT"),
            s.get("MEMSIZE").expect("MEMSIZE"),
        )
        .expect("memory must be readable on macOS");
        assert!(
            total_kb > 1024 * 1024,
            "total {} KB is implausible",
            total_kb
        );
        assert!(used_kb > 0 && used_kb <= total_kb, "used {} KB", used_kb);

        // Load: a Mac that is running this test has a non-negative load.
        let (l1, _, _) = parser::parse_sysctl_loadavg(s.get("LOADAVG").expect("LOADAVG"))
            .expect("load must be readable on macOS");
        assert!(l1 >= 0.0);

        // Uptime: non-zero, because the machine booted before it ran a test.
        let up = parser::parse_boottime(
            s.get("BOOTTIME").expect("BOOTTIME"),
            s.get("NOW").expect("NOW"),
        )
        .expect("uptime must be readable on macOS");
        assert!(up > 0, "uptime {}", up);

        // Network counters exist and are cumulative, so at least one is > 0.
        let (rx, tx) =
            parser::parse_netstat_ib(s.get("NETDEV").expect("NETDEV")).expect("netstat readable");
        assert!(rx > 0 || tx > 0, "rx {} tx {}", rx, tx);

        // Disks: at least the root filesystem, and it must be user-visible.
        let disks = parser::parse_df(s.get("DF").expect("DF"));
        assert!(disks.iter().any(|d| d.mount == "/"), "no root filesystem");
        assert!(
            disks.iter().filter(|d| d.is_user_visible()).count() >= 1,
            "every mount was filtered out as noise"
        );

        // Processes: this test process is itself running.
        let procs = parser::parse_ps(s.get("PS").expect("PS"), 6);
        assert!(!procs.is_empty(), "ps produced no processes");
        assert!(procs.iter().all(|p| p.pid > 0));
    }

    /// End-to-end over a real SSH connection.
    ///
    /// Opt-in, because it needs a reachable sshd and agent auth:
    ///
    /// ```sh
    /// ESSH_LIVE_SSH=localhost cargo test --bin essh live_ssh -- --ignored --nocapture
    /// ```
    ///
    /// This is the check that would have caught finding 1 in v1: it asserts
    /// the collector reports `Collected` and non-zero values against a host
    /// that is demonstrably running.
    #[ignore = "needs a reachable sshd; set ESSH_LIVE_SSH=<host>"]
    #[tokio::test]
    async fn live_ssh_collector_reports_real_values() {
        let host = match std::env::var("ESSH_LIVE_SSH") {
            Ok(h) if !h.is_empty() => h,
            _ => {
                eprintln!("set ESSH_LIVE_SSH=<host> to run this");
                return;
            }
        };
        let user = std::env::var("USER").unwrap_or_else(|_| "root".into());

        // Prefer an explicit key file; fall back to the agent.
        let auth = match std::env::var("ESSH_LIVE_KEY") {
            Ok(p) if !p.is_empty() => crate::ssh::AuthMethod::KeyFile {
                path: std::path::PathBuf::from(p),
                passphrase: None,
            },
            _ => crate::ssh::AuthMethod::Agent,
        };

        let cfg = crate::ssh::ConnectConfig {
            hostname: host.clone(),
            port: 22,
            username: user,
            auth,
            timeout: std::time::Duration::from_secs(5),
        };
        let (session, _fp, _banner) = crate::ssh::SshClient::connect(&cfg)
            .await
            .expect("connect to the live host");

        let collector = HostMetricsCollector::new(60, 6);

        // Two sweeps: CPU and network are rates and need a previous sample.
        collector.collect(&session.handle).await.expect("sweep 1");
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        collector.collect(&session.handle).await.expect("sweep 2");

        let m = collector.metrics().read().await.clone();
        eprintln!("platform: {}", m.status.platform.label());
        eprintln!("problems: {:?}", m.status.problems());

        assert!(
            m.status.platform.is_supported(),
            "detected an unsupported platform: {}",
            m.status.platform.label()
        );

        // The v1 bug, asserted directly: a live host must not report zeros.
        let mem = m.mem_percent().expect("memory must be collected");
        assert!(mem > 0.0 && mem < 100.0, "implausible memory {}%", mem);
        assert!(
            m.uptime_opt().expect("uptime") > 0,
            "uptime must be non-zero"
        );
        let (l1, _, _) = m.load_opt().expect("load must be collected");
        assert!(l1 >= 0.0);
        assert!(m.cpu_percent_opt().is_some(), "CPU must be collected");
        assert!(!m.user_disks().is_empty(), "at least one user volume");
        assert!(m.status.procs.is_collected(), "processes must be collected");

        eprintln!(
            "cpu {:.1}%  mem {:.1}%  up {}s  disks {}  procs {}",
            m.cpu_percent_opt().unwrap(),
            mem,
            m.uptime_secs,
            m.user_disks().len(),
            m.top_procs_cpu.len()
        );

        // And the facet collectors, over the same connection.
        let facts = crate::divergence::collect::collect_facts(
            &session.handle,
            &host,
            &m.status.platform,
            &["/etc/ssh/sshd_config".to_string()],
            &[],
        )
        .await;

        let known = facts.facets.values().filter(|v| v.is_known()).count();
        eprintln!("facets: {} known of {}", known, facts.facets.len());
        for (k, v) in &facts.facets {
            eprintln!("  {:<22} {}", k.label(), v.as_display());
        }
        assert!(
            known >= 6,
            "expected most facets to be readable, got {}",
            known
        );
        assert!(
            facts
                .facets
                .contains_key(&crate::divergence::FacetKey::Kernel),
            "kernel facet must be present"
        );
    }

    #[test]
    fn exec_errors_become_human_reasons() {
        #[derive(Debug)]
        struct E(&'static str);
        impl std::fmt::Display for E {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl std::error::Error for E {}

        assert_eq!(
            describe_exec_error(&E("operation timed out")),
            "collector timed out"
        );
        assert_eq!(
            describe_exec_error(&E("channel open failure")),
            "host refused an exec channel"
        );
    }
}
