use crate::session::{Session, SessionState};
use crate::theme::Theme;
use crate::tui::widgets;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
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
    table_state: &mut TableState,
    active_tab: super::DashboardTab,
    status_message: Option<&str>,
    search_active: bool,
    search_query: &str,
    theme: &Theme,
) {
    let footer_height = if search_active { 4 } else { 3 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),             // header + tab bar
            Constraint::Min(8),                // main content
            Constraint::Length(footer_height), // footer (+ search bar)
        ])
        .split(area);

    render_header(f, chunks[0], active_tab, theme);

    match active_tab {
        super::DashboardTab::Sessions => render_sessions_tab(f, chunks[1], sessions, theme),
        super::DashboardTab::Hosts => render_hosts_tab(
            f,
            chunks[1],
            hosts,
            filtered_indices,
            selected_host,
            table_state,
            theme,
        ),
        super::DashboardTab::Fleet => render_fleet_tab(f, chunks[1], hosts, sessions, theme),
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

fn render_header(f: &mut Frame, area: Rect, active_tab: super::DashboardTab, theme: &Theme) {
    let now = chrono::Local::now().format("%H:%M:%S").to_string();

    let tabs = [
        ("[1] Sessions", super::DashboardTab::Sessions),
        ("[2] Hosts", super::DashboardTab::Hosts),
        ("[3] Fleet", super::DashboardTab::Fleet),
        ("[4] Config", super::DashboardTab::Config),
    ];

    let mut spans: Vec<Span> = vec![
        Span::styled(" ESSH ", Style::default().fg(theme.brand).bold()),
        Span::styled("│ ", Style::default().fg(theme.separator)),
    ];

    for (label, tab) in &tabs {
        if *tab == active_tab {
            spans.push(Span::styled(
                *label,
                Style::default().fg(theme.active_tab).bold(),
            ));
        } else {
            spans.push(Span::styled(
                *label,
                Style::default().fg(theme.inactive_tab),
            ));
        }
        spans.push(Span::raw("  "));
    }

    spans.push(Span::styled("│ ", Style::default().fg(theme.separator)));
    spans.push(Span::styled("?", Style::default().fg(theme.brand)));
    spans.push(Span::styled(":Help", Style::default().fg(theme.text_muted)));
    spans.push(Span::raw("  │ "));
    spans.push(Span::styled(now, Style::default().fg(theme.text_muted)));

    let header = Paragraph::new(Line::from(spans)).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(theme.border)),
    );
    f.render_widget(header, area);
}

fn render_sessions_tab(f: &mut Frame, area: Rect, sessions: &[Session], theme: &Theme) {
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

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::bordered()
                .title("Active Sessions")
                .border_style(Style::default().fg(theme.border)),
        )
        .row_highlight_style(
            Style::default()
                .fg(theme.text_primary)
                .bg(theme.selection_bg)
                .bold(),
        )
        .highlight_symbol(">> ");
    f.render_widget(table, area);
}

fn render_hosts_tab(
    f: &mut Frame,
    area: Rect,
    hosts: &[super::HostDisplay],
    filtered_indices: &[usize],
    selected: usize,
    _table_state: &mut TableState,
    theme: &Theme,
) {
    let header = Row::new(vec![
        Cell::from("Name").style(Style::default().fg(theme.brand).bold()),
        Cell::from("Hostname").style(Style::default().fg(theme.brand).bold()),
        Cell::from("Port").style(Style::default().fg(theme.brand).bold()),
        Cell::from("User").style(Style::default().fg(theme.brand).bold()),
        Cell::from("Status").style(Style::default().fg(theme.brand).bold()),
        Cell::from("Last Seen").style(Style::default().fg(theme.brand).bold()),
        Cell::from("Tags").style(Style::default().fg(theme.brand).bold()),
    ])
    .height(1);

    // Build rows from filtered indices only
    let filtered_hosts: Vec<&super::HostDisplay> = filtered_indices
        .iter()
        .filter_map(|&i| hosts.get(i))
        .collect();

    // Determine which row in the filtered list is selected
    let selected_row = filtered_indices.iter().position(|&i| i == selected);
    let mut filtered_table_state = TableState::default();
    filtered_table_state.select(selected_row);

    let rows: Vec<Row> = filtered_hosts
        .iter()
        .map(|h| {
            let status_cell = match h.status {
                super::HostStatus::Online => {
                    Cell::from("● Online").style(Style::default().fg(theme.status_good))
                }
                super::HostStatus::Offline => {
                    Cell::from("● Offline").style(Style::default().fg(theme.status_error))
                }
                super::HostStatus::Unknown => {
                    Cell::from("○ Unknown").style(Style::default().fg(theme.text_muted))
                }
            };
            Row::new([
                Cell::from(h.name.clone()),
                Cell::from(h.hostname.clone()),
                Cell::from(h.port.to_string()),
                Cell::from(h.user.clone()),
                status_cell,
                Cell::from(h.last_seen.clone()),
                Cell::from(h.tags.clone()),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(15),
        Constraint::Percentage(20),
        Constraint::Percentage(7),
        Constraint::Percentage(10),
        Constraint::Percentage(12),
        Constraint::Percentage(18),
        Constraint::Percentage(18),
    ];

    let title = if filtered_hosts.len() == hosts.len() {
        format!("Hosts ({})", hosts.len())
    } else {
        format!("Hosts ({}/{})", filtered_hosts.len(), hosts.len())
    };
    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::bordered()
                .title(title)
                .border_style(Style::default().fg(theme.border)),
        )
        .row_highlight_style(
            Style::default()
                .fg(theme.text_primary)
                .bg(theme.selection_bg)
                .bold(),
        )
        .highlight_symbol(">> ");
    f.render_stateful_widget(table, area, &mut filtered_table_state);
}

fn render_fleet_tab(
    f: &mut Frame,
    area: Rect,
    hosts: &[super::HostDisplay],
    sessions: &[Session],
    theme: &Theme,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5), // fleet health summary
            Constraint::Min(4),    // per-host status table
        ])
        .split(area);

    // Fleet health summary
    let online = hosts
        .iter()
        .filter(|h| matches!(h.status, super::HostStatus::Online))
        .count();
    let offline = hosts
        .iter()
        .filter(|h| matches!(h.status, super::HostStatus::Offline))
        .count();
    let unknown = hosts
        .iter()
        .filter(|h| matches!(h.status, super::HostStatus::Unknown))
        .count();
    let total = hosts.len();
    let pct = if total > 0 {
        (online as f64 / total as f64) * 100.0
    } else {
        0.0
    };
    let active_sessions = sessions
        .iter()
        .filter(|s| matches!(s.state, SessionState::Active))
        .count();

    let bar = widgets::bar_gauge(pct, 40);
    let bar_color = if pct >= 80.0 {
        theme.status_good
    } else if pct >= 50.0 {
        theme.status_warn
    } else if total > 0 {
        theme.status_error
    } else {
        theme.text_muted
    };

    let summary = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("  Online: ", Style::default().fg(theme.text_muted)),
            Span::styled(
                format!("{}", online),
                Style::default().fg(theme.status_good),
            ),
            Span::raw("  │  "),
            Span::styled("Offline: ", Style::default().fg(theme.text_muted)),
            Span::styled(
                format!("{}", offline),
                Style::default().fg(theme.status_error),
            ),
            Span::raw("  │  "),
            Span::styled("Unknown: ", Style::default().fg(theme.text_muted)),
            Span::styled(
                format!("{}", unknown),
                Style::default().fg(theme.text_muted),
            ),
            Span::raw("  │  "),
            Span::styled("Total: ", Style::default().fg(theme.text_muted)),
            Span::raw(format!("{}", total)),
            Span::raw("  │  "),
            Span::styled("Sessions: ", Style::default().fg(theme.text_muted)),
            Span::styled(
                format!("{}", active_sessions),
                Style::default().fg(theme.status_info),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(bar, Style::default().fg(bar_color)),
            Span::raw(format!(" {:.0}%", pct)),
        ]),
    ])
    .block(
        Block::bordered()
            .title("Fleet Health")
            .border_style(Style::default().fg(theme.border)),
    );
    f.render_widget(summary, chunks[0]);

    // Per-host status table with latency sparklines
    let header = Row::new(vec![
        Cell::from("Host").style(Style::default().fg(theme.brand).bold()),
        Cell::from("Port").style(Style::default().fg(theme.brand).bold()),
        Cell::from("Status").style(Style::default().fg(theme.brand).bold()),
        Cell::from("Latency").style(Style::default().fg(theme.brand).bold()),
        Cell::from("History").style(Style::default().fg(theme.brand).bold()),
    ])
    .height(1);

    let rows: Vec<Row> = hosts
        .iter()
        .map(|h| {
            let (status_text, status_style) = match h.status {
                super::HostStatus::Online => ("● Online", Style::default().fg(theme.status_good)),
                super::HostStatus::Offline => {
                    ("● Offline", Style::default().fg(theme.status_error))
                }
                super::HostStatus::Unknown => ("○ Probing…", Style::default().fg(theme.text_muted)),
            };

            let latency_cell = match h.latency_ms {
                Some(ms) => {
                    let color = latency_threshold_color(theme, ms);
                    Cell::from(format!("{:.0}ms", ms)).style(Style::default().fg(color))
                }
                None => Cell::from("—").style(Style::default().fg(theme.text_muted)),
            };

            let sparkline = if h.latency_history.is_empty() {
                "                ".to_string()
            } else {
                widgets::sparkline_string(&h.latency_history, 16)
            };
            let spark_color = match h.latency_ms {
                Some(ms) => latency_threshold_color(theme, ms),
                None => theme.text_muted,
            };

            Row::new([
                Cell::from(if h.name.is_empty() {
                    h.hostname.clone()
                } else {
                    h.name.clone()
                }),
                Cell::from(h.port.to_string()),
                Cell::from(status_text).style(status_style),
                latency_cell,
                Cell::from(sparkline).style(Style::default().fg(spark_color)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(30),
        Constraint::Percentage(8),
        Constraint::Percentage(14),
        Constraint::Percentage(12),
        Constraint::Percentage(36),
    ];

    let table = Table::new(rows, widths).header(header).block(
        Block::bordered()
            .title("Host Status")
            .border_style(Style::default().fg(theme.border)),
    );
    f.render_widget(table, chunks[1]);
}

/// Netwatch latency thresholds: green < 50ms, yellow < 200ms, red ≥ 200ms
fn latency_threshold_color(theme: &Theme, ms: f64) -> Color {
    if ms < 50.0 {
        theme.status_good
    } else if ms < 200.0 {
        theme.status_warn
    } else {
        theme.status_error
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
            "  Press 't' to cycle themes, or set theme in ~/.essh/config.toml.",
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
    _tab: super::DashboardTab,
    status: Option<&str>,
    search_active: bool,
    search_query: &str,
    theme: &Theme,
) {
    let mut lines = Vec::new();

    if search_active {
        lines.push(Line::from(vec![
            Span::styled(" /", Style::default().fg(theme.brand).bold()),
            Span::styled(search_query, Style::default().fg(theme.active_tab)),
            Span::styled("█", Style::default().fg(theme.brand)),
            Span::styled("  Esc", Style::default().fg(theme.text_muted)),
            Span::styled(":Cancel  ", Style::default().fg(theme.text_muted)),
            Span::styled("Enter", Style::default().fg(theme.text_muted)),
            Span::styled(":Connect", Style::default().fg(theme.text_muted)),
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled("Enter", Style::default().fg(theme.key_hint)),
        Span::raw(":Connect  "),
        Span::styled("Alt+1-9", Style::default().fg(theme.key_hint)),
        Span::raw(":Session  "),
        Span::styled("a", Style::default().fg(theme.key_hint)),
        Span::raw(":Add  "),
        Span::styled("/", Style::default().fg(theme.key_hint)),
        Span::raw(":Search  "),
        Span::styled("r", Style::default().fg(theme.key_hint)),
        Span::raw(":Refresh  "),
        Span::styled("d", Style::default().fg(theme.key_hint)),
        Span::raw(":Delete  "),
        Span::styled("t", Style::default().fg(theme.key_hint)),
        Span::raw(":Theme  "),
        Span::styled("Ctrl+p", Style::default().fg(theme.key_hint)),
        Span::raw(":Palette  "),
        Span::styled("q", Style::default().fg(theme.key_hint)),
        Span::raw(":Quit"),
    ]));

    if let Some(msg) = status {
        lines.push(Line::from(Span::styled(
            msg.to_string(),
            Style::default().fg(theme.status_warn),
        )));
    }

    let footer = Paragraph::new(lines)
        .block(Block::bordered().border_style(Style::default().fg(theme.border)));
    f.render_widget(footer, area);
}
