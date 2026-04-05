pub mod command_palette;
pub mod dashboard;
pub mod filebrowser_view;
pub mod help;
pub mod host_monitor;
pub mod portfwd_view;
pub mod session_view;
pub mod widgets;

use ratatui::{
    layout::{Constraint, Direction, Layout},
    widgets::TableState,
    Frame,
};

use crate::diagnostics::DiagnosticsSnapshot;
use crate::filetransfer::FileBrowser;
use crate::monitor::{history::MetricHistory, HostMetrics};
use crate::portfwd::PortForwardManager;
use crate::session::manager::SessionManager;
use crate::theme::Theme;

pub struct Notification {
    pub session_label: String,
    #[allow(dead_code)]
    pub matched_text: String,
    #[allow(dead_code)]
    pub timestamp: chrono::DateTime<chrono::Local>,
}

#[derive(Clone, Debug)]
pub struct HostDisplay {
    pub name: String,
    pub hostname: String,
    pub port: u16,
    pub user: String,
    pub status: HostStatus,
    pub last_seen: String,
    pub tags: String,
    pub latency_ms: Option<f64>,
    pub latency_history: Vec<u64>,
    #[allow(dead_code)]
    pub jump_host: Option<String>,
}

#[derive(Clone, Debug)]
pub enum HostStatus {
    Online,
    Offline,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DashboardTab {
    Sessions,
    Hosts,
    Fleet,
    Config,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AppView {
    Dashboard,
    Session,
    Monitor,
    PortForwarding,
    FileBrowser,
}

pub struct App {
    pub hosts: Vec<HostDisplay>,
    pub selected_host: usize,
    pub table_state: TableState,
    pub session_manager: SessionManager,
    pub view: AppView,
    pub dashboard_tab: DashboardTab,
    pub status_message: Option<String>,
    pub monitor_sort: host_monitor::ProcessSort,
    pub monitor_process_scroll: usize,
    pub show_help: bool,
    // Host search/filter
    pub search_active: bool,
    pub search_query: String,
    // Split-pane view: terminal + monitor side-by-side
    pub split_pane: bool,
    pub split_pane_pct: u16, // terminal width percentage (20-80)
    // Per-session diagnostics snapshots (indexed by session manager index)
    pub session_diagnostics: Vec<Option<DiagnosticsSnapshot>>,
    // Per-session host metrics (indexed by session manager index)
    pub session_metrics: Vec<Option<HostMetrics>>,
    pub session_cpu_history: Vec<MetricHistory>,
    pub session_mem_history: Vec<MetricHistory>,
    pub session_net_rx_history: Vec<MetricHistory>,
    pub session_net_tx_history: Vec<MetricHistory>,
    // Background activity notifications
    pub notifications: Vec<Notification>,
    // Port forwarding
    pub port_forward_managers: Vec<PortForwardManager>,
    pub port_forward_input: String,
    pub port_forward_adding: bool,
    // File browser
    pub file_browser: Option<FileBrowser>,
    // Command palette
    pub command_palette: Option<command_palette::CommandPalette>,
    pub theme: Theme,
}

impl App {
    pub fn new(max_sessions: usize) -> Self {
        Self {
            hosts: Vec::new(),
            selected_host: 0,
            table_state: TableState::default(),
            session_manager: SessionManager::new(max_sessions),
            view: AppView::Dashboard,
            dashboard_tab: DashboardTab::Hosts,
            status_message: None,
            monitor_sort: host_monitor::ProcessSort::Cpu,
            monitor_process_scroll: 0,
            show_help: false,
            search_active: false,
            search_query: String::new(),
            split_pane: false,
            split_pane_pct: 60,
            session_diagnostics: Vec::new(),
            session_metrics: Vec::new(),
            session_cpu_history: Vec::new(),
            session_mem_history: Vec::new(),
            session_net_rx_history: Vec::new(),
            session_net_tx_history: Vec::new(),
            notifications: Vec::new(),
            port_forward_managers: Vec::new(),
            port_forward_input: String::new(),
            port_forward_adding: false,
            file_browser: None,
            command_palette: None,
            theme: crate::theme::dark(),
        }
    }

    pub fn set_hosts(&mut self, hosts: Vec<HostDisplay>) {
        self.hosts = hosts;
        if self.selected_host >= self.hosts.len() {
            self.selected_host = 0;
        }
        if !self.hosts.is_empty() {
            self.table_state.select(Some(self.selected_host));
        } else {
            self.table_state.select(None);
        }
    }

    /// Returns indices of hosts matching the current search query.
    pub fn filtered_host_indices(&self) -> Vec<usize> {
        if self.search_query.is_empty() {
            return (0..self.hosts.len()).collect();
        }
        let q = self.search_query.to_lowercase();
        self.hosts
            .iter()
            .enumerate()
            .filter(|(_, h)| {
                h.name.to_lowercase().contains(&q)
                    || h.hostname.to_lowercase().contains(&q)
                    || h.tags.to_lowercase().contains(&q)
                    || h.user.to_lowercase().contains(&q)
                    || format!("{:?}", h.status).to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn selected_host(&self) -> Option<&HostDisplay> {
        self.hosts.get(self.selected_host)
    }

    pub fn next_host(&mut self) {
        let indices = self.filtered_host_indices();
        if indices.is_empty() {
            return;
        }
        let current_pos = indices.iter().position(|&i| i == self.selected_host);
        let next = match current_pos {
            Some(pos) => indices[(pos + 1) % indices.len()],
            None => indices[0],
        };
        self.selected_host = next;
        self.table_state.select(Some(self.selected_host));
    }

    pub fn prev_host(&mut self) {
        let indices = self.filtered_host_indices();
        if indices.is_empty() {
            return;
        }
        let current_pos = indices.iter().position(|&i| i == self.selected_host);
        let prev = match current_pos {
            Some(0) => indices[indices.len() - 1],
            Some(pos) => indices[pos - 1],
            None => indices[0],
        };
        self.selected_host = prev;
        self.table_state.select(Some(self.selected_host));
    }

    /// Reset selection to the first filtered host (used when search query changes).
    pub fn select_first_filtered(&mut self) {
        let indices = self.filtered_host_indices();
        if let Some(&first) = indices.first() {
            self.selected_host = first;
            self.table_state.select(Some(first));
        }
    }

    pub fn set_status(&mut self, msg: String) {
        self.status_message = Some(msg);
    }

    pub fn add_session_tracking(&mut self, history_samples: usize) {
        self.session_diagnostics.push(None);
        self.session_metrics.push(None);
        self.session_cpu_history
            .push(MetricHistory::new(history_samples));
        self.session_mem_history
            .push(MetricHistory::new(history_samples));
        self.session_net_rx_history
            .push(MetricHistory::new(history_samples));
        self.session_net_tx_history
            .push(MetricHistory::new(history_samples));
        self.port_forward_managers.push(PortForwardManager::new());
    }

    pub fn remove_session_tracking(&mut self, index: usize) {
        if index < self.session_diagnostics.len() {
            self.session_diagnostics.remove(index);
            self.session_metrics.remove(index);
            self.session_cpu_history.remove(index);
            self.session_mem_history.remove(index);
            self.session_net_rx_history.remove(index);
            self.session_net_tx_history.remove(index);
        }
        if index < self.port_forward_managers.len() {
            self.port_forward_managers.remove(index);
        }
    }
}

pub fn render(frame: &mut Frame, app: &mut App) {
    match app.view {
        AppView::Dashboard => {
            let filtered_indices = app.filtered_host_indices();
            dashboard::render(
                frame,
                frame.area(),
                &app.session_manager.sessions,
                &app.hosts,
                &filtered_indices,
                app.selected_host,
                &mut app.table_state,
                app.dashboard_tab,
                app.status_message.as_deref(),
                app.search_active,
                &app.search_query,
                &app.theme,
            );
        }
        AppView::Session => {
            if let Some(active_idx) = app.session_manager.active_index {
                let area = frame.area();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), // tab bar
                        Constraint::Min(4),    // terminal (or terminal + monitor split)
                        Constraint::Length(2), // status bar
                        Constraint::Length(2), // footer
                    ])
                    .split(area);

                session_view::render_tab_bar(
                    frame,
                    chunks[0],
                    &app.session_manager.sessions,
                    active_idx,
                    &app.notifications,
                    &app.theme,
                );

                if app.split_pane {
                    // Split-pane: terminal on left, host monitor on right
                    let panes = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([
                            Constraint::Percentage(app.split_pane_pct),
                            Constraint::Percentage(100 - app.split_pane_pct),
                        ])
                        .split(chunks[1]);

                    // Resize virtual terminal to match the left pane
                    if let Some(session) = app.session_manager.sessions.get_mut(active_idx) {
                        session.terminal.resize(panes[0].height, panes[0].width);
                    }

                    if let Some(session) = app.session_manager.sessions.get(active_idx) {
                        session_view::render_terminal(frame, panes[0], session);
                    }

                    // Render host monitor in the right pane
                    let metrics = app
                        .session_metrics
                        .get(active_idx)
                        .and_then(|m| m.as_ref())
                        .cloned()
                        .unwrap_or_default();
                    let cpu_hist = app
                        .session_cpu_history
                        .get(active_idx)
                        .cloned()
                        .unwrap_or_else(|| MetricHistory::new(60));
                    let mem_hist = app
                        .session_mem_history
                        .get(active_idx)
                        .cloned()
                        .unwrap_or_else(|| MetricHistory::new(60));
                    let rx_hist = app
                        .session_net_rx_history
                        .get(active_idx)
                        .cloned()
                        .unwrap_or_else(|| MetricHistory::new(60));
                    let tx_hist = app
                        .session_net_tx_history
                        .get(active_idx)
                        .cloned()
                        .unwrap_or_else(|| MetricHistory::new(60));

                    host_monitor::render(
                        frame,
                        panes[1],
                        &metrics,
                        &cpu_hist,
                        &mem_hist,
                        &rx_hist,
                        &tx_hist,
                        &app.monitor_sort,
                        app.monitor_process_scroll,
                        &app.theme,
                    );
                } else {
                    // Full-width terminal
                    let term_area = chunks[1];
                    if let Some(session) = app.session_manager.sessions.get_mut(active_idx) {
                        session.terminal.resize(term_area.height, term_area.width);
                    }

                    if let Some(session) = app.session_manager.sessions.get(active_idx) {
                        session_view::render_terminal(frame, chunks[1], session);
                    }
                }

                if let Some(session) = app.session_manager.sessions.get(active_idx) {
                    let diag = app
                        .session_diagnostics
                        .get(active_idx)
                        .and_then(|d| d.as_ref());
                    session_view::render_status_bar(
                        frame,
                        chunks[2],
                        session,
                        diag,
                        app.port_forward_managers.get(active_idx),
                        &app.theme,
                    );
                }

                session_view::render_footer(frame, chunks[3], &app.theme);
            }
        }
        AppView::Monitor => {
            if let Some(active_idx) = app.session_manager.active_index {
                let area = frame.area();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), // tab bar
                        Constraint::Min(10),   // monitor
                    ])
                    .split(area);

                session_view::render_tab_bar(
                    frame,
                    chunks[0],
                    &app.session_manager.sessions,
                    active_idx,
                    &app.notifications,
                    &app.theme,
                );

                let metrics = app
                    .session_metrics
                    .get(active_idx)
                    .and_then(|m| m.as_ref())
                    .cloned()
                    .unwrap_or_default();

                let cpu_hist = app
                    .session_cpu_history
                    .get(active_idx)
                    .cloned()
                    .unwrap_or_else(|| MetricHistory::new(60));
                let mem_hist = app
                    .session_mem_history
                    .get(active_idx)
                    .cloned()
                    .unwrap_or_else(|| MetricHistory::new(60));
                let rx_hist = app
                    .session_net_rx_history
                    .get(active_idx)
                    .cloned()
                    .unwrap_or_else(|| MetricHistory::new(60));
                let tx_hist = app
                    .session_net_tx_history
                    .get(active_idx)
                    .cloned()
                    .unwrap_or_else(|| MetricHistory::new(60));

                host_monitor::render(
                    frame,
                    chunks[1],
                    &metrics,
                    &cpu_hist,
                    &mem_hist,
                    &rx_hist,
                    &tx_hist,
                    &app.monitor_sort,
                    app.monitor_process_scroll,
                    &app.theme,
                );
            }
        }
        AppView::PortForwarding => {
            // Render the session view behind the overlay
            if let Some(active_idx) = app.session_manager.active_index {
                let area = frame.area();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Min(4),
                        Constraint::Length(2),
                        Constraint::Length(2),
                    ])
                    .split(area);

                session_view::render_tab_bar(
                    frame,
                    chunks[0],
                    &app.session_manager.sessions,
                    active_idx,
                    &app.notifications,
                    &app.theme,
                );

                if let Some(session) = app.session_manager.sessions.get(active_idx) {
                    session_view::render_terminal(frame, chunks[1], session);
                    let diag = app
                        .session_diagnostics
                        .get(active_idx)
                        .and_then(|d| d.as_ref());
                    session_view::render_status_bar(
                        frame,
                        chunks[2],
                        session,
                        diag,
                        app.port_forward_managers.get(active_idx),
                        &app.theme,
                    );
                }
                session_view::render_footer(frame, chunks[3], &app.theme);

                // Port forward overlay
                if let Some(mgr) = app.port_forward_managers.get(active_idx) {
                    portfwd_view::render(
                        frame,
                        mgr,
                        &app.port_forward_input,
                        app.port_forward_adding,
                        &app.theme,
                    );
                }
            }
        }
        AppView::FileBrowser => {
            if let Some(active_idx) = app.session_manager.active_index {
                let area = frame.area();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Min(6)])
                    .split(area);

                session_view::render_tab_bar(
                    frame,
                    chunks[0],
                    &app.session_manager.sessions,
                    active_idx,
                    &app.notifications,
                    &app.theme,
                );

                if let Some(ref browser) = app.file_browser {
                    filebrowser_view::render(frame, chunks[1], browser, &app.theme);
                }
            }
        }
    }

    // Help overlay (rendered on top of any view)
    if app.show_help {
        help::render(frame, &app.theme);
    }

    // Command palette overlay (rendered on top of everything)
    if let Some(ref palette) = app.command_palette {
        command_palette::render(frame, palette, &app.theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_host(name: &str, hostname: &str, tags: &str) -> HostDisplay {
        HostDisplay {
            name: name.to_string(),
            hostname: hostname.to_string(),
            port: 22,
            user: "root".to_string(),
            status: HostStatus::Unknown,
            last_seen: String::new(),
            tags: tags.to_string(),
            latency_ms: None,
            latency_history: Vec::new(),
            jump_host: None,
        }
    }

    fn sample_app() -> App {
        let mut app = App::new(10);
        app.set_hosts(vec![
            make_host("web-prod-1", "10.0.1.1", "env=prod,role=web"),
            make_host("web-prod-2", "10.0.1.2", "env=prod,role=web"),
            make_host("db-staging", "10.0.2.1", "env=staging,role=db"),
            make_host("cache-prod", "10.0.1.10", "env=prod,role=cache"),
        ]);
        app
    }

    #[test]
    fn test_filter_no_query_returns_all() {
        let app = sample_app();
        assert_eq!(app.filtered_host_indices(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn test_filter_by_name() {
        let mut app = sample_app();
        app.search_query = "web".to_string();
        assert_eq!(app.filtered_host_indices(), vec![0, 1]);
    }

    #[test]
    fn test_filter_by_hostname() {
        let mut app = sample_app();
        app.search_query = "10.0.2".to_string();
        assert_eq!(app.filtered_host_indices(), vec![2]);
    }

    #[test]
    fn test_filter_by_tag() {
        let mut app = sample_app();
        app.search_query = "staging".to_string();
        assert_eq!(app.filtered_host_indices(), vec![2]);
    }

    #[test]
    fn test_filter_case_insensitive() {
        let mut app = sample_app();
        app.search_query = "WEB".to_string();
        assert_eq!(app.filtered_host_indices(), vec![0, 1]);
    }

    #[test]
    fn test_filter_no_match() {
        let mut app = sample_app();
        app.search_query = "nonexistent".to_string();
        assert!(app.filtered_host_indices().is_empty());
    }

    #[test]
    fn test_select_first_filtered() {
        let mut app = sample_app();
        app.search_query = "db".to_string();
        app.select_first_filtered();
        assert_eq!(app.selected_host, 2);
    }

    #[test]
    fn test_next_host_wraps_within_filter() {
        let mut app = sample_app();
        app.search_query = "web".to_string();
        app.selected_host = 0;
        app.next_host();
        assert_eq!(app.selected_host, 1);
        app.next_host();
        assert_eq!(app.selected_host, 0); // wraps back
    }

    #[test]
    fn test_prev_host_wraps_within_filter() {
        let mut app = sample_app();
        app.search_query = "web".to_string();
        app.selected_host = 0;
        app.prev_host();
        assert_eq!(app.selected_host, 1); // wraps to last
    }

    #[test]
    fn test_search_clear_restores_all() {
        let mut app = sample_app();
        app.search_query = "web".to_string();
        assert_eq!(app.filtered_host_indices().len(), 2);
        app.search_query.clear();
        assert_eq!(app.filtered_host_indices().len(), 4);
    }

    #[test]
    fn test_split_pane_default_off() {
        let app = App::new(9);
        assert!(!app.split_pane);
        assert_eq!(app.split_pane_pct, 60);
    }

    #[test]
    fn test_split_pane_toggle() {
        let mut app = App::new(9);
        assert!(!app.split_pane);
        app.split_pane = !app.split_pane;
        assert!(app.split_pane);
        app.split_pane = !app.split_pane;
        assert!(!app.split_pane);
    }

    #[test]
    fn test_split_pane_pct_bounds() {
        let mut app = App::new(9);
        app.split_pane = true;

        // Shrink to minimum
        app.split_pane_pct = 25;
        app.split_pane_pct = app.split_pane_pct.saturating_sub(5).max(20);
        assert_eq!(app.split_pane_pct, 20);
        // Can't go below 20
        app.split_pane_pct = app.split_pane_pct.saturating_sub(5).max(20);
        assert_eq!(app.split_pane_pct, 20);

        // Grow to maximum
        app.split_pane_pct = 75;
        app.split_pane_pct = (app.split_pane_pct + 5).min(80);
        assert_eq!(app.split_pane_pct, 80);
        // Can't go above 80
        app.split_pane_pct = (app.split_pane_pct + 5).min(80);
        assert_eq!(app.split_pane_pct, 80);
    }
}
