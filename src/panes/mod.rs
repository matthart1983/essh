//! Pane layout: tabs, horizontal splits and vertical splits of *sessions*.
//!
//! §3 asks for this shape, with four live sessions on screen at once:
//!
//! ```text
//! ┌────────────────────┬────────────────────┐
//! │ prod-api-01        │ prod-api-02        │
//! ├────────────────────┼────────────────────┤
//! │ prod-db            │ bastion            │
//! └────────────────────┴────────────────────┘
//! ```
//!
//! v1's "split" showed one terminal beside the *monitor*, which is a
//! different feature. This is a binary tree of panes, each leaf holding a
//! session index.
//!
//! Two invariants worth stating, because both are easy to get wrong and
//! produce a layout that quietly loses a session:
//!
//! * **Closing a pane promotes its sibling.** A split with one child left is
//!   not a split; collapsing it keeps the tree canonical and stops empty
//!   regions accumulating.
//! * **Focus always lands on a real leaf.** After any structural change,
//!   focus is re-resolved rather than left pointing at a node that is gone.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDirection {
    /// Side by side. `Alt+s` in the spec's vocabulary.
    Vertical,
    /// Stacked.
    Horizontal,
}

impl SplitDirection {
    fn to_ratatui(self) -> Direction {
        match self {
            // A "vertical split" divides the screen with a vertical line,
            // producing side-by-side panes — which ratatui calls Horizontal
            // layout. The names are opposites; this is where that is resolved.
            SplitDirection::Vertical => Direction::Horizontal,
            SplitDirection::Horizontal => Direction::Vertical,
        }
    }
}

/// A node in the layout tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Pane {
    /// A session, by its index in the session manager.
    Leaf(usize),
    Split {
        direction: SplitDirection,
        /// Fraction of the space given to the first child, 0.1–0.9.
        ratio: f32,
        first: Box<Pane>,
        second: Box<Pane>,
    },
}

/// A pane's position on screen, for rendering and mouse hit-testing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedPane {
    pub session: usize,
    pub area: Rect,
    pub focused: bool,
}

/// The layout for one tab.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneTree {
    root: Pane,
    /// The session index that currently has focus.
    focus: usize,
}

impl PaneTree {
    pub fn new(session: usize) -> Self {
        Self {
            root: Pane::Leaf(session),
            focus: session,
        }
    }

    pub fn focus(&self) -> usize {
        self.focus
    }

    pub fn set_focus(&mut self, session: usize) {
        if self.contains(session) {
            self.focus = session;
        }
    }

    pub fn contains(&self, session: usize) -> bool {
        self.sessions().contains(&session)
    }

    /// Every session in the tree, left-to-right, top-to-bottom.
    pub fn sessions(&self) -> Vec<usize> {
        fn walk(p: &Pane, out: &mut Vec<usize>) {
            match p {
                Pane::Leaf(s) => out.push(*s),
                Pane::Split { first, second, .. } => {
                    walk(first, out);
                    walk(second, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }

    pub fn len(&self) -> usize {
        self.sessions().len()
    }

    pub fn is_single(&self) -> bool {
        matches!(self.root, Pane::Leaf(_))
    }

    /// Split the focused pane, putting `new_session` in the new half and
    /// moving focus to it.
    pub fn split(&mut self, direction: SplitDirection, new_session: usize) {
        let focus = self.focus;
        Self::split_at(&mut self.root, focus, direction, new_session);
        self.focus = new_session;
    }

    fn split_at(pane: &mut Pane, target: usize, direction: SplitDirection, new_session: usize) {
        match pane {
            Pane::Leaf(s) if *s == target => {
                let existing = Pane::Leaf(*s);
                *pane = Pane::Split {
                    direction,
                    ratio: 0.5,
                    first: Box::new(existing),
                    second: Box::new(Pane::Leaf(new_session)),
                };
            }
            Pane::Leaf(_) => {}
            Pane::Split { first, second, .. } => {
                Self::split_at(first, target, direction, new_session);
                Self::split_at(second, target, direction, new_session);
            }
        }
    }

    /// Remove a session's pane. Returns false if it was the last one, since
    /// the caller must then close the tab rather than leave an empty tree.
    pub fn close(&mut self, session: usize) -> bool {
        if self.is_single() {
            return false;
        }
        Self::close_in(&mut self.root, session);
        // Focus may have pointed at the pane just removed.
        if !self.contains(self.focus) {
            self.focus = self.sessions().first().copied().unwrap_or(0);
        }
        true
    }

    fn close_in(pane: &mut Pane, target: usize) {
        if let Pane::Split { first, second, .. } = pane {
            // A split whose child is the target collapses to the sibling.
            // Leaving a one-child split behind would accumulate invisible
            // structure and dead space.
            if matches!(**first, Pane::Leaf(s) if s == target) {
                *pane = (**second).clone();
                return;
            }
            if matches!(**second, Pane::Leaf(s) if s == target) {
                *pane = (**first).clone();
                return;
            }
            Self::close_in(first, target);
            Self::close_in(second, target);
        }
    }

    /// Renumber sessions after one is removed from the session manager.
    ///
    /// Session indices shift when a session is closed, and a tree holding
    /// stale indices would render the wrong terminal into a pane — which
    /// looks exactly like data corruption.
    pub fn reindex_after_removal(&mut self, removed: usize) {
        fn walk(p: &mut Pane, removed: usize) {
            match p {
                Pane::Leaf(s) => {
                    if *s > removed {
                        *s -= 1;
                    }
                }
                Pane::Split { first, second, .. } => {
                    walk(first, removed);
                    walk(second, removed);
                }
            }
        }
        walk(&mut self.root, removed);
        if self.focus > removed {
            self.focus -= 1;
        }
    }

    /// Move focus to the next pane in reading order, wrapping.
    pub fn focus_next(&mut self) {
        let sessions = self.sessions();
        if sessions.is_empty() {
            return;
        }
        let pos = sessions.iter().position(|s| *s == self.focus).unwrap_or(0);
        self.focus = sessions[(pos + 1) % sessions.len()];
    }

    pub fn focus_prev(&mut self) {
        let sessions = self.sessions();
        if sessions.is_empty() {
            return;
        }
        let pos = sessions.iter().position(|s| *s == self.focus).unwrap_or(0);
        self.focus = sessions[(pos + sessions.len() - 1) % sessions.len()];
    }

    /// Resize the split containing the focused pane.
    pub fn resize_focused(&mut self, delta: f32) {
        let focus = self.focus;
        Self::resize_in(&mut self.root, focus, delta);
    }

    fn resize_in(pane: &mut Pane, target: usize, delta: f32) -> bool {
        match pane {
            Pane::Leaf(s) => *s == target,
            Pane::Split {
                ratio,
                first,
                second,
                ..
            } => {
                if Self::resize_in(first, target, delta) {
                    *ratio = (*ratio + delta).clamp(0.1, 0.9);
                    return true;
                }
                if Self::resize_in(second, target, delta) {
                    *ratio = (*ratio - delta).clamp(0.1, 0.9);
                    return true;
                }
                false
            }
        }
    }

    /// Compute screen rectangles for every pane.
    pub fn layout(&self, area: Rect) -> Vec<PlacedPane> {
        let mut out = Vec::new();
        self.place(&self.root, area, &mut out);
        out
    }

    fn place(&self, pane: &Pane, area: Rect, out: &mut Vec<PlacedPane>) {
        match pane {
            Pane::Leaf(s) => out.push(PlacedPane {
                session: *s,
                area,
                focused: *s == self.focus,
            }),
            Pane::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let pct = (ratio * 100.0).round().clamp(10.0, 90.0) as u16;
                let chunks = Layout::default()
                    .direction(direction.to_ratatui())
                    .constraints([
                        Constraint::Percentage(pct),
                        Constraint::Percentage(100 - pct),
                    ])
                    .split(area);
                self.place(first, chunks[0], out);
                self.place(second, chunks[1], out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect::new(0, 0, 100, 40)
    }

    #[test]
    fn a_new_tree_is_one_pane_holding_one_session() {
        let t = PaneTree::new(0);
        assert!(t.is_single());
        assert_eq!(t.len(), 1);
        assert_eq!(t.focus(), 0);
        let placed = t.layout(area());
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].area, area());
        assert!(placed[0].focused);
    }

    #[test]
    fn splitting_produces_the_layout_from_the_spec() {
        // The §3 diagram: four sessions, two over two.
        let mut t = PaneTree::new(0);
        t.split(SplitDirection::Vertical, 1); // side by side
        t.set_focus(0);
        t.split(SplitDirection::Horizontal, 2); // stack the left column
        t.set_focus(1);
        t.split(SplitDirection::Horizontal, 3); // stack the right column

        assert_eq!(t.len(), 4);
        let placed = t.layout(area());
        assert_eq!(placed.len(), 4);

        // Every pane has real area and none overlap in total coverage.
        let total: u32 = placed
            .iter()
            .map(|p| p.area.width as u32 * p.area.height as u32)
            .sum();
        assert_eq!(total, 100 * 40, "panes must tile the area exactly");
        for p in &placed {
            assert!(p.area.width > 0 && p.area.height > 0);
        }
    }

    #[test]
    fn a_vertical_split_puts_panes_side_by_side() {
        let mut t = PaneTree::new(0);
        t.split(SplitDirection::Vertical, 1);
        let placed = t.layout(area());
        assert_eq!(placed.len(), 2);
        // Same top edge, different left edge.
        assert_eq!(placed[0].area.y, placed[1].area.y);
        assert_ne!(placed[0].area.x, placed[1].area.x);
    }

    #[test]
    fn a_horizontal_split_stacks_panes() {
        let mut t = PaneTree::new(0);
        t.split(SplitDirection::Horizontal, 1);
        let placed = t.layout(area());
        assert_eq!(placed[0].area.x, placed[1].area.x);
        assert_ne!(placed[0].area.y, placed[1].area.y);
    }

    #[test]
    fn splitting_focuses_the_new_pane() {
        let mut t = PaneTree::new(0);
        t.split(SplitDirection::Vertical, 7);
        assert_eq!(t.focus(), 7);
        let placed = t.layout(area());
        assert!(placed.iter().find(|p| p.session == 7).unwrap().focused);
        assert!(!placed.iter().find(|p| p.session == 0).unwrap().focused);
    }

    #[test]
    fn closing_a_pane_promotes_its_sibling_rather_than_leaving_a_gap() {
        let mut t = PaneTree::new(0);
        t.split(SplitDirection::Vertical, 1);
        assert!(t.close(1));
        assert!(t.is_single(), "a one-child split must collapse");
        assert_eq!(t.sessions(), vec![0]);
        // And the survivor gets the whole area back.
        assert_eq!(t.layout(area())[0].area, area());
    }

    #[test]
    fn closing_the_focused_pane_moves_focus_to_a_real_one() {
        let mut t = PaneTree::new(0);
        t.split(SplitDirection::Vertical, 1);
        assert_eq!(t.focus(), 1);
        t.close(1);
        assert_eq!(t.focus(), 0, "focus must not dangle");
        assert!(t.contains(t.focus()));
    }

    #[test]
    fn closing_the_last_pane_is_refused_so_the_caller_closes_the_tab() {
        let mut t = PaneTree::new(0);
        assert!(!t.close(0));
        assert_eq!(t.len(), 1, "the tree must never become empty");
    }

    #[test]
    fn closing_a_deeply_nested_pane_collapses_only_its_own_split() {
        let mut t = PaneTree::new(0);
        t.split(SplitDirection::Vertical, 1);
        t.split(SplitDirection::Horizontal, 2);
        assert_eq!(t.len(), 3);
        t.close(2);
        assert_eq!(t.sessions(), vec![0, 1]);
        assert_eq!(t.layout(area()).len(), 2);
    }

    #[test]
    fn focus_cycles_through_every_pane_and_wraps() {
        let mut t = PaneTree::new(0);
        t.split(SplitDirection::Vertical, 1);
        t.split(SplitDirection::Horizontal, 2);
        let order = t.sessions();
        t.set_focus(order[0]);
        for expected in order.iter().skip(1) {
            t.focus_next();
            assert_eq!(t.focus(), *expected);
        }
        t.focus_next();
        assert_eq!(t.focus(), order[0], "focus wraps");
        t.focus_prev();
        assert_eq!(t.focus(), *order.last().unwrap());
    }

    #[test]
    fn resizing_moves_the_boundary_and_stays_within_bounds() {
        let mut t = PaneTree::new(0);
        t.split(SplitDirection::Vertical, 1);
        let before = t.layout(area())[0].area.width;
        t.set_focus(0);
        t.resize_focused(0.1);
        let after = t.layout(area())[0].area.width;
        assert!(after > before, "{} !> {}", after, before);

        // Clamped: a pane can never be resized out of existence.
        for _ in 0..50 {
            t.resize_focused(0.1);
        }
        let placed = t.layout(area());
        assert!(placed.iter().all(|p| p.area.width >= 5));
    }

    #[test]
    fn indices_are_renumbered_when_a_session_is_removed() {
        // Session 1 closing shifts 2 and 3 down. Without this, a pane would
        // render the wrong session's terminal.
        let mut t = PaneTree::new(0);
        t.split(SplitDirection::Vertical, 1);
        t.split(SplitDirection::Horizontal, 2);
        t.set_focus(0);
        t.split(SplitDirection::Horizontal, 3);
        assert_eq!(t.sessions().len(), 4);

        t.close(1);
        t.reindex_after_removal(1);
        let mut remaining = t.sessions();
        remaining.sort();
        assert_eq!(remaining, vec![0, 1, 2], "2 and 3 shift down to 1 and 2");
    }

    #[test]
    fn reindexing_moves_focus_with_it() {
        let mut t = PaneTree::new(0);
        t.split(SplitDirection::Vertical, 3);
        assert_eq!(t.focus(), 3);
        t.reindex_after_removal(1);
        assert_eq!(t.focus(), 2);
        assert!(t.contains(2));
    }

    #[test]
    fn set_focus_ignores_a_session_not_in_this_tree() {
        let mut t = PaneTree::new(0);
        t.set_focus(42);
        assert_eq!(t.focus(), 0, "focus must stay on a real pane");
    }

    #[test]
    fn panes_tile_without_gaps_at_awkward_sizes() {
        // Odd dimensions are where percentage layouts drop a row or column.
        let mut t = PaneTree::new(0);
        t.split(SplitDirection::Vertical, 1);
        t.split(SplitDirection::Horizontal, 2);
        for (w, h) in [(81u16, 23u16), (37, 11), (13, 5)] {
            let a = Rect::new(0, 0, w, h);
            let placed = t.layout(a);
            let total: u32 = placed
                .iter()
                .map(|p| p.area.width as u32 * p.area.height as u32)
                .sum();
            assert_eq!(total, w as u32 * h as u32, "gap at {}x{}", w, h);
        }
    }
}
