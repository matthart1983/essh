//! The ESSH 2.0 design system, transcribed from the handoff.
//!
//! Every constant here comes from `source/ESSH 2.0.html`. The handoff calls
//! itself high-fidelity — "colors, type scale, spacing, density and copy are
//! final" — so this module is the single place those values live, and the
//! screens read from it rather than inventing their own.
//!
//! ## What translates, and what does not
//!
//! The design was drawn for a GPU renderer with sub-cell primitives. This is
//! a terminal, so two things are approximated deliberately rather than faked:
//!
//! * **Vector sparklines become braille.** The handoff's own graph-primitives
//!   section carries the braille invariants forward "where braille is used",
//!   which is here. A braille cell resolves 2×4 dots, so a one-row graph has
//!   four vertical levels — the invariants below exist because of that.
//! * **The 3px peer ribbon has no equivalent.** A terminal has no sub-cell
//!   row, and spending a whole row on it would violate the chrome rule it was
//!   invented to satisfy. It is not drawn, rather than drawn wrongly.
//!
//! Everything else — palette, ramps, box idiom, density, copy — is exact.

use ratatui::style::Color;

// ── Palette: the *Watch 2.0 family ───────────────────────────────────────
// Replaces v1's Tokyo-Night-ish theme.

mod panel;
// `panel` and `Bind` are the underlying API — used by the panel tests and by
// callers that hold a `Buffer` rather than a `Frame`.
#[allow(unused_imports)]
pub use panel::{pane, panel, Bind, PanelOpts};

pub const BG: Color = Color::Rgb(0x0c, 0x14, 0x18);
pub const FG: Color = Color::Rgb(0xc8, 0xd4, 0xd9);
pub const DIM: Color = Color::Rgb(0x6d, 0x81, 0x89);
pub const FAINT: Color = Color::Rgb(0x42, 0x55, 0x5d);
pub const RULE: Color = Color::Rgb(0x1c, 0x28, 0x2e);

pub const GREEN: Color = Color::Rgb(0x5c, 0xd9, 0x89);
pub const CYAN: Color = Color::Rgb(0x5f, 0xdc, 0xff);
pub const AMBER: Color = Color::Rgb(0xf0, 0xc0, 0x60);
pub const RED: Color = Color::Rgb(0xff, 0x78, 0x78);
pub const VIOLET: Color = Color::Rgb(0xb8, 0xa8, 0xe8);

/// White, for the values the eye should land on first (`.nm`, headline
/// numerals). Distinct from `FG`, which is body text.
pub const WHITE: Color = Color::Rgb(0xff, 0xff, 0xff);

/// The dot used for a host that has never been probed. Deliberately not grey
/// text — it is a colour that reads as "no signal", not as "bad".
pub const NEVER_DOT: Color = Color::Rgb(0x33, 0x45, 0x4d);

/// Selection tint for the current row, `rgba(95,220,255,.05)` composited on
/// the background. Terminals have no alpha, so it is pre-mixed.
pub const ROW_SELECTED_BG: Color = Color::Rgb(0x10, 0x1a, 0x1f);

/// The green used inside meters and per-row sparklines — a shade below the
/// headline `GREEN` so a table of them does not glow.
pub const GREEN_MUTED: Color = Color::Rgb(0x3b, 0xb6, 0x73);

/// Divergence ramp. Consensus is faint; the further from consensus, the
/// hotter. **Red means "you are alone"** — never "this number is large".
pub const RAMP_DIV: [Color; 5] = [
    Color::Rgb(0x2c, 0x3a, 0x42),
    Color::Rgb(0x4a, 0x5a, 0x60),
    Color::Rgb(0x8a, 0x8f, 0x6a),
    Color::Rgb(0xf0, 0xc0, 0x60),
    Color::Rgb(0xff, 0x78, 0x78),
];

/// Magnitude ramp, cool → bright. **High means busy, not bad.**
///
/// v1 coloured CPU and memory `<50% green / <80% yellow / >80% red`, which
/// cries wolf on every compile. A busy machine is a working machine.
pub const RAMP_OK: [Color; 5] = [
    Color::Rgb(0x13, 0x4f, 0x42),
    Color::Rgb(0x1f, 0x7d, 0x58),
    Color::Rgb(0x3b, 0xb6, 0x73),
    Color::Rgb(0x5c, 0xd9, 0x89),
    Color::Rgb(0xa6, 0xf2, 0xc0),
];

/// Network ramp, magnitude, secondary.
pub const RAMP_NET: [Color; 5] = [
    Color::Rgb(0x10, 0x3f, 0x52),
    Color::Rgb(0x1c, 0x6f, 0x8c),
    Color::Rgb(0x3a, 0xa9, 0xc9),
    Color::Rgb(0x5f, 0xdc, 0xff),
    Color::Rgb(0xb6, 0xed, 0xff),
];

fn channels(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    }
}

fn mix(a: Color, b: Color, f: f64) -> Color {
    let (ar, ag, ab) = channels(a);
    let (br, bg, bb) = channels(b);
    let lerp = |x: u8, y: u8| -> u8 { (x as f64 + (y as f64 - x as f64) * f).round() as u8 };
    Color::Rgb(lerp(ar, br), lerp(ag, bg), lerp(ab, bb))
}

/// Sample a ramp at `f` in 0.0–1.0, interpolating between stops.
pub fn ramp_at(ramp: &[Color], f: f64) -> Color {
    if ramp.is_empty() {
        return FG;
    }
    let t = f.clamp(0.0, 1.0) * (ramp.len() - 1) as f64;
    let i = t.floor() as usize;
    if i >= ramp.len() - 1 {
        ramp[ramp.len() - 1]
    } else {
        mix(ramp[i], ramp[i + 1], t - i as f64)
    }
}

// ── Meters ───────────────────────────────────────────────────────────────

/// A filled meter, matching `.mtr` — a solid bar on a faint track.
///
/// Uses the lower-block glyph so the bar reads as a bar rather than as text.
pub fn meter(fraction: f64, width: usize) -> (String, String) {
    let filled = ((fraction.clamp(0.0, 1.0)) * width as f64).round() as usize;
    let filled = filled.min(width);
    ("█".repeat(filled), "░".repeat(width - filled))
}

// ── Sparklines ───────────────────────────────────────────────────────────

const BRAILLE_DOTS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// A one-row braille sparkline of `width` cells covering `values`.
///
/// The invariants the handoff carries forward, and why each is here:
///
/// * **Levels are absolute, never set-relative.** `ceiling` is supplied by the
///   caller. Scaling to the series' own maximum makes every graph touch the
///   top, which throws the magnitude channel away.
/// * **Colour by value on a single-row graph.** Height is constant across one
///   row, so sampling a ramp by cell height would encode nothing.
/// * A cell resolves four vertical levels; a series whose whole deviation
///   lands in one rounding bucket renders as a single repeated glyph, which is
///   why callers pass a ceiling derived from the measured peak.
pub fn sparkline(values: &[u64], width: usize, ceiling: u64) -> String {
    if width == 0 {
        return String::new();
    }
    if values.is_empty() {
        return " ".repeat(width);
    }
    let ceiling = ceiling.max(1) as f64;

    // Two horizontal dots per cell, so sample 2× the cell count.
    let samples = width * 2;
    let mut out = String::with_capacity(width * 3);
    let mut cell: u8 = 0;

    for s in 0..samples {
        // Map sample position back onto the series, newest at the right.
        let idx = if samples == 1 {
            values.len() - 1
        } else {
            (s * (values.len().saturating_sub(1))) / (samples - 1)
        };
        let v = values[idx.min(values.len() - 1)] as f64;
        let level = ((v / ceiling).clamp(0.0, 1.0) * 4.0).round().max(1.0) as usize;

        let col = s % 2;
        for row in 0..level.min(4) {
            // Braille row 0 is the bottom of the drawn column.
            cell |= BRAILLE_DOTS[col][3 - row];
        }
        if col == 1 {
            out.push(char::from_u32(0x2800 + cell as u32).unwrap_or(' '));
            cell = 0;
        }
    }
    if samples % 2 == 1 {
        out.push(char::from_u32(0x2800 + cell as u32).unwrap_or(' '));
    }
    out
}

/// The ceiling for a series, on a ladder with rungs no more than 25% apart.
///
/// Derived from the measured peak rather than fixed, so a quiet series still
/// uses the full four levels instead of flatlining along the bottom.
pub fn axis_ceiling(values: &[u64]) -> u64 {
    let peak = values.iter().copied().max().unwrap_or(1).max(1);
    let mut rung = 1u64;
    while rung < peak {
        // 1, 2, 3, 4, 5, 7, 10, 13, 17, 21, … — each ≤25% above the last
        // once past the small integers.
        rung = (rung as f64 * 1.25).ceil() as u64;
    }
    rung
}

// ── Widgets ──────────────────────────────────────────────────────────────

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Block;
use ratatui::Frame;

/// A column header cell: faint, uppercase. The `DIVERGE` column is cyan
/// (`th.s` in the handoff) because it is the one the eye should find.
pub fn header_cell(label: &str, emphasised: bool, t: &crate::theme::Theme) -> Span<'static> {
    Span::styled(
        label.to_uppercase(),
        Style::default().fg(if emphasised { t.brand } else { t.border }),
    )
}

/// A tag chip. `warn` marks the tag implicated in a divergence.
pub fn chip(text: &str, warn: bool, t: &crate::theme::Theme) -> Span<'static> {
    Span::styled(
        format!(" {} ", text),
        Style::default().fg(if warn {
            t.status_warn
        } else {
            t.text_secondary
        }),
    )
}

/// The status dot: a filled circle, coloured by state. Never-probed uses a
/// colour that reads as "no signal" rather than as a bad reading.
pub fn dot(color: Color) -> Span<'static> {
    Span::styled("●", Style::default().fg(color))
}

/// A footer of `key label` pairs, cyan key and faint label, matching `.foot`.
pub fn footer_line(pairs: &[(&str, &str)], t: &crate::theme::Theme) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (i, (key, label)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled(
            key.to_string(),
            Style::default().fg(t.key_hint).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            label.to_string(),
            Style::default().fg(t.border),
        ));
    }
    Line::from(spans)
}

/// A headline numeral with its unit and a sub-line, per the handoff's `big()`.
///
/// The sub-line is where the peer median lives: *"every headline metric
/// carries its peer median, so 23% becomes 23% against a fleet median of
/// 31%"*. A number alone is not a finding.
#[allow(dead_code)] // composed inline by the monitor's headline_box
pub fn headline<'a>(
    value: &'a str,
    unit: &'a str,
    sub: &'a str,
    t: &crate::theme::Theme,
) -> Vec<Line<'a>> {
    vec![
        Line::from(vec![
            Span::styled(
                value,
                Style::default()
                    .fg(t.text_emphasis)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(unit, Style::default().fg(t.text_secondary)),
        ]),
        Line::from(Span::styled(sub, Style::default().fg(t.border))),
    ]
}

/// An honest empty state: italic, faint, in words. Never a zero, never a
/// dash in a laid-out column, never a stub bar.
pub fn none_line(text: &str, t: &crate::theme::Theme) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default().fg(t.border).add_modifier(Modifier::ITALIC),
    ))
}

/// Paint a region with the design background, so the app owns its canvas
/// rather than inheriting whatever the host terminal happens to use.
/// Fill a region with the theme's own ground.
///
/// This used to hardcode the 2.0 palette's `BG`/`FG`, which is why a light
/// theme could not work: its near-black text landed on a painted dark panel.
/// A theme that wants the terminal's background leaves `bg` as `Color::Reset`,
/// which paints nothing.
pub fn paint_bg(f: &mut Frame, area: Rect, theme: &crate::theme::Theme) {
    f.render_widget(
        Block::default().style(Style::default().bg(theme.bg).fg(theme.text_primary)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_matches_the_handoff_exactly() {
        // These are quoted in the handoff's colour section; a drift here
        // means the screens no longer match the design they cite.
        assert_eq!(BG, Color::Rgb(0x0c, 0x14, 0x18));
        assert_eq!(FG, Color::Rgb(0xc8, 0xd4, 0xd9));
        assert_eq!(DIM, Color::Rgb(0x6d, 0x81, 0x89));
        assert_eq!(FAINT, Color::Rgb(0x42, 0x55, 0x5d));
        assert_eq!(RULE, Color::Rgb(0x1c, 0x28, 0x2e));
        assert_eq!(GREEN, Color::Rgb(0x5c, 0xd9, 0x89));
        assert_eq!(CYAN, Color::Rgb(0x5f, 0xdc, 0xff));
        assert_eq!(AMBER, Color::Rgb(0xf0, 0xc0, 0x60));
        assert_eq!(RED, Color::Rgb(0xff, 0x78, 0x78));
        assert_eq!(VIOLET, Color::Rgb(0xb8, 0xa8, 0xe8));
    }

    #[test]
    fn meter_fills_proportionally_and_never_overflows() {
        let (f, e) = meter(0.42, 10);
        assert_eq!(f.chars().count(), 4);
        assert_eq!(f.chars().count() + e.chars().count(), 10);

        let (f, e) = meter(1.5, 10);
        assert_eq!(f.chars().count(), 10);
        assert_eq!(e.chars().count(), 0);

        let (f, _) = meter(-1.0, 10);
        assert_eq!(f.chars().count(), 0);
    }

    #[test]
    fn a_sparkline_is_exactly_the_width_asked_for() {
        for w in [1usize, 7, 26, 74] {
            let s = sparkline(&[1, 5, 3, 9, 2], w, 10);
            assert_eq!(s.chars().count(), w, "width {}", w);
        }
    }

    #[test]
    fn an_empty_series_draws_nothing_rather_than_a_flat_line() {
        // Drawing a graph for data you do not have is worse than drawing
        // nothing — the handoff's rule, and the v1 defect it names.
        let s = sparkline(&[], 10, 100);
        assert_eq!(s, "          ");
        assert!(!s.contains('⣿'));
    }

    #[test]
    fn levels_are_absolute_so_a_quiet_series_stays_low() {
        // Set-relative scaling would make both of these look identical.
        let quiet = sparkline(&[1, 2, 1, 2], 8, 100);
        let busy = sparkline(&[90, 95, 92, 99], 8, 100);
        assert_ne!(quiet, busy);
        // The quiet one must use the lowest dots only.
        assert!(
            quiet.chars().all(|c| {
                let bits = c as u32 - 0x2800;
                // Only the bottom row of each column: 0x40 and 0x80.
                bits & !(0x40 | 0x80) == 0
            }),
            "quiet series climbed: {}",
            quiet
        );
    }

    #[test]
    fn axis_ceiling_climbs_in_rungs_no_more_than_25_percent_apart() {
        assert!(axis_ceiling(&[1]) >= 1);
        for peak in [3u64, 17, 42, 100, 999] {
            let c = axis_ceiling(&[peak]);
            assert!(c >= peak, "ceiling {} below peak {}", c, peak);
            assert!(
                c as f64 <= peak as f64 * 1.25 + 1.0,
                "ceiling {} too far above peak {}",
                c,
                peak
            );
        }
    }

    #[test]
    fn a_series_at_its_ceiling_reaches_the_top_row() {
        let s = sparkline(&[10, 10, 10], 4, 10);
        // Full column = all four dots in both sub-columns.
        assert!(s.chars().all(|c| c == '⣿'), "got {}", s);
    }
}
