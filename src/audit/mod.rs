use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Event types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    ConnectionAttempt,
    ConnectionEstablished,
    ConnectionFailed,
    ConnectionClosed,
    AuthSuccess,
    AuthFailure,
    HostKeyVerified,
    HostKeyChanged,
    HostKeyNewTrust,
    HostKeyRejected,
    SessionStart,
    SessionEnd,
    CommandExecuted,
}

// ---------------------------------------------------------------------------
// AuditEvent
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditEvent {
    pub timestamp: String,
    pub event_type: AuditEventType,
    pub session_id: Option<String>,
    pub hostname: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub details: serde_json::Value,
}

// ---------------------------------------------------------------------------
// AuditLogger
// ---------------------------------------------------------------------------

pub struct AuditLogger {
    pub log_path: PathBuf,
    pub enabled: bool,
}

impl AuditLogger {
    pub fn new(log_path: PathBuf, enabled: bool) -> Self {
        Self { log_path, enabled }
    }

    pub fn default_logger() -> Self {
        let path = dirs::home_dir()
            .expect("could not determine home directory")
            .join(".essh")
            .join("audit.log");
        Self::new(path, true)
    }

    pub fn log(&self, event: &AuditEvent) -> Result<(), std::io::Error> {
        if !self.enabled {
            return Ok(());
        }

        if let Some(parent) = self.log_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)?;

        let line = serde_json::to_string(event).map_err(std::io::Error::other)?;
        writeln!(file, "{}", line)?;

        Ok(())
    }

    pub fn log_connection_attempt(
        &self,
        session_id: &str,
        hostname: &str,
        port: u16,
        username: &str,
    ) {
        let event = AuditEvent {
            timestamp: Utc::now().to_rfc3339(),
            event_type: AuditEventType::ConnectionAttempt,
            session_id: Some(session_id.to_string()),
            hostname: Some(hostname.to_string()),
            port: Some(port),
            username: Some(username.to_string()),
            details: serde_json::json!({}),
        };
        let _ = self.log(&event);
    }

    pub fn log_auth_result(
        &self,
        session_id: &str,
        hostname: &str,
        port: u16,
        username: &str,
        method: &str,
        success: bool,
    ) {
        let event_type = if success {
            AuditEventType::AuthSuccess
        } else {
            AuditEventType::AuthFailure
        };
        let event = AuditEvent {
            timestamp: Utc::now().to_rfc3339(),
            event_type,
            session_id: Some(session_id.to_string()),
            hostname: Some(hostname.to_string()),
            port: Some(port),
            username: Some(username.to_string()),
            details: serde_json::json!({ "method": method }),
        };
        let _ = self.log(&event);
    }

    pub fn log_host_key_event(
        &self,
        session_id: &str,
        hostname: &str,
        port: u16,
        event_type: AuditEventType,
        fingerprint: &str,
    ) {
        let event = AuditEvent {
            timestamp: Utc::now().to_rfc3339(),
            event_type,
            session_id: Some(session_id.to_string()),
            hostname: Some(hostname.to_string()),
            port: Some(port),
            username: None,
            details: serde_json::json!({ "fingerprint": fingerprint }),
        };
        let _ = self.log(&event);
    }

    pub fn log_session_event(
        &self,
        session_id: &str,
        hostname: &str,
        port: u16,
        event_type: AuditEventType,
    ) {
        let event = AuditEvent {
            timestamp: Utc::now().to_rfc3339(),
            event_type,
            session_id: Some(session_id.to_string()),
            hostname: Some(hostname.to_string()),
            port: Some(port),
            username: None,
            details: serde_json::json!({}),
        };
        let _ = self.log(&event);
    }

    pub fn tail(&self, n: usize) -> Result<Vec<AuditEvent>, Box<dyn std::error::Error>> {
        let file = fs::File::open(&self.log_path)?;
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader.lines().collect::<Result<_, _>>()?;

        let start = lines.len().saturating_sub(n);
        let events = lines[start..]
            .iter()
            .map(|line| serde_json::from_str(line))
            .collect::<Result<Vec<AuditEvent>, _>>()?;

        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs as stdfs;

    fn temp_log_path() -> PathBuf {
        std::env::temp_dir().join(format!("audit_test_{}.log", uuid::Uuid::new_v4()))
    }

    fn make_event(event_type: AuditEventType) -> AuditEvent {
        AuditEvent {
            timestamp: Utc::now().to_rfc3339(),
            event_type,
            session_id: Some("sess-1".to_string()),
            hostname: Some("host.example.com".to_string()),
            port: Some(22),
            username: Some("alice".to_string()),
            details: serde_json::json!({}),
        }
    }

    #[test]
    fn test_audit_event_serialize() {
        let event = make_event(AuditEventType::ConnectionAttempt);
        let json = serde_json::to_string(&event).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["event_type"], "connection_attempt");
        assert_eq!(v["hostname"], "host.example.com");
        assert_eq!(v["port"], 22);
        assert_eq!(v["username"], "alice");
        assert!(v["timestamp"].is_string());
    }

    #[test]
    fn test_audit_event_type_serialize() {
        let val = serde_json::to_value(AuditEventType::ConnectionAttempt).unwrap();
        assert_eq!(val, serde_json::json!("connection_attempt"));
    }

    #[test]
    fn test_audit_logger_disabled() {
        let path = temp_log_path();
        let logger = AuditLogger::new(path.clone(), false);
        let event = make_event(AuditEventType::SessionStart);
        assert!(logger.log(&event).is_ok());
        assert!(!path.exists());
    }

    #[test]
    fn test_audit_logger_writes_to_file() {
        let path = temp_log_path();
        let logger = AuditLogger::new(path.clone(), true);
        let event = make_event(AuditEventType::ConnectionEstablished);
        logger.log(&event).unwrap();

        let contents = stdfs::read_to_string(&path).unwrap();
        let parsed: AuditEvent = serde_json::from_str(contents.trim()).unwrap();
        assert!(matches!(
            parsed.event_type,
            AuditEventType::ConnectionEstablished
        ));
        stdfs::remove_file(&path).ok();
    }

    #[test]
    fn test_audit_logger_appends() {
        let path = temp_log_path();
        let logger = AuditLogger::new(path.clone(), true);
        logger
            .log(&make_event(AuditEventType::SessionStart))
            .unwrap();
        logger.log(&make_event(AuditEventType::SessionEnd)).unwrap();

        let contents = stdfs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = contents.trim().lines().collect();
        assert_eq!(lines.len(), 2);
        stdfs::remove_file(&path).ok();
    }

    #[test]
    fn test_log_connection_attempt() {
        let path = temp_log_path();
        let logger = AuditLogger::new(path.clone(), true);
        logger.log_connection_attempt("s1", "host.example.com", 22, "alice");

        let contents = stdfs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(v["event_type"], "connection_attempt");
        stdfs::remove_file(&path).ok();
    }

    #[test]
    fn test_log_auth_result_success() {
        let path = temp_log_path();
        let logger = AuditLogger::new(path.clone(), true);
        logger.log_auth_result("s1", "host.example.com", 22, "alice", "publickey", true);

        let contents = stdfs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(v["event_type"], "auth_success");
        stdfs::remove_file(&path).ok();
    }

    #[test]
    fn test_log_auth_result_failure() {
        let path = temp_log_path();
        let logger = AuditLogger::new(path.clone(), true);
        logger.log_auth_result("s1", "host.example.com", 22, "alice", "password", false);

        let contents = stdfs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(v["event_type"], "auth_failure");
        stdfs::remove_file(&path).ok();
    }

    #[test]
    fn test_log_host_key_event() {
        let path = temp_log_path();
        let logger = AuditLogger::new(path.clone(), true);
        logger.log_host_key_event(
            "s1",
            "host.example.com",
            22,
            AuditEventType::HostKeyVerified,
            "SHA256:abc123",
        );

        let contents = stdfs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(v["event_type"], "host_key_verified");
        assert_eq!(v["details"]["fingerprint"], "SHA256:abc123");
        stdfs::remove_file(&path).ok();
    }

    #[test]
    fn test_tail() {
        let path = temp_log_path();
        let logger = AuditLogger::new(path.clone(), true);
        for _ in 0..5 {
            logger
                .log(&make_event(AuditEventType::CommandExecuted))
                .unwrap();
        }

        let events = logger.tail(3).unwrap();
        assert_eq!(events.len(), 3);
        stdfs::remove_file(&path).ok();
    }

    #[test]
    fn test_tail_more_than_available() {
        let path = temp_log_path();
        let logger = AuditLogger::new(path.clone(), true);
        logger
            .log(&make_event(AuditEventType::SessionStart))
            .unwrap();
        logger.log(&make_event(AuditEventType::SessionEnd)).unwrap();

        let events = logger.tail(10).unwrap();
        assert_eq!(events.len(), 2);
        stdfs::remove_file(&path).ok();
    }
}
