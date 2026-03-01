use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::filetransfer::{FileBrowser, FilePaneFocus};
use crate::tui::widgets;

pub fn render(f: &mut Frame, area: Rect, browser: &FileBrowser) {
    f.render_widget(Clear, area);

    // Main layout: panes area + transfer bar + footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(6),    // two-pane file listing
            Constraint::Length(2), // transfer progress
            Constraint::Length(2), // footer keybindings
        ])
        .split(area);

    // Split panes horizontally
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[0]);

    render_local_pane(f, panes[0], browser);
    render_remote_pane(f, panes[1], browser);
    render_transfer_bar(f, chunks[1], browser);
    render_footer(f, chunks[2]);
}

fn render_local_pane(f: &mut Frame, area: Rect, browser: &FileBrowser) {
    let is_active = browser.focus == FilePaneFocus::Local;
    let border_color = if is_active { Color::Yellow } else { Color::Cyan };
    let title = format!(" Local: {} ", browser.local_path.display());

    let block = Block::default()
        .title(title)
        .title_style(Style::default().fg(Color::Cyan).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let visible_height = inner.height as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(visible_height);

    // Parent directory entry
    lines.push(Line::from(Span::styled(
        "  ..",
        Style::default().fg(Color::DarkGray),
    )));

    for (i, entry) in browser.local_files.iter().enumerate() {
        if lines.len() >= visible_height {
            break;
        }
        let is_selected = i == browser.local_selected && is_active;
        let style = if is_selected {
            Style::default().fg(Color::Black).bg(Color::Yellow).bold()
        } else if entry.is_dir {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::White)
        };

        let display_name = if entry.is_dir {
            format!("{}/", entry.name)
        } else {
            entry.name.clone()
        };

        let size_str = if entry.is_dir {
            String::new()
        } else {
            widgets::format_bytes(entry.size)
        };

        let name_width = inner.width as usize - size_str.len() - 4;
        let padded = format!(
            "  {:<width$}{}",
            display_name,
            size_str,
            width = name_width
        );
        lines.push(Line::from(Span::styled(padded, style)));
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

fn render_remote_pane(f: &mut Frame, area: Rect, browser: &FileBrowser) {
    let is_active = browser.focus == FilePaneFocus::Remote;
    let border_color = if is_active { Color::Yellow } else { Color::Cyan };
    let title = format!(" Remote: {} ", browser.remote_path);

    let block = Block::default()
        .title(title)
        .title_style(Style::default().fg(Color::Cyan).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let visible_height = inner.height as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(visible_height);

    // Parent directory entry
    lines.push(Line::from(Span::styled(
        "  ..",
        Style::default().fg(Color::DarkGray),
    )));

    if browser.remote_files.is_empty() && browser.status_message.is_none() {
        lines.push(Line::from(Span::styled(
            "  (loading...)",
            Style::default().fg(Color::DarkGray),
        )));
    }

    for (i, entry) in browser.remote_files.iter().enumerate() {
        if lines.len() >= visible_height {
            break;
        }
        let is_selected = i == browser.remote_selected && is_active;
        let style = if is_selected {
            Style::default().fg(Color::Black).bg(Color::Yellow).bold()
        } else if entry.is_dir {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::White)
        };

        let display_name = if entry.is_dir {
            format!("{}/", entry.name)
        } else {
            entry.name.clone()
        };

        let size_str = if entry.is_dir {
            String::new()
        } else {
            widgets::format_bytes(entry.size)
        };

        let name_width = inner.width as usize - size_str.len() - 4;
        let padded = format!(
            "  {:<width$}{}",
            display_name,
            size_str,
            width = name_width
        );
        lines.push(Line::from(Span::styled(padded, style)));
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

fn render_transfer_bar(f: &mut Frame, area: Rect, browser: &FileBrowser) {
    let line = if let Some(ref transfer) = browser.transfer {
        let pct = transfer.percent();
        let dir_str = match transfer.direction {
            crate::filetransfer::TransferDirection::Upload => "uploading",
            crate::filetransfer::TransferDirection::Download => "downloading",
        };
        let bar_width = area.width as usize - 40;
        let bar = widgets::bar_gauge(pct, bar_width.max(5));
        let size_str = widgets::format_bytes(transfer.total_bytes);
        Line::from(vec![
            Span::styled(" Transfer: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{} {} ", dir_str, transfer.filename),
                Style::default().fg(Color::White),
            ),
            Span::styled(bar, Style::default().fg(Color::Green)),
            Span::styled(format!(" {:.0}%", pct), Style::default().fg(Color::Yellow)),
            Span::styled(format!("  {}", size_str), Style::default().fg(Color::DarkGray)),
        ])
    } else if let Some(ref msg) = browser.status_message {
        Line::from(Span::styled(
            format!(" {}", msg),
            Style::default().fg(Color::Yellow),
        ))
    } else {
        Line::from(Span::styled(
            " Ready",
            Style::default().fg(Color::DarkGray),
        ))
    };

    let paragraph = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(paragraph, area);
}

fn render_footer(f: &mut Frame, area: Rect) {
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(" Tab", Style::default().fg(Color::Cyan)),
        Span::raw(":Switch  "),
        Span::styled("u", Style::default().fg(Color::Cyan)),
        Span::raw(":Upload  "),
        Span::styled("d", Style::default().fg(Color::Cyan)),
        Span::raw(":Download  "),
        Span::styled("m", Style::default().fg(Color::Cyan)),
        Span::raw(":Mkdir  "),
        Span::styled("Del", Style::default().fg(Color::Cyan)),
        Span::raw(":Delete  "),
        Span::styled("Esc", Style::default().fg(Color::Cyan)),
        Span::raw(":Close"),
    ]))
    .block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(footer, area);
}
