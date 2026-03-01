use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML parse error: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("TOML serialization error: {0}")]
    Serialize(#[from] toml::ser::Error),
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TofuPolicy {
    Strict,
    Prompt,
    Auto,
}

impl Default for TofuPolicy {
    fn default() -> Self {
        Self::Prompt
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsDisplay {
    StatusBar,
    Overlay,
    Hidden,
}

impl Default for DiagnosticsDisplay {
    fn default() -> Self {
        Self::StatusBar
    }
}

// ---------------------------------------------------------------------------
// Sub-configs
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub default_user: Option<String>,
    pub default_key: Option<String>,
    pub tofu_policy: TofuPolicy,
    pub cache_ttl: String,
    pub log_level: String,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            default_user: None,
            default_key: None,
            tofu_policy: TofuPolicy::default(),
            cache_ttl: "30d".to_string(),
            log_level: "info".to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DiagnosticsConfig {
    pub enabled: bool,
    pub display: DiagnosticsDisplay,
    pub export_format: String,
    pub keepalive_interval: u64,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            display: DiagnosticsDisplay::default(),
            export_format: "jsonl".to_string(),
            keepalive_interval: 15,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub auto_reconnect: bool,
    pub reconnect_max_retries: u32,
    pub multiplex: bool,
    pub recording: bool,
    pub max_concurrent: usize,
    pub scrollback_lines: usize,
    pub notification_patterns: Vec<String>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            auto_reconnect: true,
            reconnect_max_retries: 5,
            multiplex: true,
            recording: false,
            max_concurrent: 9,
            scrollback_lines: 10000,
            notification_patterns: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    pub min_key_bits: u32,
    pub allowed_ciphers: Vec<String>,
    pub allowed_kex: Vec<String>,
    pub allowed_macs: Vec<String>,
    pub require_mfa_groups: Vec<String>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            min_key_bits: 3072,
            allowed_ciphers: vec![
                "chacha20-poly1305@openssh.com".to_string(),
                "aes256-gcm@openssh.com".to_string(),
                "aes128-gcm@openssh.com".to_string(),
            ],
            allowed_kex: vec![
                "curve25519-sha256".to_string(),
                "curve25519-sha256@libssh.org".to_string(),
            ],
            allowed_macs: vec![
                "hmac-sha2-256-etm@openssh.com".to_string(),
                "hmac-sha2-512-etm@openssh.com".to_string(),
            ],
            require_mfa_groups: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AuditConfig {
    pub enabled: bool,
    pub syslog_target: Option<String>,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            syslog_target: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct HostMonitorConfig {
    pub enabled: bool,
    pub cpu_interval: u64,
    pub memory_interval: u64,
    pub process_count: usize,
    pub history_samples: usize,
}

impl Default for HostMonitorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cpu_interval: 1,
            memory_interval: 2,
            process_count: 15,
            history_samples: 60,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct FleetConfig {
    pub probe_enabled: bool,
    pub probe_interval: u64,
    pub probe_timeout: u64,
    pub latency_history_samples: usize,
}

impl Default for FleetConfig {
    fn default() -> Self {
        Self {
            probe_enabled: true,
            probe_interval: 60,
            probe_timeout: 5,
            latency_history_samples: 30,
        }
    }
}

// ---------------------------------------------------------------------------
// Host / group structs
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortForwardConfig {
    pub direction: String,
    pub bind_host: String,
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostEntry {
    pub name: String,
    pub hostname: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: Option<String>,
    pub key: Option<String>,
    #[serde(default)]
    pub tags: HashMap<String, String>,
    pub jump_host: Option<String>,
    #[serde(default)]
    pub port_forwards: Vec<PortForwardConfig>,
}

fn default_port() -> u16 {
    22
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GroupDefaults {
    pub user: Option<String>,
    pub key: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostGroup {
    pub name: String,
    #[serde(default)]
    pub match_tags: HashMap<String, String>,
    #[serde(default)]
    pub defaults: GroupDefaults,
}

// ---------------------------------------------------------------------------
// AppConfig
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub general: GeneralConfig,
    pub diagnostics: DiagnosticsConfig,
    pub session: SessionConfig,
    pub security: SecurityConfig,
    pub audit: AuditConfig,
    #[serde(default)]
    pub host_monitor: HostMonitorConfig,
    #[serde(default)]
    pub fleet: FleetConfig,
    #[serde(default)]
    pub hosts: Vec<HostEntry>,
    #[serde(default)]
    pub host_groups: Vec<HostGroup>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            diagnostics: DiagnosticsConfig::default(),
            session: SessionConfig::default(),
            security: SecurityConfig::default(),
            audit: AuditConfig::default(),
            host_monitor: HostMonitorConfig::default(),
            fleet: FleetConfig::default(),
            hosts: Vec::new(),
            host_groups: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn data_dir() -> PathBuf {
        dirs::home_dir()
            .expect("could not determine home directory")
            .join(".essh")
    }

    pub fn ensure_dirs() -> Result<(), ConfigError> {
        let base = Self::data_dir();
        for sub in ["", "sessions", "recordings", "known_cas", "plugins"] {
            fs::create_dir_all(base.join(sub))?;
        }
        Ok(())
    }

    pub fn load() -> Result<Self, ConfigError> {
        let path = Self::data_dir().join("config.toml");
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = fs::read_to_string(&path)?;
        let config: AppConfig = toml::from_str(&contents)?;
        Ok(config)
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        Self::ensure_dirs()?;
        let path = Self::data_dir().join("config.toml");
        let contents = toml::to_string_pretty(self)?;
        fs::write(&path, contents)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.general.tofu_policy, TofuPolicy::Prompt);
        assert_eq!(cfg.general.cache_ttl, "30d");
        assert_eq!(cfg.general.log_level, "info");
        assert_eq!(cfg.general.default_user, None);
        assert_eq!(cfg.general.default_key, None);
        assert_eq!(cfg.diagnostics.enabled, true);
        assert_eq!(cfg.diagnostics.display, DiagnosticsDisplay::StatusBar);
        assert_eq!(cfg.diagnostics.export_format, "jsonl");
        assert_eq!(cfg.diagnostics.keepalive_interval, 15);
        assert_eq!(cfg.session.auto_reconnect, true);
        assert_eq!(cfg.session.reconnect_max_retries, 5);
        assert_eq!(cfg.session.multiplex, true);
        assert_eq!(cfg.session.recording, false);
        assert_eq!(cfg.security.min_key_bits, 3072);
        assert_eq!(cfg.audit.enabled, true);
        assert_eq!(cfg.audit.syslog_target, None);
        assert!(cfg.hosts.is_empty());
        assert!(cfg.host_groups.is_empty());
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let cfg = AppConfig::default();
        let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
        let cfg2: AppConfig = toml::from_str(&toml_str).expect("deserialize");

        assert_eq!(cfg2.general.tofu_policy, cfg.general.tofu_policy);
        assert_eq!(cfg2.general.cache_ttl, cfg.general.cache_ttl);
        assert_eq!(cfg2.general.log_level, cfg.general.log_level);
        assert_eq!(cfg2.diagnostics.enabled, cfg.diagnostics.enabled);
        assert_eq!(cfg2.diagnostics.display, cfg.diagnostics.display);
        assert_eq!(cfg2.session.auto_reconnect, cfg.session.auto_reconnect);
        assert_eq!(cfg2.session.multiplex, cfg.session.multiplex);
        assert_eq!(cfg2.security.min_key_bits, cfg.security.min_key_bits);
        assert_eq!(cfg2.security.allowed_ciphers, cfg.security.allowed_ciphers);
        assert_eq!(cfg2.security.allowed_kex, cfg.security.allowed_kex);
        assert_eq!(cfg2.security.allowed_macs, cfg.security.allowed_macs);
        assert_eq!(cfg2.audit.enabled, cfg.audit.enabled);
    }

    #[test]
    fn test_load_nonexistent_returns_default() {
        let cfg: AppConfig = toml::from_str("").expect("deserialize empty string");
        let default_cfg = AppConfig::default();

        assert_eq!(cfg.general.tofu_policy, default_cfg.general.tofu_policy);
        assert_eq!(cfg.general.cache_ttl, default_cfg.general.cache_ttl);
        assert_eq!(cfg.general.log_level, default_cfg.general.log_level);
        assert_eq!(cfg.diagnostics.enabled, default_cfg.diagnostics.enabled);
        assert_eq!(cfg.session.auto_reconnect, default_cfg.session.auto_reconnect);
        assert_eq!(cfg.security.min_key_bits, default_cfg.security.min_key_bits);
        assert_eq!(cfg.audit.enabled, default_cfg.audit.enabled);
        assert!(cfg.hosts.is_empty());
    }

    #[test]
    fn test_parse_toml_with_hosts() {
        let toml_str = r#"
            [[hosts]]
            name = "web1"
            hostname = "192.168.1.10"
            port = 2222
            user = "deploy"

            [[hosts]]
            name = "db1"
            hostname = "192.168.1.20"
        "#;

        let cfg: AppConfig = toml::from_str(toml_str).expect("parse hosts");
        assert_eq!(cfg.hosts.len(), 2);

        assert_eq!(cfg.hosts[0].name, "web1");
        assert_eq!(cfg.hosts[0].hostname, "192.168.1.10");
        assert_eq!(cfg.hosts[0].port, 2222);
        assert_eq!(cfg.hosts[0].user, Some("deploy".to_string()));

        assert_eq!(cfg.hosts[1].name, "db1");
        assert_eq!(cfg.hosts[1].hostname, "192.168.1.20");
        assert_eq!(cfg.hosts[1].port, 22);
        assert_eq!(cfg.hosts[1].user, None);
    }

    #[test]
    fn test_parse_toml_with_security() {
        let toml_str = r#"
            [security]
            min_key_bits = 4096
            allowed_ciphers = ["aes256-gcm@openssh.com"]
            allowed_kex = ["curve25519-sha256"]
            allowed_macs = ["hmac-sha2-512-etm@openssh.com"]
        "#;

        let cfg: AppConfig = toml::from_str(toml_str).expect("parse security");
        assert_eq!(cfg.security.min_key_bits, 4096);
        assert_eq!(cfg.security.allowed_ciphers, vec!["aes256-gcm@openssh.com"]);
        assert_eq!(cfg.security.allowed_kex, vec!["curve25519-sha256"]);
        assert_eq!(cfg.security.allowed_macs, vec!["hmac-sha2-512-etm@openssh.com"]);
    }

    #[test]
    fn test_tofu_policy_serde() {
        #[derive(Deserialize)]
        struct Wrapper {
            policy: TofuPolicy,
        }

        let strict: Wrapper = toml::from_str(r#"policy = "strict""#).unwrap();
        assert_eq!(strict.policy, TofuPolicy::Strict);

        let prompt: Wrapper = toml::from_str(r#"policy = "prompt""#).unwrap();
        assert_eq!(prompt.policy, TofuPolicy::Prompt);

        let auto: Wrapper = toml::from_str(r#"policy = "auto""#).unwrap();
        assert_eq!(auto.policy, TofuPolicy::Auto);
    }

    #[test]
    fn test_diagnostics_display_serde() {
        #[derive(Deserialize)]
        struct Wrapper {
            display: DiagnosticsDisplay,
        }

        let sb: Wrapper = toml::from_str(r#"display = "status_bar""#).unwrap();
        assert_eq!(sb.display, DiagnosticsDisplay::StatusBar);

        let ov: Wrapper = toml::from_str(r#"display = "overlay""#).unwrap();
        assert_eq!(ov.display, DiagnosticsDisplay::Overlay);

        let hid: Wrapper = toml::from_str(r#"display = "hidden""#).unwrap();
        assert_eq!(hid.display, DiagnosticsDisplay::Hidden);
    }

    #[test]
    fn test_host_entry_default_port() {
        let toml_str = r#"
            [[hosts]]
            name = "myhost"
            hostname = "10.0.0.1"
        "#;

        let cfg: AppConfig = toml::from_str(toml_str).expect("parse host without port");
        assert_eq!(cfg.hosts.len(), 1);
        assert_eq!(cfg.hosts[0].port, 22);
    }

    #[test]
    fn test_host_monitor_config_defaults() {
        let cfg = AppConfig::default();
        assert!(cfg.host_monitor.enabled);
        assert_eq!(cfg.host_monitor.cpu_interval, 1);
        assert_eq!(cfg.host_monitor.memory_interval, 2);
        assert_eq!(cfg.host_monitor.process_count, 15);
        assert_eq!(cfg.host_monitor.history_samples, 60);
        assert_eq!(cfg.session.max_concurrent, 9);
        assert_eq!(cfg.session.scrollback_lines, 10000);
    }

    #[test]
    fn test_fleet_config_defaults() {
        let cfg = AppConfig::default();
        assert!(cfg.fleet.probe_enabled);
        assert_eq!(cfg.fleet.probe_interval, 60);
        assert_eq!(cfg.fleet.probe_timeout, 5);
        assert_eq!(cfg.fleet.latency_history_samples, 30);
    }

    #[test]
    fn test_fleet_config_parse() {
        let toml_str = r#"
            [fleet]
            probe_enabled = false
            probe_interval = 120
            probe_timeout = 10
            latency_history_samples = 50
        "#;
        let cfg: AppConfig = toml::from_str(toml_str).expect("parse fleet config");
        assert!(!cfg.fleet.probe_enabled);
        assert_eq!(cfg.fleet.probe_interval, 120);
        assert_eq!(cfg.fleet.probe_timeout, 10);
        assert_eq!(cfg.fleet.latency_history_samples, 50);
    }

    #[test]
    fn test_data_dir() {
        let dir = AppConfig::data_dir();
        assert!(dir.ends_with(".essh"));
    }

    #[test]
    fn test_notification_patterns_serde() {
        let toml_str = r#"
            [session]
            notification_patterns = ["ERROR", "build complete", "OOM"]
        "#;
        let cfg: AppConfig = toml::from_str(toml_str).expect("parse notification_patterns");
        assert_eq!(cfg.session.notification_patterns, vec![
            "ERROR".to_string(),
            "build complete".to_string(),
            "OOM".to_string(),
        ]);

        // Round-trip
        let serialized = toml::to_string_pretty(&cfg).expect("serialize");
        let cfg2: AppConfig = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(cfg2.session.notification_patterns, cfg.session.notification_patterns);

        // Default is empty
        let default_cfg = AppConfig::default();
        assert!(default_cfg.session.notification_patterns.is_empty());
    }

    #[test]
    fn test_parse_port_forward_config() {
        let toml_str = r#"
            [[hosts]]
            name = "web1"
            hostname = "192.168.1.10"

            [[hosts.port_forwards]]
            direction = "local"
            bind_host = "127.0.0.1"
            bind_port = 8080
            target_host = "localhost"
            target_port = 80

            [[hosts.port_forwards]]
            direction = "remote"
            bind_host = "0.0.0.0"
            bind_port = 3306
            target_host = "localhost"
            target_port = 3306
        "#;

        let cfg: AppConfig = toml::from_str(toml_str).expect("parse port forwards");
        assert_eq!(cfg.hosts.len(), 1);
        assert_eq!(cfg.hosts[0].port_forwards.len(), 2);

        let pf0 = &cfg.hosts[0].port_forwards[0];
        assert_eq!(pf0.direction, "local");
        assert_eq!(pf0.bind_host, "127.0.0.1");
        assert_eq!(pf0.bind_port, 8080);
        assert_eq!(pf0.target_host, "localhost");
        assert_eq!(pf0.target_port, 80);

        let pf1 = &cfg.hosts[0].port_forwards[1];
        assert_eq!(pf1.direction, "remote");
        assert_eq!(pf1.bind_port, 3306);
        assert_eq!(pf1.target_port, 3306);
    }
}
