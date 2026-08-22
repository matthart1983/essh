//! The launcher screen: the first thing `essh` shows.
//!
//! Minimal by design — the spec's §10 says the terminal is the primary UI and
//! the fast path is *launch → search → connect*. So this is a query line, a
//! ranked list, and nothing else. No tab strip, no borders around borders.

use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::launcher::{Field, Match};
use crate::theme::Theme;

pub struct LauncherState {
    pub query: String,
    pub selected: usize,
    pub results: Vec<Match>,
    /// Set when there are no candidates at all, as opposed to no matches.
    pub empty_reason: Option<String>,
}

impl Default for LauncherState {
    fn default() -> Self {
        Self::new()
    }
}

impl LauncherState {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            selected: 0,
            results: Vec::new(),
            empty_reason: None,
        }
    }

    /// Keep the selection inside the result list after a query change.
    pub fn clamp(&mut self) {
        if self.results.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.results.len() {
            self.selected = self.results.len() - 1;
        }
    }

    pub fn next(&mut self) {
        if !self.results.is_empty() {
            self.selected = (self.selected + 1) % self.results.len();
        }
    }

    pub fn prev(&mut self) {
        if !self.results.is_empty() {
            self.selected = if self.selected == 0 {
                self.results.len() - 1
            } else {
                self.selected - 1
            };
        }
    }

    pub fn selected_match(&self) -> Option<&Match> {
        self.results.get(self.selected)
    }
}

pub fn render(f: &mut Frame, state: &LauncherState, status: &str, theme: &Theme) {
    let area = f.area();
    f.render_widget(Clear, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // query
            Constraint::Min(3),    // results
            Constraint::Length(1), // hint
        ])
        .split(area);

    // ── Query line
    let prompt = Line::from(vec![
        Span::styled("  ❯ ", Style::default().fg(theme.brand).bold()),
        Span::styled(
            state.query.clone(),
            Style::default().fg(theme.text_primary).bold(),
        ),
        Span::styled("▌", Style::default().fg(theme.brand)),
    ]);
    f.render_widget(
        Paragraph::new(prompt).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(theme.border)),
        ),
        chunks[0],
    );

    // ── Results
    if state.results.is_empty() {
        let msg = match &state.empty_reason {
            // No hosts anywhere is a different problem from no matches, and
            // the fix is different too.
            Some(reason) => vec![
                Line::from(Span::styled(
                    "  No hosts to connect to.",
                    Style::default().fg(theme.text_primary).bold(),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", reason),
                    Style::default().fg(theme.text_muted),
                )),
            ],
            None => vec![Line::from(Span::styled(
                format!("  nothing matches {:?}", state.query),
                Style::default().fg(theme.text_muted).italic(),
            ))],
        };
        f.render_widget(Paragraph::new(msg), chunks[1]);
    } else {
        let rows = chunks[1].height as usize;
        // Keep the selection visible without the list jumping around.
        let offset = state.selected.saturating_sub(rows.saturating_sub(1));

        let lines: Vec<Line> = state
            .results
            .iter()
            .enumerate()
            .skip(offset)
            .take(rows)
            .map(|(i, m)| render_row(i == state.selected, m, chunks[1].width, theme))
            .collect();
        f.render_widget(Paragraph::new(lines), chunks[1]);
    }

    // ── Hint
    let count = if state.results.is_empty() {
        String::new()
    } else {
        format!("{} of {}   ", state.selected + 1, state.results.len())
    };
    // A connection in flight takes the row: the connect blocks the loop for
    // as long as the handshake takes, and "nothing changed" is exactly what
    // a hang looks like. Saying which host is being dialled is the whole
    // difference between waiting and wondering.
    if !status.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("  {}", status),
                Style::default().fg(theme.brand).bold(),
            ))),
            chunks[2],
        );
        return;
    }

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("  {}", count),
                Style::default().fg(theme.text_muted),
            ),
            Span::styled("↵", Style::default().fg(theme.key_hint)),
            Span::styled(" connect   ", Style::default().fg(theme.text_muted)),
            Span::styled("↑↓", Style::default().fg(theme.key_hint)),
            Span::styled(" select   ", Style::default().fg(theme.text_muted)),
            Span::styled("Esc", Style::default().fg(theme.key_hint)),
            Span::styled(" dashboard", Style::default().fg(theme.text_muted)),
        ])),
        chunks[2],
    );
}

fn render_row<'a>(selected: bool, m: &'a Match, width: u16, theme: &Theme) -> Line<'a> {
    let mut spans: Vec<Span> = Vec::new();

    spans.push(Span::styled(
        if selected { "  ❯ " } else { "    " },
        Style::default().fg(theme.brand).bold(),
    ));

    // Alias, with the matched characters highlighted.
    let base = if selected {
        Style::default().fg(theme.text_primary).bold()
    } else {
        Style::default().fg(theme.text_primary)
    };
    let hit = Style::default().fg(theme.brand).bold();

    if m.highlights.is_empty() {
        spans.push(Span::styled(m.candidate.alias.clone(), base));
    } else {
        for (i, ch) in m.candidate.alias.chars().enumerate() {
            let style = if m.highlights.contains(&i) { hit } else { base };
            spans.push(Span::styled(ch.to_string(), style));
        }
    }

    // Pad to a column so the right-hand detail lines up.
    let used = 4 + m.candidate.alias.chars().count();
    let pad = 24usize.saturating_sub(used);
    spans.push(Span::raw(" ".repeat(pad.max(1))));

    let target = match (&m.candidate.user, m.candidate.port) {
        (Some(u), 22) => format!("{}@{}", u, m.candidate.hostname),
        (Some(u), p) => format!("{}@{}:{}", u, m.candidate.hostname, p),
        (None, 22) => m.candidate.hostname.clone(),
        (None, p) => format!("{}:{}", m.candidate.hostname, p),
    };
    spans.push(Span::styled(target, Style::default().fg(theme.text_muted)));

    // Say why a row is here when the alias was not what matched — otherwise
    // a result with no visible relationship to the query looks like a bug.
    if m.matched_field != Field::Alias {
        spans.push(Span::styled(
            format!("  ({} match)", m.matched_field.label()),
            Style::default().fg(theme.text_muted).italic(),
        ));
    }

    // A host reached through ProxyCommand or ControlMaster connects
    // differently. Better to know that here than to find out mid-incident.
    if m.candidate.delegated.is_some() {
        spans.push(Span::styled(
            "  via system ssh",
            Style::default().fg(theme.status_warn).italic(),
        ));
    }

    let _ = width;
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::{search, Candidate};

    fn cands() -> Vec<Candidate> {
        vec![
            Candidate {
                alias: "prod-db".into(),
                hostname: "10.0.0.5".into(),
                port: 22,
                ..Default::default()
            },
            Candidate {
                alias: "prod-api".into(),
                hostname: "10.0.1.10".into(),
                port: 22,
                ..Default::default()
            },
        ]
    }

    #[test]
    fn selection_stays_in_range_when_the_query_narrows() {
        let mut s = LauncherState::new();
        s.results = search(&cands(), "");
        s.selected = 1;
        // Narrow to a single result.
        s.results = search(&cands(), "prod-db");
        s.clamp();
        assert_eq!(s.selected, 0);
        assert!(s.selected_match().is_some());
    }

    #[test]
    fn selection_wraps_in_both_directions() {
        let mut s = LauncherState::new();
        s.results = search(&cands(), "");
        assert_eq!(s.selected, 0);
        s.prev();
        assert_eq!(s.selected, s.results.len() - 1, "up from the top wraps");
        s.next();
        assert_eq!(s.selected, 0, "down from the bottom wraps");
    }

    #[test]
    fn navigation_on_an_empty_list_does_not_panic() {
        let mut s = LauncherState::new();
        s.next();
        s.prev();
        s.clamp();
        assert!(s.selected_match().is_none());
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// The status line must render exactly the string it was given.
    ///
    /// A dropped character here is not cosmetic: the line names the host and
    /// user that failed, and "mattbo @192.168.0.54" sends someone looking for
    /// a host that does not exist.
    #[test]
    fn the_status_line_renders_every_character() {
        let theme = crate::theme::dark();
        let status = "mattbot@192.168.0.54 refused every credential";
        let mut term = Terminal::new(TestBackend::new(120, 10)).unwrap();
        let state = LauncherState::default();
        term.draw(|f| render(f, &state, status, &theme)).unwrap();

        let buf = term.backend().buffer();
        let joined: String = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            joined.contains(status),
            "the status was mangled in rendering.\nwanted: {status}\ngot:\n{joined}"
        );
    }
}
