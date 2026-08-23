use ratatui::{
    prelude::*,
    widgets::{Block, Clear, Paragraph},
};

use crate::design as d;
use crate::theme::Theme;
use crate::tui::meta_key_hint;

use super::{AppView, DashboardTab, HostDisplay, HostStatus};
use crate::session::Session;

// ---------------------------------------------------------------------------
// Palette action — what happens when you select an entry
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum PaletteAction {
    ConnectHost(usize),   // index into app.hosts
    SwitchSession(usize), // index into session_manager.sessions
    SetView(AppView),
    SetDashboardTab(DashboardTab),
    ToggleSplitPane,
    ToggleHelp,
    /// Apply a named theme and persist it. One entry per theme rather than a
    /// single "cycle": the palette exists to go straight to a thing, and
    /// cycling to `sky` means pressing `t` seven times.
    SetTheme(&'static str),
}

// ---------------------------------------------------------------------------
// Palette entry — one row in the list
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct PaletteEntry {
    pub icon: &'static str,
    pub label: String,
    pub detail: String,
    pub action: PaletteAction,
    pub score: i32, // higher = better match
}

// ---------------------------------------------------------------------------
// Command palette state
// ---------------------------------------------------------------------------

pub struct CommandPalette {
    pub query: String,
    pub entries: Vec<PaletteEntry>,
    pub selected: usize,
}

impl CommandPalette {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            entries: Vec::new(),
            selected: 0,
        }
    }

    /// Rebuild the entry list based on current query, hosts, and sessions.
    pub fn update(
        &mut self,
        hosts: &[HostDisplay],
        sessions: &[Session],
        has_sessions: bool,
        active_theme: &str,
    ) {
        let split_hint = meta_key_hint("s");
        let monitor_hint = meta_key_hint("m");
        let portfwd_hint = meta_key_hint("p");
        let files_hint = meta_key_hint("f");
        let mut entries = Vec::new();

        // Hosts — "connect <name>"
        for (i, host) in hosts.iter().enumerate() {
            let status = match host.status {
                HostStatus::Online => "●",
                HostStatus::Offline => "○",
                HostStatus::NeverProbed => "?",
            };
            entries.push(PaletteEntry {
                icon: "›_",
                label: format!(
                    "Connect: {}",
                    if host.name.is_empty() {
                        &host.hostname
                    } else {
                        &host.name
                    }
                ),
                detail: format!(
                    "{} {}@{}:{} {}",
                    status, host.user, host.hostname, host.port, host.tags
                ),
                action: PaletteAction::ConnectHost(i),
                score: 0,
            });
        }

        // Active sessions — "switch to <label>"
        for (i, session) in sessions.iter().enumerate() {
            entries.push(PaletteEntry {
                icon: "›_",
                label: format!("Session {}: {}", i + 1, session.label),
                detail: format!(
                    "{}@{}:{} — {}",
                    session.username, session.hostname, session.port, session.state
                ),
                action: PaletteAction::SwitchSession(i),
                score: 0,
            });
        }

        // Navigation commands
        entries.push(PaletteEntry {
            icon: "◇",
            label: "Dashboard: Sessions".to_string(),
            detail: "View active sessions".to_string(),
            action: PaletteAction::SetDashboardTab(DashboardTab::Sessions),
            score: 0,
        });
        entries.push(PaletteEntry {
            icon: "◇",
            label: "Dashboard: Hosts".to_string(),
            detail: "Browse and connect to hosts".to_string(),
            action: PaletteAction::SetDashboardTab(DashboardTab::Hosts),
            score: 0,
        });
        entries.push(PaletteEntry {
            icon: "◇",
            label: "Dashboard: Fleet".to_string(),
            detail: "Fleet health overview".to_string(),
            action: PaletteAction::SetDashboardTab(DashboardTab::Fleet),
            score: 0,
        });
        entries.push(PaletteEntry {
            icon: "◇",
            label: "Dashboard: Config".to_string(),
            detail: "Configuration overview".to_string(),
            action: PaletteAction::SetDashboardTab(DashboardTab::Config),
            score: 0,
        });

        if has_sessions {
            entries.push(PaletteEntry {
                icon: "▦",
                label: "Toggle: Split Pane".to_string(),
                detail: format!("Terminal + monitor side-by-side ({})", split_hint),
                action: PaletteAction::ToggleSplitPane,
                score: 0,
            });
            entries.push(PaletteEntry {
                icon: "▦",
                label: "View: Host Monitor".to_string(),
                detail: format!("Full-screen host metrics ({})", monitor_hint),
                action: PaletteAction::SetView(AppView::Monitor),
                score: 0,
            });
            entries.push(PaletteEntry {
                icon: "⇄",
                label: "View: Port Forwarding".to_string(),
                detail: format!("Manage port forwards ({})", portfwd_hint),
                action: PaletteAction::SetView(AppView::PortForwarding),
                score: 0,
            });
            entries.push(PaletteEntry {
                icon: "▤",
                label: "View: File Browser".to_string(),
                detail: format!("Upload/download files ({})", files_hint),
                action: PaletteAction::SetView(AppView::FileBrowser),
                score: 0,
            });
        }

        for name in crate::theme::THEME_NAMES {
            entries.push(PaletteEntry {
                icon: "◐",
                label: format!("Theme: {name}"),
                detail: if *name == active_theme {
                    "active".to_string()
                } else if *name == "terminal" {
                    "Use the terminal's own palette".to_string()
                } else {
                    "Apply and save".to_string()
                },
                action: PaletteAction::SetTheme(name),
                score: 0,
            });
        }

        entries.push(PaletteEntry {
            icon: "?",
            label: "Help".to_string(),
            detail: "Show keyboard shortcuts (?)".to_string(),
            action: PaletteAction::ToggleHelp,
            score: 0,
        });

        // Score and filter by query
        if !self.query.is_empty() {
            let q = self.query.to_lowercase();
            for entry in &mut entries {
                entry.score = fuzzy_score(&q, &entry.label, &entry.detail);
            }
            entries.retain(|e| e.score > 0);
            entries.sort_by_key(|e| std::cmp::Reverse(e.score));
        }

        self.entries = entries;
        // Clamp selection
        if self.selected >= self.entries.len() {
            self.selected = 0;
        }
    }

    pub fn move_down(&mut self) {
        if !self.entries.is_empty() {
            self.selected = (self.selected + 1) % self.entries.len();
        }
    }

    pub fn move_up(&mut self) {
        if !self.entries.is_empty() {
            if self.selected == 0 {
                self.selected = self.entries.len() - 1;
            } else {
                self.selected -= 1;
            }
        }
    }

    pub fn selected_action(&self) -> Option<&PaletteAction> {
        self.entries.get(self.selected).map(|e| &e.action)
    }
}

// ---------------------------------------------------------------------------
// Fuzzy scoring — simple substring matching with bonus for prefix/word starts
// ---------------------------------------------------------------------------

fn fuzzy_score(query: &str, label: &str, detail: &str) -> i32 {
    let label_lower = label.to_lowercase();
    let detail_lower = detail.to_lowercase();

    let mut score = 0i32;

    // Check each query word independently
    for word in query.split_whitespace() {
        let mut word_matched = false;

        if let Some(pos) = label_lower.find(word) {
            score += 10;
            if pos == 0 {
                score += 5; // prefix bonus
            }
            // Bonus for matching at word boundary
            if pos > 0 && !label.as_bytes()[pos - 1].is_ascii_alphanumeric() {
                score += 3;
            }
            word_matched = true;
        }

        if detail_lower.find(word).is_some() {
            score += 3;
            word_matched = true;
        }

        if !word_matched {
            return 0; // all query words must match somewhere
        }
    }

    score
}

// ---------------------------------------------------------------------------
// Rendering — centered overlay popup
// ---------------------------------------------------------------------------

/// Screen 6 — the command palette.
///
/// A floating surface on a scrim, not a bordered box with a title on the
/// rule: *"No box-drawing needed when you can composite."* A terminal cannot
/// blur, but it can dim the backdrop and lift the card off it with its own
/// background, which is the part that carries the effect.
pub fn render(frame: &mut Frame, palette: &CommandPalette, theme: &Theme) {
    let _ = theme;
    let area = frame.area();

    // ── the scrim: dim everything behind the card
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.bg).fg(theme.border)),
        area,
    );

    let width = 66u16.min(area.width.saturating_sub(6));
    let max_visible = 8usize;
    let rows = max_visible.min(palette.entries.len().max(1)) as u16;
    let height = (rows + 3).min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2;
    // The design floats the card slightly above centre.
    let y = (area.height.saturating_sub(height)) * 46 / 100;
    let card = Rect::new(x, y, width, height);

    frame.render_widget(Clear, card);
    // The card's own surface, a shade above the background so it reads as
    // lifted rather than as a hole.
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.separator).fg(theme.text_primary)),
        card,
    );

    let inner = Rect {
        x: card.x + 2,
        y: card.y + 1,
        width: card.width.saturating_sub(4),
        height: card.height.saturating_sub(2),
    };
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // ── query line: `› host ▌`
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("› ", Style::default().fg(theme.brand)),
            Span::styled(
                palette.query.clone(),
                Style::default()
                    .fg(theme.text_primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("▌", Style::default().fg(theme.brand)),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    if palette.entries.is_empty() {
        frame.render_widget(
            Paragraph::new(d::none_line("nothing matches", theme)),
            Rect::new(inner.x, inner.y + 2, inner.width, 1),
        );
        return;
    }

    let list_top = inner.y + 2;
    let list_height = inner.height.saturating_sub(2) as usize;
    let scroll = palette
        .selected
        .saturating_sub(list_height.saturating_sub(1));

    for (vi, ei) in (scroll..scroll + list_height.min(palette.entries.len())).enumerate() {
        let Some(entry) = palette.entries.get(ei) else {
            break;
        };
        let row_y = list_top + vi as u16;
        if row_y >= inner.y + inner.height {
            break;
        }
        let row = Rect::new(inner.x, row_y, inner.width, 1);
        let selected = ei == palette.selected;

        if selected {
            frame.render_widget(
                Block::default().style(Style::default().bg(theme.selection_bg)),
                row,
            );
        }

        let label_style = if selected {
            Style::default()
                .fg(theme.text_emphasis)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_primary)
        };

        // Results carry consequence — "3 facets differ", "10 · 2 reachable" —
        // so a choice is made by outcome rather than by guessing the verb.
        let left = format!("{} {}", entry.icon, entry.label);
        let hint = entry.detail.clone();
        let gap = (inner.width as usize)
            .saturating_sub(left.chars().count() + hint.chars().count() + 1)
            .max(1);

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("{} ", entry.icon),
                    Style::default().fg(if selected {
                        theme.brand
                    } else {
                        theme.text_secondary
                    }),
                ),
                Span::styled(entry.label.clone(), label_style),
                Span::raw(" ".repeat(gap)),
                Span::styled(hint, Style::default().fg(theme.border)),
            ])),
            row,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_score_exact_match() {
        assert!(fuzzy_score("connect", "Connect: bastion", "ops@bastion:22") > 0);
    }

    #[test]
    fn test_fuzzy_score_no_match() {
        assert_eq!(
            fuzzy_score("zzzzz", "Connect: bastion", "ops@bastion:22"),
            0
        );
    }

    #[test]
    fn test_fuzzy_score_multi_word() {
        let s = fuzzy_score("connect bastion", "Connect: bastion-east", "ops@bastion:22");
        assert!(s > 0);
    }

    #[test]
    fn test_fuzzy_score_multi_word_no_match() {
        assert_eq!(
            fuzzy_score("connect zzz", "Connect: bastion", "ops@bastion:22"),
            0
        );
    }

    #[test]
    fn test_fuzzy_score_prefix_bonus() {
        let prefix = fuzzy_score("con", "Connect: bastion", "");
        let mid = fuzzy_score("bas", "Connect: bastion", "");
        assert!(prefix > mid);
    }

    #[test]
    fn test_fuzzy_score_detail_match() {
        let s = fuzzy_score("prod", "Connect: bastion", "env=prod");
        assert!(s > 0);
    }

    #[test]
    fn test_palette_update_no_query() {
        let mut p = CommandPalette::new();
        p.update(&[], &[], false, "dark");
        // Should have at least the navigation + help entries
        assert!(p.entries.len() >= 5);
    }

    #[test]
    fn test_palette_update_filters() {
        let mut p = CommandPalette::new();
        p.query = "help".to_string();
        p.update(&[], &[], false, "dark");
        assert!(p.entries.iter().any(|e| e.label.contains("Help")));
    }

    #[test]
    fn test_palette_move_down_wraps() {
        let mut p = CommandPalette::new();
        p.update(&[], &[], false, "dark");
        let len = p.entries.len();
        for _ in 0..len {
            p.move_down();
        }
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn test_palette_move_up_wraps() {
        let mut p = CommandPalette::new();
        p.update(&[], &[], false, "dark");
        p.move_up();
        assert_eq!(p.selected, p.entries.len() - 1);
    }

    #[test]
    fn every_theme_is_reachable_from_the_palette() {
        // `t` cycles, which means reaching `sky` from `dark` is seven presses.
        // The palette is how you go straight there, so every theme has to have
        // an entry — including any added later.
        let mut p = CommandPalette::new();
        p.update(&[], &[], false, "dark");
        for name in crate::theme::THEME_NAMES {
            let want = format!("Theme: {name}");
            assert!(
                p.entries.iter().any(|e| e.label == want),
                "{name} has no palette entry"
            );
        }
    }

    #[test]
    fn the_active_theme_says_so_and_typing_its_name_finds_it() {
        let mut p = CommandPalette::new();
        p.update(&[], &[], false, "dracula");
        let active: Vec<&PaletteEntry> = p
            .entries
            .iter()
            .filter(|e| e.label.starts_with("Theme: ") && e.detail == "active")
            .collect();
        assert_eq!(active.len(), 1, "exactly one theme is active");
        assert_eq!(active[0].label, "Theme: dracula");

        // Fuzzy search has to actually land on it.
        p.query = "drac".to_string();
        p.update(&[], &[], false, "dark");
        assert_eq!(
            p.entries.first().map(|e| e.label.as_str()),
            Some("Theme: dracula"),
            "typing a theme name does not surface it first"
        );
    }
}
