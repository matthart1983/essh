//! The HUD: the only instrumentation allowed over a shell.
//!
//! The handoff's central rule is that **the shell gets nothing** — no tab
//! strip, no status row, no keybind footer, no borders. v1 spent four rows on
//! those before a single line of output. So signal over the terminal has to
//! be either transient or on-demand, never resident:
//!
//! > the HUD is **transient**: it appears on a change, states *why* in words,
//! > and fades. It alpha-blends *over* the shell, so nothing reflows.
//!
//! A terminal cannot alpha-blend, but it can overlay: the HUD is drawn on top
//! of the terminal's last rows rather than being given rows of its own, so the
//! remote's window size never changes when it appears. That is the property
//! that actually matters — a resident row would make the shell reflow every
//! time the HUD came and went.

use std::time::{Duration, Instant};

use ratatui::{
    prelude::*,
    widgets::{Block, Paragraph},
};

use crate::design as d;

/// How long a HUD stays up before fading. The handoff says ~4s.
const LIFETIME: Duration = Duration::from_secs(4);

/// Why the HUD appeared. It always states a reason — a HUD that just shows
/// numbers is a status bar that forgot to be permanent.
#[derive(Clone, Debug, PartialEq)]
pub enum Reason {
    /// A facet moved away from the peer set.
    Diverged(String),
    /// The link changed state.
    Link(String),
    /// A plain notice, e.g. a workspace restore summary.
    Notice(String),
}

impl Reason {
    fn glyph(&self) -> (&'static str, Color) {
        match self {
            Reason::Diverged(_) => ("▲", d::AMBER),
            Reason::Link(_) => ("◆", d::CYAN),
            Reason::Notice(_) => ("·", d::DIM),
        }
    }

    fn text(&self) -> &str {
        match self {
            Reason::Diverged(t) | Reason::Link(t) | Reason::Notice(t) => t,
        }
    }
}

/// The numbers carried on the right of the HUD, when they are known.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Vitals {
    pub rtt_ms: Option<f64>,
    pub down_bps: Option<f64>,
    pub loss_pct: Option<f64>,
}

pub struct Hud {
    reason: Reason,
    vitals: Vitals,
    shown_at: Instant,
}

#[derive(Default)]
pub struct HudState {
    current: Option<Hud>,
    /// The last thing shown, so an unchanged state does not re-trigger.
    last_reason: Option<Reason>,
}

impl HudState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Raise the HUD for a state *change*. Calling this repeatedly with the
    /// same reason does nothing — the HUD appears on a change, not on a tick,
    /// which is the whole difference between it and a status bar.
    pub fn on_change(&mut self, reason: Reason, vitals: Vitals) {
        if self.last_reason.as_ref() == Some(&reason) {
            // Refresh the numbers on an already-visible HUD, but do not
            // restart its lifetime.
            if let Some(h) = self.current.as_mut() {
                h.vitals = vitals;
            }
            return;
        }
        self.last_reason = Some(reason.clone());
        self.current = Some(Hud {
            reason,
            vitals,
            shown_at: Instant::now(),
        });
    }

    /// Drop the HUD once its lifetime is up.
    pub fn tick(&mut self) {
        if let Some(h) = &self.current {
            if h.shown_at.elapsed() >= LIFETIME {
                self.current = None;
            }
        }
    }

    /// Dismiss immediately, for `Esc`.
    // The HUD's manual controls; it currently expires on a timer.
    #[allow(dead_code)]
    pub fn dismiss(&mut self) {
        self.current = None;
    }

    #[allow(dead_code)]
    pub fn is_visible(&self) -> bool {
        self.current.is_some()
    }

    /// Forget the last reason, so the next state change raises the HUD again
    /// even if it repeats. Used when switching sessions.
    pub fn reset(&mut self) {
        self.current = None;
        self.last_reason = None;
    }
}

/// Draw the HUD over the bottom of `area`, without reserving any of it.
pub fn render(f: &mut Frame, area: Rect, state: &HudState) {
    let Some(hud) = &state.current else {
        return;
    };
    if area.height < 2 {
        return;
    }

    let row = Rect {
        x: area.x,
        y: area.y + area.height - 1,
        width: area.width,
        height: 1,
    };

    // Composite by painting the strip, since a terminal has no alpha. The
    // point of "alpha-blends over the shell" is that the shell does not
    // reflow — which holds, because these rows were never taken from it.
    f.render_widget(Block::default().style(Style::default().bg(d::RULE)), row);

    let (glyph, glyph_color) = hud.reason.glyph();
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(glyph, Style::default().fg(glyph_color)),
        Span::raw(" "),
        Span::styled(hud.reason.text().to_string(), Style::default().fg(d::FG)),
    ];

    // The right-hand vitals, each omitted when unmeasured. v1 printed
    // `RTT:—` beside a confident `●Excellent`; a HUD with nothing to say
    // should say nothing.
    let mut right: Vec<Span> = Vec::new();
    if let Some(rtt) = hud.vitals.rtt_ms {
        right.push(Span::styled("rtt ", Style::default().fg(d::DIM)));
        right.push(Span::styled(
            format!("{:.1}ms", rtt),
            Style::default().fg(d::FG),
        ));
        right.push(Span::raw("   "));
    }
    if let Some(bps) = hud.vitals.down_bps {
        right.push(Span::styled("↓ ", Style::default().fg(d::DIM)));
        right.push(Span::styled(
            crate::tui::widgets::format_bytes_rate(bps),
            Style::default().fg(d::FG),
        ));
        right.push(Span::raw("   "));
    }
    if let Some(loss) = hud.vitals.loss_pct {
        right.push(Span::styled("loss ", Style::default().fg(d::DIM)));
        right.push(Span::styled(
            format!("{:.1}%", loss),
            Style::default().fg(if loss > 0.0 { d::AMBER } else { d::GREEN }),
        ));
        right.push(Span::raw("   "));
    }
    right.push(Span::styled("⌘D detail", Style::default().fg(d::CYAN)));
    right.push(Span::raw(" "));

    let left_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let right_width: usize = right.iter().map(|s| s.content.chars().count()).sum();
    let gap = (area.width as usize)
        .saturating_sub(left_width + right_width)
        .max(1);
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right);

    f.render_widget(Paragraph::new(Line::from(spans)), row);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vitals() -> Vitals {
        Vitals {
            rtt_ms: Some(2.1),
            down_bps: Some(111.0 * 1024.0),
            loss_pct: Some(0.0),
        }
    }

    #[test]
    fn the_hud_appears_on_a_change_not_on_a_tick() {
        let mut s = HudState::new();
        assert!(!s.is_visible());

        s.on_change(
            Reason::Diverged("kernel is 1 release behind".into()),
            vitals(),
        );
        assert!(s.is_visible());

        // The same reason again must not restart it — that would make it a
        // status bar that flickers rather than a transient notice.
        let first = s.current.as_ref().unwrap().shown_at;
        s.on_change(
            Reason::Diverged("kernel is 1 release behind".into()),
            vitals(),
        );
        assert_eq!(s.current.as_ref().unwrap().shown_at, first);
    }

    #[test]
    fn a_genuinely_new_reason_raises_it_again() {
        let mut s = HudState::new();
        s.on_change(Reason::Diverged("kernel behind".into()), vitals());
        let first = s.current.as_ref().unwrap().shown_at;
        s.on_change(Reason::Link("reconnected".into()), vitals());
        assert_ne!(s.current.as_ref().unwrap().shown_at, first);
    }

    #[test]
    fn it_fades_rather_than_staying_up() {
        let mut s = HudState::new();
        s.on_change(
            Reason::Notice("restored production".into()),
            Vitals::default(),
        );
        assert!(s.is_visible());
        // Backdate it past its lifetime.
        s.current.as_mut().unwrap().shown_at = Instant::now() - LIFETIME - Duration::from_millis(1);
        s.tick();
        assert!(!s.is_visible(), "the HUD must be transient");
    }

    #[test]
    fn escape_dismisses_it_immediately() {
        let mut s = HudState::new();
        s.on_change(Reason::Notice("x".into()), Vitals::default());
        s.dismiss();
        assert!(!s.is_visible());
    }

    #[test]
    fn switching_sessions_lets_the_same_reason_raise_it_again() {
        let mut s = HudState::new();
        s.on_change(Reason::Diverged("kernel behind".into()), vitals());
        s.dismiss();
        // Without a reset, the identical reason would be suppressed.
        s.on_change(Reason::Diverged("kernel behind".into()), vitals());
        assert!(!s.is_visible());

        s.reset();
        s.on_change(Reason::Diverged("kernel behind".into()), vitals());
        assert!(s.is_visible());
    }

    #[test]
    fn every_reason_carries_words_not_just_numbers() {
        // "states why it appeared" — a HUD of bare figures is a status bar.
        for r in [
            Reason::Diverged("kernel is 1 release behind your 39 peers".into()),
            Reason::Link("reconnected after 2 attempts".into()),
            Reason::Notice("restored production".into()),
        ] {
            assert!(!r.text().is_empty());
            assert!(r.text().chars().any(|c| c.is_alphabetic()));
        }
    }
}
