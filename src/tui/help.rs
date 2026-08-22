use ratatui::{
    prelude::*,
    widgets::{Clear, Paragraph, Wrap},
};

use crate::design as d;
use crate::theme::Theme;

pub fn render(f: &mut Frame, theme: &Theme, prefix: &str, scroll: u16) {
    let area = f.area();

    let key_style = Style::default().fg(theme.key_hint).bold();
    let heading_style = Style::default().fg(theme.active_tab).bold();
    let desc_style = Style::default().fg(theme.text_primary);
    let dim = Style::default().fg(theme.text_muted);
    // Shown as the prefix, because that is the binding that works in every
    // terminal on both platforms. See `prefix_hint` for why the Alt/Option
    // form is not advertised per-row.
    let p = |k: &str| format!("    {:<14}", crate::tui::prefix_hint(prefix, k));
    let session_jump_hint = p("1-9");
    let prev_next_hint = p("←/→");
    let last_session_hint = p("Tab");
    let split_hint = p("M");
    let split_resize_hint = p("[/]");
    let monitor_hint = p("m");
    let portfwd_hint = p("p");
    let files_hint = p("f");
    let detach_hint = p("d");
    let new_hint = p("n");
    let close_hint = p("w");
    let theme_hint = p("t");

    let lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("  One press, no prefix:  ", desc_style),
            Span::styled("F1", key_style),
            Span::styled(" help   ", desc_style),
            Span::styled("F2", key_style),
            Span::styled(" monitor   ", desc_style),
            Span::styled("F3", key_style),
            Span::styled(" files   ", desc_style),
            Span::styled("F4", key_style),
            Span::styled(" forwards   ", desc_style),
            Span::styled("F5", key_style),
            Span::styled(" mini   ", desc_style),
            Span::styled("F6", key_style),
            Span::styled(" detach", desc_style),
        ]),
        Line::from(vec![
            Span::styled("                         ", desc_style),
            Span::styled("F7", key_style),
            Span::styled("/", desc_style),
            Span::styled("F8", key_style),
            Span::styled(" previous / next session   ", desc_style),
            Span::styled("F9", key_style),
            Span::styled(" new session   ", desc_style),
            Span::styled("F10", key_style),
            Span::styled(" command menu", desc_style),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("  For everything else, press ", desc_style),
            Span::styled(crate::tui::prefix_label(prefix), key_style),
            Span::styled(", let go, then the key.", desc_style),
        ]),
        Line::from(Span::styled(
            format!(
                "  {}+key also works if your terminal sends {} as Meta.",
                crate::tui::meta_key_label(),
                crate::tui::meta_key_label()
            ),
            dim,
        )),
        Line::raw(""),
        Line::styled("  In a session", heading_style),
        Line::from(vec![
            Span::styled("    ?           ", key_style),
            Span::styled("Toggle this help menu", desc_style),
        ]),
        Line::from(vec![
            Span::styled(session_jump_hint, key_style),
            Span::styled("Jump to session N", desc_style),
        ]),
        Line::from(vec![
            Span::styled(prev_next_hint, key_style),
            Span::styled("Previous / next session", desc_style),
        ]),
        Line::from(vec![
            Span::styled(last_session_hint, key_style),
            Span::styled("Switch to last session", desc_style),
        ]),
        Line::from(vec![
            Span::styled(split_hint, key_style),
            Span::styled("Toggle split-pane (terminal + monitor)", desc_style),
        ]),
        Line::from(vec![
            Span::styled(split_resize_hint, key_style),
            Span::styled("Adjust split-pane width", desc_style),
        ]),
        Line::from(vec![
            Span::styled(monitor_hint, key_style),
            Span::styled("Toggle host monitor (full-screen)", desc_style),
        ]),
        Line::from(vec![
            Span::styled(portfwd_hint, key_style),
            Span::styled("Toggle port forwarding manager", desc_style),
        ]),
        Line::from(vec![
            Span::styled(files_hint, key_style),
            Span::styled("File browser (upload/download)", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    F10         ", key_style),
            Span::styled("Command palette (fuzzy search)", desc_style),
        ]),
        Line::from(vec![
            Span::styled(new_hint, key_style),
            Span::styled("Open the launcher to start another session", desc_style),
        ]),
        Line::from(vec![
            Span::styled(detach_hint, key_style),
            Span::styled("Detach to dashboard", desc_style),
        ]),
        Line::from(vec![
            Span::styled(close_hint, key_style),
            Span::styled("Close active session", desc_style),
        ]),
        Line::from(vec![
            Span::styled(theme_hint, key_style),
            Span::styled("Cycle theme", desc_style),
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
            Span::styled("    a           ", key_style),
            Span::styled("Add host (user@host[:port])", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    r           ", key_style),
            Span::styled("Refresh hosts", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    /           ", key_style),
            Span::styled("Start host search/filter", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    e           ", key_style),
            Span::styled("Edit host (Hosts) / config (Config)", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    t           ", key_style),
            Span::styled("Cycle theme", desc_style),
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
        Line::styled("  Search", heading_style),
        Line::from(vec![
            Span::styled("    type / ⌫    ", key_style),
            Span::styled("Filter hosts as you type", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Enter / Esc ", key_style),
            Span::styled("Connect first match / cancel search", desc_style),
        ]),
        Line::raw(""),
        Line::styled("  Command Palette", heading_style),
        Line::from(vec![
            Span::styled("    ↑/↓ Tab     ", key_style),
            Span::styled("Move selection (Shift+Tab moves up)", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Enter       ", key_style),
            Span::styled("Run selected action", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Esc / Ctrl+p", key_style),
            Span::styled("Close palette", desc_style),
        ]),
        Line::raw(""),
        Line::styled("  Session", heading_style),
        Line::from(vec![
            Span::styled("    (all keys)  ", key_style),
            Span::styled("Forwarded to remote shell", desc_style),
        ]),
        Line::raw(""),
        Line::styled("  File Browser", heading_style),
        Line::from(vec![
            Span::styled("    ↑/↓ Enter   ", key_style),
            Span::styled("Select and open directories", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Backspace   ", key_style),
            Span::styled("Go to parent directory", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    Tab         ", key_style),
            Span::styled("Switch local/remote pane", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    u d m Del   ", key_style),
            Span::styled("Upload, download, mkdir, delete", desc_style),
        ]),
        Line::raw(""),
        Line::styled("  Port Forwarding", heading_style),
        Line::from(vec![
            Span::styled("    ↑/↓ a d     ", key_style),
            Span::styled("Select, add, and delete forwards", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    type Enter  ", key_style),
            Span::styled("In add mode, enter a forward spec", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    ⌫ / Esc     ", key_style),
            Span::styled("Edit or cancel add-mode input", desc_style),
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
        Line::from(vec![
            Span::styled("    t           ", key_style),
            Span::styled("Cycle theme", desc_style),
        ]),
        Line::raw(""),
        Line::styled("  Config", heading_style),
        Line::from(vec![
            Span::styled("    e           ", key_style),
            Span::styled("Open ~/.essh/config.toml and reload", desc_style),
        ]),
        Line::from(vec![
            Span::styled("    notification_patterns", key_style),
            Span::styled("  Background alert regexes", desc_style),
        ]),
        Line::raw(""),
        Line::styled("                    Press ? or Esc to close", dim),
    ];

    // Size to the content, then to the screen. A fixed cap smaller than the
    // reference clips it even on a tall terminal, which is how the last
    // section became unreachable on a 60-row display.
    let popup_width = 68u16.min(area.width.saturating_sub(4));
    // Height from the *wrapped* content, so "tall enough" is measured in rows
    // that will actually be drawn rather than in source lines.
    let wanted = wrapped_height(&lines, popup_width.saturating_sub(2)) + 2;
    let popup_height = wanted.min(area.height.saturating_sub(2)).max(3);
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup = Rect::new(x, y, popup_width, popup_height);
    f.render_widget(Clear, popup);

    // How far the content can scroll before the last line is on screen.
    //
    // Measured *after* wrapping, not from `lines.len()`. A line longer than
    // the popup occupies two rows, so counting source lines under-counts the
    // height and leaves the tail permanently out of reach — with a range
    // indicator that confidently reports the wrong total.
    let body = popup.height.saturating_sub(2);
    let rendered = wrapped_height(&lines, popup.width.saturating_sub(2));
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let overflow = rendered.saturating_sub(body);
    let scroll = scroll.min(overflow);
    let more = if overflow > 0 {
        Some(format!(
            "{}–{} of {}",
            scroll + 1,
            (scroll + body).min(rendered),
            rendered
        ))
    } else {
        None
    };

    let inner = d::pane(
        f,
        popup,
        theme,
        &d::PanelOpts {
            title: Some("help"),
            focused: true,
            foot_left: &[("↑↓", " scroll"), ("?/⎋", " close")],
            foot_right: more.as_deref(),
            ..Default::default()
        },
    );
    f.render_widget(paragraph.scroll((scroll, 0)), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn screen(width: u16, height: u16, scroll: u16) -> String {
        let theme = crate::theme::dark();
        let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
        term.draw(|f| render(f, &theme, "ctrl-a", scroll)).unwrap();
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

    /// On a terminal too short for the whole reference, the tail must still be
    /// reachable — otherwise those keys do not exist as far as the user is
    /// concerned, and nothing on screen says so.
    #[test]
    fn the_end_of_the_reference_is_reachable_on_a_short_terminal() {
        let top = screen(80, 20, 0);
        assert!(top.contains("Toggle this help menu"), "help did not render");
        assert!(
            !top.contains("Press ? or Esc to close"),
            "this terminal is too tall to exercise scrolling"
        );

        let bottom = screen(80, 20, 200);
        assert!(
            bottom.contains("Press ? or Esc to close"),
            "the last line never comes into view:\n{bottom}"
        );
    }

    /// And the overlay says that there is more, rather than clipping silently.
    #[test]
    fn a_clipped_reference_shows_its_range() {
        let s = screen(80, 20, 0);
        assert!(s.contains(" of "), "no range indicator on a clipped help:\n{s}");
    }

    /// Scrolling past the end must stop at the end, not run off into blank.
    #[test]
    fn scrolling_past_the_end_clamps() {
        assert_eq!(screen(80, 20, 200), screen(80, 20, 10_000));
    }

    #[test]
    fn a_tall_terminal_needs_no_scrolling_and_says_so() {
        let s = screen(90, 90, 0);
        assert!(s.contains("Press ? or Esc to close"));
        assert!(!s.contains(" of "), "range shown when nothing is clipped:\n{s}");
    }
}

/// How many rows these lines occupy once wrapped to `width`.
///
/// `Paragraph::line_count` is unstable, and the naive `lines.len()` is wrong
/// the moment one line is longer than the popup — which is how the last
/// section of this reference became unreachable.
fn wrapped_height(lines: &[Line<'_>], width: u16) -> u16 {
    if width == 0 {
        return lines.len() as u16;
    }
    lines
        .iter()
        .map(|l| {
            let w: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
            // A blank line still occupies one row.
            (w.max(1) as u16).div_ceil(width)
        })
        .sum()
}

#[cfg(test)]
mod wrap_tests {
    use super::*;

    #[test]
    fn a_line_longer_than_the_width_counts_as_two_rows() {
        let lines = vec![Line::raw("x".repeat(30))];
        assert_eq!(wrapped_height(&lines, 20), 2);
        assert_eq!(wrapped_height(&lines, 30), 1);
    }

    #[test]
    fn a_blank_line_still_takes_a_row() {
        assert_eq!(wrapped_height(&[Line::raw("")], 20), 1);
    }
}
