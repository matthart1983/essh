use crate::design as d;
use crate::session::{Session, SessionState};
use crate::theme::Theme;
use crate::tui::widgets;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState},
};

/// Render the dashboard view (no active session focused)
#[allow(clippy::too_many_arguments)]
pub fn render(
    f: &mut Frame,
    area: Rect,
    sessions: &[Session],
    hosts: &[super::HostDisplay],
    filtered_indices: &[usize],
    selected_host: usize,
    selected_session: usize,
    table_state: &mut TableState,
    active_tab: super::DashboardTab,
    status_message: Option<&str>,
    search_active: bool,
    search_query: &str,
    groups: &[crate::divergence::GroupSummary],
    fleet_consensus: Option<(String, crate::divergence::Consensus)>,
    facet_agreement: &[(String, f64)],
    verdicts: &[(String, crate::divergence::Verdict)],
    peer_note: Option<String>,
    theme: &Theme,
) {
    // The footer is a bordered block, so its height must account for every
    // line it will hold. Without the status row, `set_status` output was
    // laid out past the bottom edge and silently never rendered — which is
    // every error and result message in the app, not just one.
    // The footer is plain text now — the design gives it a rule above, not a
    // box around. Height is exactly the lines it will hold, so a status
    // message is never laid out past the bottom edge.
    let mut footer_height = 1;
    if search_active {
        footer_height += 1;
    }
    if status_message.is_some() {
        footer_height += 1;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),             // titlebar + tabs + underline
            Constraint::Min(8),                // main content
            Constraint::Length(footer_height), // footer (+ search bar)
        ])
        .split(area);

    d::paint_bg(f, area);
    render_header(f, chunks[0], active_tab, peer_note, theme);

    match active_tab {
        super::DashboardTab::Sessions => {
            render_sessions_tab(f, chunks[1], sessions, selected_session, theme)
        }
        super::DashboardTab::Hosts => {
            // The dead space under a short host list becomes the GROUPS panel:
            // which peer sets exist, whether they agree, and who breaks them.
            let group_height = if groups.is_empty() {
                0
            } else {
                (groups.len() as u16 + 3).min(chunks[1].height / 2)
            };
            // Sized to content. A box stretched over empty space reads as
            // missing data rather than as an absence of problems.
            let host_height = ((filtered_indices.len() as u16) + 3)
                .max(5)
                .min(chunks[1].height.saturating_sub(group_height));
            let host_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(host_height),
                    Constraint::Length(group_height),
                    Constraint::Min(0),
                ])
                .split(chunks[1]);

            render_hosts_tab(
                f,
                host_chunks[0],
                hosts,
                filtered_indices,
                selected_host,
                table_state,
                theme,
            );
            if group_height > 0 {
                render_groups_panel(f, host_chunks[1], groups, theme);
            }
        }
        super::DashboardTab::Fleet => render_fleet_tab(
            f,
            chunks[1],
            hosts,
            sessions,
            fleet_consensus,
            facet_agreement,
            verdicts,
            theme,
        ),
        super::DashboardTab::Config => render_config_tab(f, chunks[1], theme),
    }

    render_footer(
        f,
        chunks[2],
        active_tab,
        status_message,
        search_active,
        search_query,
        theme,
    );
}

/// The titlebar and tab strip.
///
/// Per the handoff: the title is centred, the right carries the peer
/// indicator (`● 10 hosts · 3 diverged`), and the active tab is marked by a
/// cyan underline rather than by a colour change alone.
fn render_header(
    f: &mut Frame,
    area: Rect,
    active_tab: super::DashboardTab,
    peer_note: Option<String>,
    _theme: &Theme,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(2)])
        .split(area);

    // ── titlebar
    let title = "ESSH";
    let right = peer_note.unwrap_or_default();
    let left_pad = (rows[0].width as usize / 2).saturating_sub(title.len() / 2);
    let mut bar = vec![
        Span::raw(" ".repeat(left_pad)),
        Span::styled(title, Style::default().fg(d::DIM)),
    ];
    if !right.is_empty() {
        let used = left_pad + title.len();
        let pad = (rows[0].width as usize)
            .saturating_sub(used + right.chars().count() + 3)
            .max(1);
        bar.push(Span::raw(" ".repeat(pad)));
        bar.push(d::dot(d::AMBER));
        bar.push(Span::raw(" "));
        bar.push(Span::styled(right, Style::default().fg(d::FAINT)));
    }
    f.render_widget(Paragraph::new(Line::from(bar)), rows[0]);

    // ── tab strip
    let tabs = [
        ("1", "Sessions", super::DashboardTab::Sessions),
        ("2", "Hosts", super::DashboardTab::Hosts),
        ("3", "Fleet", super::DashboardTab::Fleet),
        ("4", "Config", super::DashboardTab::Config),
    ];
    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    let mut underline: Vec<Span> = vec![Span::raw(" ")];
    for (n, label, tab) in &tabs {
        let on = *tab == active_tab;
        let text = format!("{} {}", n, label);
        spans.push(Span::raw(" "));
        spans.push(Span::styled(n.to_string(), Style::default().fg(d::FAINT)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            label.to_string(),
            if on {
                Style::default().fg(d::WHITE).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(d::DIM)
            },
        ));
        spans.push(Span::raw(" "));
        underline.push(Span::styled(
            if on {
                "─".repeat(text.chars().count() + 2)
            } else {
                " ".repeat(text.chars().count() + 2)
            },
            Style::default().fg(d::CYAN),
        ));
    }
    let now = chrono::Local::now().format("%H:%M:%S").to_string();
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let pad = (rows[1].width as usize)
        .saturating_sub(used + now.len() + 2)
        .max(1);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(now, Style::default().fg(d::FAINT)));

    f.render_widget(
        Paragraph::new(vec![Line::from(spans), Line::from(underline)]),
        rows[1],
    );
}

fn render_sessions_tab(
    f: &mut Frame,
    area: Rect,
    sessions: &[Session],
    selected: usize,
    theme: &Theme,
) {
    if sessions.is_empty() {
        let msg = Paragraph::new(vec![
            Line::raw(""),
            Line::styled(
                "  No active sessions.",
                Style::default().fg(theme.text_muted),
            ),
            Line::raw(""),
            Line::styled(
                "  Press [2] to browse hosts, or use 'essh connect <host>' to start a session.",
                Style::default().fg(theme.text_muted),
            ),
        ])
        .block(
            Block::bordered()
                .title("Active Sessions")
                .border_style(Style::default().fg(theme.border)),
        );
        f.render_widget(msg, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from(" # ").style(Style::default().fg(theme.brand).bold()),
        Cell::from("Label").style(Style::default().fg(theme.brand).bold()),
        Cell::from("Host").style(Style::default().fg(theme.brand).bold()),
        Cell::from("User").style(Style::default().fg(theme.brand).bold()),
        Cell::from("Status").style(Style::default().fg(theme.brand).bold()),
        Cell::from("Uptime").style(Style::default().fg(theme.brand).bold()),
    ])
    .height(1);

    let rows: Vec<Row> = sessions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let state_style = match &s.state {
                SessionState::Active => Style::default().fg(theme.status_good),
                SessionState::Suspended => Style::default().fg(theme.text_muted),
                SessionState::Reconnecting { .. } => Style::default().fg(theme.status_error),
                SessionState::Connecting => Style::default().fg(theme.status_warn),
                SessionState::Disconnected { .. } => Style::default().fg(theme.status_error),
            };
            let status_text = match &s.state {
                SessionState::Active => "● Active",
                SessionState::Suspended => "● Suspended",
                SessionState::Reconnecting { .. } => "● Recon.",
                SessionState::Connecting => "● Connecting",
                SessionState::Disconnected { .. } => "● Disconnected",
            };

            Row::new(vec![
                Cell::from(format!(" {} ", i + 1)),
                Cell::from(s.label.clone()),
                Cell::from(s.hostname.clone()),
                Cell::from(s.username.clone()),
                Cell::from(status_text).style(state_style),
                Cell::from(widgets::format_duration_short(s.uptime_secs() as i64)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Percentage(20),
        Constraint::Percentage(25),
        Constraint::Percentage(15),
        Constraint::Percentage(15),
        Constraint::Percentage(15),
    ];

    let inner = d::pane(
        f,
        area,
        theme,
        &d::PanelOpts {
            key: Some("1"),
            title: Some("sessions"),
            sub: Some(&format!("{}", sessions.len())),
            foot_left: &[("↑↓", " select"), ("↵", " attach"), ("w", " close")],
            ..Default::default()
        },
    );

    let table = Table::new(rows, widths)
        .header(header)
        // Background and weight only. Setting a foreground here would repaint
        // the whole row one colour, so the selected session would lose the
        // status colour that says whether it is alive — the row you are
        // looking at becomes the one row you cannot read the state of.
        .row_highlight_style(Style::default().bg(theme.selection_bg).bold())
        .highlight_symbol("▌ ");

    // Stateful, or the highlight is configured and never applied — which is
    // why this list had no visible cursor at all.
    let mut state = ratatui::widgets::TableState::default();
    state.select(Some(selected.min(sessions.len().saturating_sub(1))));
    f.render_stateful_widget(table, inner, &mut state);
}

/// The Fleet screen's reasoning block.
///
/// A count is not an insight: "3 hosts diverge" says something is wrong
/// without saying what. This names the host, the facets, and — when the
/// outliers are concentrated in a few hosts — says so, because "two hosts
/// hold every outlier" is a different operational problem from "everything
/// drifted a little".
fn render_verdict_box(
    f: &mut Frame,
    area: Rect,
    verdicts: &[(String, crate::divergence::Verdict)],
    total_hosts: usize,
    theme: &Theme,
) {
    let inner = d::pane(
        f,
        area,
        theme,
        &d::PanelOpts {
            title: Some("verdict"),
            right: Some("what differs, and likely why"),
            ..Default::default()
        },
    );

    let mut lines: Vec<Line> = Vec::new();

    // Only claim concentration when it is true and worth saying.
    if verdicts.len() >= 2 && total_hosts > verdicts.len() {
        lines.push(Line::from(Span::styled(
            format!(
                "{} of {} hosts hold every outlier.",
                verdicts.len(),
                total_hosts
            ),
            Style::default().fg(d::FG).add_modifier(Modifier::BOLD),
        )));
    }

    for (host, v) in verdicts.iter().take(inner.height as usize) {
        let mut spans = vec![
            Span::styled(
                format!("{host}  "),
                Style::default().fg(d::CYAN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(v.text.clone(), Style::default().fg(d::FG)),
        ];
        if !v.evidence.is_empty() {
            // The evidence keeps the sentence checkable.
            let facets: Vec<String> = v.evidence.iter().map(|e| e.label().to_string()).collect();
            spans.push(Span::styled(
                format!("  [{}]", facets.join(", ")),
                Style::default().fg(d::FAINT),
            ));
        }
        lines.push(Line::from(spans));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

/// Screen 1 — Hosts.
fn render_hosts_tab(
    f: &mut Frame,
    area: Rect,
    hosts: &[super::HostDisplay],
    filtered_indices: &[usize],
    selected: usize,
    _table_state: &mut TableState,
    theme: &Theme,
) {
    let reachable = hosts
        .iter()
        .filter(|h| h.status == super::HostStatus::Online)
        .count();
    let right = format!("{} · {} reachable", hosts.len(), reachable);
    let inner = d::pane(
        f,
        area,
        theme,
        &d::PanelOpts {
            key: Some("1"),
            title: Some("hosts"),
            sub: Some(&right),
            foot_left: &[("↑↓", " select"), ("↵", " open"), ("?", " help")],
            ..Default::default()
        },
    );

    let filtered: Vec<&super::HostDisplay> = filtered_indices
        .iter()
        .filter_map(|&i| hosts.get(i))
        .collect();

    // DIVERGE is the column the eye should find, so its header is cyan.
    let header = Row::new(vec![
        Cell::from(""),
        Cell::from(d::header_cell("name", false)),
        Cell::from(d::header_cell("hostname", false)),
        Cell::from(Line::from(d::header_cell("port", false)).right_aligned()),
        Cell::from(d::header_cell("user", false)),
        Cell::from(Line::from(d::header_cell("diverge ↓", true)).right_aligned()),
        Cell::from(d::header_cell("tags", false)),
        Cell::from(Line::from(d::header_cell("seen", false)).right_aligned()),
    ]);

    let tag_width = (inner.width as usize * 22 / 100).max(10);
    let rows: Vec<Row> = filtered
        .iter()
        .map(|h| {
            let is_selected = hosts
                .get(selected)
                .is_some_and(|s| s.hostname == h.hostname && s.port == h.port);

            let dot_color = match h.status {
                super::HostStatus::Online => d::GREEN,
                super::HostStatus::Offline => d::RED,
                // Not a grey dot implying a reading — a colour that reads as
                // "no signal".
                super::HostStatus::NeverProbed => d::NEVER_DOT,
            };

            let diverge = match h.diverge_count {
                None => Span::styled(
                    "never probed",
                    Style::default().fg(d::FAINT).add_modifier(Modifier::ITALIC),
                ),
                Some(0) => Span::styled("—", Style::default().fg(d::FAINT)),
                Some(n) => Span::styled(n.to_string(), Style::default().fg(d::divergence_count(n))),
            };

            // Tags as chips, whole or not at all, with a +N for the rest.
            let (chips, hidden) = crate::format::tag_chips(&h.tag_pairs, tag_width);
            let mut tag_spans: Vec<Span> = Vec::new();
            for c in &chips {
                let implicated = h.diverge_count.is_some_and(|n| n > 0)
                    && (c.starts_with("role=") || c.starts_with("env="));
                tag_spans.push(d::chip(c, implicated));
            }
            if hidden > 0 {
                tag_spans.push(Span::styled(
                    format!("+{}", hidden),
                    Style::default().fg(d::FAINT),
                ));
            }

            let seen = if h.last_seen.is_empty() {
                "never".to_string()
            } else {
                crate::format::relative_time(&h.last_seen)
            };

            let marker = if is_selected {
                Span::styled("▌", Style::default().fg(d::CYAN))
            } else {
                Span::raw(" ")
            };
            let row = Row::new(vec![
                Cell::from(Line::from(vec![marker, Span::raw(" "), d::dot(dot_color)])),
                Cell::from(h.name.clone()).style(Style::default().fg(d::WHITE)),
                Cell::from(h.hostname.clone()).style(Style::default().fg(d::DIM)),
                Cell::from(Line::from(h.port.to_string()).right_aligned())
                    .style(Style::default().fg(d::DIM)),
                Cell::from(h.user.clone()).style(Style::default().fg(d::DIM)),
                Cell::from(Line::from(diverge).right_aligned()),
                Cell::from(Line::from(tag_spans)),
                Cell::from(Line::from(seen).right_aligned()).style(Style::default().fg(d::FAINT)),
            ]);
            if is_selected {
                row.style(Style::default().bg(d::ROW_SELECTED_BG))
            } else {
                row
            }
        })
        .collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Length(18),
        Constraint::Length(20),
        Constraint::Length(6),
        Constraint::Length(10),
        Constraint::Length(13),
        Constraint::Min(18),
        Constraint::Length(9),
    ];

    f.render_widget(
        Table::new(rows, widths).header(header).column_spacing(2),
        inner,
    );
}

/// The GROUPS panel — the answer to "are these machines the same?".
///
/// This is what fills the ~900px of dead space under a short host list.
fn render_groups_panel(
    f: &mut Frame,
    area: Rect,
    groups: &[crate::divergence::GroupSummary],
    theme: &Theme,
) {
    let inner = d::pane(
        f,
        area,
        theme,
        &d::PanelOpts {
            key: Some("2"),
            title: Some("groups"),
            right: Some("by role · tag consensus"),
            ..Default::default()
        },
    );

    let header = Row::new(vec![
        Cell::from(""),
        Cell::from(d::header_cell("group", false)),
        Cell::from(Line::from(d::header_cell("hosts", false)).right_aligned()),
        Cell::from(Line::from(d::header_cell("at consensus", false)).right_aligned()),
        Cell::from(d::header_cell("note", false)),
    ]);

    let rows: Vec<Row> = groups
        .iter()
        .take(inner.height.saturating_sub(1) as usize)
        .map(|g| {
            let probed = g.probed();
            let severity = if probed == 0 {
                0.0
            } else {
                1.0 - (g.at_consensus as f64 / probed as f64)
            };
            let col = if probed == 0 {
                d::FAINT
            } else if severity > 0.0 {
                d::divergence(severity)
            } else {
                d::GREEN
            };
            let consensus = if probed == 0 {
                "—".to_string()
            } else {
                format!("{} / {}", g.at_consensus, probed)
            };
            Row::new(vec![
                Cell::from(Line::from(d::dot(col))),
                Cell::from(g.label.clone()).style(Style::default().fg(d::WHITE)),
                Cell::from(Line::from(g.host_count.to_string()).right_aligned())
                    .style(Style::default().fg(d::DIM)),
                Cell::from(Line::from(consensus).right_aligned()).style(Style::default().fg(col)),
                Cell::from(g.note()).style(if probed == 0 {
                    Style::default().fg(d::FAINT).add_modifier(Modifier::ITALIC)
                } else {
                    Style::default().fg(col)
                }),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(2),
        Constraint::Length(22),
        Constraint::Length(7),
        Constraint::Length(14),
        Constraint::Min(20),
    ];
    f.render_widget(Table::new(rows, widths).header(header), inner);
}

/// Screen 2 — Fleet. Facet consensus is the headline; reachability is one
/// quiet line beneath it.
#[allow(clippy::too_many_arguments)]
fn render_fleet_tab(
    f: &mut Frame,
    area: Rect,
    hosts: &[super::HostDisplay],
    sessions: &[Session],
    fleet_consensus: Option<(String, crate::divergence::Consensus)>,
    facet_agreement: &[(String, f64)],
    verdicts: &[(String, crate::divergence::Verdict)],
    theme: &Theme,
) {
    let _ = (sessions, theme);
    // Boxes are sized to their content. Stretching a box with one row in it
    // to fill the window produces a large empty rectangle, which reads as
    // missing data rather than as an absence of problems.
    let listed = hosts
        .iter()
        .filter(|h| h.diverge_count != Some(0))
        .count()
        .max(1);
    let table_height = ((listed + 4) as u16).min(area.height.saturating_sub(7));

    // The consensus box grows to fit its facet list. Fixed at six rows it
    // showed four facets and dropped the rest without saying so, which is the
    // one thing a consensus panel must never do: quietly omit the checks.
    // Headline (4) + border (2) + however many grid rows the facets need.
    let per_row = ((area.width as usize).saturating_sub(2) / 34).max(1);
    let facet_rows = facet_agreement.len().div_ceil(per_row) as u16;
    let consensus_height = (6u16 + facet_rows)
        .max(6)
        .min(area.height.saturating_sub(6));

    // The verdict block: one line of reasoning per host that has one, plus a
    // lead when the outliers are concentrated.
    let verdict_height = if verdicts.is_empty() {
        0
    } else {
        (verdicts.len() as u16 + 3).min(area.height / 3)
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(consensus_height),
            Constraint::Length(verdict_height),
            Constraint::Length(table_height),
            Constraint::Min(0),
        ])
        .split(area);

    render_consensus_box(f, rows[0], hosts, &fleet_consensus, facet_agreement, theme);
    if verdict_height > 0 {
        render_verdict_box(f, rows[1], verdicts, hosts.len(), theme);
    }
    render_fleet_table(f, rows[2], hosts, theme);
}

/// The CONSENSUS box: a 26px numeral in the design, the biggest thing a
/// terminal can offer here, plus what it actually means.
fn render_consensus_box(
    f: &mut Frame,
    area: Rect,
    hosts: &[super::HostDisplay],
    consensus: &Option<(String, crate::divergence::Consensus)>,
    facet_agreement: &[(String, f64)],
    theme: &Theme,
) {
    let right = consensus
        .as_ref()
        .map(|(label, c)| format!("{} facet-checks · {}", c.total_checks, label))
        .unwrap_or_default();
    let inner = d::pane(
        f,
        area,
        theme,
        &d::PanelOpts {
            key: Some("3"),
            title: Some("consensus"),
            right: Some(&right),
            ..Default::default()
        },
    );

    // Headline across the top, facets in a grid beneath it.
    //
    // Side-by-side wasted most of the box: the headline is four lines, so a
    // sixteen-row box left twelve rows of empty space on the left while the
    // facet list was squeezed into one narrow column with its labels
    // truncated.
    let rows_split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(0)])
        .split(inner);
    let cols = [rows_split[0], rows_split[1]];

    let mut lines: Vec<Line> = Vec::new();
    match consensus {
        Some((_, c)) if c.total_checks > 0 => {
            let pct = c.percent();
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:.1}%", pct),
                    Style::default().fg(d::WHITE).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("of {} facet-checks agree", c.total_checks),
                    Style::default().fg(d::DIM),
                ),
            ]));
            let plural = |n: usize, w: &str| {
                if n == 1 {
                    format!("{} {}", n, w)
                } else {
                    format!("{} {}s", n, w)
                }
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "{} diverge across {}.",
                    plural(c.diverging_facets, "facet"),
                    plural(c.diverging_hosts, "host")
                ),
                Style::default().fg(d::DIM),
            )));
            if c.unprobed_hosts > 0 {
                // The distinction v1's `Offline: 8` destroyed.
                lines.push(d::none_line(&format!(
                    "{} hosts have never been probed, so their facets are unknown rather than in agreement.",
                    c.unprobed_hosts
                )));
            }
        }
        _ => {
            lines.push(d::none_line(
                "no facets collected yet — connect to a host to compare it against its peers",
            ));
        }
    }

    // Reachability, demoted to one dim line. v1 gave this a pink 20% bar
    // that "read as an alarm about nothing".
    let online = hosts
        .iter()
        .filter(|h| h.status == super::HostStatus::Online)
        .count();
    let never = hosts
        .iter()
        .filter(|h| h.status == super::HostStatus::NeverProbed)
        .count();
    let mut reach = vec![
        Span::styled("reachable ", Style::default().fg(d::FAINT)),
        Span::styled(online.to_string(), Style::default().fg(d::DIM)),
        Span::styled(" of ", Style::default().fg(d::FAINT)),
        Span::styled(hosts.len().to_string(), Style::default().fg(d::DIM)),
    ];
    if never > 0 {
        reach.push(Span::styled(
            format!(" · {} never probed", never),
            Style::default().fg(d::FAINT),
        ));
    }
    lines.push(Line::from(reach));
    f.render_widget(Paragraph::new(lines), cols[0]);

    // Per-facet agreement meters, so the box says *where* the fleet is
    // ragged rather than only how ragged it is.
    if !facet_agreement.is_empty() && cols[1].height > 0 {
        // The facet name is the information; the bar is the decoration. It
        // used to be the other way round — a 14-cell bar next to a name
        // truncated to 14 characters, so full bars read as one green slab
        // and the labels said "/etc/nginx/ng…".
        let meter_w = 8usize;
        let cell_w = 34usize;
        let per_row = ((cols[1].width as usize) / cell_w).max(1);
        let label_w = cell_w.saturating_sub(meter_w + 8).max(10);
        let capacity = per_row * cols[1].height as usize;
        let shown = capacity.min(facet_agreement.len());

        let cells: Vec<Vec<Span>> = facet_agreement
            .iter()
            .take(shown)
            .map(|(label, frac)| {
                // Full agreement is the boring case and should recede; a
                // column of bright bars at 100% reads as a solid block and
                // buries the one facet that actually diverges.
                let col = if *frac >= 1.0 {
                    d::GREEN_MUTED
                } else {
                    d::divergence(1.0 - frac)
                };
                let (fill, track) = d::meter(*frac, meter_w);
                vec![
                    Span::styled(
                        format!(
                            "{:<w$}",
                            crate::format::truncate_right(label, label_w),
                            w = label_w
                        ),
                        // A diverging facet's name is the thing to read.
                        Style::default().fg(if *frac >= 1.0 { d::FAINT } else { d::FG }),
                    ),
                    Span::styled(fill, Style::default().fg(col)),
                    Span::styled(track, Style::default().fg(d::FAINT)),
                    Span::styled(format!("{:>4.0}% ", frac * 100.0), Style::default().fg(col)),
                    Span::raw("  "),
                ]
            })
            .collect();

        let mut lines: Vec<Line> = cells
            .chunks(per_row)
            .map(|chunk| Line::from(chunk.iter().flatten().cloned().collect::<Vec<_>>()))
            .collect();
        // Never drop checks silently: an omitted facet is an unexamined one.
        if shown < facet_agreement.len() {
            lines.push(Line::from(Span::styled(
                format!("+{} more facets", facet_agreement.len() - shown),
                Style::default().fg(d::FAINT),
            )));
        }
        f.render_widget(Paragraph::new(lines), cols[1]);
    }
}

/// HOSTS BY DIVERGENCE, sorted worst first.
fn render_fleet_table(f: &mut Frame, area: Rect, hosts: &[super::HostDisplay], theme: &Theme) {
    let inner = d::pane(
        f,
        area,
        theme,
        &d::PanelOpts {
            key: Some("4"),
            title: Some("hosts"),
            sub: Some("by divergence"),
            right: Some("sort ↓ diverge"),
            ..Default::default()
        },
    );

    let mut ordered: Vec<&super::HostDisplay> = hosts.iter().collect();
    ordered.sort_by(|a, b| match (a.diverge_count, b.diverge_count) {
        (Some(x), Some(y)) => y.cmp(&x),
        // Unprobed hosts sink to the bottom: they are not "in agreement".
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.name.cmp(&b.name),
    });

    let header = Row::new(vec![
        Cell::from(""),
        Cell::from(d::header_cell("host", false)),
        Cell::from(Line::from(d::header_cell("diverge ↓", true)).right_aligned()),
        Cell::from(Line::from(d::header_cell("rtt", false)).right_aligned()),
        Cell::from(d::header_cell("60s", false)),
    ]);

    let at_consensus = ordered
        .iter()
        .filter(|h| h.diverge_count == Some(0))
        .count();

    let body: Vec<Row> = ordered
        .iter()
        .filter(|h| h.diverge_count != Some(0))
        .take(inner.height.saturating_sub(2) as usize)
        .map(|h| match h.diverge_count {
            // Never probed: one honest line, not a row of dashes.
            None => Row::new(vec![
                Cell::from(Line::from(d::dot(d::NEVER_DOT))),
                Cell::from(h.name.clone()).style(Style::default().fg(d::DIM)),
                Cell::from(d::none_line("never probed — no facts to compare")),
                Cell::from(""),
                Cell::from(""),
            ]),
            Some(n) => {
                let col = d::divergence_count(n);
                let spark = if h.latency_history.is_empty() {
                    Span::raw("")
                } else {
                    let ceiling = d::axis_ceiling(&h.latency_history);
                    Span::styled(
                        d::sparkline(&h.latency_history, 12, ceiling),
                        Style::default().fg(d::GREEN_MUTED),
                    )
                };
                Row::new(vec![
                    Cell::from(Line::from(d::dot(col))),
                    Cell::from(h.name.clone()).style(Style::default().fg(d::WHITE)),
                    Cell::from(
                        Line::from(Span::styled(n.to_string(), Style::default().fg(col)))
                            .right_aligned(),
                    ),
                    Cell::from(
                        Line::from(match h.latency_ms {
                            Some(ms) => format!("{:.0}ms", ms),
                            None => "—".into(),
                        })
                        .right_aligned(),
                    )
                    .style(Style::default().fg(d::DIM)),
                    Cell::from(Line::from(spark)),
                ])
            }
        })
        .collect();

    let widths = [
        Constraint::Length(2),
        Constraint::Percentage(28),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Min(14),
    ];
    f.render_widget(
        Table::new(body, widths).header(header).column_spacing(2),
        inner,
    );

    // Agreement collapses to one quiet line — boring is the correct answer.
    if at_consensus > 0 && inner.height > 2 {
        let row = Rect {
            x: inner.x,
            y: inner.y + inner.height - 1,
            width: inner.width,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    at_consensus.to_string(),
                    Style::default().fg(d::GREEN).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" hosts at full consensus", Style::default().fg(d::DIM)),
            ])),
            row,
        );
    }
}

fn render_config_tab(f: &mut Frame, area: Rect, theme: &Theme) {
    let content = Paragraph::new(vec![
        Line::raw(""),
        Line::styled("  Configuration", Style::default().fg(theme.brand).bold()),
        Line::raw(""),
        Line::styled(
            "  Config file: ~/.essh/config.toml",
            Style::default().fg(theme.text_muted),
        ),
        Line::styled(
            "  Cache DB:    ~/.essh/cache.db",
            Style::default().fg(theme.text_muted),
        ),
        Line::styled(
            "  Audit log:   ~/.essh/audit.log",
            Style::default().fg(theme.text_muted),
        ),
        Line::styled(
            format!("  Theme:       {}", theme.name),
            Style::default().fg(theme.text_muted),
        ),
        Line::raw(""),
        Line::styled(
            "  Press 'e' to edit config, or 't' to cycle themes.",
            Style::default().fg(theme.text_muted),
        ),
        Line::styled(
            "  Changes reload from ~/.essh/config.toml without restarting.",
            Style::default().fg(theme.text_muted),
        ),
    ])
    .block(
        Block::bordered()
            .title("Config")
            .border_style(Style::default().fg(theme.border)),
    );
    f.render_widget(content, area);
}

fn render_footer(
    f: &mut Frame,
    area: Rect,
    tab: super::DashboardTab,
    status: Option<&str>,
    search_active: bool,
    search_query: &str,
    _theme: &Theme,
) {
    let mut lines: Vec<Line> = Vec::new();

    if search_active {
        lines.push(Line::from(vec![
            Span::styled(" / ", Style::default().fg(d::CYAN)),
            Span::styled(
                search_query.to_string(),
                Style::default().fg(d::FG).add_modifier(Modifier::BOLD),
            ),
            Span::styled("▌", Style::default().fg(d::CYAN)),
        ]));
    }

    // The design's footer vocabulary: cyan key, faint label.
    let pairs: &[(&str, &str)] = match tab {
        super::DashboardTab::Fleet => &[
            ("⏎", "connect"),
            ("D", "divergence"),
            ("r", "probe all"),
            ("^P", "palette"),
            ("q", "quit"),
        ],
        super::DashboardTab::Hosts => &[
            ("⏎", "connect"),
            ("D", "divergence"),
            ("/", "search"),
            ("a", "add"),
            ("r", "probe"),
            ("e", "edit"),
            ("^P", "palette"),
            ("q", "quit"),
        ],
        _ => &[("⏎", "connect"), ("^P", "palette"), ("q", "quit")],
    };
    lines.push(d::footer_line(pairs));

    if let Some(msg) = status {
        lines.push(Line::from(Span::styled(
            format!(" {}", msg),
            Style::default().fg(d::AMBER),
        )));
    }

    f.render_widget(Paragraph::new(lines), area);
}

pub fn render_add_host_dialog(
    f: &mut Frame,
    editing: bool,
    input: &str,
    error: Option<&str>,
    theme: &Theme,
) {
    let area = f.area();
    let popup_width = 66u16.min(area.width.saturating_sub(4));
    let popup_height = 8u16.min(area.height.saturating_sub(2));
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup = Rect::new(x, y, popup_width, popup_height);

    f.render_widget(Clear, popup);

    let surface_style = Style::default()
        .fg(theme.text_primary)
        .bg(theme.selection_bg);
    let title = if editing { " Edit Host " } else { " Add Host " };
    let hint = if editing {
        "  Update user@host[:port] or host[:port] for the selected host."
    } else {
        "  Enter user@host[:port] or host[:port]."
    };

    let block = Block::default()
        .title(title)
        .title_style(
            Style::default()
                .fg(theme.brand)
                .bg(theme.selection_bg)
                .bold(),
        )
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.brand).bg(theme.selection_bg))
        .style(surface_style);
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let mut lines = vec![
        Line::styled(
            hint,
            Style::default()
                .fg(theme.text_secondary)
                .bg(theme.selection_bg),
        ),
        Line::styled("", surface_style),
        Line::from(vec![
            Span::styled(
                "  > ",
                Style::default()
                    .fg(theme.brand)
                    .bg(theme.selection_bg)
                    .bold(),
            ),
            Span::styled(
                input,
                Style::default()
                    .fg(theme.text_primary)
                    .bg(theme.selection_bg),
            ),
            Span::styled("█", Style::default().fg(theme.brand).bg(theme.selection_bg)),
        ]),
        Line::styled("", surface_style),
    ];

    if let Some(error) = error {
        lines.push(Line::styled(
            format!("  {}", error),
            Style::default()
                .fg(theme.status_error)
                .bg(theme.selection_bg),
        ));
    } else {
        lines.push(Line::styled(
            "  Enter: save  Esc: cancel",
            Style::default()
                .fg(theme.text_secondary)
                .bg(theme.selection_bg),
        ));
    }

    let paragraph = Paragraph::new(lines).style(surface_style).block(
        Block::default()
            .style(surface_style)
            .border_style(Style::default().fg(theme.border).bg(theme.selection_bg)),
    );
    f.render_widget(paragraph, inner);
}

#[cfg(test)]
mod fleet_tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn facets(n: usize) -> Vec<(String, f64)> {
        (0..n)
            .map(|i| (format!("facet-{i:02}"), if i == 3 { 0.5 } else { 1.0 }))
            .collect()
    }

    fn screen(width: u16, height: u16, n: usize) -> String {
        let theme = crate::theme::dark();
        let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
        let agreement = facets(n);
        term.draw(|f| {
            render_fleet_tab(f, f.area(), &[], &[], None, &agreement, &[], &theme);
        })
        .unwrap();
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every facet must be accounted for — shown, or counted as hidden.
    ///
    /// The box was a fixed six rows, so it displayed four of sixteen checks
    /// and dropped the other twelve without a word. A consensus panel that
    /// quietly omits checks is worse than no panel: it reports agreement it
    /// never looked at.
    #[test]
    fn no_facet_is_dropped_without_saying_so() {
        for (w, h) in [(160u16, 40u16), (100, 30), (80, 24)] {
            let s = screen(w, h, 16);
            let shown = (0..16)
                .filter(|i| s.contains(&format!("facet-{i:02}")))
                .count();
            assert!(
                shown == 16 || s.contains("more facets"),
                "{w}x{h}: showed {shown}/16 facets and never said the rest were hidden:\n{s}"
            );
        }
    }

    /// A wide window should fit all sixteen without a "more" note at all.
    #[test]
    fn a_wide_window_shows_every_facet() {
        let s = screen(160, 40, 16);
        for i in 0..16 {
            assert!(
                s.contains(&format!("facet-{i:02}")),
                "facet-{i:02} missing:\n{s}"
            );
        }
        assert!(!s.contains("more facets"), "nothing should be hidden:\n{s}");
    }

    /// The Fleet screen must carry the verdict, not just a count.
    ///
    /// The handoff leads this screen with reasoning — which host, which
    /// facet, whether it is one cause or two. A percentage and a table say
    /// something is wrong without saying what, which is the thing the
    /// redesign existed to fix.
    #[test]
    fn the_fleet_screen_shows_the_verdict() {
        use crate::divergence::{FacetKey, Verdict};
        let theme = crate::theme::dark();
        let verdicts = vec![
            (
                "10.0.1.10".to_string(),
                Verdict {
                    text: "is a kernel behind and carries a hand-edited nginx.conf".into(),
                    evidence: vec![FacetKey::Kernel],
                    pattern: "test",
                },
            ),
            (
                "10.0.5.30".to_string(),
                Verdict {
                    text: "diverges only on disk".into(),
                    evidence: vec![],
                    pattern: "test",
                },
            ),
        ];
        let agreement = facets(4);
        let mut term = Terminal::new(TestBackend::new(160, 40)).unwrap();
        term.draw(|f| {
            render_fleet_tab(f, f.area(), &[], &[], None, &agreement, &verdicts, &theme);
        })
        .unwrap();
        let buf = term.backend().buffer();
        let screen: String = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(screen.contains("verdict"), "no verdict panel:\n{screen}");
        assert!(
            screen.contains("10.0.1.10"),
            "the host is not named:\n{screen}"
        );
        assert!(
            screen.contains("kernel behind"),
            "the reasoning is missing:\n{screen}"
        );
        // The evidence keeps the claim checkable.
        assert!(screen.contains("[kernel]"), "no evidence shown:\n{screen}");
    }

    /// With nothing diverging there is nothing to reason about, and the panel
    /// must not appear as an empty box.
    #[test]
    fn a_fleet_at_consensus_shows_no_verdict_box() {
        let theme = crate::theme::dark();
        let agreement = facets(4);
        let mut term = Terminal::new(TestBackend::new(160, 40)).unwrap();
        term.draw(|f| {
            render_fleet_tab(f, f.area(), &[], &[], None, &agreement, &[], &theme);
        })
        .unwrap();
        let buf = term.backend().buffer();
        let screen: String = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !screen.contains("verdict"),
            "empty verdict box drawn:\n{screen}"
        );
    }

    /// The grid must not overrun its box into the panel below.
    #[test]
    fn the_facet_grid_stays_inside_its_box() {
        let s = screen(160, 40, 16);
        let hosts_row = s
            .lines()
            .position(|l| l.contains("hosts"))
            .expect("the hosts panel should render");
        let last_facet = s
            .lines()
            .position(|l| l.contains("facet-15"))
            .expect("the last facet should render");
        assert!(
            last_facet < hosts_row,
            "the facet grid ran past its box into the hosts panel"
        );
    }
}
