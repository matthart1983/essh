use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Result of a single TCP probe to a host.
#[derive(Clone, Debug)]
pub struct ProbeResult {
    pub online: bool,
    pub latency_ms: Option<f64>,
    #[allow(dead_code)]
    pub last_probed: Instant,
}

/// Per-host probe state including latency history for sparklines.
#[derive(Clone, Debug)]
pub struct HostProbeState {
    pub result: ProbeResult,
    pub latency_history: Vec<u64>,
    pub history_capacity: usize,
}

impl HostProbeState {
    pub fn new(capacity: usize) -> Self {
        Self {
            result: ProbeResult {
                online: false,
                latency_ms: None,
                last_probed: Instant::now(),
            },
            latency_history: Vec::with_capacity(capacity),
            history_capacity: capacity,
        }
    }

    pub fn record(&mut self, result: ProbeResult) {
        if let Some(ms) = result.latency_ms {
            self.latency_history.push(ms as u64);
            if self.latency_history.len() > self.history_capacity {
                self.latency_history.remove(0);
            }
        }
        self.result = result;
    }
}

/// Fleet-wide probe state, keyed by "hostname:port".
pub struct FleetProber {
    pub states: HashMap<String, HostProbeState>,
    pub probe_timeout: Duration,
    pub history_capacity: usize,
    pub last_probe_cycle: Option<Instant>,
    pub probe_interval: Duration,
}

impl FleetProber {
    pub fn new(probe_interval_secs: u64, probe_timeout_secs: u64, history_capacity: usize) -> Self {
        Self {
            states: HashMap::new(),
            probe_timeout: Duration::from_secs(probe_timeout_secs),
            history_capacity,
            last_probe_cycle: None,
            probe_interval: Duration::from_secs(probe_interval_secs),
        }
    }

    /// Returns true if enough time has elapsed since the last probe cycle.
    pub fn should_probe(&self) -> bool {
        match self.last_probe_cycle {
            None => true,
            Some(last) => last.elapsed() >= self.probe_interval,
        }
    }

    /// Probe a single host via TCP connect. Returns the result.
    pub async fn probe_host(hostname: &str, port: u16, timeout: Duration) -> ProbeResult {
        let addr = format!("{}:{}", hostname, port);
        let start = Instant::now();
        let result = tokio::time::timeout(timeout, tokio::net::TcpStream::connect(&addr)).await;
        let elapsed = start.elapsed();

        match result {
            Ok(Ok(_stream)) => ProbeResult {
                online: true,
                latency_ms: Some(elapsed.as_secs_f64() * 1000.0),
                last_probed: Instant::now(),
            },
            _ => ProbeResult {
                online: false,
                latency_ms: None,
                last_probed: Instant::now(),
            },
        }
    }

    /// Run a full probe cycle against all registered hosts.
    pub async fn probe_all(&mut self, hosts: &[(String, u16)]) {
        let timeout = self.probe_timeout;
        let capacity = self.history_capacity;

        // Probe all hosts concurrently
        let mut handles = Vec::new();
        for (hostname, port) in hosts {
            let hostname = hostname.clone();
            let port = *port;
            handles.push(tokio::spawn(async move {
                let result = Self::probe_host(&hostname, port, timeout).await;
                (format!("{}:{}", hostname, port), result)
            }));
        }

        for handle in handles {
            if let Ok((key, result)) = handle.await {
                let state = self
                    .states
                    .entry(key)
                    .or_insert_with(|| HostProbeState::new(capacity));
                state.record(result);
            }
        }

        self.last_probe_cycle = Some(Instant::now());
    }

    /// Get probe state for a specific host.
    pub fn get_state(&self, hostname: &str, port: u16) -> Option<&HostProbeState> {
        let key = format!("{}:{}", hostname, port);
        self.states.get(&key)
    }
}

/// Return the color name for a latency value using Netwatch thresholds.
/// green < 50 ms, yellow < 200 ms, red ≥ 200 ms
#[allow(dead_code)]
pub fn latency_color_class(ms: f64) -> &'static str {
    if ms < 50.0 {
        "green"
    } else if ms < 200.0 {
        "yellow"
    } else {
        "red"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_probe_state_history_capacity() {
        let mut state = HostProbeState::new(3);
        for i in 0..5 {
            state.record(ProbeResult {
                online: true,
                latency_ms: Some(i as f64 * 10.0),
                last_probed: Instant::now(),
            });
        }
        assert_eq!(state.latency_history.len(), 3);
        assert_eq!(state.latency_history, vec![20, 30, 40]);
    }

    #[test]
    fn test_host_probe_state_offline_no_history() {
        let mut state = HostProbeState::new(10);
        state.record(ProbeResult {
            online: true,
            latency_ms: Some(42.0),
            last_probed: Instant::now(),
        });
        state.record(ProbeResult {
            online: false,
            latency_ms: None,
            last_probed: Instant::now(),
        });
        // Offline probe doesn't add to history
        assert_eq!(state.latency_history.len(), 1);
        assert!(!state.result.online);
    }

    #[test]
    fn test_latency_color_class() {
        assert_eq!(latency_color_class(10.0), "green");
        assert_eq!(latency_color_class(49.9), "green");
        assert_eq!(latency_color_class(50.0), "yellow");
        assert_eq!(latency_color_class(199.9), "yellow");
        assert_eq!(latency_color_class(200.0), "red");
        assert_eq!(latency_color_class(1000.0), "red");
    }

    #[test]
    fn test_fleet_prober_should_probe_initially() {
        let prober = FleetProber::new(60, 5, 30);
        assert!(prober.should_probe());
    }

    #[test]
    fn test_fleet_prober_get_state_missing() {
        let prober = FleetProber::new(60, 5, 30);
        assert!(prober.get_state("nonexistent", 22).is_none());
    }

    #[tokio::test]
    async fn test_probe_host_unreachable() {
        // Probe a host that should not be reachable (reserved IP)
        let result = FleetProber::probe_host("192.0.2.1", 1, Duration::from_millis(100)).await;
        assert!(!result.online);
        assert!(result.latency_ms.is_none());
    }
}
