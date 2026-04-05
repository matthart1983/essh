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
pub fn sparkline_string(data: &[u64], width: usize) -> String {
    let blocks = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if data.is_empty() {
        return " ".repeat(width);
    }

    let max = *data.iter().max().unwrap_or(&1).max(&1);
    let start = if data.len() > width {
        data.len() - width
    } else {
        0
    };
    let visible = &data[start..];

    let mut result = String::with_capacity(width);
    for &val in visible {
        let idx = if max == 0 {
            0
        } else {
            ((val as f64 / max as f64) * 7.0) as usize
        };
        result.push(blocks[idx.min(7)]);
    }
    // Pad with spaces if not enough data
    while result.chars().count() < width {
        result.insert(0, ' ');
    }
    result
}

/// Get color for a percentage value using threshold-based coloring.
/// 0-50%: Green, 50-80%: Yellow, 80-100%: Red
pub fn pct_color(theme: &Theme, pct: f64) -> Color {
    if pct >= 80.0 {
        theme.status_error
    } else if pct >= 50.0 {
        theme.status_warn
    } else {
        theme.status_good
    }
}

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
    fn test_sparkline_string() {
        let data = vec![0, 25, 50, 75, 100];
        let s = sparkline_string(&data, 5);
        assert_eq!(s.chars().count(), 5);
    }

    #[test]
    fn test_bar_gauge() {
        let bar = bar_gauge(50.0, 10);
        assert_eq!(bar.chars().count(), 10);
    }

    #[test]
    fn test_pct_color() {
        let theme = crate::theme::dark();
        assert_eq!(pct_color(&theme, 30.0), Color::Green);
        assert_eq!(pct_color(&theme, 60.0), Color::Yellow);
        assert_eq!(pct_color(&theme, 90.0), Color::Red);
    }
}
