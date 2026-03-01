use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Asciicast v2 header — first line of the .cast file.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CastHeader {
    pub version: u32,
    pub width: u32,
    pub height: u32,
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::HashMap<String, String>>,
}

/// A single asciicast v2 event: [time, type, data]
#[derive(Debug, Clone)]
pub struct CastEvent {
    pub time: f64,
    pub event_type: String, // "o" = output, "i" = input
    pub data: String,
}

impl CastEvent {
    pub fn to_json(&self) -> String {
        format!(
            "[{:.6}, \"{}\", {}]",
            self.time,
            self.event_type,
            serde_json::to_string(&self.data).unwrap_or_default()
        )
    }

    pub fn from_json(line: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        let arr = v.as_array()?;
        if arr.len() < 3 {
            return None;
        }
        Some(CastEvent {
            time: arr[0].as_f64()?,
            event_type: arr[1].as_str()?.to_string(),
            data: arr[2].as_str()?.to_string(),
        })
    }
}

/// Records terminal I/O to an asciicast v2 file.
pub struct SessionRecorder {
    writer: Mutex<BufWriter<File>>,
    start: Instant,
}

impl SessionRecorder {
    /// Create a new recorder, writing to the given path.
    /// Writes the asciicast header immediately.
    pub fn new(path: &Path, width: u32, height: u32, title: Option<String>) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);

        let header = CastHeader {
            version: 2,
            width,
            height,
            timestamp: chrono::Utc::now().timestamp(),
            title,
            env: None,
        };
        let header_json = serde_json::to_string(&header).unwrap_or_default();
        writeln!(writer, "{}", header_json)?;
        writer.flush()?;

        Ok(Self {
            writer: Mutex::new(writer),
            start: Instant::now(),
        })
    }

    /// Record output data (remote → terminal).
    pub fn record_output(&self, data: &[u8]) {
        self.record_event("o", data);
    }

    /// Record input data (user → remote).
    pub fn record_input(&self, data: &[u8]) {
        self.record_event("i", data);
    }

    fn record_event(&self, event_type: &str, data: &[u8]) {
        let elapsed = self.start.elapsed().as_secs_f64();
        let text = String::from_utf8_lossy(data);
        let event = CastEvent {
            time: elapsed,
            event_type: event_type.to_string(),
            data: text.to_string(),
        };
        if let Ok(mut w) = self.writer.lock() {
            writeln!(w, "{}", event.to_json()).ok();
            // Flush periodically would be better, but for correctness flush each event
            w.flush().ok();
        }
    }
}

/// Parse a .cast file into header + events.
pub fn parse_cast_file(path: &Path) -> std::io::Result<(CastHeader, Vec<CastEvent>)> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    let header_line = lines
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "Empty cast file"))??;
    let header: CastHeader = serde_json::from_str(&header_line)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let mut events = Vec::new();
    for line in lines {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(event) = CastEvent::from_json(&line) {
            events.push(event);
        }
    }

    Ok((header, events))
}

/// List available recordings.
pub fn list_recordings() -> std::io::Result<Vec<(String, PathBuf)>> {
    let dir = recording_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut recordings = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("cast") {
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            recordings.push((name, path));
        }
    }
    recordings.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(recordings)
}

/// Get the recordings directory path.
pub fn recording_dir() -> PathBuf {
    dirs::home_dir()
        .expect("home dir")
        .join(".essh")
        .join("recordings")
}

/// Build the recording file path for a session.
pub fn recording_path(session_id: &str) -> PathBuf {
    recording_dir().join(format!("{}.cast", session_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_cast_event_roundtrip() {
        let event = CastEvent {
            time: 1.234567,
            event_type: "o".to_string(),
            data: "hello world\r\n".to_string(),
        };
        let json = event.to_json();
        let parsed = CastEvent::from_json(&json).unwrap();
        assert!((parsed.time - 1.234567).abs() < 0.001);
        assert_eq!(parsed.event_type, "o");
        assert_eq!(parsed.data, "hello world\r\n");
    }

    #[test]
    fn test_cast_event_special_chars() {
        let event = CastEvent {
            time: 0.5,
            event_type: "o".to_string(),
            data: "line1\nline2\ttab\"quote".to_string(),
        };
        let json = event.to_json();
        let parsed = CastEvent::from_json(&json).unwrap();
        assert_eq!(parsed.data, "line1\nline2\ttab\"quote");
    }

    #[test]
    fn test_cast_event_input_type() {
        let event = CastEvent {
            time: 2.0,
            event_type: "i".to_string(),
            data: "ls\r".to_string(),
        };
        let json = event.to_json();
        let parsed = CastEvent::from_json(&json).unwrap();
        assert_eq!(parsed.event_type, "i");
    }

    #[test]
    fn test_recorder_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.cast");
        let recorder = SessionRecorder::new(&path, 80, 24, Some("test".to_string())).unwrap();
        recorder.record_output(b"hello");
        recorder.record_input(b"world");
        drop(recorder);

        let (header, events) = parse_cast_file(&path).unwrap();
        assert_eq!(header.version, 2);
        assert_eq!(header.width, 80);
        assert_eq!(header.height, 24);
        assert_eq!(header.title, Some("test".to_string()));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "o");
        assert_eq!(events[0].data, "hello");
        assert_eq!(events[1].event_type, "i");
        assert_eq!(events[1].data, "world");
    }

    #[test]
    fn test_recorder_timestamps_increase() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("timestamps.cast");
        let recorder = SessionRecorder::new(&path, 80, 24, None).unwrap();
        recorder.record_output(b"first");
        std::thread::sleep(std::time::Duration::from_millis(10));
        recorder.record_output(b"second");
        drop(recorder);

        let (_, events) = parse_cast_file(&path).unwrap();
        assert!(events[1].time > events[0].time);
    }

    #[test]
    fn test_parse_cast_file_empty_events() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.cast");
        let recorder = SessionRecorder::new(&path, 120, 40, None).unwrap();
        drop(recorder);

        let (header, events) = parse_cast_file(&path).unwrap();
        assert_eq!(header.width, 120);
        assert_eq!(header.height, 40);
        assert!(events.is_empty());
    }

    #[test]
    fn test_cast_header_serde() {
        let header = CastHeader {
            version: 2,
            width: 80,
            height: 24,
            timestamp: 1700000000,
            title: Some("my session".to_string()),
            env: None,
        };
        let json = serde_json::to_string(&header).unwrap();
        let parsed: CastHeader = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.version, 2);
        assert_eq!(parsed.width, 80);
        assert_eq!(parsed.title, Some("my session".to_string()));
    }
}
