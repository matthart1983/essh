use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph},
};

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
    pub fn update(&mut self, hosts: &[HostDisplay], sessions: &[Session], has_sessions: bool) {
        let mut entries = Vec::new();

        // Hosts — "connect <name>"
        for (i, host) in hosts.iter().enumerate() {
            let status = match host.status {
                HostStatus::Online => "●",
                HostStatus::Offline => "○",
                HostStatus::Unknown => "?",
            };
            entries.push(PaletteEntry {
                icon: "🖥",
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
                icon: "⚡",
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
            icon: "📋",
            label: "Dashboard: Sessions".to_string(),
            detail: "View active sessions".to_string(),
            action: PaletteAction::SetDashboardTab(DashboardTab::Sessions),
            score: 0,
        });
        entries.push(PaletteEntry {
            icon: "📋",
            label: "Dashboard: Hosts".to_string(),
            detail: "Browse and connect to hosts".to_string(),
            action: PaletteAction::SetDashboardTab(DashboardTab::Hosts),
            score: 0,
        });
        entries.push(PaletteEntry {
            icon: "📋",
            label: "Dashboard: Fleet".to_string(),
            detail: "Fleet health overview".to_string(),
            action: PaletteAction::SetDashboardTab(DashboardTab::Fleet),
            score: 0,
        });
        entries.push(PaletteEntry {
            icon: "📋",
            label: "Dashboard: Config".to_string(),
            detail: "Configuration overview".to_string(),
            action: PaletteAction::SetDashboardTab(DashboardTab::Config),
            score: 0,
        });

        if has_sessions {
            entries.push(PaletteEntry {
                icon: "🔲",
                label: "Toggle: Split Pane".to_string(),
                detail: "Terminal + monitor side-by-side (Alt+s)".to_string(),
                action: PaletteAction::ToggleSplitPane,
                score: 0,
            });
            entries.push(PaletteEntry {
                icon: "📊",
                label: "View: Host Monitor".to_string(),
                detail: "Full-screen host metrics (Alt+m)".to_string(),
                action: PaletteAction::SetView(AppView::Monitor),
                score: 0,
            });
            entries.push(PaletteEntry {
                icon: "🔀",
                label: "View: Port Forwarding".to_string(),
                detail: "Manage port forwards (Alt+p)".to_string(),
                action: PaletteAction::SetView(AppView::PortForwarding),
                score: 0,
            });
            entries.push(PaletteEntry {
                icon: "📁",
                label: "View: File Browser".to_string(),
                detail: "Upload/download files (Alt+f)".to_string(),
                action: PaletteAction::SetView(AppView::FileBrowser),
                score: 0,
            });
        }

        entries.push(PaletteEntry {
            icon: "❓",
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
            entries.sort_by(|a, b| b.score.cmp(&a.score));
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

pub fn render(frame: &mut Frame, palette: &CommandPalette) {
    let area = frame.area();

    let popup_width = 70u16.min(area.width.saturating_sub(4));
    let max_visible = 12usize;
    // 3 = border top + input line + border bottom, +1 per entry
    let popup_height = (3 + max_visible.min(palette.entries.len().max(1)) as u16 + 1)
        .min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = area.height / 6; // bias towards top
    let popup = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Command Palette ")
        .title_style(Style::default().fg(Color::Cyan).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Input line
    let input_area = Rect::new(inner.x, inner.y, inner.width, 1);
    let input_line = Line::from(vec![
        Span::styled(" > ", Style::default().fg(Color::Cyan).bold()),
        Span::styled(&palette.query, Style::default().fg(Color::White)),
        Span::styled("█", Style::default().fg(Color::Cyan)),
    ]);
    frame.render_widget(Paragraph::new(input_line), input_area);

    // Entry list
    let list_top = inner.y + 1;
    let list_height = inner.height.saturating_sub(1) as usize;
    let visible_count = list_height.min(palette.entries.len());

    // Scroll so that `selected` is visible
    let scroll_offset = if palette.selected >= list_height {
        palette.selected - list_height + 1
    } else {
        0
    };

    for (vi, ei) in (scroll_offset..scroll_offset + visible_count).enumerate() {
        if ei >= palette.entries.len() {
            break;
        }
        let entry = &palette.entries[ei];
        let row_y = list_top + vi as u16;
        if row_y >= inner.y + inner.height {
            break;
        }
        let row_area = Rect::new(inner.x, row_y, inner.width, 1);

        let is_selected = ei == palette.selected;
        let bg = if is_selected {
            Color::DarkGray
        } else {
            Color::Reset
        };
        let label_style = if is_selected {
            Style::default().fg(Color::Yellow).bold().bg(bg)
        } else {
            Style::default().fg(Color::White).bg(bg)
        };
        let detail_style = Style::default().fg(Color::DarkGray).bg(bg);

        // Truncate detail to fit
        let icon_label = format!(" {} {}", entry.icon, entry.label);
        let max_detail = (inner.width as usize).saturating_sub(icon_label.len() + 3);
        let detail_truncated: String = if entry.detail.len() > max_detail {
            format!("{}…", &entry.detail[..max_detail.saturating_sub(1)])
        } else {
            entry.detail.clone()
        };

        let line = Line::from(vec![
            Span::styled(icon_label, label_style),
            Span::styled("  ", Style::default().bg(bg)),
            Span::styled(detail_truncated, detail_style),
        ]);
        frame.render_widget(Paragraph::new(line), row_area);
    }

    // If no results
    if palette.entries.is_empty() {
        let empty_area = Rect::new(inner.x, list_top, inner.width, 1);
        let msg = Paragraph::new(Line::from(Span::styled(
            "  No matches",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(msg, empty_area);
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
        p.update(&[], &[], false);
        // Should have at least the navigation + help entries
        assert!(p.entries.len() >= 5);
    }

    #[test]
    fn test_palette_update_filters() {
        let mut p = CommandPalette::new();
        p.query = "help".to_string();
        p.update(&[], &[], false);
        assert!(p.entries.iter().any(|e| e.label.contains("Help")));
    }

    #[test]
    fn test_palette_move_down_wraps() {
        let mut p = CommandPalette::new();
        p.update(&[], &[], false);
        let len = p.entries.len();
        for _ in 0..len {
            p.move_down();
        }
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn test_palette_move_up_wraps() {
        let mut p = CommandPalette::new();
        p.update(&[], &[], false);
        p.move_up();
        assert_eq!(p.selected, p.entries.len() - 1);
    }
}
