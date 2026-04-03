use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

pub fn render(f: &mut Frame) {
    let area = f.area();

    // Center a popup ~60 cols wide, ~28 rows tall
    let popup_width = 60u16.min(area.width.saturating_sub(4));
    let popup_height = 40u16.min(area.height.saturating_sub(2));
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup = Rect::new(x, y, popup_width, popup_height);

    // Clear the area behind the popup
    f.render_widget(Clear, popup);

    let key_style = Style::default().fg(Color::Cyan).bold();
    let heading_style = Style::default().fg(Color::Yellow).bold();
    let desc_style = Style::default().fg(Color::White);
    let dim = Style::default().fg(Color::DarkGray);

    let lines = vec![
        Line::raw(""),
        Line::styled("  Global", heading_style),
        Line::from(vec![
            Span::styled("    ?           ", key_style),
            Span::styled("Toggle this help menu", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Alt+1-9     ", key_style),
            Span::styled("Jump to session N", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Alt+←/→     ", key_style),
            Span::styled("Previous / next session", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Alt+Tab     ", key_style),
            Span::styled("Switch to last session", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Alt+s       ", key_style),
            Span::styled("Toggle split-pane (terminal + monitor)", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Alt+[/]     ", key_style),
            Span::styled("Adjust split-pane width", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Alt+m       ", key_style),
            Span::styled("Toggle host monitor (full-screen)", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Alt+p       ", key_style),
            Span::styled("Toggle port forwarding manager", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Alt+f       ", key_style),
            Span::styled("File browser (upload/download)", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Ctrl+p      ", key_style),
            Span::styled("Command palette (fuzzy search)", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Alt+d       ", key_style),
            Span::styled("Detach to dashboard", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Alt+w       ", key_style),
            Span::styled("Close active session", desc_style),
        ]),
        Line::raw(""),
        Line::styled("  Dashboard", heading_style),
        Line::from(vec![
            Span::styled("    1-4         ", key_style),
            Span::styled("Switch tab (Sessions/Hosts/Fleet/Config)", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    j/k ↑/↓     ", key_style),
            Span::styled("Navigate host list", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Enter       ", key_style),
            Span::styled("Connect to selected host", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    r           ", key_style),
            Span::styled("Refresh hosts", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    d           ", key_style),
            Span::styled("Delete selected host", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    q / Ctrl+c  ", key_style),
            Span::styled("Quit", desc_style),
        ]),
        Line::raw(""),
        Line::styled("  Session", heading_style),
        Line::from(vec![
            Span::styled("    (all keys)  ", key_style),
            Span::styled("Forwarded to remote shell", desc_style),
        ]),
        Line::raw(""),
        Line::styled("  Monitor", heading_style),
        Line::from(vec![
            Span::styled("    s           ", key_style),
            Span::styled("Toggle sort (CPU / Memory)", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    ↑/↓         ", key_style),
            Span::styled("Scroll process list", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Esc         ", key_style),
            Span::styled("Return to terminal", desc_style),
        ]),
        Line::raw(""),
        Line::styled("  Config", heading_style),
        Line::from(vec![
            Span::styled("    notification_patterns", key_style),
            Span::styled("  Background alert regexes", desc_style),
        ]),
        Line::raw(""),
        Line::styled("                    Press ? or Esc to close", dim),
    ];

    let block = Block::default()
        .title(" Help ")
        .title_style(Style::default().fg(Color::Cyan).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(paragraph, popup);
}
