//! Display formatting shared by the dashboard, the monitor and the fleet view.
//!
//! These exist because v1 spent its columns on precision nobody reads —
//! `2026-03-02T09:40:05.401277+00:00` in a list view — and truncated from the
//! wrong end, so `region=us-east-1` became `region=us-ea` and the part that
//! identified the value was the part thrown away.

use chrono::{DateTime, Utc};

/// Render an RFC-3339 timestamp as an age: `now`, `4m ago`, `2d ago`.
///
/// A list view is scanned, not read. Nobody has ever needed microseconds in
/// one, and the full ISO form costs 32 columns to say "recently".
pub fn relative_time(iso: &str) -> String {
    match DateTime::parse_from_rfc3339(iso) {
        Ok(then) => relative_since(then.with_timezone(&Utc), Utc::now()),
        Err(_) => "unknown".to_string(),
    }
}

/// Testable core of [`relative_time`] with an injectable "now".
pub fn relative_since(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - then).num_seconds();
    if secs < 0 {
        // A clock disagreement, not a duration. Don't invent "in 3 minutes".
        return "clock skew".to_string();
    }
    match secs {
        0..=44 => "now".to_string(),
        45..=5399 => format!("{}m ago", (secs as f64 / 60.0).round() as i64),
        5400..=86_399 => format!("{}h ago", (secs as f64 / 3600.0).round() as i64),
        86_400..=2_591_999 => format!("{}d ago", secs / 86_400),
        2_592_000..=31_535_999 => format!("{}mo ago", secs / 2_592_000),
        _ => format!("{}y ago", secs / 31_536_000),
    }
}

/// Truncate a path from the left, keeping the identifying tail.
///
/// `/private/var/folders/6c/T/AppTranslocation/917C42E9/d/Foo.app` says nothing
/// in its first 40 characters and everything in its last 20.
pub fn truncate_left(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width || width == 0 {
        return s.to_string();
    }
    if width == 1 {
        return "…".to_string();
    }
    let tail: String = chars[chars.len() - (width - 1)..].iter().collect();
    format!("…{}", tail)
}

/// Truncate from the right, for values whose head identifies them —
/// facet names, where the leading words carry the meaning.
pub fn truncate_right(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width || width == 0 {
        return s.to_string();
    }
    if width == 1 {
        return "…".to_string();
    }
    let head: String = chars[..width - 1].iter().collect();
    format!("{}…", head)
}

/// Lay out `key=value` tags as whole chips within `width`, with a `+N` count
/// for the ones that did not fit.
///
/// The rule is that a chip is shown whole or not at all. v1 clipped mid-word
/// and produced `env=`, which reads as an empty value rather than a hidden one.
pub fn tag_chips(tags: &[(String, String)], width: usize) -> (Vec<String>, usize) {
    let mut shown: Vec<String> = Vec::new();
    let mut used = 0usize;

    for (k, v) in tags {
        let chip = if v.is_empty() {
            k.clone()
        } else {
            format!("{}={}", k, v)
        };
        let cost = chip.chars().count() + if shown.is_empty() { 0 } else { 1 };
        // Reserve room for a "+N" marker if anything will be left over.
        let remaining_after = tags.len() - shown.len() - 1;
        let reserve = if remaining_after > 0 { 4 } else { 0 };

        if used + cost + reserve > width {
            break;
        }
        used += cost;
        shown.push(chip);
    }

    let hidden = tags.len() - shown.len();
    (shown, hidden)
}

/// Sort tags so the display order is stable across renders.
pub fn sorted_tags(tags: &std::collections::HashMap<String, String>) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = tags.iter().map(|(k, s)| (k.clone(), s.clone())).collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn relative_time_replaces_the_iso_wall_of_text() {
        let now = at("2026-03-04T09:40:05Z");
        // The exact value from the v1 hosts list.
        assert_eq!(relative_since(at("2026-03-02T09:40:05Z"), now), "2d ago");
        assert_eq!(relative_since(at("2026-03-04T09:38:05Z"), now), "2m ago");
        assert_eq!(relative_since(at("2026-03-04T09:40:00Z"), now), "now");
        assert_eq!(relative_since(at("2026-03-04T04:40:05Z"), now), "5h ago");
        assert_eq!(relative_since(at("2025-09-04T09:40:05Z"), now), "6mo ago");
        assert_eq!(relative_since(at("2023-03-04T09:40:05Z"), now), "3y ago");
    }

    #[test]
    fn a_future_timestamp_is_named_not_rendered_as_a_duration() {
        let now = at("2026-03-04T09:40:05Z");
        assert_eq!(relative_since(at("2026-03-05T09:40:05Z"), now), "clock skew");
    }

    #[test]
    fn unparseable_timestamps_say_unknown_rather_than_guessing() {
        assert_eq!(relative_time("not a date"), "unknown");
        assert_eq!(relative_time(""), "unknown");
    }

    #[test]
    fn relative_time_accepts_the_format_the_cache_writes() {
        // CacheDb stores chrono's Utc::now().to_rfc3339().
        let iso = Utc.with_ymd_and_hms(2026, 3, 2, 9, 40, 5).unwrap().to_rfc3339();
        assert_ne!(relative_time(&iso), "unknown");
    }

    #[test]
    fn paths_truncate_from_the_left_so_the_tail_survives() {
        let p = "/private/var/folders/6c/T/AppTranslocation/917C42E9/d/Foo.app";
        let out = truncate_left(p, 20);
        assert_eq!(out.chars().count(), 20);
        assert!(out.starts_with('…'));
        assert!(out.ends_with("Foo.app"), "tail lost: {}", out);
        // Short enough already: untouched.
        assert_eq!(truncate_left("/", 20), "/");
    }

    #[test]
    fn tag_chips_are_shown_whole_or_not_at_all() {
        let tags = vec![
            ("region".to_string(), "us-east-1".to_string()),
            ("env".to_string(), "prod".to_string()),
            ("role".to_string(), "web".to_string()),
        ];
        // Wide enough for everything.
        let (shown, hidden) = tag_chips(&tags, 80);
        assert_eq!(shown.len(), 3);
        assert_eq!(hidden, 0);

        // Narrow: the v1 failure was rendering "region=us-ea" and "env=".
        let (shown, hidden) = tag_chips(&tags, 20);
        assert_eq!(hidden, 3 - shown.len());
        for chip in &shown {
            assert!(!chip.ends_with('='), "clipped mid-value: {}", chip);
            assert!(
                tags.iter().any(|(k, v)| *chip == format!("{}={}", k, v)),
                "chip {} is not a whole tag",
                chip
            );
        }
    }

    #[test]
    fn tag_chips_leave_room_for_the_overflow_marker() {
        let tags: Vec<(String, String)> = (0..6)
            .map(|i| (format!("k{}", i), "value".to_string()))
            .collect();
        let width = 24;
        let (shown, hidden) = tag_chips(&tags, width);
        assert!(hidden > 0, "test needs an overflow case");
        let rendered: usize =
            shown.iter().map(|c| c.chars().count()).sum::<usize>() + shown.len().saturating_sub(1);
        assert!(
            rendered + 4 <= width,
            "no room left for +{} marker: {} of {}",
            hidden,
            rendered,
            width
        );
    }

    #[test]
    fn tag_chips_handle_a_valueless_tag() {
        let tags = vec![("decommissioned".to_string(), String::new())];
        let (shown, _) = tag_chips(&tags, 40);
        assert_eq!(shown, vec!["decommissioned".to_string()]);
    }
}
