use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
};
use crate::session::{SessionState, Session};
use crate::tui::widgets;

/// Render the dashboard view (no active session focused)
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
) {
    let footer_height = if search_active { 4 } else { 3 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // header + tab bar
            Constraint::Min(8),    // main content
            Constraint::Length(footer_height), // footer (+ search bar)
        ])
        .split(area);

    render_header(f, chunks[0], active_tab);

    match active_tab {
        super::DashboardTab::Sessions => render_sessions_tab(f, chunks[1], sessions),
        super::DashboardTab::Hosts => render_hosts_tab(f, chunks[1], hosts, filtered_indices, selected_host, table_state),
        super::DashboardTab::Fleet => render_fleet_tab(f, chunks[1], hosts, sessions),
        super::DashboardTab::Config => render_config_tab(f, chunks[1]),
    }

    render_footer(f, chunks[2], active_tab, status_message, search_active, search_query);
}

fn render_header(f: &mut Frame, area: Rect, active_tab: super::DashboardTab) {
    let now = chrono::Local::now().format("%H:%M:%S").to_string();
    
    let tabs = [
        ("[1] Sessions", super::DashboardTab::Sessions),
        ("[2] Hosts", super::DashboardTab::Hosts),
        ("[3] Fleet", super::DashboardTab::Fleet),
        ("[4] Config", super::DashboardTab::Config),
    ];

    let mut spans: Vec<Span> = vec![
        Span::styled(" ESSH ", Style::default().fg(Color::Cyan).bold()),
        Span::styled("│ ", Style::default().fg(Color::DarkGray)),
    ];

    for (label, tab) in &tabs {
        if *tab == active_tab {
            spans.push(Span::styled(*label, Style::default().fg(Color::Yellow).bold()));
        } else {
            spans.push(Span::raw(*label));
        }
        spans.push(Span::raw("  "));
    }

    spans.push(Span::styled("│ ", Style::default().fg(Color::DarkGray)));
    spans.push(Span::styled("?", Style::default().fg(Color::Cyan)));
    spans.push(Span::styled(":Help", Style::default().fg(Color::DarkGray)));
    spans.push(Span::raw("  │ "));
    spans.push(Span::styled(now, Style::default().fg(Color::DarkGray)));

    let header = Paragraph::new(Line::from(spans))
        .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(header, area);
}

fn render_sessions_tab(f: &mut Frame, area: Rect, sessions: &[Session]) {
    if sessions.is_empty() {
        let msg = Paragraph::new(vec![
            Line::raw(""),
            Line::styled("  No active sessions.", Style::default().fg(Color::DarkGray)),
            Line::raw(""),
            Line::styled("  Press [2] to browse hosts, or use 'essh connect <host>' to start a session.", Style::default().fg(Color::DarkGray)),
        ])
        .block(Block::bordered().title("Active Sessions").border_style(Style::default().fg(Color::DarkGray)));
        f.render_widget(msg, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from(" # ").style(Style::default().fg(Color::Cyan).bold()),
        Cell::from("Label").style(Style::default().fg(Color::Cyan).bold()),
        Cell::from("Host").style(Style::default().fg(Color::Cyan).bold()),
        Cell::from("User").style(Style::default().fg(Color::Cyan).bold()),
        Cell::from("Status").style(Style::default().fg(Color::Cyan).bold()),
        Cell::from("Uptime").style(Style::default().fg(Color::Cyan).bold()),
    ]).height(1);

    let rows: Vec<Row> = sessions.iter().enumerate().map(|(i, s)| {
        let state_style = match &s.state {
            SessionState::Active => Style::default().fg(Color::Green),
            SessionState::Suspended => Style::default().fg(Color::DarkGray),
            SessionState::Reconnecting { .. } => Style::default().fg(Color::Red),
            SessionState::Connecting => Style::default().fg(Color::Yellow),
            SessionState::Disconnected { .. } => Style::default().fg(Color::Red),
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
    }).collect();

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
        .block(Block::bordered().title("Active Sessions").border_style(Style::default().fg(Color::DarkGray)))
        .row_highlight_style(Style::default().bg(Color::DarkGray).bold())
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
) {
    let header = Row::new(vec![
        Cell::from("Name").style(Style::default().fg(Color::Cyan).bold()),
        Cell::from("Hostname").style(Style::default().fg(Color::Cyan).bold()),
        Cell::from("Port").style(Style::default().fg(Color::Cyan).bold()),
        Cell::from("User").style(Style::default().fg(Color::Cyan).bold()),
        Cell::from("Status").style(Style::default().fg(Color::Cyan).bold()),
        Cell::from("Last Seen").style(Style::default().fg(Color::Cyan).bold()),
        Cell::from("Tags").style(Style::default().fg(Color::Cyan).bold()),
    ]).height(1);

    // Build rows from filtered indices only
    let filtered_hosts: Vec<&super::HostDisplay> = filtered_indices
        .iter()
        .filter_map(|&i| hosts.get(i))
        .collect();

    // Determine which row in the filtered list is selected
    let selected_row = filtered_indices.iter().position(|&i| i == selected);
    let mut filtered_table_state = TableState::default();
    filtered_table_state.select(selected_row);

    let rows: Vec<Row> = filtered_hosts.iter().map(|h| {
        let status_cell = match h.status {
            super::HostStatus::Online => Cell::from("● Online").style(Style::default().fg(Color::Green)),
            super::HostStatus::Offline => Cell::from("● Offline").style(Style::default().fg(Color::Red)),
            super::HostStatus::Unknown => Cell::from("○ Unknown").style(Style::default().fg(Color::DarkGray)),
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
    }).collect();

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
        .block(Block::bordered().title(title).border_style(Style::default().fg(Color::DarkGray)))
        .row_highlight_style(Style::default().bg(Color::DarkGray).bold())
        .highlight_symbol(">> ");
    f.render_stateful_widget(table, area, &mut filtered_table_state);
}

fn render_fleet_tab(f: &mut Frame, area: Rect, hosts: &[super::HostDisplay], sessions: &[Session]) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),  // fleet health summary
            Constraint::Min(4),    // per-host status table
        ])
        .split(area);

    // Fleet health summary
    let online = hosts.iter().filter(|h| matches!(h.status, super::HostStatus::Online)).count();
    let offline = hosts.iter().filter(|h| matches!(h.status, super::HostStatus::Offline)).count();
    let unknown = hosts.iter().filter(|h| matches!(h.status, super::HostStatus::Unknown)).count();
    let total = hosts.len();
    let pct = if total > 0 { (online as f64 / total as f64) * 100.0 } else { 0.0 };
    let active_sessions = sessions.iter().filter(|s| matches!(s.state, SessionState::Active)).count();

    let bar = widgets::bar_gauge(pct, 40);
    let bar_color = if pct >= 80.0 {
        Color::Green
    } else if pct >= 50.0 {
        Color::Yellow
    } else if total > 0 {
        Color::Red
    } else {
        Color::DarkGray
    };

    let summary = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("  Online: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", online), Style::default().fg(Color::Green)),
            Span::raw("  │  "),
            Span::styled("Offline: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", offline), Style::default().fg(Color::Red)),
            Span::raw("  │  "),
            Span::styled("Unknown: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", unknown), Style::default().fg(Color::DarkGray)),
            Span::raw("  │  "),
            Span::styled("Total: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{}", total)),
            Span::raw("  │  "),
            Span::styled("Sessions: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", active_sessions), Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(bar, Style::default().fg(bar_color)),
            Span::raw(format!(" {:.0}%", pct)),
        ]),
    ])
    .block(Block::bordered().title("Fleet Health").border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(summary, chunks[0]);

    // Per-host status table with latency sparklines
    let header = Row::new(vec![
        Cell::from("Host").style(Style::default().fg(Color::Cyan).bold()),
        Cell::from("Port").style(Style::default().fg(Color::Cyan).bold()),
        Cell::from("Status").style(Style::default().fg(Color::Cyan).bold()),
        Cell::from("Latency").style(Style::default().fg(Color::Cyan).bold()),
        Cell::from("History").style(Style::default().fg(Color::Cyan).bold()),
    ]).height(1);

    let rows: Vec<Row> = hosts.iter().map(|h| {
        let (status_text, status_style) = match h.status {
            super::HostStatus::Online => ("● Online", Style::default().fg(Color::Green)),
            super::HostStatus::Offline => ("● Offline", Style::default().fg(Color::Red)),
            super::HostStatus::Unknown => ("○ Probing…", Style::default().fg(Color::DarkGray)),
        };

        let latency_cell = match h.latency_ms {
            Some(ms) => {
                let color = latency_threshold_color(ms);
                Cell::from(format!("{:.0}ms", ms)).style(Style::default().fg(color))
            }
            None => Cell::from("—").style(Style::default().fg(Color::DarkGray)),
        };

        let sparkline = if h.latency_history.is_empty() {
            "                ".to_string()
        } else {
            widgets::sparkline_string(&h.latency_history, 16)
        };
        let spark_color = match h.latency_ms {
            Some(ms) => latency_threshold_color(ms),
            None => Color::DarkGray,
        };

        Row::new([
            Cell::from(if h.name.is_empty() { h.hostname.clone() } else { h.name.clone() }),
            Cell::from(h.port.to_string()),
            Cell::from(status_text).style(status_style),
            latency_cell,
            Cell::from(sparkline).style(Style::default().fg(spark_color)),
        ])
    }).collect();

    let widths = [
        Constraint::Percentage(30),
        Constraint::Percentage(8),
        Constraint::Percentage(14),
        Constraint::Percentage(12),
        Constraint::Percentage(36),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::bordered().title("Host Status").border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(table, chunks[1]);
}

/// Netwatch latency thresholds: green < 50ms, yellow < 200ms, red ≥ 200ms
fn latency_threshold_color(ms: f64) -> Color {
    if ms < 50.0 {
        Color::Green
    } else if ms < 200.0 {
        Color::Yellow
    } else {
        Color::Red
    }
}

fn render_config_tab(f: &mut Frame, area: Rect) {
    let content = Paragraph::new(vec![
        Line::raw(""),
        Line::styled("  Configuration", Style::default().fg(Color::Cyan).bold()),
        Line::raw(""),
        Line::styled("  Config file: ~/.essh/config.toml", Style::default().fg(Color::DarkGray)),
        Line::styled("  Cache DB:    ~/.essh/cache.db", Style::default().fg(Color::DarkGray)),
        Line::styled("  Audit log:   ~/.essh/audit.log", Style::default().fg(Color::DarkGray)),
        Line::raw(""),
        Line::styled("  Use 'essh config edit' or press 'e' to open config in $EDITOR.", Style::default().fg(Color::DarkGray)),
    ])
    .block(Block::bordered().title("Config").border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(content, area);
}

fn render_footer(
    f: &mut Frame,
    area: Rect,
    _tab: super::DashboardTab,
    status: Option<&str>,
    search_active: bool,
    search_query: &str,
) {
    let mut lines = Vec::new();

    if search_active {
        lines.push(Line::from(vec![
            Span::styled(" /", Style::default().fg(Color::Cyan).bold()),
            Span::styled(search_query, Style::default().fg(Color::Yellow)),
            Span::styled("█", Style::default().fg(Color::Cyan)),
            Span::styled("  Esc", Style::default().fg(Color::DarkGray)),
            Span::styled(":Cancel  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Enter", Style::default().fg(Color::DarkGray)),
            Span::styled(":Connect", Style::default().fg(Color::DarkGray)),
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled("Enter", Style::default().fg(Color::Cyan)),
        Span::raw(":Connect  "),
        Span::styled("Alt+1-9", Style::default().fg(Color::Cyan)),
        Span::raw(":Session  "),
        Span::styled("a", Style::default().fg(Color::Cyan)),
        Span::raw(":Add  "),
        Span::styled("/", Style::default().fg(Color::Cyan)),
        Span::raw(":Search  "),
        Span::styled("r", Style::default().fg(Color::Cyan)),
        Span::raw(":Refresh  "),
        Span::styled("d", Style::default().fg(Color::Cyan)),
        Span::raw(":Delete  "),
        Span::styled("q", Style::default().fg(Color::Cyan)),
        Span::raw(":Quit"),
    ]));

    if let Some(msg) = status {
        lines.push(Line::from(Span::styled(msg.to_string(), Style::default().fg(Color::Yellow))));
    }

    let footer = Paragraph::new(lines)
        .block(Block::bordered().border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(footer, area);
}
