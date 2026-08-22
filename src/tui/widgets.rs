use ratatui::style::Color;

use crate::theme::Theme;

/// Format bytes per second into human-readable rate string.
pub fn format_bytes_rate(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1_000_000_000.0 {
        format!("{:.1} GB/s", bytes_per_sec / 1_000_000_000.0)
    } else if bytes_per_sec >= 1_000_000.0 {
        format!("{:.1} MB/s", bytes_per_sec / 1_000_000.0)
    } else if bytes_per_sec >= 1_000.0 {
        format!("{:.1} KB/s", bytes_per_sec / 1_000.0)
    } else {
        format!("{:.0}  B/s", bytes_per_sec)
    }
}

/// Format byte count into human-readable total string.
pub fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{} B", bytes)
    }
}

/// Format KB into human-readable string.
pub fn format_kb(kb: u64) -> String {
    format_bytes(kb * 1024)
}

/// Format seconds into human-readable uptime string like "42d 3h 17m".
pub fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins)
    }
}

/// Format seconds into short duration string like "2h 14m".
pub fn format_duration_short(secs: i64) -> String {
    let secs = secs.unsigned_abs();
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else if mins > 0 {
        format!("{}m", mins)
    } else {
        format!("{}s", secs)
    }
}

/// Render a sparkline string from sample data using Unicode block characters.
/// Values are normalized to max value in the dataset (or provided max).
/// Render a horizontal bar gauge like "████████░░░░░░░░░ 45%"
/// Returns a string of the bar portion (without the percentage).
pub fn bar_gauge(pct: f64, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f64) as usize;
    let empty = width.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

/// Get connection quality color.
pub fn quality_color(theme: &Theme, quality: &str) -> Color {
    match quality {
        "Excellent" | "Good" => theme.status_good,
        "Fair" => theme.status_warn,
        _ => theme.status_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes_rate() {
        assert_eq!(format_bytes_rate(500.0), "500  B/s");
        assert_eq!(format_bytes_rate(1500.0), "1.5 KB/s");
        assert_eq!(format_bytes_rate(1_500_000.0), "1.5 MB/s");
        assert_eq!(format_bytes_rate(1_500_000_000.0), "1.5 GB/s");
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1500), "1.5 KB");
        assert_eq!(format_bytes(1_500_000), "1.5 MB");
        assert_eq!(format_bytes(1_500_000_000), "1.5 GB");
    }

    #[test]
    fn test_format_uptime() {
        assert_eq!(format_uptime(3661234), "42d 9h 0m");
        assert_eq!(format_uptime(7380), "2h 3m");
        assert_eq!(format_uptime(300), "5m");
    }

    #[test]
    fn test_bar_gauge() {
        let bar = bar_gauge(50.0, 10);
        assert_eq!(bar.chars().count(), 10);
    }
}

