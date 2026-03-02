mod audit;
mod cache;
mod cli;
mod config;
mod diagnostics;
mod event;
mod filetransfer;
mod fleet;
mod monitor;
mod notify;
mod portfwd;
mod recording;
mod session;
mod ssh;
mod tui;

use std::io::{self, Read as _, Write as _};
use std::time::Duration;

use clap::Parser;
use crossterm::{
    event::{KeyCode, KeyModifiers},
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::prelude::*;

use audit::{AuditEventType, AuditLogger};
use cache::{CacheDb, HostKeyStatus};
use cli::{AuditAction, Cli, Commands, ConfigAction, HostsAction, KeysAction, SessionAction};
use config::{AppConfig, TofuPolicy};
use diagnostics::DiagnosticsEngine;
use event::{AppEvent, EventHandler};
use session::{Session, SessionState};
use ssh::{AuthMethod, ConnectConfig, SshClient, SshSession};
use notify::NotificationMatcher;
use tui::{App, AppView, DashboardTab, HostDisplay, HostStatus, Notification};

// ---------------------------------------------------------------------------
// Session runtime data (held alongside the TUI App)
// ---------------------------------------------------------------------------

enum SessionInput {
    Data(Vec<u8>),
    Resize { cols: u32, rows: u32 },
}

struct SessionRuntime {
    ssh_session: SshSession,
    channel_tx: tokio::sync::mpsc::Sender<SessionInput>,
    diagnostics: DiagnosticsEngine,
    monitor: Option<monitor::HostMetricsCollector>,
    connect_config: ConnectConfig,
}

/// Tracks reconnect state for a session during exponential backoff.
struct ReconnectTracker {
    attempt: u32,
    max_retries: u32,
    last_attempt: std::time::Instant,
    backoff_secs: u64,
}

impl ReconnectTracker {
    fn new(max_retries: u32) -> Self {
        Self {
            attempt: 0,
            max_retries,
            last_attempt: std::time::Instant::now(),
            backoff_secs: 0, // first retry immediate, then 1s, 2s, 4s...
        }
    }

    fn should_retry(&self) -> bool {
        self.attempt <= self.max_retries
            && self.last_attempt.elapsed() >= Duration::from_secs(self.backoff_secs)
    }

    fn record_attempt(&mut self) {
        self.attempt += 1;
        self.last_attempt = std::time::Instant::now();
        self.backoff_secs = (1u64 << self.attempt.min(5)).min(30); // 1, 2, 4, 8, 16, 30
    }

    fn exhausted(&self) -> bool {
        self.attempt > self.max_retries
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    AppConfig::ensure_dirs()?;
    let app_config = AppConfig::load()?;

    match cli.command {
        None => run_tui(app_config).await,
        Some(cmd) => run_command(cmd, app_config).await,
    }
}

// ---------------------------------------------------------------------------
// CLI command dispatch (unchanged from original)
// ---------------------------------------------------------------------------

async fn run_command(cmd: Commands, config: AppConfig) -> anyhow::Result<()> {
    let audit = AuditLogger::default_logger();

    match cmd {
        Commands::Connect {
            target,
            port,
            identity,
            password,
        } => {
            let (user, host) = parse_target(&target, &config);
            let auth = if password {
                let pw = prompt_password(&format!("{}@{}'s password: ", user, host))?;
                AuthMethod::Password(pw)
            } else if let Some(key_path) = identity {
                AuthMethod::KeyFile(key_path)
            } else if let Some(ref default_key) = config.general.default_key {
                let expanded = shellexpand::tilde(default_key).to_string();
                AuthMethod::KeyFile(expanded.into())
            } else if std::env::var("SSH_AUTH_SOCK").is_ok() {
                AuthMethod::Agent
            } else {
                let pw = prompt_password(&format!("{}@{}'s password: ", user, host))?;
                AuthMethod::Password(pw)
            };

            let connect_config = ConnectConfig {
                hostname: host.clone(),
                port,
                username: user.clone(),
                auth,
            };

            connect_and_shell(connect_config, &config, &audit).await?;
        }

        Commands::Hosts { action } => match action {
            HostsAction::List { tag } => {
                let db = CacheDb::open_default()?;
                let hosts = if let Some(tag_str) = tag {
                    let (k, v) = tag_str
                        .split_once('=')
                        .ok_or_else(|| anyhow::anyhow!("Tag must be key=value"))?;
                    db.find_hosts_by_tag(k, v)?
                } else {
                    db.list_hosts()?
                };

                if hosts.is_empty() {
                    println!("No cached hosts.");
                } else {
                    println!(
                        "{:<20} {:<30} {:<6} {:<16} {:<24} {}",
                        "Fingerprint", "Hostname", "Port", "Key Type", "Last Seen", "Tags"
                    );
                    println!("{}", "-".repeat(110));
                    for h in &hosts {
                        let tags: Vec<String> =
                            h.tags.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
                        println!(
                            "{:<20} {:<30} {:<6} {:<16} {:<24} {}",
                            &h.fingerprint[..20.min(h.fingerprint.len())],
                            h.hostname,
                            h.port,
                            h.key_type,
                            h.last_seen,
                            tags.join(", ")
                        );
                    }
                }
            }
            HostsAction::Add {
                hostname,
                port,
                name: _,
                user: _,
                tag,
            } => {
                let db = CacheDb::open_default()?;
                let tags: std::collections::HashMap<String, String> = tag
                    .iter()
                    .filter_map(|t| t.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
                    .collect();
                db.trust_host(&hostname, None, port, "unknown", "unknown")?;
                if !tags.is_empty() {
                    db.set_host_tags(&hostname, port, &tags)?;
                }
                println!("Host {} added to cache.", hostname);
            }
            HostsAction::Remove { hostname, port } => {
                let db = CacheDb::open_default()?;
                if db.remove_host(&hostname, port)? {
                    println!("Host {} removed.", hostname);
                } else {
                    println!("Host {} not found in cache.", hostname);
                }
            }
            HostsAction::Import { path } => {
                let ssh_config_path = path.unwrap_or_else(|| {
                    dirs::home_dir()
                        .expect("home dir")
                        .join(".ssh")
                        .join("config")
                });
                import_ssh_config(&ssh_config_path)?;
            }
            HostsAction::Health { group } => {
                health_check(&config, group.as_deref()).await?;
            }
        },

        Commands::Keys { action } => match action {
            KeysAction::List => {
                let db = CacheDb::open_default()?;
                let keys = db.list_keys()?;
                if keys.is_empty() {
                    println!("No cached keys.");
                } else {
                    println!(
                        "{:<20} {:<40} {:<12} {:<24}",
                        "Name", "Path", "Type", "Added"
                    );
                    println!("{}", "-".repeat(96));
                    for k in &keys {
                        println!(
                            "{:<20} {:<40} {:<12} {:<24}",
                            k.name, k.path, k.key_type, k.added_at
                        );
                    }
                }
            }
            KeysAction::Add { path, name } => {
                let db = CacheDb::open_default()?;
                let key_name = name.unwrap_or_else(|| {
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                });
                let key = russh_keys::load_secret_key(&path, None)?;
                let key_type = format!("{:?}", key);
                let key_type_short = key_type.split('(').next().unwrap_or("unknown").to_string();
                db.add_key(
                    &key_name,
                    &path.to_string_lossy(),
                    &key_type_short,
                    &key_type_short,
                )?;
                println!("Key '{}' added.", key_name);
            }
            KeysAction::Remove { name } => {
                let db = CacheDb::open_default()?;
                if db.remove_key(&name)? {
                    println!("Key '{}' removed.", name);
                } else {
                    println!("Key '{}' not found.", name);
                }
            }
        },

        Commands::Session { action } => match action {
            SessionAction::List => {
                // List recordings
                match recording::list_recordings() {
                    Ok(recs) if recs.is_empty() => {
                        println!("No recordings found.");
                        // Fall back to listing diagnostics sessions
                        let session_dir = AppConfig::data_dir().join("sessions");
                        if session_dir.exists() {
                            let mut count = 0;
                            for entry in std::fs::read_dir(&session_dir)? {
                                let entry = entry?;
                                println!("  {}", entry.file_name().to_string_lossy());
                                count += 1;
                            }
                            if count > 0 {
                                println!("({} diagnostics session logs)", count);
                            }
                        }
                    }
                    Ok(recs) => {
                        println!("{:<40} {}", "Session ID", "File");
                        println!("{}", "-".repeat(80));
                        for (name, path) in &recs {
                            println!("{:<40} {}", name, path.display());
                        }
                        println!("\n{} recording(s) found.", recs.len());
                    }
                    Err(e) => println!("Error listing recordings: {}", e),
                }
            }
            SessionAction::Replay { id } => {
                let cast_path = recording::recording_path(&id);
                if cast_path.exists() {
                    replay_recording(&cast_path).await?;
                } else {
                    // Fall back to diagnostics replay
                    let diag_path = AppConfig::data_dir()
                        .join("sessions")
                        .join(format!("{}.jsonl", id));
                    if diag_path.exists() {
                        let content = std::fs::read_to_string(&diag_path)?;
                        for line in content.lines() {
                            let snap: diagnostics::DiagnosticsSnapshot =
                                serde_json::from_str(line)?;
                            println!(
                                "[{}] RTT={:?}ms  ↑{:.0}B/s  ↓{:.0}B/s  Loss={:.1}%  Quality={:?}",
                                snap.timestamp,
                                snap.rtt_ms,
                                snap.throughput_up_bps,
                                snap.throughput_down_bps,
                                snap.packet_loss_pct,
                                snap.quality
                            );
                        }
                    } else {
                        println!("Session {} not found.", id);
                        println!("Checked: {}", cast_path.display());
                        println!("         {}", diag_path.display());
                    }
                }
            }
        },

        Commands::Diag { session_id } => {
            let path = AppConfig::data_dir()
                .join("sessions")
                .join(format!("{}.jsonl", session_id));
            if path.exists() {
                let content = std::fs::read_to_string(&path)?;
                let lines: Vec<&str> = content.lines().collect();
                if let Some(last) = lines.last() {
                    let snap: diagnostics::DiagnosticsSnapshot = serde_json::from_str(last)?;
                    println!("Session: {}", snap.session_id);
                    println!("Last snapshot: {}", snap.timestamp);
                    println!("RTT: {:?} ms", snap.rtt_ms);
                    println!("Bytes sent: {}", snap.bytes_sent);
                    println!("Bytes received: {}", snap.bytes_received);
                    println!("Throughput ↑: {:.1} B/s", snap.throughput_up_bps);
                    println!("Throughput ↓: {:.1} B/s", snap.throughput_down_bps);
                    println!("Packet loss: {:.1}%", snap.packet_loss_pct);
                    println!("Quality: {:?}", snap.quality);
                    println!("Uptime: {}s", snap.uptime_secs);
                }
            } else {
                println!("No diagnostics found for session {}.", session_id);
            }
        }

        Commands::Run { group, command } => {
            run_on_group(&config, &group, &command).await?;
        }

        Commands::Config { action } => match action {
            ConfigAction::Edit => {
                let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
                let path = AppConfig::data_dir().join("config.toml");
                if !path.exists() {
                    config.save()?;
                }
                std::process::Command::new(editor).arg(&path).status()?;
            }
            ConfigAction::Show => {
                let toml_str = toml::to_string_pretty(&config)?;
                println!("{}", toml_str);
            }
            ConfigAction::Init => {
                let default = AppConfig::default();
                default.save()?;
                println!(
                    "Default config written to {}",
                    AppConfig::data_dir().join("config.toml").display()
                );
            }
        },

        Commands::Audit { action } => match action {
            AuditAction::Tail { lines } => {
                let audit = AuditLogger::default_logger();
                match audit.tail(lines) {
                    Ok(events) => {
                        for e in &events {
                            println!(
                                "[{}] {:?} host={} session={}",
                                e.timestamp,
                                e.event_type,
                                e.hostname.as_deref().unwrap_or("-"),
                                e.session_id.as_deref().unwrap_or("-"),
                            );
                        }
                    }
                    Err(e) => println!("Could not read audit log: {}", e),
                }
            }
        },
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Direct CLI SSH connect + interactive shell (non-TUI mode)
// ---------------------------------------------------------------------------

async fn connect_and_shell(
    connect_config: ConnectConfig,
    app_config: &AppConfig,
    audit: &AuditLogger,
) -> anyhow::Result<()> {
    let session_id = uuid::Uuid::new_v4().to_string();

    audit.log_connection_attempt(
        &session_id,
        &connect_config.hostname,
        connect_config.port,
        &connect_config.username,
    );

    println!(
        "Connecting to {}@{}:{}...",
        connect_config.username, connect_config.hostname, connect_config.port
    );

    let (mut session, fingerprint, banner) = SshClient::connect(&connect_config).await?;

    // TOFU host key check
    let db = CacheDb::open_default()?;
    let status = db.check_host_key(&connect_config.hostname, connect_config.port, &fingerprint)?;
    handle_tofu(&db, &connect_config, &fingerprint, &status, app_config, audit, &session_id)?;

    db.update_last_seen(&connect_config.hostname, connect_config.port)?;

    audit.log_auth_result(
        &session_id,
        &connect_config.hostname,
        connect_config.port,
        &connect_config.username,
        &format!("{:?}", connect_config.auth),
        true,
    );

    if let Some(ref b) = banner {
        println!("Server banner: {}", b.trim());
    }

    let log_dir = AppConfig::data_dir().join("sessions");
    let diag = DiagnosticsEngine::new(
        &session_id,
        &connect_config.hostname,
        connect_config.port,
        Some(log_dir.as_path()),
    );
    diag.set_connection_info(
        banner.clone(),
        None,
        None,
        None,
        None,
        Some(format!("{:?}", connect_config.auth)),
    )
    .await;

    let (cols, rows) = terminal::size()?;
    let channel = session
        .open_shell("xterm-256color", cols as u32, rows as u32)
        .await?;

    audit.log_session_event(
        &session_id,
        &connect_config.hostname,
        connect_config.port,
        AuditEventType::SessionStart,
    );

    println!("Connected. Session ID: {}", session_id);

    run_interactive_shell(channel, diag).await?;

    audit.log_session_event(
        &session_id,
        &connect_config.hostname,
        connect_config.port,
        AuditEventType::SessionEnd,
    );

    session.close().await.ok();
    println!("Session ended.");
    Ok(())
}

async fn run_interactive_shell(
    mut channel: russh::Channel<russh::client::Msg>,
    diag: DiagnosticsEngine,
) -> anyhow::Result<()> {
    let is_tty = std::io::IsTerminal::is_terminal(&io::stdin());
    if is_tty {
        terminal::enable_raw_mode()?;
    }
    let _raw_guard = scopeguard::guard((), move |_| {
        if is_tty {
            terminal::disable_raw_mode().ok();
        }
    });

    // Diagnostics logging task
    let diag_metrics = diag.metrics();
    let log_dir = AppConfig::data_dir().join("sessions");
    let session_id_for_log = diag_metrics.read().await.session_id.clone();
    let log_engine = DiagnosticsEngine::new(
        &session_id_for_log,
        &diag_metrics.read().await.hostname,
        diag_metrics.read().await.port,
        Some(log_dir.as_path()),
    );

    let diag_metrics_clone = diag.metrics();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            let _ = log_engine.write_log_entry().await;
            let _m = diag_metrics_clone.read().await;
        }
    });

    // Stdin reader task
    let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(32);
    std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        let stdin = io::stdin();
        let mut handle = stdin.lock();
        loop {
            match handle.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if stdin_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    loop {
        tokio::select! {
            Some(msg) = channel.wait() => {
                match msg {
                    russh::ChannelMsg::Data { data } => {
                        let bytes = data.to_vec();
                        diag.record_bytes_received(bytes.len() as u64).await;
                        io::stdout().write_all(&bytes)?;
                        io::stdout().flush()?;
                    }
                    russh::ChannelMsg::ExtendedData { data, ext } => {
                        if ext == 1 {
                            let bytes = data.to_vec();
                            io::stderr().write_all(&bytes)?;
                            io::stderr().flush()?;
                        }
                    }
                    russh::ChannelMsg::Eof | russh::ChannelMsg::Close => break,
                    russh::ChannelMsg::ExitStatus { exit_status } => {
                        if exit_status != 0 {
                            eprintln!("\r\nRemote process exited with status {}", exit_status);
                        }
                        break;
                    }
                    _ => {}
                }
            }
            Some(data) = stdin_rx.recv() => {
                diag.record_bytes_sent(data.len() as u64).await;
                channel.data(&data[..]).await?;
            }
            else => break,
        }
    }

    diag.write_log_entry().await.ok();
    Ok(())
}

// ---------------------------------------------------------------------------
// TUI mode — concurrent sessions
// ---------------------------------------------------------------------------

async fn run_tui(config: AppConfig) -> anyhow::Result<()> {
    let mut app = App::new(config.session.max_concurrent);

    load_hosts_into_app(&mut app, &config)?;

    io::stdout().execute(EnterAlternateScreen)?;
    terminal::enable_raw_mode()?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = tui_main_loop(&mut terminal, &mut app, &config).await;

    terminal::disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    result
}

async fn tui_main_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    config: &AppConfig,
) -> anyhow::Result<()> {
    let mut events = EventHandler::new(Duration::from_millis(100));
    let audit = AuditLogger::default_logger();

    // Session runtime data, indexed same as session_manager.sessions
    let mut runtimes: Vec<Option<SessionRuntime>> = Vec::new();

    // Per-session channel receivers for remote output
    let mut session_output_rxs: Vec<Option<tokio::sync::mpsc::Receiver<Vec<u8>>>> = Vec::new();

    // Tick counter for periodic monitor collection
    let mut tick_count: u64 = 0;

    // Per-session reconnect trackers (indexed same as sessions)
    let mut reconnect_trackers: Vec<Option<ReconnectTracker>> = Vec::new();

    // Notification matcher for background session output
    let notification_matcher = NotificationMatcher::new(&config.session.notification_patterns);

    // Fleet health prober
    let mut fleet_prober = fleet::FleetProber::new(
        config.fleet.probe_interval,
        config.fleet.probe_timeout,
        config.fleet.latency_history_samples,
    );

    loop {
        // Draw
        terminal.draw(|frame| {
            tui::render(frame, app);
        })?;

        // Poll session output (non-blocking drain from all active sessions)
        for (i, rx_opt) in session_output_rxs.iter_mut().enumerate() {
            if let Some(rx) = rx_opt {
                loop {
                    match rx.try_recv() {
                        Ok(data) => {
                            if let Some(session) = app.session_manager.sessions.get_mut(i) {
                                session.terminal.process(&data);
                                if app.session_manager.active_index != Some(i) {
                                    session.has_new_output = true;
                                    if !notification_matcher.is_empty() {
                                        let text = String::from_utf8_lossy(&data);
                                        if let Some(matched) = notification_matcher.check(&text) {
                                            app.notifications.push(Notification {
                                                session_label: session.label.clone(),
                                                matched_text: matched,
                                                timestamp: chrono::Local::now(),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                            // Channel I/O task exited — session disconnected
                            if let Some(session) = app.session_manager.sessions.get(i) {
                                if matches!(session.state, SessionState::Active) {
                                    let should_reconnect = config.session.auto_reconnect
                                        && runtimes.get(i).and_then(|r| r.as_ref()).is_some();
                                    if should_reconnect {
                                        let max = config.session.reconnect_max_retries;
                                        if let Some(session) = app.session_manager.sessions.get_mut(i) {
                                            session.state = SessionState::Reconnecting { attempt: 1, max };
                                        }
                                        while reconnect_trackers.len() <= i {
                                            reconnect_trackers.push(None);
                                        }
                                        reconnect_trackers[i] = Some(ReconnectTracker::new(max));
                                    } else if let Some(session) = app.session_manager.sessions.get_mut(i) {
                                        session.state = SessionState::Disconnected {
                                            reason: "Connection lost".to_string(),
                                        };
                                    }
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }

        // Handle events
        match events.next().await? {
            AppEvent::Key(key) => {
                let handled = handle_key_event(
                    key,
                    app,
                    config,
                    &audit,
                    &mut runtimes,
                    &mut session_output_rxs,
                    &mut reconnect_trackers,
                )
                .await?;
                if handled == KeyAction::Quit {
                    // Close all sessions
                    for rt in runtimes.iter_mut().flatten() {
                        rt.ssh_session.close().await.ok();
                    }
                    return Ok(());
                }
            }
            AppEvent::Tick => {
                tick_count += 1;

                // Update diagnostics snapshots every 10 ticks (1s)
                if tick_count % 10 == 0 {
                    for (i, rt_opt) in runtimes.iter().enumerate() {
                        if let Some(rt) = rt_opt {
                            let snap = rt.diagnostics.snapshot().await;
                            if let Some(slot) = app.session_diagnostics.get_mut(i) {
                                *slot = Some(snap);
                            }
                        }
                    }
                }

                // Collect host metrics every 20 ticks (2s)
                if tick_count % 20 == 0 {
                    for (i, rt_opt) in runtimes.iter().enumerate() {
                        if let Some(rt) = rt_opt {
                            if let Some(ref mon) = rt.monitor {
                                let handle = &rt.ssh_session.handle;
                                // Non-blocking: spawn collection as a task
                                let metrics_arc = mon.metrics();
                                let cpu_h = mon.cpu_history();
                                let mem_h = mon.mem_history();
                                let rx_h = mon.net_rx_history();
                                let tx_h = mon.net_tx_history();
                                // We do a quick clone of handle for the spawned task
                                // Actually we can't easily clone Handle, so collect inline
                                if let Err(_) = mon.collect(handle).await {
                                    // Metric collection failed, skip
                                }
                                // Update app state from collector
                                if let Some(slot) = app.session_metrics.get_mut(i) {
                                    *slot = Some(metrics_arc.read().await.clone());
                                }
                                if let Some(h) = app.session_cpu_history.get_mut(i) {
                                    *h = cpu_h.read().await.clone();
                                }
                                if let Some(h) = app.session_mem_history.get_mut(i) {
                                    *h = mem_h.read().await.clone();
                                }
                                if let Some(h) = app.session_net_rx_history.get_mut(i) {
                                    *h = rx_h.read().await.clone();
                                }
                                if let Some(h) = app.session_net_tx_history.get_mut(i) {
                                    *h = tx_h.read().await.clone();
                                }
                            }
                        }
                    }
                }

                // Auto-reconnect: attempt reconnection for disconnected sessions
                for i in 0..app.session_manager.sessions.len() {
                    let is_reconnecting = matches!(
                        app.session_manager.sessions.get(i).map(|s| &s.state),
                        Some(SessionState::Reconnecting { .. })
                    );
                    if !is_reconnecting {
                        continue;
                    }

                    let tracker = reconnect_trackers
                        .get_mut(i)
                        .and_then(|t| t.as_mut());
                    let tracker = match tracker {
                        Some(t) => t,
                        None => continue,
                    };

                    if tracker.exhausted() {
                        if let Some(session) = app.session_manager.sessions.get_mut(i) {
                            session.state = SessionState::Disconnected {
                                reason: "Reconnect failed (max retries)".to_string(),
                            };
                        }
                        reconnect_trackers[i] = None;
                        continue;
                    }

                    if !tracker.should_retry() {
                        continue;
                    }

                    // Get connect config from current runtime
                    let connect_config = match runtimes.get(i).and_then(|r| r.as_ref()) {
                        Some(rt) => rt.connect_config.clone(),
                        None => continue,
                    };

                    tracker.record_attempt();
                    let attempt = tracker.attempt;
                    let max = tracker.max_retries;

                    if let Some(session) = app.session_manager.sessions.get_mut(i) {
                        session.state = SessionState::Reconnecting { attempt, max };
                    }
                    app.set_status(format!(
                        "Reconnecting to {} ({}/{})...",
                        connect_config.hostname, attempt, max
                    ));

                    // Attempt reconnect
                    match reconnect_session(
                        i,
                        &connect_config,
                        config,
                        &mut runtimes,
                        &mut session_output_rxs,
                    )
                    .await
                    {
                        Ok(()) => {
                            if let Some(session) = app.session_manager.sessions.get_mut(i) {
                                session.state = SessionState::Active;
                            }
                            reconnect_trackers[i] = None;
                            app.set_status(format!("Reconnected to {}", connect_config.hostname));
                        }
                        Err(_) => {
                            // Will retry on next tick cycle
                        }
                    }
                }

                // Fleet health probes
                if config.fleet.probe_enabled && fleet_prober.should_probe() {
                    let hosts: Vec<(String, u16)> = app
                        .hosts
                        .iter()
                        .map(|h| (h.hostname.clone(), h.port))
                        .collect();
                    if !hosts.is_empty() {
                        fleet_prober.probe_all(&hosts).await;
                        // Update host displays with probe results
                        for host in app.hosts.iter_mut() {
                            if let Some(state) =
                                fleet_prober.get_state(&host.hostname, host.port)
                            {
                                if state.result.online {
                                    host.status = HostStatus::Online;
                                    host.latency_ms = state.result.latency_ms;
                                } else {
                                    host.status = HostStatus::Offline;
                                    host.latency_ms = None;
                                }
                                host.latency_history = state.latency_history.clone();
                            }
                        }
                    }
                }
            }
            AppEvent::Resize(w, h) => {
                // Forward PTY resize to all active sessions
                for rt_opt in runtimes.iter() {
                    if let Some(rt) = rt_opt {
                        rt.channel_tx.send(SessionInput::Resize {
                            cols: w as u32,
                            rows: h as u32,
                        }).await.ok();
                    }
                }
            }
        }
    }
}

#[derive(PartialEq)]
enum KeyAction {
    Handled,
    Quit,
}

async fn handle_key_event(
    key: crossterm::event::KeyEvent,
    app: &mut App,
    config: &AppConfig,
    audit: &AuditLogger,
    runtimes: &mut Vec<Option<SessionRuntime>>,
    output_rxs: &mut Vec<Option<tokio::sync::mpsc::Receiver<Vec<u8>>>>,
    reconnect_trackers: &mut Vec<Option<ReconnectTracker>>,
) -> anyhow::Result<KeyAction> {
    // Command palette — intercept before everything else
    if app.command_palette.is_some() {
        return handle_palette_key(key, app, config, audit, runtimes, output_rxs).await;
    }

    // Ctrl+P: open command palette (from any view)
    if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
        let mut palette = tui::command_palette::CommandPalette::new();
        palette.update(
            &app.hosts,
            &app.session_manager.sessions,
            app.session_manager.has_sessions(),
        );
        app.command_palette = Some(palette);
        return Ok(KeyAction::Handled);
    }

    // Help toggle — intercept before anything else
    if app.show_help {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc => app.show_help = false,
            _ => {}
        }
        return Ok(KeyAction::Handled);
    }
    if key.code == KeyCode::Char('?') && !key.modifiers.contains(KeyModifiers::ALT) {
        // Don't intercept '?' when in an active session (it should go to remote shell)
        if app.view != AppView::Session {
            app.show_help = true;
            return Ok(KeyAction::Handled);
        }
    }

    let is_alt = key.modifiers.contains(KeyModifiers::ALT);

    // Global keybindings (work in all views)
    if is_alt {
        match key.code {
            // Alt+1 through Alt+9: switch to session
            KeyCode::Char(c @ '1'..='9') => {
                let idx = (c as usize) - ('1' as usize);
                if app.session_manager.switch_to(idx) {
                    if let Some(session) = app.session_manager.sessions.get(idx) {
                        let label = session.label.clone();
                        app.notifications.retain(|n| n.session_label != label);
                    }
                    app.view = AppView::Session;
                }
                return Ok(KeyAction::Handled);
            }
            // Alt+Left: previous session
            KeyCode::Left => {
                app.session_manager.switch_prev();
                if app.session_manager.has_sessions() {
                    if let Some(session) = app.session_manager.active_session() {
                        let label = session.label.clone();
                        app.notifications.retain(|n| n.session_label != label);
                    }
                    app.view = AppView::Session;
                }
                return Ok(KeyAction::Handled);
            }
            // Alt+Right: next session
            KeyCode::Right => {
                app.session_manager.switch_next();
                if app.session_manager.has_sessions() {
                    if let Some(session) = app.session_manager.active_session() {
                        let label = session.label.clone();
                        app.notifications.retain(|n| n.session_label != label);
                    }
                    app.view = AppView::Session;
                }
                return Ok(KeyAction::Handled);
            }
            // Alt+Tab: last session
            KeyCode::Tab => {
                app.session_manager.switch_last();
                if app.session_manager.has_sessions() {
                    if let Some(session) = app.session_manager.active_session() {
                        let label = session.label.clone();
                        app.notifications.retain(|n| n.session_label != label);
                    }
                    app.view = AppView::Session;
                }
                return Ok(KeyAction::Handled);
            }
            // Alt+m: toggle monitor (µ is Option+m on macOS)
            KeyCode::Char('m') | KeyCode::Char('µ') => {
                if app.session_manager.has_sessions() {
                    app.view = if app.view == AppView::Monitor {
                        AppView::Session
                    } else {
                        app.monitor_process_scroll = 0;
                        AppView::Monitor
                    };
                }
                return Ok(KeyAction::Handled);
            }
            // Alt+s: toggle split-pane view (ß is Option+s on macOS)
            KeyCode::Char('s') | KeyCode::Char('ß') => {
                if app.session_manager.has_sessions() {
                    app.split_pane = !app.split_pane;
                }
                return Ok(KeyAction::Handled);
            }
            // Alt+[: shrink terminal pane (more monitor)
            KeyCode::Char('[') => {
                if app.split_pane {
                    app.split_pane_pct = app.split_pane_pct.saturating_sub(5).max(20);
                }
                return Ok(KeyAction::Handled);
            }
            // Alt+]: grow terminal pane (less monitor)
            KeyCode::Char(']') => {
                if app.split_pane {
                    app.split_pane_pct = (app.split_pane_pct + 5).min(80);
                }
                return Ok(KeyAction::Handled);
            }
            // Alt+d: detach (go back to dashboard) (∂ is Option+d on macOS)
            KeyCode::Char('d') | KeyCode::Char('∂') => {
                app.view = AppView::Dashboard;
                return Ok(KeyAction::Handled);
            }
            // Alt+h or Alt+?: toggle help (˙ is Option+h on macOS)
            KeyCode::Char('h') | KeyCode::Char('?') | KeyCode::Char('˙') => {
                app.show_help = !app.show_help;
                return Ok(KeyAction::Handled);
            }
            // Alt+p: toggle port forwarding manager (π is Option+p on macOS)
            KeyCode::Char('p') | KeyCode::Char('π') => {
                if app.session_manager.has_sessions() {
                    app.view = if app.view == AppView::PortForwarding {
                        app.port_forward_adding = false;
                        app.port_forward_input.clear();
                        AppView::Session
                    } else {
                        AppView::PortForwarding
                    };
                }
                return Ok(KeyAction::Handled);
            }
            // Alt+f: toggle file browser (ƒ is Option+f on macOS)
            KeyCode::Char('f') | KeyCode::Char('ƒ') => {
                if app.session_manager.has_sessions() {
                    if app.view == AppView::FileBrowser {
                        app.view = AppView::Session;
                        app.file_browser = None;
                    } else {
                        let mut browser = filetransfer::FileBrowser::new();
                        browser.list_local_files();
                        app.file_browser = Some(browser);
                        app.view = AppView::FileBrowser;
                        // Trigger remote listing
                        if let Some(idx) = app.session_manager.active_index {
                            if let Some(Some(rt)) = runtimes.get(idx) {
                                let remote_path = "/home".to_string();
                                match list_remote_files(&rt.ssh_session.handle, &remote_path).await {
                                    Ok(files) => {
                                        if let Some(ref mut fb) = app.file_browser {
                                            fb.remote_files = files;
                                        }
                                    }
                                    Err(e) => {
                                        if let Some(ref mut fb) = app.file_browser {
                                            fb.status_message = Some(format!("Remote listing failed: {}", e));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                return Ok(KeyAction::Handled);
            }
            // Alt+w: close active session (∑ is Option+w on macOS)
            KeyCode::Char('w') | KeyCode::Char('∑') => {
                if let Some(idx) = app.session_manager.active_index {
                    // Close SSH connection
                    if let Some(Some(rt)) = runtimes.get_mut(idx) {
                        rt.ssh_session.close().await.ok();
                    }
                    // Remove runtime and tracking
                    if idx < runtimes.len() {
                        runtimes.remove(idx);
                    }
                    if idx < output_rxs.len() {
                        output_rxs.remove(idx);
                    }
                    if idx < reconnect_trackers.len() {
                        reconnect_trackers.remove(idx);
                    }
                    app.session_manager.remove_session(idx);
                    app.remove_session_tracking(idx);

                    if !app.session_manager.has_sessions() {
                        app.view = AppView::Dashboard;
                    }
                }
                return Ok(KeyAction::Handled);
            }
            _ => {}
        }
    }

    // macOS: Option key sends Unicode chars without ALT modifier flag.
    // Catch them here so they work regardless of terminal emulator config.
    if !is_alt {
        match key.code {
            KeyCode::Char('µ') => {
                // Option+m: toggle monitor
                if app.session_manager.has_sessions() {
                    app.view = if app.view == AppView::Monitor {
                        AppView::Session
                    } else {
                        app.monitor_process_scroll = 0;
                        AppView::Monitor
                    };
                }
                return Ok(KeyAction::Handled);
            }
            KeyCode::Char('ß') => {
                // Option+s: toggle split-pane
                if app.session_manager.has_sessions() {
                    app.split_pane = !app.split_pane;
                }
                return Ok(KeyAction::Handled);
            }
            KeyCode::Char('∂') => {
                // Option+d: detach to dashboard
                app.view = AppView::Dashboard;
                return Ok(KeyAction::Handled);
            }
            KeyCode::Char('˙') => {
                // Option+h: toggle help
                app.show_help = !app.show_help;
                return Ok(KeyAction::Handled);
            }
            KeyCode::Char('π') => {
                // Option+p: toggle port forwarding
                if app.session_manager.has_sessions() {
                    app.view = if app.view == AppView::PortForwarding {
                        app.port_forward_adding = false;
                        app.port_forward_input.clear();
                        AppView::Session
                    } else {
                        AppView::PortForwarding
                    };
                }
                return Ok(KeyAction::Handled);
            }
            KeyCode::Char('ƒ') => {
                // Option+f: toggle file browser
                if app.session_manager.has_sessions() {
                    if app.view == AppView::FileBrowser {
                        app.view = AppView::Session;
                        app.file_browser = None;
                    } else {
                        let mut browser = filetransfer::FileBrowser::new();
                        browser.list_local_files();
                        app.file_browser = Some(browser);
                        app.view = AppView::FileBrowser;
                    }
                }
                return Ok(KeyAction::Handled);
            }
            _ => {}
        }
    }

    // View-specific keybindings
    match app.view {
        AppView::Dashboard => {
            handle_dashboard_key(key, app, config, audit, runtimes, output_rxs).await
        }
        AppView::Session => {
            handle_session_key(key, app, runtimes).await
        }
        AppView::Monitor => {
            handle_monitor_key(key, app)
        }
        AppView::PortForwarding => {
            handle_portfwd_key(key, app)
        }
        AppView::FileBrowser => {
            handle_filebrowser_key(key, app, runtimes).await
        }
    }
}

async fn handle_filebrowser_key(
    key: crossterm::event::KeyEvent,
    app: &mut App,
    runtimes: &mut Vec<Option<SessionRuntime>>,
) -> anyhow::Result<KeyAction> {
    let browser = match app.file_browser.as_mut() {
        Some(b) => b,
        None => return Ok(KeyAction::Handled),
    };

    match key.code {
        KeyCode::Tab => {
            browser.toggle_focus();
        }
        KeyCode::Down => {
            browser.next_file();
        }
        KeyCode::Up => {
            browser.prev_file();
        }
        KeyCode::Enter => {
            match browser.focus {
                filetransfer::FilePaneFocus::Local => {
                    browser.enter_dir_local();
                }
                filetransfer::FilePaneFocus::Remote => {
                    let had_dir = browser.selected_remote().map(|e| e.is_dir).unwrap_or(false);
                    if had_dir {
                        browser.enter_dir_remote();
                        // Refresh remote listing
                        let remote_path = browser.remote_path.clone();
                        if let Some(idx) = app.session_manager.active_index {
                            if let Some(Some(rt)) = runtimes.get(idx) {
                                match list_remote_files(&rt.ssh_session.handle, &remote_path).await {
                                    Ok(files) => {
                                        if let Some(ref mut fb) = app.file_browser {
                                            fb.remote_files = files;
                                        }
                                    }
                                    Err(e) => {
                                        if let Some(ref mut fb) = app.file_browser {
                                            fb.status_message = Some(format!("Error: {}", e));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        KeyCode::Backspace => {
            match browser.focus {
                filetransfer::FilePaneFocus::Local => {
                    browser.parent_local();
                }
                filetransfer::FilePaneFocus::Remote => {
                    browser.parent_remote();
                    let remote_path = browser.remote_path.clone();
                    if let Some(idx) = app.session_manager.active_index {
                        if let Some(Some(rt)) = runtimes.get(idx) {
                            match list_remote_files(&rt.ssh_session.handle, &remote_path).await {
                                Ok(files) => {
                                    if let Some(ref mut fb) = app.file_browser {
                                        fb.remote_files = files;
                                    }
                                }
                                Err(e) => {
                                    if let Some(ref mut fb) = app.file_browser {
                                        fb.status_message = Some(format!("Error: {}", e));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        KeyCode::Char('u') => {
            // Upload: local pane focused, send selected local file to remote
            if browser.focus == filetransfer::FilePaneFocus::Local {
                if let (Some(local_entry), Some(idx)) = (
                    browser.selected_local().cloned(),
                    app.session_manager.active_index,
                ) {
                    if !local_entry.is_dir {
                        let remote_dest = if browser.remote_path.ends_with('/') {
                            format!("{}{}", browser.remote_path, local_entry.name)
                        } else {
                            format!("{}/{}", browser.remote_path, local_entry.name)
                        };
                        browser.transfer = Some(filetransfer::TransferProgress {
                            filename: local_entry.name.clone(),
                            direction: filetransfer::TransferDirection::Upload,
                            bytes_transferred: 0,
                            total_bytes: local_entry.size,
                            complete: false,
                        });
                        if let Some(Some(rt)) = runtimes.get(idx) {
                            match upload_file(&rt.ssh_session.handle, &local_entry.path, &remote_dest).await {
                                Ok(_) => {
                                    if let Some(ref mut fb) = app.file_browser {
                                        if let Some(ref mut t) = fb.transfer {
                                            t.bytes_transferred = t.total_bytes;
                                            t.complete = true;
                                        }
                                        fb.status_message = Some(format!("Uploaded {}", local_entry.name));
                                        // Refresh remote
                                        let rp = fb.remote_path.clone();
                                        match list_remote_files(&rt.ssh_session.handle, &rp).await {
                                            Ok(files) => fb.remote_files = files,
                                            Err(_) => {}
                                        }
                                    }
                                }
                                Err(e) => {
                                    if let Some(ref mut fb) = app.file_browser {
                                        fb.transfer = None;
                                        fb.status_message = Some(format!("Upload failed: {}", e));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        KeyCode::Char('d') => {
            // Download: remote pane focused, download selected remote file to local
            if browser.focus == filetransfer::FilePaneFocus::Remote {
                if let (Some(remote_entry), Some(idx)) = (
                    browser.selected_remote().cloned(),
                    app.session_manager.active_index,
                ) {
                    if !remote_entry.is_dir {
                        let remote_src = if browser.remote_path.ends_with('/') {
                            format!("{}{}", browser.remote_path, remote_entry.name)
                        } else {
                            format!("{}/{}", browser.remote_path, remote_entry.name)
                        };
                        let local_dest = browser.local_path.join(&remote_entry.name);
                        browser.transfer = Some(filetransfer::TransferProgress {
                            filename: remote_entry.name.clone(),
                            direction: filetransfer::TransferDirection::Download,
                            bytes_transferred: 0,
                            total_bytes: remote_entry.size,
                            complete: false,
                        });
                        if let Some(Some(rt)) = runtimes.get(idx) {
                            match download_file(&rt.ssh_session.handle, &remote_src, &local_dest).await {
                                Ok(_) => {
                                    if let Some(ref mut fb) = app.file_browser {
                                        if let Some(ref mut t) = fb.transfer {
                                            t.bytes_transferred = t.total_bytes;
                                            t.complete = true;
                                        }
                                        fb.status_message = Some(format!("Downloaded {}", remote_entry.name));
                                        fb.list_local_files();
                                    }
                                }
                                Err(e) => {
                                    if let Some(ref mut fb) = app.file_browser {
                                        fb.transfer = None;
                                        fb.status_message = Some(format!("Download failed: {}", e));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        KeyCode::Char('m') => {
            // Mkdir on the focused pane
            match browser.focus {
                filetransfer::FilePaneFocus::Local => {
                    let new_dir = browser.local_path.join("new_folder");
                    match std::fs::create_dir(&new_dir) {
                        Ok(_) => {
                            browser.status_message = Some("Created new_folder".into());
                            browser.list_local_files();
                        }
                        Err(e) => {
                            browser.status_message = Some(format!("Mkdir failed: {}", e));
                        }
                    }
                }
                filetransfer::FilePaneFocus::Remote => {
                    let new_dir = if browser.remote_path.ends_with('/') {
                        format!("{}new_folder", browser.remote_path)
                    } else {
                        format!("{}/new_folder", browser.remote_path)
                    };
                    if let Some(idx) = app.session_manager.active_index {
                        if let Some(Some(rt)) = runtimes.get(idx) {
                            match exec_remote_command(&rt.ssh_session.handle, &format!("mkdir -p '{}'", new_dir)).await {
                                Ok(_) => {
                                    if let Some(ref mut fb) = app.file_browser {
                                        fb.status_message = Some("Created new_folder".into());
                                        let rp = fb.remote_path.clone();
                                        match list_remote_files(&rt.ssh_session.handle, &rp).await {
                                            Ok(files) => fb.remote_files = files,
                                            Err(_) => {}
                                        }
                                    }
                                }
                                Err(e) => {
                                    if let Some(ref mut fb) = app.file_browser {
                                        fb.status_message = Some(format!("Mkdir failed: {}", e));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        KeyCode::Delete => {
            match browser.focus {
                filetransfer::FilePaneFocus::Local => {
                    if let Some(entry) = browser.selected_local().cloned() {
                        let result = if entry.is_dir {
                            std::fs::remove_dir_all(&entry.path)
                        } else {
                            std::fs::remove_file(&entry.path)
                        };
                        match result {
                            Ok(_) => {
                                browser.status_message = Some(format!("Deleted {}", entry.name));
                                browser.list_local_files();
                            }
                            Err(e) => {
                                browser.status_message = Some(format!("Delete failed: {}", e));
                            }
                        }
                    }
                }
                filetransfer::FilePaneFocus::Remote => {
                    if let Some(entry) = browser.selected_remote().cloned() {
                        let full_path = if browser.remote_path.ends_with('/') {
                            format!("{}{}", browser.remote_path, entry.name)
                        } else {
                            format!("{}/{}", browser.remote_path, entry.name)
                        };
                        let cmd = if entry.is_dir {
                            format!("rm -rf '{}'", full_path)
                        } else {
                            format!("rm -f '{}'", full_path)
                        };
                        if let Some(idx) = app.session_manager.active_index {
                            if let Some(Some(rt)) = runtimes.get(idx) {
                                match exec_remote_command(&rt.ssh_session.handle, &cmd).await {
                                    Ok(_) => {
                                        if let Some(ref mut fb) = app.file_browser {
                                            fb.status_message = Some(format!("Deleted {}", entry.name));
                                            let rp = fb.remote_path.clone();
                                            match list_remote_files(&rt.ssh_session.handle, &rp).await {
                                                Ok(files) => fb.remote_files = files,
                                                Err(_) => {}
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        if let Some(ref mut fb) = app.file_browser {
                                            fb.status_message = Some(format!("Delete failed: {}", e));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        KeyCode::Esc => {
            app.view = AppView::Session;
            app.file_browser = None;
        }
        _ => {}
    }
    Ok(KeyAction::Handled)
}

fn handle_portfwd_key(
    key: crossterm::event::KeyEvent,
    app: &mut App,
) -> anyhow::Result<KeyAction> {
    if app.port_forward_adding {
        match key.code {
            KeyCode::Esc => {
                app.port_forward_adding = false;
                app.port_forward_input.clear();
            }
            KeyCode::Enter => {
                if let Some(active_idx) = app.session_manager.active_index {
                    if let Some((dir, bind_port, target_host, target_port)) =
                        portfwd::parse_forward_spec(&app.port_forward_input)
                    {
                        if let Some(mgr) = app.port_forward_managers.get_mut(active_idx) {
                            match dir {
                                portfwd::ForwardDirection::Local => {
                                    mgr.add_local("127.0.0.1", bind_port, &target_host, target_port);
                                }
                                portfwd::ForwardDirection::Remote => {
                                    mgr.add_remote("0.0.0.0", bind_port, &target_host, target_port);
                                }
                            }
                        }
                    }
                }
                app.port_forward_adding = false;
                app.port_forward_input.clear();
            }
            KeyCode::Backspace => {
                app.port_forward_input.pop();
            }
            KeyCode::Char(c) => {
                app.port_forward_input.push(c);
            }
            _ => {}
        }
        return Ok(KeyAction::Handled);
    }

    match key.code {
        KeyCode::Char('a') => {
            app.port_forward_adding = true;
            app.port_forward_input.clear();
        }
        KeyCode::Char('d') => {
            if let Some(active_idx) = app.session_manager.active_index {
                if let Some(mgr) = app.port_forward_managers.get_mut(active_idx) {
                    if let Some(id) = mgr.selected_id().map(|s| s.to_string()) {
                        mgr.remove(&id);
                    }
                }
            }
        }
        KeyCode::Down => {
            if let Some(active_idx) = app.session_manager.active_index {
                if let Some(mgr) = app.port_forward_managers.get_mut(active_idx) {
                    mgr.select_next();
                }
            }
        }
        KeyCode::Up => {
            if let Some(active_idx) = app.session_manager.active_index {
                if let Some(mgr) = app.port_forward_managers.get_mut(active_idx) {
                    mgr.select_prev();
                }
            }
        }
        KeyCode::Esc => {
            app.view = AppView::Session;
        }
        _ => {}
    }
    Ok(KeyAction::Handled)
}

async fn handle_palette_key(
    key: crossterm::event::KeyEvent,
    app: &mut App,
    config: &AppConfig,
    audit: &AuditLogger,
    runtimes: &mut Vec<Option<SessionRuntime>>,
    output_rxs: &mut Vec<Option<tokio::sync::mpsc::Receiver<Vec<u8>>>>,
) -> anyhow::Result<KeyAction> {
    use tui::command_palette::PaletteAction;

    let palette = match app.command_palette.as_mut() {
        Some(p) => p,
        None => return Ok(KeyAction::Handled),
    };

    match key.code {
        KeyCode::Esc => {
            app.command_palette = None;
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.command_palette = None;
        }
        KeyCode::Down | KeyCode::Tab => {
            palette.move_down();
        }
        KeyCode::Up | KeyCode::BackTab => {
            palette.move_up();
        }
        KeyCode::Backspace => {
            palette.query.pop();
            palette.update(
                &app.hosts,
                &app.session_manager.sessions,
                app.session_manager.has_sessions(),
            );
        }
        KeyCode::Enter => {
            if let Some(action) = palette.selected_action().cloned() {
                app.command_palette = None;
                match action {
                    PaletteAction::ConnectHost(idx) => {
                        if let Some(host) = app.hosts.get(idx).cloned() {
                            open_session(app, config, audit, &host, runtimes, output_rxs).await?;
                        }
                    }
                    PaletteAction::SwitchSession(idx) => {
                        if app.session_manager.switch_to(idx) {
                            if let Some(session) = app.session_manager.sessions.get(idx) {
                                let label = session.label.clone();
                                app.notifications.retain(|n| n.session_label != label);
                            }
                            app.view = AppView::Session;
                        }
                    }
                    PaletteAction::SetView(view) => {
                        if app.session_manager.has_sessions() || view == AppView::Dashboard {
                            app.view = view;
                        }
                    }
                    PaletteAction::SetDashboardTab(tab) => {
                        app.view = AppView::Dashboard;
                        app.dashboard_tab = tab;
                    }
                    PaletteAction::ToggleSplitPane => {
                        if app.session_manager.has_sessions() {
                            app.split_pane = !app.split_pane;
                        }
                    }
                    PaletteAction::ToggleHelp => {
                        app.show_help = !app.show_help;
                    }
                }
            } else {
                app.command_palette = None;
            }
        }
        KeyCode::Char(c) => {
            palette.query.push(c);
            palette.update(
                &app.hosts,
                &app.session_manager.sessions,
                app.session_manager.has_sessions(),
            );
        }
        _ => {}
    }

    Ok(KeyAction::Handled)
}

async fn handle_dashboard_key(
    key: crossterm::event::KeyEvent,
    app: &mut App,
    config: &AppConfig,
    audit: &AuditLogger,
    runtimes: &mut Vec<Option<SessionRuntime>>,
    output_rxs: &mut Vec<Option<tokio::sync::mpsc::Receiver<Vec<u8>>>>,
) -> anyhow::Result<KeyAction> {
    // Search mode input handling
    if app.search_active {
        match key.code {
            KeyCode::Esc => {
                app.search_active = false;
                app.search_query.clear();
                app.select_first_filtered();
            }
            KeyCode::Enter => {
                app.search_active = false;
                // Connect to the currently selected (first matching) host
                if let Some(host) = app.selected_host().cloned() {
                    open_session(app, config, audit, &host, runtimes, output_rxs).await?;
                }
            }
            KeyCode::Backspace => {
                app.search_query.pop();
                app.select_first_filtered();
            }
            KeyCode::Char(c) => {
                app.search_query.push(c);
                app.select_first_filtered();
            }
            _ => {}
        }
        return Ok(KeyAction::Handled);
    }

    match key.code {
        KeyCode::Char('q') => return Ok(KeyAction::Quit),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Ok(KeyAction::Quit)
        }
        KeyCode::Down | KeyCode::Char('j') => app.next_host(),
        KeyCode::Up | KeyCode::Char('k') => app.prev_host(),
        KeyCode::Char('1') => app.dashboard_tab = DashboardTab::Sessions,
        KeyCode::Char('2') => app.dashboard_tab = DashboardTab::Hosts,
        KeyCode::Char('3') => app.dashboard_tab = DashboardTab::Fleet,
        KeyCode::Char('4') => app.dashboard_tab = DashboardTab::Config,
        KeyCode::Char('/') => {
            app.search_active = true;
            app.search_query.clear();
        }
        KeyCode::Char('r') => {
            load_hosts_into_app(app, config)?;
            app.set_status("Hosts refreshed.".to_string());
        }
        KeyCode::Char('d') => {
            if let Some(host) = app.selected_host().cloned() {
                let db = CacheDb::open_default()?;
                db.remove_host(&host.hostname, host.port)?;
                load_hosts_into_app(app, config)?;
                app.set_status(format!("Removed {}.", host.hostname));
            }
        }
        KeyCode::Enter => {
            if let Some(host) = app.selected_host().cloned() {
                // Open a new session to this host
                open_session(app, config, audit, &host, runtimes, output_rxs).await?;
            }
        }
        _ => {}
    }
    Ok(KeyAction::Handled)
}

async fn handle_session_key(
    key: crossterm::event::KeyEvent,
    app: &mut App,
    runtimes: &mut Vec<Option<SessionRuntime>>,
) -> anyhow::Result<KeyAction> {
    // In session view, forward all non-Alt keys to the remote shell
    if key.modifiers.contains(KeyModifiers::ALT) {
        return Ok(KeyAction::Handled); // already handled in global
    }

    if let Some(idx) = app.session_manager.active_index {
        if let Some(Some(rt)) = runtimes.get(idx) {
            // Convert key event to bytes and send to remote
            let bytes = key_to_bytes(key);
            if !bytes.is_empty() {
                rt.channel_tx.send(SessionInput::Data(bytes)).await.ok();
            }
        }
    }

    Ok(KeyAction::Handled)
}

fn handle_monitor_key(
    key: crossterm::event::KeyEvent,
    app: &mut App,
) -> anyhow::Result<KeyAction> {
    match key.code {
        KeyCode::Esc => {
            app.view = AppView::Session;
        }
        KeyCode::Char('s') => {
            app.monitor_sort = match app.monitor_sort {
                tui::host_monitor::ProcessSort::Cpu => tui::host_monitor::ProcessSort::Memory,
                tui::host_monitor::ProcessSort::Memory => tui::host_monitor::ProcessSort::Cpu,
            };
            app.monitor_process_scroll = 0;
        }
        KeyCode::Down => {
            app.monitor_process_scroll = app.monitor_process_scroll.saturating_add(1);
        }
        KeyCode::Up => {
            app.monitor_process_scroll = app.monitor_process_scroll.saturating_sub(1);
        }
        _ => {}
    }
    Ok(KeyAction::Handled)
}

async fn open_session(
    app: &mut App,
    config: &AppConfig,
    audit: &AuditLogger,
    host: &HostDisplay,
    runtimes: &mut Vec<Option<SessionRuntime>>,
    output_rxs: &mut Vec<Option<tokio::sync::mpsc::Receiver<Vec<u8>>>>,
) -> anyhow::Result<()> {
    let session_id = uuid::Uuid::new_v4().to_string();

    let user = if host.user.is_empty() {
        config
            .general
            .default_user
            .clone()
            .unwrap_or_else(whoami)
    } else {
        host.user.clone()
    };

    let auth = if let Some(ref key) = config.general.default_key {
        let expanded = shellexpand::tilde(key).to_string();
        AuthMethod::KeyFile(expanded.into())
    } else if std::env::var("SSH_AUTH_SOCK").is_ok() {
        AuthMethod::Agent
    } else {
        app.set_status("No key or ssh-agent available. Set general.default_key in config.".to_string());
        return Ok(());
    };

    let connect_config = ConnectConfig {
        hostname: host.hostname.clone(),
        port: host.port,
        username: user.clone(),
        auth,
    };

    let label = if host.name.is_empty() {
        host.hostname.clone()
    } else {
        host.name.clone()
    };

    // Create session in connecting state
    let session = Session::new(
        session_id.clone(),
        label.clone(),
        host.hostname.clone(),
        host.port,
        user.clone(),
        config.session.scrollback_lines,
    );

    let idx = match app.session_manager.add_session(session) {
        Ok(idx) => idx,
        Err(msg) => {
            app.set_status(msg);
            return Ok(());
        }
    };
    app.add_session_tracking(config.host_monitor.history_samples);

    audit.log_connection_attempt(
        &session_id,
        &connect_config.hostname,
        connect_config.port,
        &connect_config.username,
    );

    app.view = AppView::Session;

    // Resolve jump host chain from config
    let jump_host_name = config
        .hosts
        .iter()
        .find(|h| h.name == host.name || h.hostname == host.hostname)
        .and_then(|h| h.jump_host.as_deref())
        .filter(|j| !j.is_empty())
        .map(|s| s.to_string());

    let connect_result = if let Some(ref jump_name) = jump_host_name {
        // Resolve jump host config
        let jump_entry = config.hosts.iter().find(|h| h.name == *jump_name || h.hostname == *jump_name);
        let jump_hostname = jump_entry.map(|e| e.hostname.clone()).unwrap_or_else(|| jump_name.clone());
        let jump_port = jump_entry.map(|e| e.port).unwrap_or(22);
        let jump_user = jump_entry
            .and_then(|e| e.user.clone())
            .or_else(|| config.general.default_user.clone())
            .unwrap_or_else(whoami);
        let jump_auth = jump_entry
            .and_then(|e| e.key.as_ref())
            .map(|k| AuthMethod::KeyFile(shellexpand::tilde(k).to_string().into()))
            .unwrap_or_else(|| connect_config.auth.clone());

        let jump_config = ConnectConfig {
            hostname: jump_hostname,
            port: jump_port,
            username: jump_user,
            auth: jump_auth,
        };

        app.set_status(format!("Connecting via jump host {}...", jump_name));
        SshClient::connect_via_jump(&jump_config, &connect_config).await
    } else {
        SshClient::connect(&connect_config).await
    };

    // Connect (directly or via jump host)
    match connect_result {
        Ok((mut ssh_session, fingerprint, banner)) => {
            // TOFU check
            let db = CacheDb::open_default()?;
            let status = db.check_host_key(&connect_config.hostname, connect_config.port, &fingerprint)?;
            match status {
                HostKeyStatus::Trusted | HostKeyStatus::Unknown => {
                    if matches!(status, HostKeyStatus::Unknown) {
                        db.trust_host(
                            &connect_config.hostname,
                            None,
                            connect_config.port,
                            &fingerprint,
                            "ssh",
                        )?;
                    }
                }
                HostKeyStatus::Changed { .. } => {
                    // Auto-accept in TUI mode for now (could add dialog later)
                    db.trust_host(
                        &connect_config.hostname,
                        None,
                        connect_config.port,
                        &fingerprint,
                        "ssh",
                    )?;
                }
            }
            db.update_last_seen(&connect_config.hostname, connect_config.port)?;

            // Set up diagnostics
            let log_dir = AppConfig::data_dir().join("sessions");
            let diag = DiagnosticsEngine::new(
                &session_id,
                &connect_config.hostname,
                connect_config.port,
                Some(log_dir.as_path()),
            );
            diag.set_connection_info(
                banner,
                None,
                None,
                None,
                None,
                Some(format!("{:?}", connect_config.auth)),
            )
            .await;

            // Open shell channel
            let (cols, rows) = terminal::size()?;
            let channel = ssh_session
                .open_shell("xterm-256color", cols as u32, rows as u32)
                .await?;

            // Set up channel I/O forwarding
            let (output_tx, output_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
            let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<SessionInput>(64);

            // Session recording (asciicast v2)
            let recorder: Option<std::sync::Arc<recording::SessionRecorder>> =
                if config.session.recording {
                    let cast_path = recording::recording_path(&session_id);
                    match recording::SessionRecorder::new(&cast_path, cols as u32, rows as u32, Some(label.clone())) {
                        Ok(r) => Some(std::sync::Arc::new(r)),
                        Err(_) => None,
                    }
                } else {
                    None
                };

            // Spawn channel I/O task
            let diag_metrics = diag.metrics();
            let mut channel = channel;
            let rec = recorder.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        Some(msg) = channel.wait() => {
                            match msg {
                                russh::ChannelMsg::Data { data } => {
                                    let bytes = data.to_vec();
                                    if let Some(ref r) = rec {
                                        r.record_output(&bytes);
                                    }
                                    {
                                        let mut m = diag_metrics.write().await;
                                        m.bytes_received += bytes.len() as u64;
                                    }
                                    if output_tx.send(bytes).await.is_err() {
                                        break;
                                    }
                                }
                                russh::ChannelMsg::Eof | russh::ChannelMsg::Close => break,
                                russh::ChannelMsg::ExitStatus { .. } => break,
                                _ => {}
                            }
                        }
                        Some(input) = input_rx.recv() => {
                            match input {
                                SessionInput::Data(data) => {
                                    if let Some(ref r) = rec {
                                        r.record_input(&data);
                                    }
                                    {
                                        let mut m = diag_metrics.write().await;
                                        m.bytes_sent += data.len() as u64;
                                    }
                                    if channel.data(&data[..]).await.is_err() {
                                        break;
                                    }
                                }
                                SessionInput::Resize { cols, rows } => {
                                    channel.window_change(cols, rows, 0, 0).await.ok();
                                }
                            }
                        }
                        else => break,
                    }
                }
            });

            // Set up host monitor
            let monitor = if config.host_monitor.enabled {
                Some(monitor::HostMetricsCollector::new(
                    config.host_monitor.history_samples,
                    config.host_monitor.process_count,
                ))
            } else {
                None
            };

            // Update session state and jump host info
            if let Some(session) = app.session_manager.sessions.get_mut(idx) {
                session.state = SessionState::Active;
                session.jump_host = ssh_session.jump_host.clone();
            }

            audit.log_session_event(
                &session_id,
                &connect_config.hostname,
                connect_config.port,
                AuditEventType::SessionStart,
            );

            // Store runtime
            let runtime = SessionRuntime {
                ssh_session,
                channel_tx: input_tx,
                diagnostics: diag,
                monitor,
                connect_config,
            };

            // Ensure vectors are large enough
            while runtimes.len() <= idx {
                runtimes.push(None);
            }
            while output_rxs.len() <= idx {
                output_rxs.push(None);
            }
            runtimes[idx] = Some(runtime);
            output_rxs[idx] = Some(output_rx);

            app.set_status(format!("Connected to {}", label));
        }
        Err(e) => {
            if let Some(session) = app.session_manager.sessions.get_mut(idx) {
                session.state = SessionState::Disconnected {
                    reason: e.to_string(),
                };
            }
            app.set_status(format!("Connection failed: {}", e));
        }
    }

    Ok(())
}

/// Reconnect a session at the given index, replacing its runtime and output channel.
/// The session's VirtualTerminal (scrollback) is preserved.
async fn reconnect_session(
    idx: usize,
    connect_config: &ConnectConfig,
    config: &AppConfig,
    runtimes: &mut Vec<Option<SessionRuntime>>,
    output_rxs: &mut Vec<Option<tokio::sync::mpsc::Receiver<Vec<u8>>>>,
) -> anyhow::Result<()> {
    // Close old SSH session if still present
    if let Some(Some(rt)) = runtimes.get_mut(idx) {
        rt.ssh_session.close().await.ok();
    }

    // Connect
    let (mut ssh_session, _fingerprint, banner) = SshClient::connect(connect_config).await?;

    // Diagnostics
    let session_id = uuid::Uuid::new_v4().to_string();
    let log_dir = AppConfig::data_dir().join("sessions");
    let diag = DiagnosticsEngine::new(
        &session_id,
        &connect_config.hostname,
        connect_config.port,
        Some(log_dir.as_path()),
    );
    diag.set_connection_info(
        banner,
        None,
        None,
        None,
        None,
        Some(format!("{:?}", connect_config.auth)),
    )
    .await;

    // Open shell channel
    let (cols, rows) = terminal::size()?;
    let channel = ssh_session
        .open_shell("xterm-256color", cols as u32, rows as u32)
        .await?;

    // Set up channel I/O
    let (output_tx, output_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<SessionInput>(64);

    // Recording for reconnected sessions
    let rec: Option<std::sync::Arc<recording::SessionRecorder>> = if config.session.recording {
        let cast_path = recording::recording_path(&session_id);
        recording::SessionRecorder::new(&cast_path, cols as u32, rows as u32, None)
            .ok()
            .map(std::sync::Arc::new)
    } else {
        None
    };

    let diag_metrics = diag.metrics();
    let mut channel = channel;
    let rec_clone = rec.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(msg) = channel.wait() => {
                    match msg {
                        russh::ChannelMsg::Data { data } => {
                            let bytes = data.to_vec();
                            if let Some(ref r) = rec_clone {
                                r.record_output(&bytes);
                            }
                            {
                                let mut m = diag_metrics.write().await;
                                m.bytes_received += bytes.len() as u64;
                            }
                            if output_tx.send(bytes).await.is_err() {
                                break;
                            }
                        }
                        russh::ChannelMsg::Eof | russh::ChannelMsg::Close => break,
                        russh::ChannelMsg::ExitStatus { .. } => break,
                        _ => {}
                    }
                }
                Some(input) = input_rx.recv() => {
                    match input {
                        SessionInput::Data(data) => {
                            if let Some(ref r) = rec_clone {
                                r.record_input(&data);
                            }
                            {
                                let mut m = diag_metrics.write().await;
                                m.bytes_sent += data.len() as u64;
                            }
                            if channel.data(&data[..]).await.is_err() {
                                break;
                            }
                        }
                        SessionInput::Resize { cols, rows } => {
                            channel.window_change(cols, rows, 0, 0).await.ok();
                        }
                    }
                }
                else => break,
            }
        }
    });

    // Monitor
    let monitor = if config.host_monitor.enabled {
        Some(monitor::HostMetricsCollector::new(
            config.host_monitor.history_samples,
            config.host_monitor.process_count,
        ))
    } else {
        None
    };

    let runtime = SessionRuntime {
        ssh_session,
        channel_tx: input_tx,
        diagnostics: diag,
        monitor,
        connect_config: connect_config.clone(),
    };

    while runtimes.len() <= idx {
        runtimes.push(None);
    }
    while output_rxs.len() <= idx {
        output_rxs.push(None);
    }
    runtimes[idx] = Some(runtime);
    output_rxs[idx] = Some(output_rx);

    Ok(())
}

/// Replay an asciicast recording with accurate timing.
/// Supports: Space = pause/resume, +/- = speed, q = quit.
async fn replay_recording(path: &std::path::Path) -> anyhow::Result<()> {
    let (header, events) = recording::parse_cast_file(path)?;

    if events.is_empty() {
        println!("Recording is empty.");
        return Ok(());
    }

    // Only replay output events
    let output_events: Vec<&recording::CastEvent> = events
        .iter()
        .filter(|e| e.event_type == "o")
        .collect();

    if output_events.is_empty() {
        println!("No output events in recording.");
        return Ok(());
    }

    println!(
        "Replaying session ({}x{}) — {} events. Space:pause  +/-:speed  q:quit",
        header.width,
        header.height,
        output_events.len()
    );

    // Use raw mode for clean output
    terminal::enable_raw_mode()?;
    let _raw_guard = scopeguard::guard((), |_| {
        terminal::disable_raw_mode().ok();
    });

    let mut speed: f64 = 1.0;
    let mut paused = false;

    for (i, event) in output_events.iter().enumerate() {
        // Calculate delay from previous event
        let delay = if i == 0 {
            0.0
        } else {
            (event.time - output_events[i - 1].time) / speed
        };

        // Cap delay to 2 seconds (avoid long pauses from idle sessions)
        let delay = delay.min(2.0);

        if delay > 0.001 {
            let delay_dur = Duration::from_secs_f64(delay);
            let mut remaining = delay_dur;

            while !remaining.is_zero() {
                let poll_time = remaining.min(Duration::from_millis(50));
                if crossterm::event::poll(poll_time).unwrap_or(false) {
                    if let Ok(crossterm::event::Event::Key(key)) = crossterm::event::read() {
                        match key.code {
                            KeyCode::Char('q') => {
                                io::stdout().write_all(b"\r\n")?;
                                io::stdout().flush()?;
                                return Ok(());
                            }
                            KeyCode::Char(' ') => {
                                paused = !paused;
                                if paused {
                                    // Wait until unpaused
                                    loop {
                                        if let Ok(crossterm::event::Event::Key(k)) =
                                            crossterm::event::read()
                                        {
                                            match k.code {
                                                KeyCode::Char(' ') => break,
                                                KeyCode::Char('q') => {
                                                    io::stdout().write_all(b"\r\n")?;
                                                    io::stdout().flush()?;
                                                    return Ok(());
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                }
                            }
                            KeyCode::Char('+') | KeyCode::Char('=') => {
                                speed = (speed * 2.0).min(16.0);
                            }
                            KeyCode::Char('-') => {
                                speed = (speed / 2.0).max(0.25);
                            }
                            _ => {}
                        }
                    }
                }
                remaining = remaining.saturating_sub(poll_time);
            }
        }

        // Write event data
        io::stdout().write_all(event.data.as_bytes())?;
        io::stdout().flush()?;
    }

    io::stdout().write_all(b"\r\n")?;
    io::stdout().flush()?;
    Ok(())
}

/// Convert a crossterm KeyEvent into bytes to send to the remote shell
fn key_to_bytes(key: crossterm::event::KeyEvent) -> Vec<u8> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                // Ctrl+A = 0x01, Ctrl+C = 0x03, etc.
                let byte = (c as u8).wrapping_sub(b'a').wrapping_add(1);
                vec![byte]
            } else {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                s.as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::F(n) => match n {
            1 => b"\x1bOP".to_vec(),
            2 => b"\x1bOQ".to_vec(),
            3 => b"\x1bOR".to_vec(),
            4 => b"\x1bOS".to_vec(),
            5 => b"\x1b[15~".to_vec(),
            6 => b"\x1b[17~".to_vec(),
            7 => b"\x1b[18~".to_vec(),
            8 => b"\x1b[19~".to_vec(),
            9 => b"\x1b[20~".to_vec(),
            10 => b"\x1b[21~".to_vec(),
            11 => b"\x1b[23~".to_vec(),
            12 => b"\x1b[24~".to_vec(),
            _ => vec![],
        },
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// TOFU handling
// ---------------------------------------------------------------------------

fn handle_tofu(
    db: &CacheDb,
    connect_config: &ConnectConfig,
    fingerprint: &str,
    status: &HostKeyStatus,
    app_config: &AppConfig,
    audit: &AuditLogger,
    session_id: &str,
) -> anyhow::Result<()> {
    match status {
        HostKeyStatus::Trusted => {
            audit.log_host_key_event(
                session_id,
                &connect_config.hostname,
                connect_config.port,
                AuditEventType::HostKeyVerified,
                fingerprint,
            );
        }
        HostKeyStatus::Unknown => match app_config.general.tofu_policy {
            TofuPolicy::Auto => {
                db.trust_host(
                    &connect_config.hostname,
                    None,
                    connect_config.port,
                    fingerprint,
                    "ssh",
                )?;
                audit.log_host_key_event(
                    session_id,
                    &connect_config.hostname,
                    connect_config.port,
                    AuditEventType::HostKeyNewTrust,
                    fingerprint,
                );
                println!("Host key cached (auto-trust).");
            }
            TofuPolicy::Prompt => {
                println!("Unknown host key: {}", fingerprint);
                print!("Trust this host? [y/N] ");
                io::stdout().flush()?;
                let mut answer = String::new();
                io::stdin().read_line(&mut answer)?;
                if answer.trim().eq_ignore_ascii_case("y") {
                    db.trust_host(
                        &connect_config.hostname,
                        None,
                        connect_config.port,
                        fingerprint,
                        "ssh",
                    )?;
                    audit.log_host_key_event(
                        session_id,
                        &connect_config.hostname,
                        connect_config.port,
                        AuditEventType::HostKeyNewTrust,
                        fingerprint,
                    );
                } else {
                    audit.log_host_key_event(
                        session_id,
                        &connect_config.hostname,
                        connect_config.port,
                        AuditEventType::HostKeyRejected,
                        fingerprint,
                    );
                    anyhow::bail!("Host key rejected by user.");
                }
            }
            TofuPolicy::Strict => {
                audit.log_host_key_event(
                    session_id,
                    &connect_config.hostname,
                    connect_config.port,
                    AuditEventType::HostKeyRejected,
                    fingerprint,
                );
                anyhow::bail!(
                    "Unknown host key for {}. Add it first (strict TOFU).",
                    connect_config.hostname
                );
            }
        },
        HostKeyStatus::Changed {
            old_fingerprint,
            old_last_seen,
        } => {
            audit.log_host_key_event(
                session_id,
                &connect_config.hostname,
                connect_config.port,
                AuditEventType::HostKeyChanged,
                fingerprint,
            );
            eprintln!("⚠️  WARNING: HOST KEY HAS CHANGED!");
            eprintln!("  Old fingerprint: {}", old_fingerprint);
            eprintln!("  New fingerprint: {}", fingerprint);
            eprintln!("  Last seen:       {}", old_last_seen);
            eprintln!("This could indicate a man-in-the-middle attack.");
            print!("Accept new key? [y/N] ");
            io::stdout().flush()?;
            let mut answer = String::new();
            io::stdin().read_line(&mut answer)?;
            if answer.trim().eq_ignore_ascii_case("y") {
                db.trust_host(
                    &connect_config.hostname,
                    None,
                    connect_config.port,
                    fingerprint,
                    "ssh",
                )?;
            } else {
                anyhow::bail!("Connection aborted — host key change rejected.");
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_target(target: &str, config: &AppConfig) -> (String, String) {
    if let Some((user, host)) = target.split_once('@') {
        (user.to_string(), host.to_string())
    } else {
        if let Some(entry) = config.hosts.iter().find(|h| h.name == target) {
            let user = entry
                .user
                .clone()
                .or_else(|| config.general.default_user.clone())
                .unwrap_or_else(whoami);
            return (user, entry.hostname.clone());
        }
        let user = config
            .general
            .default_user
            .clone()
            .unwrap_or_else(whoami);
        (user, target.to_string())
    }
}

fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".to_string())
}

fn prompt_password(prompt: &str) -> anyhow::Result<String> {
    eprint!("{}", prompt);
    io::stderr().flush()?;
    terminal::enable_raw_mode()?;
    let mut pw = String::new();
    loop {
        if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
            match key.code {
                KeyCode::Enter => {
                    terminal::disable_raw_mode()?;
                    eprintln!();
                    return Ok(pw);
                }
                KeyCode::Backspace => {
                    pw.pop();
                }
                KeyCode::Char(c) => pw.push(c),
                _ => {}
            }
        }
    }
}

fn load_hosts_into_app(app: &mut App, config: &AppConfig) -> anyhow::Result<()> {
    let mut displays = Vec::new();

    if let Ok(db) = CacheDb::open_default() {
        if let Ok(hosts) = db.list_hosts() {
            for h in hosts {
                let tags: Vec<String> =
                    h.tags.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
                // Resolve jump_host from config if this host matches
                let jump_host = config.hosts.iter()
                    .find(|e| e.hostname == h.hostname && e.port == h.port)
                    .and_then(|e| e.jump_host.clone())
                    .filter(|j| !j.is_empty());
                displays.push(HostDisplay {
                    name: h.hostname.clone(),
                    hostname: h.hostname,
                    port: h.port,
                    user: String::new(),
                    status: HostStatus::Unknown,
                    last_seen: h.last_seen,
                    tags: tags.join(", "),
                    latency_ms: None,
                    latency_history: Vec::new(),
                    jump_host,
                });
            }
        }
    }

    for entry in &config.hosts {
        if !displays
            .iter()
            .any(|d| d.hostname == entry.hostname && d.port == entry.port)
        {
            let tags: Vec<String> = entry
                .tags
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect();
            displays.push(HostDisplay {
                name: entry.name.clone(),
                hostname: entry.hostname.clone(),
                port: entry.port,
                user: entry.user.clone().unwrap_or_default(),
                status: HostStatus::Unknown,
                last_seen: String::new(),
                tags: tags.join(", "),
                latency_ms: None,
                latency_history: Vec::new(),
                jump_host: entry.jump_host.clone().filter(|j| !j.is_empty()),
            });
        }
    }

    app.set_hosts(displays);
    Ok(())
}

fn import_ssh_config(path: &std::path::Path) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(path)?;
    let db = CacheDb::open_default()?;
    let mut count = 0;

    let mut current_host: Option<String> = None;
    let mut current_hostname: Option<String> = None;
    let mut current_port: u16 = 22;
    let mut current_user: Option<String> = None;

    let flush = |host: &Option<String>,
                 hostname: &Option<String>,
                 port: u16,
                 _user: &Option<String>,
                 db: &CacheDb,
                 count: &mut u32| {
        if let Some(ref hn) = hostname {
            db.trust_host(hn, None, port, "imported", "unknown").ok();
            *count += 1;
            println!(
                "  Imported: {} -> {}:{}",
                host.as_deref().unwrap_or(hn),
                hn,
                port
            );
        }
    };

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.splitn(2, char::is_whitespace).collect();
        if parts.len() < 2 {
            continue;
        }
        let key = parts[0].to_lowercase();
        let value = parts[1].trim();

        match key.as_str() {
            "host" => {
                flush(
                    &current_host,
                    &current_hostname,
                    current_port,
                    &current_user,
                    &db,
                    &mut count,
                );
                current_host = Some(value.to_string());
                current_hostname = None;
                current_port = 22;
                current_user = None;
            }
            "hostname" => current_hostname = Some(value.to_string()),
            "port" => current_port = value.parse().unwrap_or(22),
            "user" => current_user = Some(value.to_string()),
            _ => {}
        }
    }
    flush(
        &current_host,
        &current_hostname,
        current_port,
        &current_user,
        &db,
        &mut count,
    );

    println!("Imported {} hosts from {}", count, path.display());
    Ok(())
}

async fn health_check(config: &AppConfig, group: Option<&str>) -> anyhow::Result<()> {
    let db = CacheDb::open_default()?;
    let hosts = if let Some(group_name) = group {
        if let Some(grp) = config.host_groups.iter().find(|g| g.name == group_name) {
            let mut matched = Vec::new();
            for (tag_key, tag_val) in &grp.match_tags {
                matched.extend(db.find_hosts_by_tag(tag_key, tag_val)?);
            }
            matched
        } else {
            anyhow::bail!("Host group '{}' not found in config.", group_name);
        }
    } else {
        db.list_hosts()?
    };

    if hosts.is_empty() {
        println!("No hosts to check.");
        return Ok(());
    }

    println!("Checking {} hosts...", hosts.len());

    for host in &hosts {
        let addr = format!("{}:{}", host.hostname, host.port);
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::TcpStream::connect(&addr),
        )
        .await;
        match result {
            Ok(Ok(_)) => println!("  ✓ {}:{} — reachable", host.hostname, host.port),
            Ok(Err(e)) => println!("  ✗ {}:{} — {}", host.hostname, host.port, e),
            Err(_) => println!("  ✗ {}:{} — timeout", host.hostname, host.port),
        }
    }
    Ok(())
}

async fn run_on_group(
    config: &AppConfig,
    group_name: &str,
    command: &[String],
) -> anyhow::Result<()> {
    let db = CacheDb::open_default()?;
    let grp = config
        .host_groups
        .iter()
        .find(|g| g.name == group_name)
        .ok_or_else(|| anyhow::anyhow!("Host group '{}' not found.", group_name))?;

    let mut hosts = Vec::new();
    for (tag_key, tag_val) in &grp.match_tags {
        hosts.extend(db.find_hosts_by_tag(tag_key, tag_val)?);
    }

    if hosts.is_empty() {
        println!("No hosts matched group '{}'.", group_name);
        return Ok(());
    }

    let cmd_str = command.join(" ");
    println!(
        "Running '{}' on {} hosts in group '{}'...",
        cmd_str,
        hosts.len(),
        group_name
    );

    let user = grp
        .defaults
        .user
        .clone()
        .or_else(|| config.general.default_user.clone())
        .unwrap_or_else(whoami);

    let auth = if let Some(ref key) = grp
        .defaults
        .key
        .as_ref()
        .or(config.general.default_key.as_ref())
    {
        let expanded = shellexpand::tilde(key).to_string();
        AuthMethod::KeyFile(expanded.into())
    } else if std::env::var("SSH_AUTH_SOCK").is_ok() {
        AuthMethod::Agent
    } else {
        anyhow::bail!("No key configured for group and no ssh-agent available.");
    };

    let mut handles = Vec::new();
    for host in hosts {
        let user = user.clone();
        let auth = auth.clone();
        let cmd_str = cmd_str.clone();
        handles.push(tokio::spawn(async move {
            let connect_config = ConnectConfig {
                hostname: host.hostname.clone(),
                port: host.port,
                username: user,
                auth,
            };
            let result: Result<(SshSession, String, Option<String>), _> =
                SshClient::connect(&connect_config).await;
            match result {
                Ok((session, _, _)) => {
                    let channel_result = session.handle.channel_open_session().await;
                    match channel_result {
                        Ok(mut channel) => {
                            channel.exec(true, cmd_str.as_bytes()).await.ok();
                            let mut output = Vec::new();
                            while let Some(msg) = channel.wait().await {
                                match msg {
                                    russh::ChannelMsg::Data { data } => {
                                        output.extend_from_slice(&data);
                                    }
                                    russh::ChannelMsg::Eof | russh::ChannelMsg::Close => break,
                                    _ => {}
                                }
                            }
                            println!("--- {} ---", host.hostname);
                            io::stdout().write_all(&output).ok();
                            println!();
                            session.close().await.ok();
                        }
                        Err(e) => {
                            eprintln!("--- {} --- ERROR: {}", host.hostname, e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("--- {} --- CONNECT ERROR: {}", host.hostname, e);
                }
            }
        }));
    }

    for handle in handles {
        handle.await?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// File transfer helpers (exec-based remote operations)
// ---------------------------------------------------------------------------

async fn exec_remote_command(
    handle: &russh::client::Handle<ssh::ClientHandler>,
    command: &str,
) -> anyhow::Result<String> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to open session: {}", e))?;

    channel
        .exec(true, command.to_string())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to exec: {}", e))?;

    let mut output = Vec::new();
    let mut channel = channel;
    while let Some(msg) = channel.wait().await {
        match msg {
            russh::ChannelMsg::Data { data } => {
                output.extend_from_slice(&data);
            }
            russh::ChannelMsg::Eof | russh::ChannelMsg::Close | russh::ChannelMsg::ExitStatus { .. } => {
                break;
            }
            _ => {}
        }
    }

    Ok(String::from_utf8_lossy(&output).to_string())
}

async fn list_remote_files(
    handle: &russh::client::Handle<ssh::ClientHandler>,
    path: &str,
) -> anyhow::Result<Vec<filetransfer::RemoteFileEntry>> {
    let output = exec_remote_command(handle, &format!("ls -la '{}'", path)).await?;
    Ok(filetransfer::parse_ls_output(&output))
}

async fn upload_file(
    handle: &russh::client::Handle<ssh::ClientHandler>,
    local_path: &std::path::Path,
    remote_path: &str,
) -> anyhow::Result<()> {
    let data = std::fs::read(local_path)?;

    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to open session: {}", e))?;

    channel
        .exec(true, format!("cat > '{}'", remote_path))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to exec: {}", e))?;

    // Send file data in chunks
    for chunk in data.chunks(32768) {
        channel
            .data(chunk)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send data: {}", e))?;
    }

    channel.eof().await.map_err(|e| anyhow::anyhow!("Failed to send EOF: {}", e))?;

    // Wait for close
    let mut channel = channel;
    while let Some(msg) = channel.wait().await {
        match msg {
            russh::ChannelMsg::Eof | russh::ChannelMsg::Close | russh::ChannelMsg::ExitStatus { .. } => break,
            _ => {}
        }
    }

    Ok(())
}

async fn download_file(
    handle: &russh::client::Handle<ssh::ClientHandler>,
    remote_path: &str,
    local_path: &std::path::Path,
) -> anyhow::Result<()> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to open session: {}", e))?;

    channel
        .exec(true, format!("cat '{}'", remote_path))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to exec: {}", e))?;

    let mut file_data = Vec::new();
    let mut channel = channel;
    while let Some(msg) = channel.wait().await {
        match msg {
            russh::ChannelMsg::Data { data } => {
                file_data.extend_from_slice(&data);
            }
            russh::ChannelMsg::Eof | russh::ChannelMsg::Close | russh::ChannelMsg::ExitStatus { .. } => break,
            _ => {}
        }
    }

    std::fs::write(local_path, &file_data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconnect_tracker_initial_state() {
        let tracker = ReconnectTracker::new(5);
        assert_eq!(tracker.attempt, 0);
        assert_eq!(tracker.max_retries, 5);
        assert!(!tracker.exhausted());
        // Should retry immediately (backoff_secs = 0)
        assert!(tracker.should_retry());
    }

    #[test]
    fn test_reconnect_tracker_backoff() {
        let mut tracker = ReconnectTracker::new(5);
        tracker.record_attempt(); // attempt 1
        assert_eq!(tracker.attempt, 1);
        assert_eq!(tracker.backoff_secs, 2); // 1 << 1 = 2
        tracker.record_attempt(); // attempt 2
        assert_eq!(tracker.backoff_secs, 4); // 1 << 2 = 4
        tracker.record_attempt(); // attempt 3
        assert_eq!(tracker.backoff_secs, 8); // 1 << 3 = 8
        tracker.record_attempt(); // attempt 4
        assert_eq!(tracker.backoff_secs, 16); // 1 << 4 = 16
        tracker.record_attempt(); // attempt 5
        assert_eq!(tracker.backoff_secs, 30); // 1 << 5 = 32, capped to 30
    }

    #[test]
    fn test_reconnect_tracker_exhausted() {
        let mut tracker = ReconnectTracker::new(3);
        tracker.record_attempt(); // 1
        tracker.record_attempt(); // 2
        tracker.record_attempt(); // 3
        assert!(!tracker.exhausted());
        tracker.record_attempt(); // 4 > max
        assert!(tracker.exhausted());
    }

    #[test]
    fn test_reconnect_tracker_should_retry_respects_backoff() {
        let mut tracker = ReconnectTracker::new(5);
        tracker.record_attempt(); // sets backoff_secs > 0
        // Immediately after attempt, should NOT be ready (backoff not elapsed)
        assert!(!tracker.should_retry());
    }

    #[test]
    fn test_parse_target_user_at_host() {
        let config = AppConfig::default();
        let (user, host) = parse_target("deploy@web.example.com", &config);
        assert_eq!(user, "deploy");
        assert_eq!(host, "web.example.com");
    }

    #[test]
    fn test_parse_target_bare_host() {
        let config = AppConfig::default();
        let (user, host) = parse_target("web.example.com", &config);
        assert_eq!(host, "web.example.com");
        assert!(!user.is_empty());
    }

    #[test]
    fn test_parse_target_named_host_in_config() {
        let mut config = AppConfig::default();
        config.hosts.push(config::HostEntry {
            name: "bastion".to_string(),
            hostname: "bastion.internal.corp".to_string(),
            port: 22,
            user: Some("ops".to_string()),
            key: None,
            tags: std::collections::HashMap::new(),
            jump_host: None,
            port_forwards: Vec::new(),
        });
        let (user, host) = parse_target("bastion", &config);
        assert_eq!(user, "ops");
        assert_eq!(host, "bastion.internal.corp");
    }

    #[test]
    fn test_jump_host_resolution_from_config() {
        let mut config = AppConfig::default();
        config.hosts.push(config::HostEntry {
            name: "bastion".to_string(),
            hostname: "bastion.corp.com".to_string(),
            port: 22,
            user: Some("ops".to_string()),
            key: None,
            tags: std::collections::HashMap::new(),
            jump_host: None,
            port_forwards: Vec::new(),
        });
        config.hosts.push(config::HostEntry {
            name: "db-primary".to_string(),
            hostname: "db01.internal.corp".to_string(),
            port: 22,
            user: Some("dba".to_string()),
            key: None,
            tags: std::collections::HashMap::new(),
            jump_host: Some("bastion".to_string()),
            port_forwards: Vec::new(),
        });

        // Find jump host for db-primary
        let target = config.hosts.iter().find(|h| h.name == "db-primary").unwrap();
        let jump_name = target.jump_host.as_deref().filter(|j| !j.is_empty());
        assert_eq!(jump_name, Some("bastion"));

        // Resolve jump host entry
        let jump_entry = config.hosts.iter().find(|h| h.name == *jump_name.unwrap());
        assert!(jump_entry.is_some());
        let jump_entry = jump_entry.unwrap();
        assert_eq!(jump_entry.hostname, "bastion.corp.com");
        assert_eq!(jump_entry.user, Some("ops".to_string()));
    }

    #[test]
    fn test_jump_host_empty_string_ignored() {
        let mut config = AppConfig::default();
        config.hosts.push(config::HostEntry {
            name: "web".to_string(),
            hostname: "web.example.com".to_string(),
            port: 22,
            user: None,
            key: None,
            tags: std::collections::HashMap::new(),
            jump_host: Some("".to_string()),
            port_forwards: Vec::new(),
        });

        let target = config.hosts.iter().find(|h| h.name == "web").unwrap();
        let jump_name = target.jump_host.as_deref().filter(|j| !j.is_empty());
        assert_eq!(jump_name, None);
    }
}
