use crate::diagnostics::DiagnosticsSnapshot;
use crate::portfwd::PortForwardManager;
use crate::session::{Session, SessionState};
use crate::theme::Theme;
use crate::tui::widgets;
use crate::tui::Notification;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};

/// Render the session tab bar at the top
pub fn render_tab_bar(
    f: &mut Frame,
    area: Rect,
    sessions: &[Session],
    active_index: usize,
    notifications: &[Notification],
    theme: &Theme,
) {
    let now = chrono::Local::now().format("%H:%M:%S").to_string();
    let mut spans: Vec<Span> = vec![
        Span::styled(" ESSH ", Style::default().fg(theme.brand).bold()),
        Span::styled("── ", Style::default().fg(theme.separator)),
    ];

    for (i, session) in sessions.iter().enumerate() {
        let has_notifications = notifications
            .iter()
            .any(|n| n.session_label == session.label);
        if let SessionState::Reconnecting { attempt, max } = &session.state {
            let label = format!(
                "[{}] {} ● Recon. {}/{} ",
                i + 1,
                session.label,
                attempt,
                max
            );
            if i == active_index {
                spans.push(Span::styled(
                    label,
                    Style::default().fg(theme.status_error).bold(),
                ));
            } else {
                spans.push(Span::styled(label, Style::default().fg(theme.status_error)));
            }
        } else if let SessionState::Disconnected { .. } = &session.state {
            let label = format!("[{}] {} ● Disconn. ", i + 1, session.label);
            if i == active_index {
                spans.push(Span::styled(
                    label,
                    Style::default().fg(theme.status_error).bold(),
                ));
            } else {
                spans.push(Span::styled(label, Style::default().fg(theme.status_error)));
            }
        } else {
            let label = format!("[{}] {} ", i + 1, session.label);
            if i == active_index {
                spans.push(Span::styled(
                    label,
                    Style::default().fg(theme.active_tab).bold(),
                ));
            } else if has_notifications {
                spans.push(Span::styled(
                    label,
                    Style::default().fg(theme.brand).underlined(),
                ));
                spans.push(Span::styled(
                    "! ",
                    Style::default().fg(theme.active_tab).bold(),
                ));
            } else if session.has_new_output {
                spans.push(Span::styled(
                    label,
                    Style::default().fg(theme.brand).underlined(),
                ));
            } else if matches!(session.state, SessionState::Suspended) {
                spans.push(Span::styled(label, Style::default().fg(theme.text_muted)));
            } else {
                spans.push(Span::styled(label, Style::default().fg(theme.text_primary)));
            }
        }
        spans.push(Span::raw(" "));
    }

    spans.push(Span::styled(
        format!("── {}", now),
        Style::default().fg(theme.text_muted),
    ));

    let tab_bar = Paragraph::new(Line::from(spans)).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(theme.border)),
    );
    f.render_widget(tab_bar, area);
}

/// Render the terminal output area for a session using the vt100 virtual terminal
pub fn render_terminal(f: &mut Frame, area: Rect, session: &Session) {
    let screen_lines = session.terminal.screen_lines();
    let visible_height = area.height as usize;
    let visible_width = area.width as usize;

    // Build ratatui Lines from the virtual terminal screen
    let mut lines: Vec<Line> = Vec::with_capacity(visible_height);
    for (row_idx, row) in screen_lines.iter().enumerate() {
        if row_idx >= visible_height {
            break;
        }
        let mut spans: Vec<Span> = Vec::new();
        let mut current_text = String::new();
        let mut current_style = Style::default();

        for (col_idx, cell) in row.iter().enumerate() {
            if col_idx >= visible_width {
                break;
            }
            let mut style = Style::default();
            if let Some(fg) = cell.fg {
                style = style.fg(if cell.inverse {
                    cell.bg.unwrap_or(Color::Reset)
                } else {
                    fg
                });
            }
            if let Some(bg) = cell.bg {
                style = style.bg(if cell.inverse {
                    cell.fg.unwrap_or(Color::Reset)
                } else {
                    bg
                });
            } else if cell.inverse {
                if let Some(fg) = cell.fg {
                    style = style.bg(fg);
                }
            }
            if cell.bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            if cell.underline {
                style = style.add_modifier(Modifier::UNDERLINED);
            }

            if style == current_style {
                current_text.push_str(&cell.text);
            } else {
                if !current_text.is_empty() {
                    spans.push(Span::styled(
                        std::mem::take(&mut current_text),
                        current_style,
                    ));
                }
                current_text = cell.text.clone();
                current_style = style;
            }
        }
        if !current_text.is_empty() {
            spans.push(Span::styled(current_text, current_style));
        }
        lines.push(Line::from(spans));
    }

    // Render cursor position
    let (cursor_row, cursor_col) = session.terminal.cursor_position();
    let cursor_x = area.x + cursor_col;
    let cursor_y = area.y + cursor_row;
    if cursor_x < area.x + area.width && cursor_y < area.y + area.height {
        f.set_cursor_position((cursor_x, cursor_y));
    }

    let terminal_block = Paragraph::new(lines).block(Block::default());
    f.render_widget(terminal_block, area);
}

/// Render the diagnostics status bar at the bottom of a session
pub fn render_status_bar(
    f: &mut Frame,
    area: Rect,
    session: &Session,
    diag: Option<&DiagnosticsSnapshot>,
    pfm: Option<&PortForwardManager>,
    theme: &Theme,
) {
    let mut spans = if let Some(d) = diag {
        let rtt_text = match d.rtt_ms {
            Some(rtt) => format!("{:.1}ms", rtt),
            None => "—".to_string(),
        };
        let quality_str = format!("{:?}", d.quality);
        let q_color = widgets::quality_color(theme, &quality_str);

        vec![
            Span::styled("RTT:", Style::default().fg(theme.text_muted)),
            Span::raw(rtt_text),
            Span::raw("  "),
            Span::styled("↑", Style::default().fg(theme.rx_rate)),
            Span::raw(widgets::format_bytes_rate(d.throughput_up_bps)),
            Span::raw("  "),
            Span::styled("↓", Style::default().fg(theme.tx_rate)),
            Span::raw(widgets::format_bytes_rate(d.throughput_down_bps)),
            Span::raw("  "),
            Span::styled("Loss:", Style::default().fg(theme.text_muted)),
            Span::raw(format!("{:.1}%", d.packet_loss_pct)),
            Span::raw("  "),
            Span::styled(format!("●{}", quality_str), Style::default().fg(q_color)),
            Span::raw("  "),
            Span::styled("Up:", Style::default().fg(theme.text_muted)),
            Span::raw(widgets::format_duration_short(d.uptime_secs)),
        ]
    } else {
        vec![Span::styled(
            match &session.state {
                SessionState::Connecting => "Connecting...".to_string(),
                SessionState::Disconnected { reason } => format!("Disconnected: {}", reason),
                SessionState::Reconnecting { attempt, max } => {
                    format!("Reconnecting ({}/{})", attempt, max)
                }
                _ => {
                    if let Some(ref jump) = session.jump_host {
                        format!(
                            "{}@{}:{} via {}",
                            session.username, session.hostname, session.port, jump
                        )
                    } else {
                        format!("{}@{}:{}", session.username, session.hostname, session.port)
                    }
                }
            },
            Style::default().fg(theme.text_muted),
        )]
    };

    // Append port forward summary if any
    if let Some(mgr) = pfm {
        let summary = mgr.summary();
        if !summary.is_empty() {
            spans.push(Span::raw("  "));
            spans.push(Span::styled("Fwd:", Style::default().fg(theme.text_muted)));
            spans.push(Span::styled(
                summary,
                Style::default().fg(theme.status_good),
            ));
        }
    }

    let status = Paragraph::new(Line::from(spans)).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(theme.border)),
    );
    f.render_widget(status, area);
}

/// Render the session footer with keybindings
pub fn render_footer(f: &mut Frame, area: Rect, theme: &Theme) {
    let footer = Paragraph::new(Line::from(vec![
        Span::styled("Alt+←→", Style::default().fg(theme.key_hint)),
        Span::raw(":Switch  "),
        Span::styled("Alt+s", Style::default().fg(theme.key_hint)),
        Span::raw(":Split  "),
        Span::styled("Alt+m", Style::default().fg(theme.key_hint)),
        Span::raw(":Monitor  "),
        Span::styled("Alt+f", Style::default().fg(theme.key_hint)),
        Span::raw(":Files  "),
        Span::styled("Alt+d", Style::default().fg(theme.key_hint)),
        Span::raw(":Detach  "),
        Span::styled("Alt+w", Style::default().fg(theme.key_hint)),
        Span::raw(":Close  "),
        Span::styled("Alt+t", Style::default().fg(theme.key_hint)),
        Span::raw(":Theme  "),
        Span::styled("Alt+h", Style::default().fg(theme.key_hint)),
        Span::raw(":Help"),
    ]))
    .block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(theme.border)),
    );
    f.render_widget(footer, area);
}
