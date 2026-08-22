//! The divergence overlay: how does this host differ from its peers?
//!
//! Presentation follows three rules from the design:
//!
//! * **Agreement collapses.** Facets where everyone matches become one dim
//!   line, not one row each. Boring is the correct answer and should be quiet.
//! * **Numbers carry their peer context.** `disk 84%` is not a finding;
//!   `84% · median 62% · you are p100` is.
//! * **Unprobed is not agreement.** Peers we have no facts for are named
//!   separately, and never counted as consensus.

use ratatui::{
    prelude::*,
    widgets::{Block, Clear, Paragraph, Wrap},
};

use crate::design as d;
use crate::divergence::{verdict_for, HostDivergence};
use crate::theme::Theme;

/// Render the overlay as a floating card on a scrim.
///
/// The same treatment as the command palette, for the same reason the design
/// gives there: *"a floating surface on a scrim. No box-drawing needed when
/// you can composite."* A terminal cannot blur, but dimming the backdrop and
/// lifting the card onto its own surface carries the effect.
pub fn render(
    f: &mut Frame,
    area: Rect,
    divergence: Option<&HostDivergence>,
    collectable: (usize, usize, usize),
    platform: &str,
    theme: &Theme,
) {
    // ── the scrim
    f.render_widget(
        Block::default().style(Style::default().bg(d::BG).fg(d::FAINT)),
        area,
    );

    let lines = match divergence {
        Some(dv) => body(dv, collectable, platform, theme),
        None => vec![
            Line::from(Span::styled(
                "No peer set for this host.",
                Style::default().fg(d::WHITE).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Divergence compares a host against others sharing a tag. Tag two \
                 or more hosts alike — role=web, env=prod — and they become peers.",
                Style::default().fg(d::DIM),
            )),
        ],
    };

    // Height accounts for the card's padding *and* the hint row, so content
    // is never laid out underneath it.
    let width = 86u16.min(area.width.saturating_sub(6));
    let wrapped = estimate_wrapped(&lines, width.saturating_sub(4));
    let height = (wrapped + 5).min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) * 46 / 100;
    let card = Rect::new(x, y, width, height);

    f.render_widget(Clear, card);
    f.render_widget(
        Block::default().style(Style::default().bg(d::RULE).fg(d::FG)),
        card,
    );

    let inner = Rect {
        x: card.x + 2,
        y: card.y + 1,
        width: card.width.saturating_sub(4),
        height: card.height.saturating_sub(2),
    };
    // Content stops one row short of the hint.
    let body_area = Rect {
        height: inner.height.saturating_sub(1),
        ..inner
    };
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body_area);

    if inner.height > 1 {
        let hint = Rect {
            x: inner.x,
            y: inner.y + inner.height - 1,
            width: inner.width,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(d::footer_line(&[("⎋", "close"), ("D", "toggle")])),
            hint,
        );
    }
}

fn body<'a>(
    d: &'a HostDivergence,
    collectable: (usize, usize, usize),
    platform: &'a str,
    // The divergence card is a raised modal, not a panel: it keeps its own
    // fill and rule rather than the theme's, so nothing here reads the theme.
    _theme: &Theme,
) -> Vec<Line<'a>> {
    let mut lines = Vec::new();

    lines.push(Line::from(vec![
        Span::styled(
            d.host.clone(),
            Style::default().fg(d::WHITE).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  vs {}", d.peer_set), Style::default().fg(d::DIM)),
    ]));

    // Never claim a facet count we did not attempt. On macOS several of the
    // seventeen simply do not exist, and saying "17 facets" would be a lie
    // told by omission.
    let (usable, total, privileged) = collectable;
    let mut coverage = format!("{} of {} facets collectable on {}", usable, total, platform);
    if privileged > 0 {
        // Collectable is not the same as readable: a config-hash command runs
        // fine and still returns "permission denied" as an unprivileged user.
        coverage.push_str(&format!(
            " · {} need privileges and may report permission denied",
            privileged
        ));
    }
    lines.push(Line::from(Span::styled(
        coverage,
        Style::default().fg(d::DIM),
    )));
    lines.push(Line::from(""));

    if !d.is_probed() {
        lines.push(Line::from(Span::styled(
            "Never probed — no facts to compare.",
            Style::default().fg(d::FAINT).add_modifier(Modifier::ITALIC),
        )));
        return lines;
    }

    // Verdict first, with its evidence, so the claim is checkable.
    if let Some(v) = verdict_for(d) {
        lines.push(Line::from(Span::styled(
            "VERDICT",
            Style::default().fg(d::CYAN).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(v.text, Style::default().fg(d::FG))));
        lines.push(Line::from(Span::styled(
            format!(
                "  evidence: {}",
                v.evidence
                    .iter()
                    .map(|k| k.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Style::default().fg(d::FAINT).add_modifier(Modifier::ITALIC),
        )));
        lines.push(Line::from(""));
    }

    let diverging = d.diverging();
    if diverging.is_empty() {
        lines.push(Line::from(Span::styled(
            "At consensus on every collected facet.",
            Style::default().fg(d::GREEN),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "DIVERGING",
            Style::default().fg(d::CYAN).add_modifier(Modifier::BOLD),
        )));
        for c in &diverging {
            // Colour by how alone this host is, not by magnitude: red means
            // "you are the only one", never "the number is large".
            // Red means "you are alone", straight off the divergence ramp —
            // never a magnitude judgement.
            let colour = d::divergence(c.severity);
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<20} ", c.key.label()),
                    Style::default().fg(d::DIM),
                ),
                Span::styled(c.summary(), Style::default().fg(colour)),
            ]));
        }
        lines.push(Line::from(""));
    }

    // Agreement collapses to a single line.
    if !d.identical.is_empty() {
        let names: Vec<String> = d.identical.iter().map(|k| k.label()).collect();
        lines.push(Line::from(Span::styled(
            format!("{} facets identical across peers", d.identical.len()),
            Style::default().fg(d::DIM),
        )));
        lines.push(Line::from(Span::styled(
            format!("  {}", names.join(" · ")),
            Style::default().fg(d::FAINT).add_modifier(Modifier::ITALIC),
        )));
    }

    // Unprobed peers are named, never folded into the consensus.
    if !d.unprobed_peers.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "{} peers never probed — excluded from every count above: {}",
                d.unprobed_peers.len(),
                d.unprobed_peers.join(", ")
            ),
            Style::default().fg(d::FAINT).add_modifier(Modifier::ITALIC),
        )));
    }

    lines
}

/// How many rows `lines` will occupy once wrapped to `width`.
///
/// The card sizes itself to its content, so it has to know what wrapping will
/// do — otherwise a long verdict silently loses its last lines.
fn estimate_wrapped(lines: &[Line], width: u16) -> u16 {
    if width == 0 {
        return lines.len() as u16;
    }
    lines
        .iter()
        .map(|l| {
            let w: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
            (w.max(1).div_ceil(width as usize)) as u16
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_is_accounted_for_so_content_is_not_clipped() {
        let long = "x".repeat(200);
        let lines = vec![
            Line::from("short"),
            Line::from(long.as_str()),
            Line::from("short"),
        ];
        // 200 chars at width 80 is three rows, plus the two short lines.
        assert_eq!(estimate_wrapped(&lines, 80), 1 + 3 + 1);
    }

    #[test]
    fn an_empty_line_still_occupies_a_row() {
        assert_eq!(estimate_wrapped(&[Line::from("")], 40), 1);
    }
}
