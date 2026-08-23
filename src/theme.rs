use ratatui::prelude::Color;

/// Semantic color roles for the ESSH TUI.
#[derive(Debug, Clone)]
// Some slots are unused by the current screens but are part of the theme
// contract every palette must fill, so they stay.
#[allow(dead_code)]
pub struct Theme {
    pub name: &'static str,

    // Brand / chrome
    pub brand: Color,
    pub active_tab: Color,
    pub inactive_tab: Color,
    pub border: Color,
    pub separator: Color,

    // Text
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub text_inverse: Color,
    /// Brighter than `text_primary`, for the one string in a row that the eye
    /// should land on first — a hostname, a heading. Not `active_tab`: that is
    /// chrome, this is content.
    pub text_emphasis: Color,

    // Status / semantic
    pub status_good: Color,
    pub status_warn: Color,
    pub status_error: Color,
    pub status_info: Color,

    // Data
    pub rx_rate: Color,
    pub tx_rate: Color,
    pub key_hint: Color,

    // Selection
    pub selection_bg: Color,
    pub highlight_bg: Color,

    /// Panel fill.
    ///
    /// Most themes leave this `Color::Reset` so the terminal's own background
    /// shows through — the historical look. A theme that inverts the UI, like
    /// `paper`, has to paint a real background or its dark text lands on the
    /// terminal's dark background and disappears. The root renderer fills this
    /// before dispatching to a screen.
    pub bg: Color,

    /// A `status_good` that has to sit quietly — a filled capacity bar, a
    /// sparkline under a healthy host. Reads as "fine" without pulling the eye
    /// the way the full-strength status colour does.
    pub muted_good: Color,

    /// Never measured. Distinct from `status_error`, which means measured and
    /// bad, and from `text_muted`, which is text. A host that has never been
    /// probed is not a failing host.
    pub absent: Color,
}

impl Theme {
    /// True when every slot resolves through the terminal's own palette rather
    /// than fixed RGB. Effects that synthesise colour — the gradient ramps —
    /// switch off rather than emit 24-bit values the theme exists to avoid.
    pub fn defers_to_terminal(&self) -> bool {
        self.name == "terminal"
    }

    /// Divergence severity ramp, cold to hot.
    pub fn ramp_divergence(&self, f: f64) -> Color {
        self.ramp(&crate::design::RAMP_DIV, f, Color::Red)
    }

    /// Throughput ramp for network rates.
    pub fn ramp_net(&self, f: f64) -> Color {
        self.ramp(&crate::design::RAMP_NET, f, Color::Cyan)
    }

    /// Magnitude of a 0-100 reading — CPU, memory. Cool to bright, and never
    /// red: a busy machine is a working machine.
    pub fn magnitude(&self, pct: f64) -> Color {
        self.ramp(&crate::design::RAMP_OK, pct / 100.0, Color::Cyan)
    }

    /// A 0-100 reading where high IS bad — capacity, utilisation.
    ///
    /// Stepped, not a ramp: a disk at 79% and one at 81% should look
    /// different, and a continuous gradient makes that boundary invisible.
    pub fn bounded_bad(&self, pct: f64) -> Color {
        match pct {
            p if p >= 90.0 => self.status_error,
            p if p >= 80.0 => self.status_warn,
            _ => self.muted_good,
        }
    }

    /// Severity by count of diverged items.
    ///
    /// Zero is `border`, not the ramp's cold end: agreement should be quiet
    /// rather than celebrated, and the ramp's floor still reads as a value.
    pub fn divergence_count(&self, n: usize) -> Color {
        if n == 0 {
            self.border
        } else {
            self.ramp_divergence((n as f64 / 3.0).min(1.0))
        }
    }

    fn ramp(&self, stops: &[Color; 5], f: f64, ansi: Color) -> Color {
        if self.defers_to_terminal() {
            // A five-stop RGB gradient has no 16-colour equivalent. Degrade to
            // one themed slot rather than leak fixed RGB into the one theme
            // whose whole promise is that it pins none.
            return ansi;
        }
        crate::design::ramp_at(stops, f)
    }
}

pub const THEME_NAMES: &[&str] = &[
    "dark",
    // Defers every slot to the terminal's palette — see `terminal()`.
    "terminal",
    "light",
    "solarized",
    "dracula",
    "nord",
    // Fixed palette over the terminal's own background, and the same palette
    // painting its own — see `ocean` and `sky`.
    "ocean",
    "sky",
];

pub fn by_name(name: &str) -> Theme {
    match name.to_lowercase().as_str() {
        // "system" and "ansi" are what users coming from other TUIs reach for;
        // accept both rather than silently falling back to dark and looking
        // like the feature is missing.
        "terminal" | "system" | "ansi" => terminal(),
        "light" | "paper" => light(),
        "solarized" => solarized(),
        "dracula" => dracula(),
        "nord" => nord(),
        "ocean" => ocean(),
        "sky" => sky(),
        _ => dark(),
    }
}

pub fn next_theme_name(current: &str) -> &'static str {
    let idx = THEME_NAMES
        .iter()
        .position(|name| *name == current)
        .unwrap_or(0);
    THEME_NAMES[(idx + 1) % THEME_NAMES.len()]
}

/// Defers entirely to the terminal's own palette: ANSI slots 0–15 for colour
/// and `Color::Reset` for foreground and background, so whatever the user's
/// terminal theme defines is what essh renders. Nothing here is a fixed RGB
/// value — that is the whole point. A pywal, matugen or terminal-profile setup
/// gets an essh that matches the rest of the desktop.
///
/// Two consequences follow from that promise, both handled rather than fudged:
/// the gradient ramps degrade to a single ANSI slot, because a five-stop RGB
/// gradient has no 16-colour equivalent (see `Theme::ramp`); and `bg` is
/// `Reset`, so nothing paints over the terminal's own background.
///
/// `selection_bg` uses `Indexed(8)` — bright black, the only slot
/// conventionally rendered as a neutral mid-grey in both light and dark
/// terminal themes. A theme mapping slot 8 close to its background will show a
/// faint selection bar; that is a property of the user's theme, and a
/// saturated slot would be worse everywhere else.
pub fn terminal() -> Theme {
    Theme {
        name: "terminal",
        brand: Color::Cyan,
        active_tab: Color::Yellow,
        inactive_tab: Color::DarkGray,
        border: Color::DarkGray,
        separator: Color::DarkGray,
        // Reset = the terminal's configured foreground, exactly.
        text_primary: Color::Reset,
        text_secondary: Color::Gray,
        text_muted: Color::DarkGray,
        text_inverse: Color::Black,
        text_emphasis: Color::White,
        status_good: Color::Green,
        status_warn: Color::Yellow,
        status_error: Color::Red,
        status_info: Color::Cyan,
        rx_rate: Color::Green,
        tx_rate: Color::Blue,
        key_hint: Color::Yellow,
        selection_bg: Color::Indexed(8),
        highlight_bg: Color::Indexed(8),
        // The reason this theme exists: never paint over the terminal.
        bg: Color::Reset,
        muted_good: Color::Green,
        absent: Color::DarkGray,
    }
}

/// The ESSH 2.0 theme: the *Watch 2.0 family palette from the design
/// handoff. Every value comes from [`crate::design`] rather than being
/// respelled here, so the design system stays the single source.
pub fn dark() -> Theme {
    use crate::design as d;
    Theme {
        name: "dark",
        brand: d::CYAN,
        active_tab: d::WHITE,
        inactive_tab: d::DIM,
        // FAINT, not RULE. `RULE` (#1c282e) is the handoff's 1px hairline;
        // as a full box-drawing cell on #0c1418 it is invisible, which is why
        // the panels read as floating labels. This is the terminal-weight
        // equivalent, and matches netwatch's DarkGray.
        border: d::FAINT,
        separator: d::RULE,
        text_primary: d::FG,
        text_secondary: d::DIM,
        text_muted: d::DIM,
        text_inverse: d::BG,
        text_emphasis: d::WHITE,
        status_good: d::GREEN,
        status_warn: d::AMBER,
        status_error: d::RED,
        status_info: d::CYAN,
        rx_rate: d::CYAN,
        tx_rate: d::VIOLET,
        key_hint: d::CYAN,
        selection_bg: d::ROW_SELECTED_BG,
        highlight_bg: d::ROW_SELECTED_BG,
        bg: d::BG,
        muted_good: d::GREEN_MUTED,
        absent: d::NEVER_DOT,
    }
}

pub fn light() -> Theme {
    Theme {
        name: "light",
        brand: Color::Rgb(0, 120, 180),
        active_tab: Color::Rgb(180, 100, 0),
        inactive_tab: Color::Rgb(140, 140, 140),
        border: Color::Rgb(180, 180, 180),
        separator: Color::Rgb(180, 180, 180),
        text_primary: Color::Rgb(30, 30, 30),
        text_secondary: Color::Rgb(80, 80, 80),
        text_muted: Color::Rgb(140, 140, 140),
        text_inverse: Color::White,
        text_emphasis: Color::Rgb(0, 0, 0),
        status_good: Color::Rgb(0, 140, 60),
        status_warn: Color::Rgb(200, 140, 0),
        status_error: Color::Rgb(200, 40, 40),
        status_info: Color::Rgb(0, 120, 180),
        rx_rate: Color::Rgb(0, 140, 60),
        tx_rate: Color::Rgb(0, 90, 180),
        key_hint: Color::Rgb(180, 100, 0),
        selection_bg: Color::Rgb(220, 230, 240),
        highlight_bg: Color::Rgb(200, 215, 230),
        bg: Color::Rgb(250, 250, 248),
        muted_good: Color::Rgb(60, 160, 100),
        absent: Color::Rgb(190, 190, 190),
    }
}

pub fn solarized() -> Theme {
    let base03 = Color::Rgb(0, 43, 54);
    let base01 = Color::Rgb(88, 110, 117);
    let base0 = Color::Rgb(131, 148, 150);
    let base1 = Color::Rgb(147, 161, 161);
    let yellow = Color::Rgb(181, 137, 0);
    let orange = Color::Rgb(203, 75, 22);
    let red = Color::Rgb(220, 50, 47);
    let green = Color::Rgb(133, 153, 0);
    let cyan = Color::Rgb(42, 161, 152);
    let blue = Color::Rgb(38, 139, 210);
    let violet = Color::Rgb(108, 113, 196);

    Theme {
        name: "solarized",
        brand: cyan,
        active_tab: yellow,
        inactive_tab: base01,
        border: base01,
        separator: base01,
        text_primary: base0,
        text_secondary: base1,
        text_muted: base01,
        text_inverse: base03,
        text_emphasis: Color::Rgb(253, 246, 227),
        status_good: green,
        status_warn: yellow,
        status_error: red,
        status_info: cyan,
        rx_rate: green,
        tx_rate: blue,
        key_hint: orange,
        selection_bg: Color::Rgb(7, 54, 66),
        highlight_bg: violet,
        bg: base03,
        muted_good: Color::Rgb(101, 123, 0),
        absent: base01,
    }
}

pub fn dracula() -> Theme {
    let bg = Color::Rgb(40, 42, 54);
    let fg = Color::Rgb(248, 248, 242);
    let comment = Color::Rgb(98, 114, 164);
    let cyan = Color::Rgb(139, 233, 253);
    let green = Color::Rgb(80, 250, 123);
    let orange = Color::Rgb(255, 184, 108);
    let pink = Color::Rgb(255, 121, 198);
    let purple = Color::Rgb(189, 147, 249);
    let red = Color::Rgb(255, 85, 85);
    let yellow = Color::Rgb(241, 250, 140);

    Theme {
        name: "dracula",
        brand: purple,
        active_tab: pink,
        inactive_tab: comment,
        border: comment,
        separator: comment,
        text_primary: fg,
        text_secondary: Color::Rgb(200, 200, 210),
        text_muted: comment,
        text_inverse: bg,
        text_emphasis: Color::Rgb(255, 255, 255),
        status_good: green,
        status_warn: yellow,
        status_error: red,
        status_info: cyan,
        rx_rate: green,
        tx_rate: cyan,
        key_hint: orange,
        selection_bg: Color::Rgb(68, 71, 90),
        highlight_bg: Color::Rgb(98, 114, 164),
        bg,
        muted_good: Color::Rgb(56, 176, 87),
        absent: comment,
    }
}

pub fn nord() -> Theme {
    let polar0 = Color::Rgb(46, 52, 64);
    let snow0 = Color::Rgb(216, 222, 233);
    let snow1 = Color::Rgb(229, 233, 240);
    let frost0 = Color::Rgb(143, 188, 187);
    let frost1 = Color::Rgb(136, 192, 208);
    let frost2 = Color::Rgb(129, 161, 193);
    let frost3 = Color::Rgb(94, 129, 172);
    let aurora_red = Color::Rgb(191, 97, 106);
    let aurora_orange = Color::Rgb(208, 135, 112);
    let aurora_yellow = Color::Rgb(235, 203, 139);
    let aurora_green = Color::Rgb(163, 190, 140);

    Theme {
        name: "nord",
        brand: frost1,
        active_tab: frost0,
        inactive_tab: frost3,
        border: Color::Rgb(67, 76, 94),
        separator: Color::Rgb(67, 76, 94),
        text_primary: snow0,
        text_secondary: snow1,
        text_muted: Color::Rgb(76, 86, 106),
        text_inverse: polar0,
        text_emphasis: Color::Rgb(236, 239, 244),
        status_good: aurora_green,
        status_warn: aurora_yellow,
        status_error: aurora_red,
        status_info: frost1,
        rx_rate: aurora_green,
        tx_rate: frost2,
        key_hint: aurora_orange,
        selection_bg: Color::Rgb(59, 66, 82),
        highlight_bg: Color::Rgb(76, 86, 106),
        bg: polar0,
        muted_good: Color::Rgb(126, 148, 108),
        absent: Color::Rgb(76, 86, 106),
    }
}

/// Apple Terminal.app's default ANSI palette, over whatever background the
/// terminal already has. Ported from netwatch so the two tools name the same
/// colours the same way.
///
/// The pair with `sky`: identical colours, and the only difference is whether
/// the theme paints its own ground. This one does not, so it sits on a
/// terminal profile the user already likes.
pub fn ocean() -> Theme {
    Theme {
        bg: Color::Reset,
        ..sky_palette("ocean")
    }
}

/// `ocean`'s palette painting Apple Terminal.app's Ocean background, so the
/// look does not depend on the terminal profile being set to match.
pub fn sky() -> Theme {
    Theme {
        bg: Color::Rgb(0x22, 0x4F, 0xBC),
        ..sky_palette("sky")
    }
}

fn sky_palette(name: &'static str) -> Theme {
    let white = Color::Rgb(0xCB, 0xCC, 0xCD);
    let bright_white = Color::Rgb(0xFF, 0xFF, 0xFF);
    let bright_red = Color::Rgb(0xFC, 0x39, 0x1F);
    let bright_green = Color::Rgb(0x31, 0xE7, 0x22);
    let bright_yellow = Color::Rgb(0xEA, 0xEC, 0x23);
    let bright_cyan = Color::Rgb(0x14, 0xF0, 0xF0);
    // bright_black on the deep-blue ground fails WCAG AA; a lighter neutral
    // keeps borders, separators and muted text legible.
    let muted_readable = Color::Rgb(0xB5, 0xB6, 0xB7);

    Theme {
        name,
        brand: bright_cyan,
        active_tab: bright_white,
        inactive_tab: white,
        border: muted_readable,
        separator: muted_readable,
        text_primary: bright_white,
        text_secondary: white,
        text_muted: muted_readable,
        text_inverse: Color::Rgb(0, 0, 0),
        text_emphasis: bright_white,
        status_good: bright_green,
        status_warn: bright_yellow,
        status_error: bright_red,
        status_info: bright_cyan,
        rx_rate: bright_green,
        tx_rate: bright_cyan,
        key_hint: bright_yellow,
        selection_bg: Color::Rgb(0x21, 0x6D, 0xFF),
        highlight_bg: Color::Rgb(0x3A, 0x6B, 0xE8),
        bg: Color::Reset,
        muted_good: Color::Rgb(0x27, 0xB0, 0x1B),
        absent: muted_readable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The invariant that keeps theming real.
    ///
    /// essh had this whole struct, five palettes, config persistence and a
    /// cycle key — and picking a theme recoloured about two thirds of the
    /// screen, because 129 call sites painted from `design::` constants the
    /// theme could not reach. Under dracula, 34% of the launcher's painted
    /// cells were still *Watch teal.
    ///
    /// A struct field cannot enforce that. This can: colour constants are
    /// private to the design module and the palettes built from them, and
    /// every screen goes through a `Theme`.
    #[test]
    fn no_screen_paints_from_a_hardcoded_colour() {
        const CONSTS: &[&str] = &[
            "BG",
            "FG",
            "DIM",
            "FAINT",
            "RULE",
            "GREEN",
            "CYAN",
            "AMBER",
            "RED",
            "VIOLET",
            "WHITE",
            "NEVER_DOT",
            "ROW_SELECTED_BG",
            "GREEN_MUTED",
            "RAMP_DIV",
            "RAMP_OK",
            "RAMP_NET",
        ];
        const HELPERS: &[&str] = &[
            "divergence(",
            "divergence_count(",
            "magnitude(",
            "bounded_bad(",
            "ramp_at(",
        ];
        // The two files allowed to name a colour: where they are defined, and
        // where palettes are assembled from them.
        const ALLOWED: &[&str] = &["src/design/mod.rs", "src/theme.rs"];

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read src") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let rel = path
                    .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                if ALLOWED.iter().any(|a| rel == *a) {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("read");
                for (n, line) in text.lines().enumerate() {
                    if line.trim_start().starts_with("//") {
                        continue;
                    }
                    for c in CONSTS {
                        for prefix in ["design::", "d::"] {
                            let pat = format!("{prefix}{c}");
                            // Guard against DIM matching nothing longer, etc.
                            if let Some(i) = line.find(&pat) {
                                let after = line[i + pat.len()..].chars().next();
                                if !after.is_some_and(|ch| ch.is_alphanumeric() || ch == '_') {
                                    offenders.push(format!("{rel}:{}: {}", n + 1, line.trim()));
                                }
                            }
                        }
                    }
                    for h in HELPERS {
                        for prefix in ["design::", "d::"] {
                            if line.contains(&format!("{prefix}{h}")) {
                                offenders.push(format!("{rel}:{}: {}", n + 1, line.trim()));
                            }
                        }
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "colour taken from the design palette instead of the theme:\n  {}",
            offenders.join("\n  ")
        );
    }

    /// Channel split, for the ramp rules below.
    fn channels(c: Color) -> (u8, u8, u8) {
        match c {
            Color::Rgb(r, g, b) => (r, g, b),
            other => panic!("expected rgb, got {other:?}"),
        }
    }

    #[test]
    fn divergence_ramp_runs_faint_to_red() {
        let t = dark();
        // Consensus is quiet; being alone is red.
        assert_eq!(t.ramp_divergence(0.0), crate::design::RAMP_DIV[0]);
        assert_eq!(t.ramp_divergence(1.0), crate::design::RED);
        // "The further from consensus, the hotter" — so warmth climbs
        // monotonically. Total brightness does not, and should not be the
        // measure: amber is a brighter colour than red but reads cooler.
        let mut last_red = -1i32;
        for i in 0..=10 {
            let (r, _, _) = channels(t.ramp_divergence(i as f64 / 10.0));
            assert!(r as i32 >= last_red, "warmth dipped at {}", i);
            last_red = r as i32;
        }
        // And the cool end really is cool.
        let (r0, _, b0) = channels(t.ramp_divergence(0.0));
        assert!(b0 > r0, "consensus should not read warm");
    }

    #[test]
    fn magnitude_never_reddens() {
        let t = dark();
        // The rule this exists to enforce: a busy machine is a working
        // machine, so no magnitude ever renders as an alarm.
        for pct in [0.0, 25.0, 50.0, 75.0, 95.0, 100.0] {
            let c = t.magnitude(pct);
            assert_ne!(c, crate::design::RED, "magnitude at {}% went red", pct);
            assert_ne!(c, crate::design::AMBER, "magnitude at {}% went amber", pct);
        }
        // And it gets brighter, not darker, as the value climbs.
        let (_, g_low, _) = channels(t.magnitude(10.0));
        let (_, g_high, _) = channels(t.magnitude(90.0));
        assert!(g_high > g_low);
    }

    #[test]
    fn bounded_bad_is_reserved_for_values_that_really_are_bad() {
        let t = dark();
        assert_eq!(t.bounded_bad(42.0), crate::design::GREEN_MUTED);
        assert_eq!(t.bounded_bad(84.0), crate::design::AMBER);
        assert_eq!(t.bounded_bad(93.0), crate::design::RED);
    }

    #[test]
    fn a_diverge_count_of_zero_is_faint_not_green() {
        let t = dark();
        // Agreement should be quiet, not celebrated.
        assert_eq!(t.divergence_count(0), crate::design::FAINT);
        assert_eq!(t.divergence_count(3), crate::design::RED);
        assert_ne!(t.divergence_count(1), t.divergence_count(2));
    }

    /// The second hole, found the same way as the first — by rendering and
    /// looking rather than by reading.
    ///
    /// Colour can escape the theme two ways: a screen naming a constant, which
    /// `no_screen_paints_from_a_hardcoded_colour` catches, or a design helper
    /// baking one into a `Span` it hands back. `header_cell`, `chip`,
    /// `footer_line`, `headline` and `none_line` all did the second, from
    /// inside the one file the first check exempts — so every table header and
    /// footer in essh stayed *Watch-teal under every theme.
    ///
    /// Any design helper that returns styled output has to take a `Theme`.
    #[test]
    fn design_helpers_that_return_style_take_a_theme() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/design/mod.rs"),
        )
        .expect("read design");

        let mut offenders = Vec::new();
        for (n, line) in src.lines().enumerate() {
            let line = line.trim();
            if !line.starts_with("pub fn ") {
                continue;
            }
            // Signature may wrap; join until the return arrow or the brace.
            let mut sig = line.to_string();
            if !sig.contains("->") && !sig.ends_with('{') {
                for cont in src.lines().skip(n + 1).take(8) {
                    sig.push_str(cont.trim());
                    if cont.contains('{') {
                        break;
                    }
                }
            }
            let styled = ["-> Span", "-> Line", "-> Style", "-> Vec<Line", "-> Color"];
            // A helper that TAKES a colour is a pass-through — the caller
            // chose it, and the caller is themed. Only helpers that source a
            // colour themselves need the theme.
            let takes_colour = sig.contains(": Color") || sig.contains("&[Color]");
            if styled.iter().any(|r| sig.contains(r)) && !sig.contains("Theme") && !takes_colour {
                offenders.push(format!("src/design/mod.rs:{}: {}", n + 1, line));
            }
        }
        assert!(
            offenders.is_empty(),
            "design helper returns styled output without a Theme, so no palette \
             can reach it:\n  {}",
            offenders.join("\n  ")
        );
    }

    #[test]
    fn all_builtin_themes_load() {
        for name in THEME_NAMES {
            let theme = by_name(name);
            assert_eq!(theme.name, *name);
        }
    }

    #[test]
    fn unknown_theme_falls_back_to_dark() {
        let theme = by_name("nonexistent");
        assert_eq!(theme.name, "dark");
    }

    #[test]
    fn next_theme_wraps() {
        assert_eq!(next_theme_name("dark"), "terminal");
        assert_eq!(next_theme_name(THEME_NAMES[THEME_NAMES.len() - 1]), "dark");
        // Cycling has to reach every theme and come home, or one becomes
        // unreachable from the keyboard.
        let mut seen = vec!["dark"];
        let mut cur = "dark";
        for _ in 0..THEME_NAMES.len() {
            cur = next_theme_name(cur);
            seen.push(cur);
        }
        assert_eq!(cur, "dark", "cycle did not return to its start");
        for name in THEME_NAMES {
            assert!(seen.contains(name), "{name} is unreachable by cycling");
        }
    }

    #[test]
    fn aliases_resolve_rather_than_falling_back_to_dark() {
        // A saved config from an older release, or a name a user reaches for
        // out of habit, must not silently land on dark.
        for alias in ["system", "ansi"] {
            assert_eq!(by_name(alias).name, "terminal", "{alias}");
        }
        assert_eq!(by_name("paper").name, "light");
    }

    #[test]
    fn the_terminal_theme_pins_no_rgb() {
        // Its entire promise. Any fixed RGB here ignores the palette the user
        // themed their desktop with.
        let t = terminal();
        let slots = [
            t.brand,
            t.active_tab,
            t.inactive_tab,
            t.border,
            t.separator,
            t.text_primary,
            t.text_secondary,
            t.text_muted,
            t.text_inverse,
            t.text_emphasis,
            t.status_good,
            t.status_warn,
            t.status_error,
            t.status_info,
            t.rx_rate,
            t.tx_rate,
            t.key_hint,
            t.selection_bg,
            t.highlight_bg,
            t.bg,
            t.muted_good,
            t.absent,
        ];
        for c in slots {
            assert!(!matches!(c, Color::Rgb(..)), "{c:?} is fixed RGB");
        }
        // Including the gradients, which are the easy thing to forget.
        for f in [0.0, 0.5, 1.0] {
            for c in [t.ramp_divergence(f), t.ramp_net(f), t.magnitude(f * 100.0)] {
                assert!(!matches!(c, Color::Rgb(..)), "a ramp leaked {c:?}");
            }
        }
        // And a themed palette still uses real colour.
        assert!(matches!(dark().ramp_net(0.5), Color::Rgb(..)));
    }

    #[test]
    fn every_theme_that_paints_a_ground_has_readable_text_on_it() {
        // The bug this pins: `light` had near-black text and no background, so
        // on a dark terminal it painted black on black.
        fn lum(c: Color) -> f64 {
            let Color::Rgb(r, g, b) = c else {
                return f64::NAN;
            };
            let f = |v: u8| {
                let s = v as f64 / 255.0;
                if s <= 0.03928 {
                    s / 12.92
                } else {
                    ((s + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
        }
        for name in THEME_NAMES {
            let t = by_name(name);
            let (x, y) = (lum(t.bg), lum(t.text_primary));
            if x.is_nan() || y.is_nan() {
                continue; // defers to the terminal; not ours to judge
            }
            let (hi, lo) = if x > y { (x, y) } else { (y, x) };
            let ratio = (hi + 0.05) / (lo + 0.05);
            assert!(
                ratio >= 4.5,
                "{name}: text on its own ground is {ratio:.1}:1"
            );
        }
    }
}

#[cfg(test)]
mod rendered {
    use ratatui::{backend::TestBackend, style::Color, Terminal};

    /// The check that would have caught both escapes, and the only one that
    /// tests what a user sees.
    ///
    /// Static analysis found the constants a screen named. It could not find
    /// colour baked into a `Span` by a helper in the one file the analysis
    /// exempts — that took rendering a frame and looking at the pixels. So
    /// this renders one and asserts the obvious thing: under a theme that is
    /// not `dark`, none of dark's colours may reach the screen.
    #[test]
    fn a_theme_recolours_the_whole_screen() {
        use crate::design as d;
        let dark_only = [
            ("BG", d::BG),
            ("FG", d::FG),
            ("DIM", d::DIM),
            ("FAINT", d::FAINT),
            ("RULE", d::RULE),
            ("GREEN", d::GREEN),
            ("CYAN", d::CYAN),
            ("AMBER", d::AMBER),
            ("RED", d::RED),
            ("VIOLET", d::VIOLET),
            ("NEVER_DOT", d::NEVER_DOT),
            ("GREEN_MUTED", d::GREEN_MUTED),
        ];

        for name in super::THEME_NAMES {
            if *name == "dark" {
                continue;
            }
            let mut app = crate::tui::App::new(4);
            app.theme = super::by_name(name);
            let mut term = Terminal::new(TestBackend::new(132, 36)).unwrap();
            term.draw(|f| crate::tui::render(f, &mut app)).unwrap();
            let buf = term.backend().buffer();

            let mut leaked: Vec<&str> = Vec::new();
            for cell in buf.content() {
                if cell.symbol().trim().is_empty() {
                    continue;
                }
                for (cname, c) in dark_only {
                    if cell.fg == c && !leaked.contains(&cname) {
                        leaked.push(cname);
                    }
                }
            }
            assert!(
                leaked.is_empty(),
                "{name} still paints with the dark palette: {leaked:?}"
            );
        }
    }

    /// `terminal`'s promise, checked at the pixel rather than at the struct.
    #[test]
    fn the_terminal_theme_puts_no_rgb_on_screen() {
        let mut app = crate::tui::App::new(4);
        app.theme = super::terminal();
        let mut term = Terminal::new(TestBackend::new(132, 36)).unwrap();
        term.draw(|f| crate::tui::render(f, &mut app)).unwrap();
        let buf = term.backend().buffer();
        for cell in buf.content() {
            assert!(
                !matches!(cell.fg, Color::Rgb(..)) && !matches!(cell.bg, Color::Rgb(..)),
                "terminal theme emitted {:?}/{:?}",
                cell.fg,
                cell.bg
            );
        }
    }
}
