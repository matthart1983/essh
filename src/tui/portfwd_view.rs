use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph, Row, Table},
};

use crate::portfwd::PortForwardManager;
use crate::theme::Theme;

pub fn render(
    f: &mut Frame,
    manager: &PortForwardManager,
    input: &str,
    adding: bool,
    theme: &Theme,
) {
    let area = f.area();

    // Center a popup ~65 cols wide, ~20 rows tall
    let popup_width = 65u16.min(area.width.saturating_sub(4));
    let popup_height = 20u16.min(area.height.saturating_sub(2));
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup = Rect::new(x, y, popup_width, popup_height);

    f.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Port Forwards ")
        .title_style(Style::default().fg(theme.brand).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));

    let inner = block.inner(popup);
    f.render_widget(block, popup);

    // Layout: table area + footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(if adding { 2 } else { 1 }),
        ])
        .split(inner);

    // Table
    if manager.is_empty() {
        let empty = Paragraph::new(Line::styled(
            "  No port forwards configured",
            Style::default().fg(theme.text_muted),
        ));
        f.render_widget(empty, chunks[0]);
    } else {
        let header = Row::new(vec!["Dir", "Bind", "Target", "Status"])
            .style(Style::default().fg(theme.brand).bold())
            .bottom_margin(1);

        let rows: Vec<Row> = manager
            .forwards
            .iter()
            .enumerate()
            .map(|(i, fwd)| {
                let style = if i == manager.selected {
                    Style::default().fg(theme.active_tab).bold()
                } else {
                    Style::default().fg(theme.text_primary)
                };
                let status = if fwd.active { "Active" } else { "Inactive" };
                Row::new(vec![
                    format!("{}", fwd.direction),
                    format!("{}:{}", fwd.bind_host, fwd.bind_port),
                    format!("{}:{}", fwd.target_host, fwd.target_port),
                    status.to_string(),
                ])
                .style(style)
            })
            .collect();

        let table = Table::new(
            rows,
            [
                Constraint::Length(5),
                Constraint::Length(20),
                Constraint::Length(20),
                Constraint::Length(10),
            ],
        )
        .header(header);

        f.render_widget(table, chunks[0]);
    }

    // Footer
    if adding {
        let input_line = Paragraph::new(Line::from(vec![
            Span::styled("Format: ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "L:bind_port:target_host:target_port  ",
                Style::default().fg(theme.text_muted),
            ),
            Span::styled("> ", Style::default().fg(theme.brand)),
            Span::raw(input),
            Span::styled("█", Style::default().fg(theme.brand)),
        ]));
        f.render_widget(input_line, chunks[1]);
    } else {
        let footer = Paragraph::new(Line::from(vec![
            Span::styled("a", Style::default().fg(theme.key_hint)),
            Span::raw(":Add  "),
            Span::styled("d", Style::default().fg(theme.key_hint)),
            Span::raw(":Delete  "),
            Span::styled("t", Style::default().fg(theme.key_hint)),
            Span::raw(":Theme  "),
            Span::styled("Esc", Style::default().fg(theme.key_hint)),
            Span::raw(":Close"),
        ]));
        f.render_widget(footer, chunks[1]);
    }
}
