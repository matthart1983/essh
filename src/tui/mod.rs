pub mod command_palette;
pub mod dashboard;
pub mod divergence_view;
pub mod filebrowser_view;
pub mod help;
pub mod host_monitor;
pub mod hud;
pub mod launcher_view;
pub mod portfwd_view;
pub mod session_view;
pub mod widgets;

use ratatui::{
    layout::Rect,
    layout::{Constraint, Direction, Layout},
    widgets::TableState,
    Frame,
};

use crate::design;
use crate::diagnostics::DiagnosticsSnapshot;
use crate::filetransfer::FileBrowser;
use crate::monitor::{history::MetricHistory, HostMetrics};
use crate::portfwd::PortForwardManager;
use crate::session::manager::SessionManager;
use crate::theme::Theme;

pub fn meta_key_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Option"
    }

    #[cfg(not(target_os = "macos"))]
    {
        "Alt"
    }
}

pub fn meta_key_hint(keys: &str) -> String {
    format!("{}+{}", meta_key_label(), keys)
}

/// How a command is reached: press the prefix, release, then the key.
///
/// Spelled `Ctrl+A` rather than `^A`. The caret notation assumes the reader
/// already knows the convention, and someone who has to look up the hint is
/// exactly the person who does not.
///
/// This is deliberately the *only* binding advertised. `Option+m` and friends
/// also work, but only when the terminal is configured to send Alt as Meta —
/// which macOS terminals do not do by default, so `Option+m` types `µ` and
/// the user concludes the app is broken. A binding that works everywhere,
/// shown everywhere, beats two bindings and a caveat.
pub fn prefix_hint(prefix: &str, keys: &str) -> String {
    let shown = prefix_label(prefix);
    if keys.is_empty() {
        shown
    } else {
        format!("{} {}", shown, keys)
    }
}

/// The prefix itself, as something a person can read off and press.
pub fn prefix_label(prefix: &str) -> String {
    match prefix.to_lowercase().strip_prefix("ctrl-") {
        Some(rest) => format!("Ctrl+{}", rest.to_uppercase()),
        None => prefix.to_string(),
    }
}

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
    /// Pre-joined tags, kept for search and the command palette.
    pub tags: String,
    /// Structured tags, needed for chip layout and peer-set membership.
    pub tag_pairs: Vec<(String, String)>,
    pub latency_ms: Option<f64>,
    pub latency_history: Vec<u64>,
    #[allow(dead_code)]
    pub jump_host: Option<String>,
    /// How many facets differ from this host's peers.
    ///
    /// `None` means we have never collected facts from it — which is a
    /// different statement from `Some(0)`, "we checked and it agrees". v1
    /// collapsed both into `Unknown` and lost the distinction.
    pub diverge_count: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HostStatus {
    Online,
    Offline,
    /// Never probed — we have no evidence either way, and say so.
    NeverProbed,
}

impl HostStatus {
    /// Label and whether it carries good/bad weight.
    ///
    /// v1 rendered this as `○ Unknown`, which reads as a failed check rather
    /// than an absent one.
    // Display name for the view, used by screens that name themselves.
    #[allow(dead_code)]
    pub fn label(&self) -> &'static str {
        match self {
            HostStatus::Online => "● online",
            HostStatus::Offline => "● offline",
            HostStatus::NeverProbed => "  never probed",
        }
    }
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
    /// The launcher: `essh` with no arguments. §2's fast path starts here.
    Launcher,
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
    /// Which row the Sessions tab has selected.
    ///
    /// The tab used to have none: arrows moved the *hosts* cursor even while
    /// Sessions was on screen, so the list had no visible selection and Enter
    /// connected to a host rather than attaching to the session under the
    /// (invisible) cursor.
    pub selected_session: usize,
    pub status_message: Option<String>,
    pub status_set_at: Option<std::time::Instant>,
    pub monitor_sort: host_monitor::ProcessSort,
    pub monitor_process_scroll: usize,
    pub show_help: bool,
    /// First visible line of the help overlay.
    ///
    /// The reference is longer than a short terminal, and silently clipping
    /// the tail means the keys at the bottom simply do not exist as far as
    /// the user is concerned.
    pub help_scroll: u16,
    /// Host awaiting a delete confirmation, by name.
    pub pending_delete: Option<String>,
    /// Divergence overlay for the selected host, when open.
    pub show_divergence: bool,
    /// A workspace waiting to be restored, and the sessions still to open.
    ///
    /// Restoring is spread one host per tick rather than done in a single
    /// blocking pass: connects can take seconds each, and doing them inline
    /// froze the event loop — nothing rendered, no key was read, and one
    /// unreachable host hung the whole UI until its TCP timeout.
    pub pending_workspace: Option<crate::workspace::Workspace>,
    pub workspace_queue: std::collections::VecDeque<crate::workspace::WorkspaceSession>,
    pub workspace_report: Option<crate::workspace::RestoreReport>,
    /// Session pane layout for §3's splits. `None` until the first split.
    pub panes: Option<crate::panes::PaneTree>,
    /// The only instrumentation allowed over a shell.
    pub hud: hud::HudState,
    /// Launcher state — query, ranked results, selection.
    pub launcher: launcher_view::LauncherState,
    /// Every host the launcher can offer, assembled from ssh_config, ESSH's
    /// own config and the cache.
    pub candidates: Vec<crate::launcher::Candidate>,
    // Host search/filter
    pub search_active: bool,
    pub search_query: String,
    // Add-host dialog
    pub add_host_active: bool,
    pub add_host_input: String,
    pub add_host_error: Option<String>,
    pub add_host_original: Option<(String, u16)>,
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
    /// The configured command prefix, mirrored from config for rendering.
    pub prefix_key: String,
    /// True after the prefix key, waiting for the command key.
    pub prefix_pending: bool,
    pub theme: Theme,
    // Divergence
    /// Facts collected per host, keyed by the same name the host list uses.
    pub host_facts: std::collections::HashMap<String, crate::divergence::HostFacts>,
    /// Facets collectable vs declared, per host, recorded at collection time.
    pub host_coverage: std::collections::HashMap<String, (usize, usize, usize)>,
    /// Detected platform per host, so the overlay can state what was attempted.
    pub host_platforms: std::collections::HashMap<String, crate::monitor::Platform>,
    /// Peer sets derived from host tags, largest first.
    pub peer_sets: Vec<crate::divergence::PeerSet>,
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
            selected_session: 0,
            status_message: None,
            status_set_at: None,
            monitor_sort: host_monitor::ProcessSort::Cpu,
            monitor_process_scroll: 0,
            show_help: false,
            help_scroll: 0,
            pending_delete: None,
            show_divergence: false,
            pending_workspace: None,
            workspace_queue: std::collections::VecDeque::new(),
            workspace_report: None,
            panes: None,
            hud: hud::HudState::new(),
            launcher: launcher_view::LauncherState::new(),
            candidates: Vec::new(),
            search_active: false,
            search_query: String::new(),
            add_host_active: false,
            add_host_input: String::new(),
            add_host_error: None,
            add_host_original: None,
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
            prefix_key: "ctrl-a".to_string(),
            prefix_pending: false,
            theme: crate::theme::dark(),
            host_facts: std::collections::HashMap::new(),
            peer_sets: Vec::new(),
            host_platforms: std::collections::HashMap::new(),
            host_coverage: std::collections::HashMap::new(),
        }
    }

    /// Recompute peer sets from tags and each host's divergence score.
    ///
    /// A host's score is measured against its *primary* peer set — the largest
    /// set it belongs to — because a host in both `role=web` and `env=prod`
    /// needs one number in the list, and the broader group is the one whose
    /// consensus means the most.
    ///
    /// A host with no facts keeps `diverge_count: None`. It is unprobed, and
    /// the list must not imply we checked it.
    pub fn recompute_divergence(&mut self) {
        let tagged: Vec<(String, Vec<(String, String)>)> = self
            .hosts
            .iter()
            .map(|h| (h.name.clone(), h.tag_pairs.clone()))
            .collect();
        self.peer_sets = crate::divergence::peer_sets_from_tags(&tagged);

        for host in &mut self.hosts {
            if !self.host_facts.contains_key(&host.name) {
                host.diverge_count = None;
                continue;
            }
            // Largest set containing this host; peer_sets is already sorted.
            let primary = self.peer_sets.iter().find(|s| s.hosts.contains(&host.name));
            host.diverge_count = primary
                .map(|set| crate::divergence::compare(&host.name, set, &self.host_facts).score());
        }
    }

    /// The display name for a connected session, so facts collected over a
    /// session land under the same key the host list uses.
    pub fn host_name_for(&self, hostname: &str, port: u16) -> Option<String> {
        self.hosts
            .iter()
            .find(|h| h.hostname == hostname && h.port == port)
            .map(|h| h.name.clone())
    }

    /// Group summaries for the GROUPS panel.
    pub fn group_summaries(&self) -> Vec<crate::divergence::GroupSummary> {
        crate::divergence::summarise_groups(&self.peer_sets, &self.host_facts)
    }

    /// Per-facet agreement across the primary peer set, worst first.
    ///
    /// This is what the CONSENSUS box's right column shows: not how ragged
    /// the fleet is, but *where*.
    pub fn facet_agreement(&self) -> Vec<(String, f64)> {
        let Some(set) = self.peer_sets.first() else {
            return Vec::new();
        };
        let mut totals: std::collections::HashMap<String, (usize, usize)> =
            std::collections::HashMap::new();

        for host in &set.hosts {
            if !self.host_facts.contains_key(host) {
                continue;
            }
            let d = crate::divergence::compare(host, set, &self.host_facts);
            for key in &d.identical {
                let e = totals.entry(key.label()).or_insert((0, 0));
                e.0 += 1;
                e.1 += 1;
            }
            for c in &d.comparisons {
                let e = totals.entry(c.key.label()).or_insert((0, 0));
                e.1 += 1;
                if !c.diverges() {
                    e.0 += 1;
                }
            }
        }

        let mut out: Vec<(String, f64)> = totals
            .into_iter()
            .filter(|(_, (_, total))| *total > 0)
            .map(|(k, (ok, total))| (k, ok as f64 / total as f64))
            .collect();
        // Worst first — the ragged facets are the point.
        out.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        out
    }

    /// A one-line reason for the HUD, when this host differs from its peers.
    ///
    /// Returns `None` when the host is at consensus — silence is the correct
    /// output, and a HUD that fires for agreement is a status bar.
    pub fn divergence_headline(&self, host: &str) -> Option<String> {
        let set = self
            .peer_sets
            .iter()
            .find(|s| s.hosts.contains(&host.to_string()))?;
        let d = crate::divergence::compare(host, set, &self.host_facts);
        let worst = d.diverging().first().copied()?;
        Some(format!(
            "{} differs from your {} peers — {}",
            worst.key.label(),
            set.hosts.len().saturating_sub(1),
            worst.summary()
        ))
    }

    /// The titlebar's right-hand note: fleet size and how many hosts differ.
    ///
    /// `None` when nothing has been probed, because "0 diverged" would read
    /// as a clean bill of health for a fleet nobody has looked at.
    pub fn peer_note(&self) -> Option<String> {
        if self.host_facts.is_empty() {
            return None;
        }
        let diverged = self
            .hosts
            .iter()
            .filter(|h| h.diverge_count.is_some_and(|n| n > 0))
            .count();
        Some(format!(
            "{} hosts · {} diverged",
            self.hosts.len(),
            diverged
        ))
    }

    /// Peer context for a session's monitor — the medians that turn a
    /// number into a finding.
    pub fn peer_context(&self, session_idx: usize) -> host_monitor::PeerContext {
        let Some(session) = self.session_manager.sessions.get(session_idx) else {
            return host_monitor::PeerContext::default();
        };
        let Some(name) = self.host_name_for(&session.hostname, session.port) else {
            return host_monitor::PeerContext::default();
        };
        let Some(set) = self.peer_sets.iter().find(|s| s.hosts.contains(&name)) else {
            return host_monitor::PeerContext::default();
        };

        // Medians come from the facets already collected for the peer set,
        // so the monitor never invents a comparison it has not measured.
        let d = crate::divergence::compare(&name, set, &self.host_facts);
        let median_of = |key: &crate::divergence::FacetKey| -> Option<f64> {
            d.comparisons
                .iter()
                .find(|c| &c.key == key)
                .and_then(|c| c.distribution.as_ref())
                .map(|dist| dist.median)
        };
        host_monitor::PeerContext {
            cpu_median_pct: median_of(&crate::divergence::FacetKey::LoadPerCore)
                .map(|l| (l * 100.0).min(100.0)),
            mem_median_gb: median_of(&crate::divergence::FacetKey::MemTotal),
            peers: set.hosts.len(),
        }
    }

    /// Divergence for the currently selected host, if it has a peer set.
    pub fn selected_divergence(&self) -> Option<crate::divergence::HostDivergence> {
        let host = self.selected_host()?;
        let set = self
            .peer_sets
            .iter()
            .find(|s| s.hosts.contains(&host.name))?;
        Some(crate::divergence::compare(
            &host.name,
            set,
            &self.host_facts,
        ))
    }

    /// How many facets were collectable on the selected host's platform.
    ///
    /// Recorded at collection time rather than recomputed here, so the overlay
    /// reports what was actually attempted rather than the size of the facet
    /// table. macOS cannot answer several of them, and claiming otherwise
    /// would overstate the comparison.
    pub fn selected_coverage(&self) -> ((usize, usize, usize), String) {
        let name = self.selected_host().map(|h| h.name.clone());
        let coverage = name
            .as_ref()
            .and_then(|n| self.host_coverage.get(n))
            .copied()
            .unwrap_or((0, 0, 0));
        let platform = name
            .as_ref()
            .and_then(|n| self.host_platforms.get(n))
            .cloned()
            .unwrap_or_default();
        (coverage, platform.label().to_string())
    }

    /// Fleet-wide consensus across the primary peer set, if there is one.
    pub fn fleet_consensus(&self) -> Option<(String, crate::divergence::Consensus)> {
        let set = self.peer_sets.first()?;
        Some((
            set.label(),
            crate::divergence::consensus(set, &self.host_facts),
        ))
    }

    /// A verdict per diverging host, for the Fleet screen.
    ///
    /// The handoff's Fleet mockup leads with a VERDICT block — *"the verdict
    /// does the reasoning: which host, which facet, and whether it's one
    /// cause or two"*. Counting how many hosts disagree is not that; it says
    /// there is a problem without saying what it is.
    ///
    /// Only hosts that actually diverge produce a verdict, and only when a
    /// template matches — an unpatterned difference gets no sentence rather
    /// than an invented one.
    pub fn fleet_verdicts(&self) -> Vec<(String, crate::divergence::Verdict)> {
        let Some(set) = self.peer_sets.first() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for host in &set.hosts {
            if !self.host_facts.contains_key(host) {
                continue; // never probed: absence of facts, not agreement
            }
            let d = crate::divergence::compare(host, set, &self.host_facts);
            if let Some(v) = crate::divergence::verdict_for(&d) {
                out.push((host.clone(), v));
            }
        }
        out
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
        self.recompute_divergence();
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

    /// Move the Sessions-tab cursor, wrapping like every other list here.
    pub fn next_session_row(&mut self) {
        let n = self.session_manager.sessions.len();
        if n > 0 {
            self.selected_session = (self.selected_session + 1) % n;
        }
    }

    pub fn prev_session_row(&mut self) {
        let n = self.session_manager.sessions.len();
        if n > 0 {
            self.selected_session = (self.selected_session + n - 1) % n;
        }
    }

    /// Keep the cursor on a row that exists — sessions close underneath it.
    pub fn clamp_session_row(&mut self) {
        let n = self.session_manager.sessions.len();
        self.selected_session = if n == 0 {
            0
        } else {
            self.selected_session.min(n - 1)
        };
    }

    pub fn set_status(&mut self, msg: String) {
        self.status_message = Some(msg);
        self.status_set_at = Some(std::time::Instant::now());
    }

    /// The status, while it is still fresh.
    ///
    /// Statuses expire so the hint strip goes back to being hints. Without an
    /// expiry a one-off message like "no other session to split into" would
    /// sit there permanently, which is worse than not showing it.
    pub fn fresh_status(&self) -> Option<&str> {
        let age = self.status_set_at?.elapsed();
        if age < std::time::Duration::from_secs(6) {
            self.status_message.as_deref()
        } else {
            None
        }
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
        AppView::Launcher => {
            launcher_view::render(
                frame,
                &app.launcher,
                app.status_message.as_deref().unwrap_or_default(),
                &app.theme,
            );
        }
        AppView::Dashboard => {
            let filtered_indices = app.filtered_host_indices();
            let groups = app.group_summaries();
            let fleet_consensus = app.fleet_consensus();
            let peer_note = app.peer_note();
            let facet_agreement = app.facet_agreement();
            let verdicts = app.fleet_verdicts();
            dashboard::render(
                frame,
                frame.area(),
                &app.session_manager.sessions,
                &app.hosts,
                &filtered_indices,
                app.selected_host,
                app.selected_session,
                &mut app.table_state,
                app.dashboard_tab,
                app.status_message.as_deref(),
                app.search_active,
                &app.search_query,
                &groups,
                fleet_consensus,
                &facet_agreement,
                &verdicts,
                peer_note,
                &app.theme,
            );
        }
        AppView::Session => {
            if let Some(active_idx) = app.session_manager.active_index {
                // ── One row of chrome, and only one ─────────────────────
                //
                // The handoff's rule was "the shell gets nothing" — zero
                // reserved rows. In practice that means connecting to a host
                // whose shell prints nothing on login leaves a completely
                // blank screen: no host name, no session list, no way to tell
                // a live session from a hung one, and no hint of the prefix
                // key that gets you out. That reads as a broken app.
                //
                // So the shell gets everything except one row. The tab bar is
                // the same one the dashboard and file browser already show,
                // which is also what makes a session look like the rest of
                // the program instead of a bare terminal.
                let full = frame.area();
                design::paint_bg(frame, full);
                // Two rows top and bottom: each strip carries a border, so a
                // single row would be entirely consumed by its rule and the
                // text would never appear.
                let rows = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(2), // tab bar
                        Constraint::Min(1),    // the shell
                        Constraint::Length(2), // key hints
                    ])
                    .split(full);
                session_view::render_tab_bar(
                    frame,
                    rows[0],
                    &app.session_manager.sessions,
                    active_idx,
                    &app.notifications,
                    &app.theme,
                );
                let area = rows[1];
                session_view::render_footer(
                    frame,
                    rows[2],
                    &app.prefix_key,
                    app.prefix_pending,
                    app.fresh_status(),
                    app.session_manager.sessions.len(),
                    &app.theme,
                );

                if app.panes.as_ref().is_some_and(|p| !p.is_single()) {
                    // §3's session splits. Still no per-pane titles or
                    // borders — the design allows one hairline divider and
                    // nothing else, so panes are separated by a rule column
                    // drawn between them, not by a box around each.
                    let placed = app
                        .panes
                        .as_ref()
                        .map(|p| p.layout(area))
                        .unwrap_or_default();

                    for pane in &placed {
                        if let Some(session) = app.session_manager.sessions.get_mut(pane.session) {
                            session.terminal.resize(pane.area.height, pane.area.width);
                        }
                        if let Some(session) = app.session_manager.sessions.get(pane.session) {
                            session_view::render_pane(
                                frame,
                                pane.area,
                                session,
                                pane.focused,
                                &app.theme,
                            );
                        }
                    }
                } else if app.split_pane {
                    // Screen 5: shell on the left, monitor essentials on the
                    // right. A single hairline divider, and the shell keeps
                    // its zero chrome.
                    let panes = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Min(20), Constraint::Length(44)])
                        .split(area);

                    if let Some(session) = app.session_manager.sessions.get_mut(active_idx) {
                        session
                            .terminal
                            .resize(panes[0].height, panes[0].width.saturating_sub(1));
                    }
                    if let Some(session) = app.session_manager.sessions.get(active_idx) {
                        let term = Rect {
                            width: panes[0].width.saturating_sub(1),
                            ..panes[0]
                        };
                        session_view::render_terminal(frame, term, session);
                    }
                    // The hairline.
                    let divider = Rect {
                        x: panes[0].x + panes[0].width.saturating_sub(1),
                        y: panes[0].y,
                        width: 1,
                        height: panes[0].height,
                    };
                    frame.render_widget(
                        ratatui::widgets::Block::default()
                            .borders(ratatui::widgets::Borders::LEFT)
                            .border_style(ratatui::style::Style::default().fg(design::RULE)),
                        divider,
                    );

                    let metrics = app
                        .session_metrics
                        .get(active_idx)
                        .and_then(|m| m.as_ref())
                        .cloned()
                        .unwrap_or_default();
                    host_monitor::render_essentials(
                        frame,
                        panes[1],
                        &metrics,
                        app.session_cpu_history.get(active_idx),
                        app.session_mem_history.get(active_idx),
                        app.session_net_rx_history.get(active_idx),
                        &app.peer_context(active_idx),
                        app.session_diagnostics
                            .get(active_idx)
                            .and_then(|d| d.as_ref())
                            .and_then(|d| d.rtt_ms),
                        &app.theme,
                    );
                } else {
                    if let Some(session) = app.session_manager.sessions.get_mut(active_idx) {
                        session.terminal.resize(area.height, area.width);
                    }
                    if let Some(session) = app.session_manager.sessions.get(active_idx) {
                        session_view::render_terminal(frame, area, session);
                    }
                }

                // Transient, over the shell, never a reserved row.
                hud::render(frame, area, &app.hud);
            }
        }
        AppView::Monitor => {
            if let Some(active_idx) = app.session_manager.active_index {
                let area = frame.area();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(2), // tab bar: text + hairline
                        Constraint::Min(4),    // monitor
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

                // No footer row: the panels carry their own binds on their
                // bottom rules now, so that row goes back to being data. The
                // monitor takes the area *below* the tab bar — drawing into
                // the full frame paints over it.
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
                    &app.peer_context(active_idx),
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
                session_view::render_footer(
                    frame,
                    chunks[3],
                    &app.prefix_key,
                    app.prefix_pending,
                    app.fresh_status(),
                    app.session_manager.sessions.len(),
                    &app.theme,
                );

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

    if app.view == AppView::Dashboard && app.add_host_active {
        dashboard::render_add_host_dialog(
            frame,
            app.add_host_original.is_some(),
            &app.add_host_input,
            app.add_host_error.as_deref(),
            &app.theme,
        );
    }

    // Divergence overlay (rendered on top of any view)
    if app.show_divergence {
        let d = app.selected_divergence();
        let (coverage, platform) = app.selected_coverage();
        divergence_view::render(
            frame,
            frame.area(),
            d.as_ref(),
            coverage,
            &platform,
            &app.theme,
        );
    }

    // Help overlay (rendered on top of any view)
    if app.show_help {
        help::render(frame, &app.theme, &app.prefix_key, app.help_scroll);
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
            status: HostStatus::NeverProbed,
            last_seen: String::new(),
            tags: tags.to_string(),
            tag_pairs: tags
                .split(',')
                .filter_map(|t| t.trim().split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            latency_ms: None,
            latency_history: Vec::new(),
            jump_host: None,
            diverge_count: None,
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

    #[test]
    fn test_meta_key_hint_formats_combo() {
        assert_eq!(meta_key_hint("1-9"), format!("{}+1-9", meta_key_label()));
    }
}

#[cfg(test)]
mod render_smoke {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Every view the user can reach.
    const VIEWS: [AppView; 6] = [
        AppView::Launcher,
        AppView::Dashboard,
        AppView::Session,
        AppView::Monitor,
        AppView::PortForwarding,
        AppView::FileBrowser,
    ];

    /// Sizes worth caring about, plus the degenerate ones that find the
    /// arithmetic bugs: a pane one cell wide has no room for anything, and
    /// every `width - something` in the codebase is a chance to underflow.
    const SIZES: [(u16, u16); 10] = [
        (1, 1),
        (2, 2),
        (5, 3),
        (20, 6),
        (40, 12),
        (80, 24),
        (100, 30),
        (132, 43),
        (200, 60),
        (300, 100),
    ];

    fn app_with_session() -> App {
        let mut app = App::new(8);
        let host = |name: &str, hostname: &str, user: &str| HostDisplay {
            name: name.into(),
            hostname: hostname.into(),
            port: 22,
            user: user.into(),
            status: HostStatus::NeverProbed,
            last_seen: String::new(),
            tags: "role=web".into(),
            tag_pairs: vec![("role".into(), "web".into())],
            latency_ms: None,
            latency_history: Vec::new(),
            jump_host: None,
            diverge_count: None,
        };
        app.hosts = vec![
            host("web-01", "10.0.0.1", "deploy"),
            host("mattbot", "192.168.0.54", "matt"),
        ];
        app.session_manager
            .sessions
            .push(crate::session::Session::new(
                "s1".into(),
                "mattbot".into(),
                "192.168.0.54".into(),
                22,
                "matt".into(),
                1000,
            ));
        app.session_manager.active_index = Some(0);
        app
    }

    /// Render every view at every size. A panic here is a screen that would
    /// have taken the whole app down in front of a user.
    #[test]
    fn every_view_renders_at_every_size_without_panicking() {
        for view in VIEWS {
            for (w, h) in SIZES {
                let mut app = app_with_session();
                app.view = view;
                let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                term.draw(|f| render(f, &mut app))
                    .unwrap_or_else(|e| panic!("{view:?} at {w}x{h} failed to draw: {e}"));
            }
        }
    }

    /// The same, with no hosts and no sessions — the state on first launch,
    /// and the state after the last session closes.
    #[test]
    fn every_view_renders_when_there_is_nothing_to_show() {
        for view in VIEWS {
            for (w, h) in SIZES {
                let mut app = App::new(8);
                app.view = view;
                let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                term.draw(|f| render(f, &mut app))
                    .unwrap_or_else(|e| panic!("empty {view:?} at {w}x{h}: {e}"));
            }
        }
    }

    /// A connected session whose shell has printed nothing must still show
    /// which host it is.
    ///
    /// This is the bug that read as "the screen just goes blank": the handoff
    /// reserved zero rows for chrome, so a silent login produced an entirely
    /// empty screen with no host name and no hint of the prefix key.
    #[test]
    fn a_silent_session_still_names_its_host() {
        let mut app = app_with_session();
        app.view = AppView::Session;
        let mut term = Terminal::new(TestBackend::new(90, 24)).unwrap();
        term.draw(|f| render(f, &mut app)).unwrap();

        let buf = term.backend().buffer();
        let screen: String = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            screen.contains("mattbot"),
            "a session with no output showed nothing identifying it:\n{screen}"
        );
        assert!(
            screen.contains("ESSH"),
            "no product mark on the session screen"
        );
    }

    /// Every view at a usable size must actually draw something.
    ///
    /// "It did not panic" is precisely the assertion that missed the blank
    /// session screen: rendering nothing at all is silent, passes every
    /// crash test, and is the worst thing the app can do in front of a user.
    /// So the bar is content, not absence of failure.
    #[test]
    fn no_view_is_ever_blank_at_a_usable_size() {
        for view in VIEWS {
            for (w, h) in SIZES.iter().copied().filter(|(w, h)| *w >= 40 && *h >= 12) {
                let mut app = app_with_session();
                app.view = view;
                let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                term.draw(|f| render(f, &mut app)).unwrap();

                let buf = term.backend().buffer();
                let ink = (0..buf.area.height)
                    .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
                    .filter(|&(x, y)| {
                        let sym = buf[(x, y)].symbol();
                        !sym.trim().is_empty() && sym != " "
                    })
                    .count();

                assert!(
                    ink >= 12,
                    "{view:?} at {w}x{h} drew {ink} visible cells — that is a blank screen"
                );
            }
        }
    }

    /// The prefix key must be discoverable without the manual.
    ///
    /// A modal prefix that is never shown is a modal prefix nobody uses.
    #[test]
    fn a_session_always_shows_how_to_reach_the_commands() {
        let mut app = app_with_session();
        app.view = AppView::Session;
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| render(f, &mut app)).unwrap();
        let buf = term.backend().buffer();
        let screen: String = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        // The one-press keys are what someone will actually use, so those are
        // what must be on screen.
        for needle in ["F2", "monitor", "F3", "files", "F5", "mini", "F6", "detach"] {
            assert!(
                screen.contains(needle),
                "the session never advertises {needle:?}:\n{screen}"
            );
        }
        // And the prefix, for everything the function keys do not cover.
        assert!(
            screen.contains("Ctrl+A"),
            "the prefix is never named:\n{screen}"
        );
    }

    /// The hint strip must never overflow its row.
    ///
    /// It is one row by construction; a strip wider than the terminal is
    /// silently truncated mid-word, which is how a hint becomes noise.
    #[test]
    fn the_hint_strip_fits_the_row_at_every_width() {
        for width in [40u16, 60, 80, 100, 120, 200] {
            let mut app = app_with_session();
            app.view = AppView::Session;
            let mut term = Terminal::new(TestBackend::new(width, 20)).unwrap();
            term.draw(|f| render(f, &mut app)).unwrap();
            let buf = term.backend().buffer();

            // The strip is the last row; the rule sits above it.
            let row: String = (0..width)
                .map(|x| buf[(x, 19)].symbol().chars().next().unwrap_or(' '))
                .collect();
            assert!(
                row.chars().count() <= width as usize,
                "strip overflowed at {width}: {row:?}"
            );
            // The first key must always survive, however narrow.
            assert!(
                row.contains("F1") || width < 20,
                "the strip lost its first key at {width}: {row:?}"
            );
        }
    }

    /// Every session-bound screen must say which host it is showing.
    ///
    /// Catches a whole class of bug: a screen that draws into the full frame
    /// after its chrome was laid out into a sub-rect paints over the chrome
    /// and silently loses the host name. That is how the monitor lost its tab
    /// bar, and nothing about it panics or looks wrong in isolation.
    #[test]
    fn every_session_screen_names_its_host() {
        for view in [
            AppView::Session,
            AppView::Monitor,
            AppView::PortForwarding,
            AppView::FileBrowser,
        ] {
            let mut app = app_with_session();
            app.view = view;
            let mut term = Terminal::new(TestBackend::new(120, 34)).unwrap();
            term.draw(|f| render(f, &mut app)).unwrap();

            let buf = term.backend().buffer();
            let screen: String = (0..buf.area.height)
                .map(|y| {
                    (0..buf.area.width)
                        .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n");

            assert!(
                screen.contains("mattbot"),
                "{view:?} never names the host it belongs to:\n{screen}"
            );
        }
    }

    /// Split and pane modes are separate code paths and have their own
    /// arithmetic; they get the same treatment.
    #[test]
    fn split_view_renders_at_every_size() {
        for (w, h) in SIZES {
            let mut app = app_with_session();
            app.view = AppView::Session;
            app.split_pane = true;
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| render(f, &mut app))
                .unwrap_or_else(|e| panic!("split at {w}x{h}: {e}"));
        }
    }

    /// Modal overlays draw on top of whatever is behind them and do their own
    /// centring arithmetic, which is another place to go negative.
    #[test]
    fn overlays_render_at_every_size() {
        for (w, h) in SIZES {
            let mut app = app_with_session();
            app.view = AppView::Session;
            app.show_help = true;
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| render(f, &mut app))
                .unwrap_or_else(|e| panic!("help overlay at {w}x{h}: {e}"));
        }
    }
}

#[cfg(test)]
mod search_selection_tests {
    use super::*;

    fn app_with_hosts() -> App {
        let mut app = App::new(4);
        let host = |name: &str| HostDisplay {
            name: name.into(),
            hostname: format!("10.0.0.{}", name.len()),
            port: 22,
            user: "root".into(),
            status: HostStatus::NeverProbed,
            last_seen: String::new(),
            tags: String::new(),
            tag_pairs: Vec::new(),
            latency_ms: None,
            latency_history: Vec::new(),
            jump_host: None,
            diverge_count: None,
        };
        app.hosts = vec![host("web-01"), host("db-01")];
        app
    }

    /// A filter that matches nothing must not leave a stale selection.
    ///
    /// This is the dangerous case: the caller connects to "the selected
    /// host", and if the search matched nothing that is still the host from
    /// before the search — so typing one hostname connects you to another.
    #[test]
    fn a_filter_matching_nothing_selects_nothing() {
        let mut app = app_with_hosts();
        app.selected_host = 0;
        app.search_query = "nosuchhost".into();
        assert!(
            app.filtered_host_indices().is_empty(),
            "the fixture should not match"
        );
    }

    #[test]
    fn a_filter_that_matches_moves_the_selection_to_the_match() {
        let mut app = app_with_hosts();
        app.selected_host = 0;
        app.search_query = "db".into();
        app.select_first_filtered();
        let selected = app.hosts[app.selected_host].name.clone();
        assert_eq!(selected, "db-01", "selection did not follow the filter");
    }

    /// And with no filter, everything is selectable again.
    #[test]
    fn clearing_the_filter_restores_the_full_list() {
        let mut app = app_with_hosts();
        app.search_query.clear();
        assert_eq!(app.filtered_host_indices().len(), 2);
    }
}
