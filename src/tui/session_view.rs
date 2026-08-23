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
        // A quality verdict with no round-trip behind it is a claim we cannot
        // support. v1 showed `RTT:—` beside `●Excellent`: a dash where a number
        // belongs, next to a confident assessment derived from nothing. Until
        // a keepalive has come back, say so and withhold the verdict.
        match d.rtt_ms {
            Some(rtt) => {
                let quality_str = format!("{:?}", d.quality);
                let q_color = widgets::quality_color(theme, &quality_str);
                vec![
                    Span::styled("RTT:", Style::default().fg(theme.text_muted)),
                    Span::raw(format!("{:.1}ms", rtt)),
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
            }
            None => vec![
                Span::styled(
                    "link quality unmeasured — waiting for the first keepalive",
                    Style::default().fg(theme.text_muted).italic(),
                ),
                Span::raw("  "),
                Span::styled("Up:", Style::default().fg(theme.text_muted)),
                Span::raw(widgets::format_duration_short(d.uptime_secs)),
            ],
        }
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

/// The session's key hints.
///
/// Present at all times rather than on demand. The handoff reserved no rows
/// for this, on the theory that the shell should own every cell — but a
/// modal prefix nobody can see is a modal prefix nobody uses, and "I cannot
/// memorise the keys" is the predictable result. Two rows is the price of
/// the app being usable without the manual.
pub fn render_footer(
    f: &mut Frame,
    area: Rect,
    prefix: &str,
    pending: bool,
    status: Option<&str>,
    sessions: usize,
    theme: &Theme,
) {
    use crate::tui::prefix_hint;

    // A message takes the row while it is fresh. Commands that decline to do
    // something — "no other session to split into" — used to say so into a
    // status nothing rendered, so the key looked broken rather than refused.
    if let Some(msg) = status.filter(|_| !pending) {
        let footer = Paragraph::new(Line::from(vec![
            Span::styled(" ▲ ", Style::default().fg(theme.status_warn).bold()),
            Span::styled(msg.to_string(), Style::default().fg(theme.text_primary)),
        ]))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.status_warn)),
        );
        f.render_widget(footer, area);
        return;
    }

    // While the prefix is pending the strip becomes the menu for it.
    if pending {
        let mut spans = vec![Span::styled(
            format!(" {} ", prefix_hint(prefix, "—")),
            Style::default().fg(theme.brand).bold(),
        )];
        for (key, label) in [
            ("n", "new session"),
            ("k", "menu"),
            ("m", "monitor"),
            ("f", "files"),
            ("p", "forwards"),
            ("s", "split"),
            ("d", "detach"),
            ("w", "close"),
            ("1-9", "session"),
            ("←→", "switch"),
            ("⇥", "last"),
            ("t", "theme"),
            ("?", "help"),
        ] {
            spans.push(Span::styled(
                key.to_string(),
                Style::default().fg(theme.key_hint).bold(),
            ));
            spans.push(Span::styled(
                format!(" {}  ", label),
                Style::default().fg(theme.text_primary),
            ));
        }
        spans.push(Span::styled(
            "(again sends it to the shell)",
            Style::default().fg(theme.text_muted).italic(),
        ));
        let footer = Paragraph::new(Line::from(spans)).block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.brand)),
        );
        f.render_widget(footer, area);
        return;
    }

    // Idle: one key per thing, and only the things that apply right now.
    //
    // Two mechanisms — function keys for some commands, a prefix for others —
    // means remembering which is which. Everything common has its own key, so
    // the rule is just "press what the strip says". The prefix is still there
    // for the rest, named at the end.
    let mut keys: Vec<(&str, &str)> = vec![
        ("F1", "help"),
        ("F2", "monitor"),
        ("F3", "files"),
        ("F4", "forwards"),
        ("F5", "mini"),
        ("F9", "new"),
        ("F10", "menu"),
    ];
    // Switching only means something once there is somewhere to switch to.
    if sessions > 1 {
        keys.push(("F7/F8", "prev/next"));
    }
    keys.push(("F6", "detach"));

    // On a stock Mac the whole F-key row above is media keys: F1 is
    // brightness, F10 is mute. A user pressing what this strip advertises
    // changes their volume and nothing else, which makes the strip worse than
    // no strip. Name the modifier that makes them work, and name the prefix
    // route that needs no system setting at all.
    let tail = if cfg!(target_os = "macos") {
        // Kept short deliberately: the strip drops the tail whole when it does
        // not fit, so a longer, friendlier sentence means no hint at all.
        format!("· fn+F-keys · {} more", prefix_hint(prefix, ""))
    } else {
        format!("· {} for more", prefix_hint(prefix, ""))
    };

    let width = area.width as usize;
    let keys_width: usize = keys
        .iter()
        .map(|(k, l)| k.chars().count() + l.chars().count() + 4)
        .sum::<usize>()
        + 1;

    let mut spans = vec![Span::raw(" ")];
    for (key, label) in &keys {
        spans.push(Span::styled(
            (*key).to_string(),
            Style::default().fg(theme.key_hint).bold(),
        ));
        spans.push(Span::styled(
            format!(" {}   ", label),
            Style::default().fg(theme.text_muted),
        ));
    }
    if keys_width + tail.chars().count() < width {
        spans.push(Span::styled(
            tail.clone(),
            Style::default().fg(theme.text_muted),
        ));
    }
    let _ = theme.border;

    let footer = Paragraph::new(Line::from(spans)).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(theme.border)),
    );
    f.render_widget(footer, area);
}

/// Render one session inside a split pane.
///
/// The design allows **no per-pane border titles** even in split — *"just a
/// single hairline divider"*. So a pane is the terminal and nothing else;
/// focus is shown by a one-column cyan bar on its left edge, which costs no
/// row and no title.
pub fn render_pane(f: &mut Frame, area: Rect, session: &Session, focused: bool, theme: &Theme) {
    if area.width < 2 {
        return;
    }
    // The divider/focus bar: one column, cyan when focused, rule otherwise.
    let bar = Rect {
        x: area.x,
        y: area.y,
        width: 1,
        height: area.height,
    };
    let color = if focused {
        theme.brand
    } else {
        theme.separator
    };
    f.render_widget(
        Paragraph::new(
            (0..area.height)
                .map(|_| Line::from(Span::styled("▏", Style::default().fg(color))))
                .collect::<Vec<_>>(),
        ),
        bar,
    );

    let term = Rect {
        x: area.x + 1,
        y: area.y,
        width: area.width - 1,
        height: area.height,
    };
    render_terminal(f, term, session);
}
