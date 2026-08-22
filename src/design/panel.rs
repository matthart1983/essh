//! The panel: the *Watch family's box idiom.
//!
//! ESSH used ratatui's `Block` with a faint title and a separate footer row.
//! netwatch, diskwatch and syswatch all use this instead — a rounded hairline
//! panel that carries its own metadata *in the border*:
//!
//! ```text
//! ╭─┤1├─┤ conns ├─┤ 40 · 1 selected ├───────────────────┤ sort ↓ down ├─╮
//! │                                                                     │
//! ╰─┤ ↑↓ select  p ause  ? help ├──────────────────────┤ 1–12 of 40 ├──╯
//! ```
//!
//! Three things follow from that, and they are the reason ESSH's screens
//! looked like a different program:
//!
//! 1. **Rounded corners.** Square corners read as a cage; rounded hairlines
//!    read as furniture. It is the single biggest cue that a TUI was designed
//!    after 1994.
//! 2. **Zero rows spent on chrome.** Titles, counts, sort state, keybinds and
//!    paging all live *on* rules that were going to be drawn anyway. A
//!    separate footer row costs a row of data on every screen.
//! 3. **A hotkey badge per panel.** `┤1├` means panels are addressable, so
//!    focus is a number rather than a tab cycle.
//!
//! Ported from netwatch's `ui::dense::paint::panel` and adapted to ratatui
//! 0.30, whose `Buffer` indexes by tuple rather than `get_mut`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

const TL: char = '╭';
const TR: char = '╮';
const BL: char = '╰';
const BR: char = '╯';
const H: char = '─';
const V: char = '│';

/// A keybind segment for the bottom border: `(key, rest)`, drawn as an
/// accented key followed by dim text — `q` + `uit`, `↑↓` + ` select`.
pub type Bind<'a> = (&'a str, &'a str);

/// Everything a [`panel`] carries in its own border.
#[derive(Default)]
pub struct PanelOpts<'a> {
    /// Bracketed hotkey at the top-left: the `1` in `╭─┤1├─┤ net ├`.
    pub key: Option<&'a str>,
    pub title: Option<&'a str>,
    /// Dim qualifier after the title — the host, the row count.
    pub sub: Option<&'a str>,
    /// Right-hand info on the top border.
    pub right: Option<&'a str>,
    pub right_style: Option<Style>,
    /// Keybind strip on the bottom border.
    pub foot_left: &'a [Bind<'a>],
    /// Paging / range on the bottom border.
    pub foot_right: Option<&'a str>,
    /// Draw the border in the focus accent rather than the rule colour.
    pub focused: bool,
}

/// Draw a rounded panel and return its **interior** rect.
pub fn panel(buf: &mut Buffer, area: Rect, t: &Theme, o: &PanelOpts) -> Rect {
    if area.width < 2 || area.height < 2 {
        return area;
    }
    let border = Style::default().fg(if o.focused { t.brand } else { t.border });
    let x0 = area.x;
    let y0 = area.y;
    let x1 = area.x + area.width - 1;
    let y1 = area.y + area.height - 1;

    for x in (x0 + 1)..x1 {
        set(buf, x, y0, H, border);
        set(buf, x, y1, H, border);
    }
    for y in (y0 + 1)..y1 {
        set(buf, x0, y, V, border);
        set(buf, x1, y, V, border);
    }
    set(buf, x0, y0, TL, border);
    set(buf, x1, y0, TR, border);
    set(buf, x0, y1, BL, border);
    set(buf, x1, y1, BR, border);

    // ── top border inserts ──
    let mut cx = x0 + 1;
    if o.key.is_some() || o.title.is_some() {
        // One rule cell before the first bracket, so a corner never butts an
        // insert.
        cx = put(buf, cx, y0, "─", border);
    }
    if let Some(k) = o.key {
        cx = put(buf, cx, y0, "┤", border);
        cx = put(
            buf,
            cx,
            y0,
            k,
            Style::default().fg(t.key_hint).add_modifier(Modifier::BOLD),
        );
        cx = put(buf, cx, y0, "├─", border);
    }
    if let Some(title) = o.title {
        cx = put(buf, cx, y0, "┤ ", border);
        cx = put(
            buf,
            cx,
            y0,
            title,
            Style::default().fg(t.brand).add_modifier(Modifier::BOLD),
        );
        cx = put(buf, cx, y0, " ├", border);
    }
    if let Some(sub) = o.sub {
        cx = put(buf, cx, y0, "─┤ ", border);
        cx = put(buf, cx, y0, sub, Style::default().fg(t.text_muted));
        let _ = put(buf, cx, y0, " ├", border);
    }
    if let Some(right) = o.right {
        let style = o
            .right_style
            .unwrap_or_else(|| Style::default().fg(t.text_muted));
        insert_right(buf, x0, x1, y0, right, style, border);
    }

    // ── bottom border inserts ──
    if !o.foot_left.is_empty() {
        let mut fx = x0 + 2;
        fx = put(buf, fx, y1, "┤ ", border);
        for (i, (key, rest)) in o.foot_left.iter().enumerate() {
            if i > 0 {
                fx = put(buf, fx, y1, "  ", border);
            }
            fx = put(
                buf,
                fx,
                y1,
                key,
                Style::default().fg(t.key_hint).add_modifier(Modifier::BOLD),
            );
            fx = put(buf, fx, y1, rest, Style::default().fg(t.text_muted));
        }
        let _ = put(buf, fx, y1, " ├", border);
    }
    if let Some(fr) = o.foot_right {
        insert_right(
            buf,
            x0,
            x1,
            y1,
            fr,
            Style::default().fg(t.text_muted),
            border,
        );
    }

    Rect::new(x0 + 1, y0 + 1, area.width - 2, area.height - 2)
}

/// [`panel`], for callers holding a [`Frame`] rather than a [`Buffer`].
///
/// Returns the interior rect, so the call reads the same shape as the
/// `let inner = block.inner(area); f.render_widget(block, area);` pair it
/// replaces — one line instead of three, and no borrow juggling.
pub fn pane(f: &mut ratatui::Frame, area: Rect, t: &Theme, o: &PanelOpts) -> Rect {
    panel(f.buffer_mut(), area, t, o)
}

/// `┤ text ├` ending one rule cell short of the corner at `x1`.
///
/// Silently declines rather than wrapping when the panel is too narrow to
/// hold the insert — a right-hand label colliding with the title is worse
/// than a missing one, and at these widths the title is the load-bearing half.
fn insert_right(
    buf: &mut Buffer,
    x0: u16,
    x1: u16,
    y: u16,
    text: &str,
    style: Style,
    border: Style,
) {
    let w = text.width() as u16 + 4; // ┤ + space + text + space + ├
    // `- 1` leaves a rule cell between the insert and the corner; without it
    // the bracket butts the corner and the panel reads as if it overflowed.
    if w + 2 > x1 || x1 - 1 - w <= x0 {
        return;
    }
    let x = x1 - 1 - w;
    let mut cx = put(buf, x, y, "┤ ", border);
    cx = put(buf, cx, y, text, style);
    let _ = put(buf, cx, y, " ├", border);
}

fn set(buf: &mut Buffer, x: u16, y: u16, ch: char, style: Style) {
    if x >= buf.area.right() || y >= buf.area.bottom() {
        return;
    }
    // ratatui 0.30 removed `Buffer::get_mut` in favour of tuple indexing.
    let cell = &mut buf[(x, y)];
    cell.set_char(ch);
    cell.set_style(style);
}

/// Write `s` at `(x, y)` and return the column after it.
fn put(buf: &mut Buffer, x: u16, y: u16, s: &str, style: Style) -> u16 {
    if x >= buf.area.right() || y >= buf.area.bottom() {
        return x;
    }
    let max = (buf.area.right() - x) as usize;
    buf.set_stringn(x, y, s, max, style);
    x + s.width() as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;

    fn buf(w: u16, h: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, w, h))
    }

    fn row(b: &Buffer, y: u16) -> String {
        (0..b.area.width)
            .map(|x| b[(x, y)].symbol().chars().next().unwrap_or(' '))
            .collect()
    }

    #[test]
    fn a_panel_draws_rounded_corners_and_returns_its_interior() {
        let mut b = buf(20, 5);
        let t = theme::dark();
        let inner = panel(&mut b, Rect::new(0, 0, 20, 5), &t, &PanelOpts::default());
        assert_eq!(inner, Rect::new(1, 1, 18, 3));
        // Rounded, not square: the whole point of the idiom.
        assert_eq!(b[(0, 0)].symbol(), "╭");
        assert_eq!(b[(19, 0)].symbol(), "╮");
        assert_eq!(b[(0, 4)].symbol(), "╰");
        assert_eq!(b[(19, 4)].symbol(), "╯");
    }

    #[test]
    fn the_title_and_hotkey_live_in_the_top_border() {
        let mut b = buf(48, 4);
        let t = theme::dark();
        panel(
            &mut b,
            Rect::new(0, 0, 48, 4),
            &t,
            &PanelOpts {
                key: Some("1"),
                title: Some("monitor"),
                sub: Some("web-01"),
                ..Default::default()
            },
        );
        let top = row(&b, 0);
        assert!(
            top.starts_with("╭─┤1├─┤ monitor ├─┤ web-01 ├"),
            "top border was {top:?}"
        );
    }

    #[test]
    fn keybinds_live_in_the_bottom_border_costing_no_row() {
        // The reason this matters: a separate footer row costs a row of data
        // on every screen, on every host, forever.
        let mut b = buf(40, 4);
        let t = theme::dark();
        panel(
            &mut b,
            Rect::new(0, 0, 40, 4),
            &t,
            &PanelOpts {
                foot_left: &[("↑↓", " select"), ("q", "uit")],
                ..Default::default()
            },
        );
        let bottom = row(&b, 3);
        assert!(
            bottom.starts_with("╰─┤ ↑↓ select  quit ├"),
            "bottom border was {bottom:?}"
        );
    }

    #[test]
    fn right_hand_inserts_end_one_cell_short_of_the_corner() {
        let mut b = buf(30, 3);
        let t = theme::dark();
        panel(
            &mut b,
            Rect::new(0, 0, 30, 3),
            &t,
            &PanelOpts {
                right: Some("ok"),
                ..Default::default()
            },
        );
        let top = row(&b, 0);
        assert!(top.ends_with("┤ ok ├─╮"), "top border was {top:?}");
    }

    #[test]
    fn a_narrow_panel_drops_the_right_insert_rather_than_colliding() {
        // A right label overrunning the title is worse than no label.
        let mut b = buf(10, 3);
        let t = theme::dark();
        panel(
            &mut b,
            Rect::new(0, 0, 10, 3),
            &t,
            &PanelOpts {
                title: Some("monitor"),
                right: Some("a very long status"),
                ..Default::default()
            },
        );
        let top = row(&b, 0);
        // The title is truncated by the panel width, which is correct; what
        // must not happen is the right label displacing it.
        assert!(top.starts_with("╭─┤ monit"), "the title must survive: {top:?}");
        assert!(!top.contains('├'), "no right insert should fit: {top:?}");
    }

    #[test]
    fn a_degenerate_rect_does_not_panic() {
        let mut b = buf(4, 4);
        let t = theme::dark();
        for (w, h) in [(0, 0), (1, 1), (1, 4), (4, 1)] {
            let r = Rect::new(0, 0, w, h);
            let _ = panel(&mut b, r, &t, &PanelOpts::default());
        }
    }

    #[test]
    fn focus_changes_the_border_colour_not_the_layout() {
        let t = theme::dark();
        let mut plain = buf(20, 4);
        let a = panel(&mut plain, Rect::new(0, 0, 20, 4), &t, &PanelOpts::default());
        let mut focused = buf(20, 4);
        let b2 = panel(
            &mut focused,
            Rect::new(0, 0, 20, 4),
            &t,
            &PanelOpts {
                focused: true,
                ..Default::default()
            },
        );
        assert_eq!(a, b2, "focus must not move anything");
        assert_ne!(plain[(0, 0)].style().fg, focused[(0, 0)].style().fg);
    }
}

