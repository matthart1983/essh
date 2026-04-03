use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMetrics {
    pub session_id: String,
    pub connected_at: DateTime<Utc>,
    pub hostname: String,
    pub port: u16,
    pub server_banner: Option<String>,
    pub kex_algorithm: Option<String>,
    pub cipher: Option<String>,
    pub mac: Option<String>,
    pub compression: Option<String>,
    pub auth_method: Option<String>,
    pub rtt_ms: Option<f64>,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub keepalive_sent: u64,
    pub keepalive_received: u64,
    pub channels_active: u32,
    pub last_rekey_at: Option<DateTime<Utc>>,
    pub bytes_since_rekey: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ConnectionQuality {
    Excellent,
    Good,
    Fair,
    Poor,
    Critical,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticsSnapshot {
    pub timestamp: String,
    pub session_id: String,
    pub rtt_ms: Option<f64>,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub throughput_up_bps: f64,
    pub throughput_down_bps: f64,
    pub packet_loss_pct: f64,
    pub quality: ConnectionQuality,
    pub uptime_secs: i64,
    pub channels_active: u32,
}

pub struct DiagnosticsEngine {
    metrics: Arc<RwLock<SessionMetrics>>,
    log_file: Option<PathBuf>,
}

impl SessionMetrics {
    pub fn new(session_id: String, hostname: String, port: u16) -> Self {
        Self {
            session_id,
            connected_at: Utc::now(),
            hostname,
            port,
            server_banner: None,
            kex_algorithm: None,
            cipher: None,
            mac: None,
            compression: None,
            auth_method: None,
            rtt_ms: None,
            bytes_sent: 0,
            bytes_received: 0,
            keepalive_sent: 0,
            keepalive_received: 0,
            channels_active: 0,
            last_rekey_at: None,
            bytes_since_rekey: 0,
        }
    }

    pub fn uptime(&self) -> chrono::Duration {
        Utc::now() - self.connected_at
    }

    pub fn packet_loss_pct(&self) -> f64 {
        if self.keepalive_sent > 0 {
            (1.0 - self.keepalive_received as f64 / self.keepalive_sent as f64) * 100.0
        } else {
            0.0
        }
    }

    pub fn connection_quality(&self) -> ConnectionQuality {
        let rtt = self.rtt_ms.unwrap_or(0.0);
        let loss = self.packet_loss_pct();

        if rtt < 20.0 && loss < 1.0 {
            ConnectionQuality::Excellent
        } else if rtt < 50.0 && loss < 3.0 {
            ConnectionQuality::Good
        } else if rtt < 100.0 && loss < 5.0 {
            ConnectionQuality::Fair
        } else if rtt < 200.0 && loss < 10.0 {
            ConnectionQuality::Poor
        } else {
            ConnectionQuality::Critical
        }
    }

    pub fn throughput_up_bps(&self, elapsed_secs: f64) -> f64 {
        self.bytes_sent as f64 / elapsed_secs
    }

    pub fn throughput_down_bps(&self, elapsed_secs: f64) -> f64 {
        self.bytes_received as f64 / elapsed_secs
    }
}

impl DiagnosticsEngine {
    pub fn new(session_id: &str, hostname: &str, port: u16, log_dir: Option<&Path>) -> Self {
        let metrics = SessionMetrics::new(session_id.to_string(), hostname.to_string(), port);
        let log_file = log_dir.map(|dir| dir.join(format!("{}.jsonl", session_id)));

        Self {
            metrics: Arc::new(RwLock::new(metrics)),
            log_file,
        }
    }

    pub fn metrics(&self) -> Arc<RwLock<SessionMetrics>> {
        Arc::clone(&self.metrics)
    }

    pub async fn snapshot(&self) -> DiagnosticsSnapshot {
        let m = self.metrics.read().await;
        let uptime = m.uptime();
        let elapsed_secs = uptime.num_seconds().max(1) as f64;

        DiagnosticsSnapshot {
            timestamp: Utc::now().to_rfc3339(),
            session_id: m.session_id.clone(),
            rtt_ms: m.rtt_ms,
            bytes_sent: m.bytes_sent,
            bytes_received: m.bytes_received,
            throughput_up_bps: m.throughput_up_bps(elapsed_secs),
            throughput_down_bps: m.throughput_down_bps(elapsed_secs),
            packet_loss_pct: m.packet_loss_pct(),
            quality: m.connection_quality(),
            uptime_secs: uptime.num_seconds(),
            channels_active: m.channels_active,
        }
    }

    pub async fn record_bytes_sent(&self, n: u64) {
        let mut m = self.metrics.write().await;
        m.bytes_sent += n;
    }

    pub async fn record_bytes_received(&self, n: u64) {
        let mut m = self.metrics.write().await;
        m.bytes_received += n;
    }

    #[allow(dead_code)]
    pub async fn record_rtt(&self, rtt_ms: f64) {
        let mut m = self.metrics.write().await;
        m.rtt_ms = Some(rtt_ms);
    }

    #[allow(dead_code)]
    pub async fn record_keepalive_sent(&self) {
        let mut m = self.metrics.write().await;
        m.keepalive_sent += 1;
    }

    #[allow(dead_code)]
    pub async fn record_keepalive_received(&self) {
        let mut m = self.metrics.write().await;
        m.keepalive_received += 1;
    }

    pub async fn set_connection_info(
        &self,
        server_banner: Option<String>,
        kex: Option<String>,
        cipher: Option<String>,
        mac: Option<String>,
        compression: Option<String>,
        auth_method: Option<String>,
    ) {
        let mut m = self.metrics.write().await;
        m.server_banner = server_banner;
        m.kex_algorithm = kex;
        m.cipher = cipher;
        m.mac = mac;
        m.compression = compression;
        m.auth_method = auth_method;
    }

    #[allow(dead_code)]
    pub async fn set_channels_active(&self, count: u32) {
        let mut m = self.metrics.write().await;
        m.channels_active = count;
    }

    pub async fn write_log_entry(&self) -> Result<(), std::io::Error> {
        let snapshot = self.snapshot().await;

        if let Some(ref path) = self.log_file {
            let line = serde_json::to_string(&snapshot).map_err(std::io::Error::other)?;
            let mut file = OpenOptions::new().create(true).append(true).open(path)?;
            writeln!(file, "{}", line)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_session_metrics_new() {
        let m = SessionMetrics::new("sess-1".into(), "host.example.com".into(), 22);
        assert_eq!(m.session_id, "sess-1");
        assert_eq!(m.hostname, "host.example.com");
        assert_eq!(m.port, 22);
        assert_eq!(m.bytes_sent, 0);
        assert_eq!(m.bytes_received, 0);
        assert_eq!(m.keepalive_sent, 0);
        assert_eq!(m.keepalive_received, 0);
        assert_eq!(m.channels_active, 0);
        assert_eq!(m.bytes_since_rekey, 0);
        assert!(m.rtt_ms.is_none());
        assert!(m.server_banner.is_none());
        assert!(m.kex_algorithm.is_none());
        assert!(m.cipher.is_none());
        assert!(m.mac.is_none());
        assert!(m.compression.is_none());
        assert!(m.auth_method.is_none());
        assert!(m.last_rekey_at.is_none());
    }

    #[test]
    fn test_packet_loss_zero_when_no_keepalives() {
        let m = SessionMetrics::new("s".into(), "h".into(), 22);
        assert_eq!(m.packet_loss_pct(), 0.0);
    }

    #[test]
    fn test_packet_loss_calculation() {
        let mut m = SessionMetrics::new("s".into(), "h".into(), 22);
        m.keepalive_sent = 10;
        m.keepalive_received = 8;
        let loss = m.packet_loss_pct();
        assert!((loss - 20.0).abs() < 1e-10);
    }

    #[test]
    fn test_connection_quality_excellent() {
        let mut m = SessionMetrics::new("s".into(), "h".into(), 22);
        m.rtt_ms = Some(10.0);
        // 0% loss (no keepalives → 0.0)
        assert_eq!(m.connection_quality(), ConnectionQuality::Excellent);
    }

    #[test]
    fn test_connection_quality_good() {
        let mut m = SessionMetrics::new("s".into(), "h".into(), 22);
        m.rtt_ms = Some(30.0);
        m.keepalive_sent = 100;
        m.keepalive_received = 98; // 2% loss
        assert_eq!(m.connection_quality(), ConnectionQuality::Good);
    }

    #[test]
    fn test_connection_quality_fair() {
        let mut m = SessionMetrics::new("s".into(), "h".into(), 22);
        m.rtt_ms = Some(80.0);
        m.keepalive_sent = 100;
        m.keepalive_received = 96; // 4% loss
        assert_eq!(m.connection_quality(), ConnectionQuality::Fair);
    }

    #[test]
    fn test_connection_quality_poor() {
        let mut m = SessionMetrics::new("s".into(), "h".into(), 22);
        m.rtt_ms = Some(150.0);
        m.keepalive_sent = 100;
        m.keepalive_received = 92; // 8% loss
        assert_eq!(m.connection_quality(), ConnectionQuality::Poor);
    }

    #[test]
    fn test_connection_quality_critical() {
        let mut m = SessionMetrics::new("s".into(), "h".into(), 22);
        m.rtt_ms = Some(300.0);
        m.keepalive_sent = 100;
        m.keepalive_received = 85; // 15% loss
        assert_eq!(m.connection_quality(), ConnectionQuality::Critical);
    }

    #[test]
    fn test_throughput_calculation() {
        let mut m = SessionMetrics::new("s".into(), "h".into(), 22);
        m.bytes_sent = 1024;
        assert!((m.throughput_up_bps(2.0) - 512.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_diagnostics_engine_new() {
        let engine = DiagnosticsEngine::new("sess-1", "host", 22, None);
        let metrics = engine.metrics();
        let m = metrics.read().await;
        assert_eq!(m.session_id, "sess-1");
        assert_eq!(m.hostname, "host");
        assert_eq!(m.port, 22);
    }

    #[tokio::test]
    async fn test_record_bytes() {
        let engine = DiagnosticsEngine::new("s", "h", 22, None);
        engine.record_bytes_sent(100).await;
        engine.record_bytes_sent(50).await;
        engine.record_bytes_received(200).await;

        let snap = engine.snapshot().await;
        assert_eq!(snap.bytes_sent, 150);
        assert_eq!(snap.bytes_received, 200);
    }

    #[tokio::test]
    async fn test_set_connection_info() {
        let engine = DiagnosticsEngine::new("s", "h", 22, None);
        engine
            .set_connection_info(
                Some("OpenSSH_9.0".into()),
                Some("curve25519-sha256".into()),
                Some("aes256-gcm".into()),
                Some("hmac-sha2-256".into()),
                Some("none".into()),
                Some("publickey".into()),
            )
            .await;

        let m = engine.metrics().read().await.clone();
        assert_eq!(m.server_banner.as_deref(), Some("OpenSSH_9.0"));
        assert_eq!(m.kex_algorithm.as_deref(), Some("curve25519-sha256"));
        assert_eq!(m.cipher.as_deref(), Some("aes256-gcm"));
        assert_eq!(m.mac.as_deref(), Some("hmac-sha2-256"));
        assert_eq!(m.compression.as_deref(), Some("none"));
        assert_eq!(m.auth_method.as_deref(), Some("publickey"));
    }

    #[tokio::test]
    async fn test_write_log_entry() {
        let dir = std::env::temp_dir().join(format!("diag-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        let engine = DiagnosticsEngine::new("log-sess", "h", 22, Some(dir.as_path()));
        engine.record_bytes_sent(42).await;
        engine.write_log_entry().await.unwrap();

        let log_path = dir.join("log-sess.jsonl");
        assert!(log_path.exists());

        let contents = fs::read_to_string(&log_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(parsed["session_id"], "log-sess");
        assert_eq!(parsed["bytes_sent"], 42);

        // cleanup
        let _ = fs::remove_dir_all(&dir);
    }
}
