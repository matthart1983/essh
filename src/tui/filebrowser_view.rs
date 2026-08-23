use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::design as d;
use crate::filetransfer::{FileBrowser, FilePaneFocus};
use crate::theme::Theme;
use crate::tui::widgets;

pub fn render(f: &mut Frame, area: Rect, browser: &FileBrowser, theme: &Theme) {
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

    render_local_pane(f, panes[0], browser, theme);
    render_remote_pane(f, panes[1], browser, theme);
    render_transfer_bar(f, chunks[1], browser, theme);
    render_footer(f, chunks[2], theme);
}

fn render_local_pane(f: &mut Frame, area: Rect, browser: &FileBrowser, theme: &Theme) {
    let is_active = browser.focus == FilePaneFocus::Local;
    let border_color = if is_active {
        theme.active_tab
    } else {
        theme.brand
    };
    let _ = border_color;
    // The path is the panel's qualifier, tail-first: when it does not fit, the
    // directory you are in matters more than the root you came from.
    let path = browser.local_path.display().to_string();
    let path = elide_left(&path, area.width.saturating_sub(24) as usize);
    let inner = d::pane(
        f,
        area,
        theme,
        &d::PanelOpts {
            key: Some("1"),
            title: Some("local"),
            sub: Some(&path),
            focused: is_active,
            foot_left: &[("↑↓", " select"), ("↵", " open"), ("⌫", " up")],
            ..Default::default()
        },
    );

    let visible_height = inner.height as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(visible_height);

    // Parent directory entry
    lines.push(Line::from(Span::styled(
        "  ..",
        Style::default().fg(theme.text_muted),
    )));

    for (i, entry) in browser.local_files.iter().enumerate() {
        if lines.len() >= visible_height {
            break;
        }
        let is_selected = i == browser.local_selected && is_active;
        let style = if is_selected {
            // Dark fill plus a caret, not an inverted bar: the row keeps its
            // own colour, so a directory still reads as a directory when it
            // is the one selected.
            Style::default()
                .fg(if entry.is_dir {
                    theme.brand
                } else {
                    theme.text_primary
                })
                .bg(theme.selection_bg)
                .bold()
        } else if entry.is_dir {
            Style::default().fg(theme.brand)
        } else {
            Style::default().fg(theme.text_primary)
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

        // Saturating: in release this subtraction wraps rather than panics,
        // and a wrapped usize as a format width asks `format!` to build a
        // string of ~2^64 spaces — the pane goes unresponsive and the process
        // grows until it is killed. A narrow pane must degrade, not hang.
        let name_width = (inner.width as usize)
            .saturating_sub(size_str.len())
            .saturating_sub(4);
        let marker = if is_selected { "▶ " } else { "  " };
        let padded = format!(
            "{marker}{:<width$}{}",
            display_name,
            size_str,
            width = name_width
        );
        lines.push(Line::from(Span::styled(padded, style)));
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

fn render_remote_pane(f: &mut Frame, area: Rect, browser: &FileBrowser, theme: &Theme) {
    let is_active = browser.focus == FilePaneFocus::Remote;
    let border_color = if is_active {
        theme.active_tab
    } else {
        theme.brand
    };
    let _ = border_color;
    let path = elide_left(&browser.remote_path, area.width.saturating_sub(26) as usize);
    let inner = d::pane(
        f,
        area,
        theme,
        &d::PanelOpts {
            key: Some("2"),
            title: Some("remote"),
            sub: Some(&path),
            focused: is_active,
            foot_left: &[("u", " upload"), ("d", " download"), ("⇥", " switch")],
            ..Default::default()
        },
    );

    let visible_height = inner.height as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(visible_height);

    // Parent directory entry
    lines.push(Line::from(Span::styled(
        "  ..",
        Style::default().fg(theme.text_muted),
    )));

    if browser.remote_files.is_empty() && browser.status_message.is_none() {
        lines.push(Line::from(Span::styled(
            "  (loading...)",
            Style::default().fg(theme.text_muted),
        )));
    }

    for (i, entry) in browser.remote_files.iter().enumerate() {
        if lines.len() >= visible_height {
            break;
        }
        let is_selected = i == browser.remote_selected && is_active;
        let style = if is_selected {
            // Dark fill plus a caret, not an inverted bar: the row keeps its
            // own colour, so a directory still reads as a directory when it
            // is the one selected.
            Style::default()
                .fg(if entry.is_dir {
                    theme.brand
                } else {
                    theme.text_primary
                })
                .bg(theme.selection_bg)
                .bold()
        } else if entry.is_dir {
            Style::default().fg(theme.brand)
        } else {
            Style::default().fg(theme.text_primary)
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

        // Saturating: in release this subtraction wraps rather than panics,
        // and a wrapped usize as a format width asks `format!` to build a
        // string of ~2^64 spaces — the pane goes unresponsive and the process
        // grows until it is killed. A narrow pane must degrade, not hang.
        let name_width = (inner.width as usize)
            .saturating_sub(size_str.len())
            .saturating_sub(4);
        let marker = if is_selected { "▶ " } else { "  " };
        let padded = format!(
            "{marker}{:<width$}{}",
            display_name,
            size_str,
            width = name_width
        );
        lines.push(Line::from(Span::styled(padded, style)));
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

fn render_transfer_bar(f: &mut Frame, area: Rect, browser: &FileBrowser, theme: &Theme) {
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
            Span::styled(" Transfer: ", Style::default().fg(theme.text_muted)),
            Span::styled(
                format!("{} {} ", dir_str, transfer.filename),
                Style::default().fg(theme.text_primary),
            ),
            Span::styled(bar, Style::default().fg(theme.status_good)),
            Span::styled(
                format!(" {:.0}%", pct),
                Style::default().fg(theme.status_warn),
            ),
            Span::styled(
                format!("  {}", size_str),
                Style::default().fg(theme.text_muted),
            ),
        ])
    } else if let Some(ref msg) = browser.status_message {
        Line::from(Span::styled(
            format!(" {}", msg),
            Style::default().fg(theme.status_warn),
        ))
    } else {
        Line::from(Span::styled(
            " Ready",
            Style::default().fg(theme.text_muted),
        ))
    };

    let paragraph = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(theme.border)),
    );
    f.render_widget(paragraph, area);
}

fn render_footer(f: &mut Frame, area: Rect, theme: &Theme) {
    let _ = theme;
    // The navigation and transfer binds live on the panel rules; these are the
    // ones that belong to the screen rather than to a pane.
    let footer = Paragraph::new(d::footer_line(
        &[
            ("m", "mkdir"),
            ("Del", "delete"),
            ("t", "theme"),
            ("⎋", "close"),
        ],
        theme,
    ));
    f.render_widget(footer, area);
}

/// Trim a path from the left, keeping the tail: `…/deploy/releases/current`.
///
/// Paths are read right-to-left — the leaf is the answer, the root is context
/// — so truncating the tail throws away the half that matters.
fn elide_left(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    let tail: String = s
        .chars()
        .rev()
        .take(max.saturating_sub(1))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// A pane too narrow for the size column must still render.
    ///
    /// The width arithmetic used to underflow, and an underflowed `usize` as a
    /// format width makes `format!` try to allocate an unbounded string —
    /// which presents as the app locking up, not as a crash.
    #[test]
    fn a_very_narrow_pane_does_not_hang() {
        let theme = crate::theme::dark();
        for width in [1u16, 2, 4, 8, 12] {
            let mut term = Terminal::new(TestBackend::new(width, 6)).unwrap();
            let mut browser = FileBrowser::new();
            // A long name plus a size string is what makes the width
            // arithmetic go negative.
            browser.local_files = vec![crate::filetransfer::LocalFileEntry {
                name: "a-file-with-a-long-name.tar.gz".into(),
                path: std::path::PathBuf::from("/tmp/a-file-with-a-long-name.tar.gz"),
                is_dir: false,
                size: 1_234_567,
            }];
            term.draw(|f| render(f, f.area(), &browser, &theme))
                .unwrap();
        }
    }

    #[test]
    fn a_path_is_elided_from_the_left_keeping_the_leaf() {
        // The leaf is the answer; the root is context.
        assert_eq!(elide_left("/a/b/c", 10), "/a/b/c");
        assert_eq!(elide_left("/very/long/path/to/here", 10), "…h/to/here");
        assert_eq!(elide_left("anything", 0), "");
    }
}
