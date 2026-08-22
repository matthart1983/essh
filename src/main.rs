mod audit;
mod bench;
mod cache;
mod cli;
mod config;
mod design;
mod diagnose;
mod diagnostics;
mod divergence;
mod event;
mod filetransfer;
mod fleet;
mod format;
mod launcher;
mod monitor;
mod notify;
mod panes;
mod portfwd;
mod recording;
mod session;
mod ssh;
mod sshconfig;
mod theme;
mod tui;
mod workspace;

use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{collections::HashSet, process::Command};

use clap::Parser;
use crossterm::{
    event::{KeyCode, KeyModifiers},
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::prelude::*;

use audit::{AuditEventType, AuditLogger};
use cache::{CacheDb, HostKeyStatus};
use cli::{
    AuditAction, Cli, Commands, ConfigAction, HostsAction, KeysAction, SessionAction,
    WorkspaceAction,
};
use config::{AppConfig, TofuPolicy};
use diagnostics::DiagnosticsEngine;
use event::{AppEvent, EventHandler};
use notify::NotificationMatcher;
use session::{Session, SessionState};
use ssh::{AuthMethod, ConnectConfig, SshClient, SshError, SshSession};
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

struct TuiState {
    reconnect_trackers: Vec<Option<ReconnectTracker>>,
    notification_matcher: NotificationMatcher,
    fleet_prober: fleet::FleetProber,
    fleet_probe_task: Option<FleetProbeTask>,
    /// A connection the user asked for, to be made *after* the next draw.
    ///
    /// Connecting is awaited inline on the event loop, so the frame showing
    /// "connecting to X" has to be painted before the await starts —
    /// otherwise the first thing the user sees is the screen freezing, with
    /// no indication that anything is happening or to what host.
    pending_connect: Option<HostDisplay>,
}

type FleetProbeTask = tokio::task::JoinHandle<Vec<(String, u16, fleet::ProbeResult)>>;

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
            let used_identity = identity.is_some();
            let mut auth_candidates = if password {
                vec![prompt_password_auth(&user, &host)?]
            } else if let Some(key_path) = identity {
                vec![AuthMethod::KeyFile {
                    path: key_path,
                    passphrase: None,
                }]
            } else {
                configured_auth_methods(
                    None,
                    config.general.default_key.as_deref(),
                    ssh_agent_available(),
                )?
            };

            if auth_candidates.is_empty() {
                auth_candidates.push(prompt_password_auth(&user, &host)?);
            }

            match connect_and_shell_with_auth_candidates(
                host.clone(),
                port,
                user.clone(),
                auth_candidates,
                &config,
                &audit,
            )
            .await
            {
                Ok(()) => {}
                Err(err)
                    if !password
                        && !used_identity
                        && err
                            .downcast_ref::<SshError>()
                            .is_some_and(should_try_next_auth_candidate) =>
                {
                    let connect_config = ConnectConfig {
                        hostname: host.clone(),
                        port,
                        username: user.clone(),
                        auth: prompt_password_auth(&user, &host)?,
                        timeout: config.connect_timeout(),
                    };

                    connect_and_shell(connect_config, &config, &audit).await?;
                }
                Err(err) => return Err(err),
            }
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
                        "{:<20} {:<30} {:<6} {:<16} {:<24} Tags",
                        "Fingerprint", "Hostname", "Port", "Key Type", "Last Seen"
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
                    .filter_map(|t| {
                        t.split_once('=')
                            .map(|(k, v)| (k.to_string(), v.to_string()))
                    })
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
                let auth = resolve_key_auth_method(
                    AuthMethod::KeyFile {
                        path: path.clone(),
                        passphrase: None,
                    },
                    prompt_key_passphrase,
                )?;
                let passphrase = match auth {
                    AuthMethod::KeyFile { passphrase, .. } => passphrase,
                    _ => unreachable!("key auth resolution must return a keyfile"),
                };
                let key = russh_keys::load_secret_key(&path, passphrase.as_deref())?;
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
                        println!("{:<40} File", "Session ID");
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

        Commands::Workspace { action } => {
            let dir = workspace::Workspace::default_dir();
            match action {
                WorkspaceAction::List => {
                    let names = workspace::Workspace::list_in(&dir);
                    if names.is_empty() {
                        println!("No workspaces yet.");
                        println!();
                        println!("  essh workspace save production bastion prod-api-01 prod-db");
                    }
                    for n in names {
                        match workspace::Workspace::load_from(&dir, &n) {
                            Ok(ws) => println!(
                                "  {:<20} {} sessions{}",
                                ws.name,
                                ws.sessions.len(),
                                ws.description
                                    .map(|d| format!("  — {}", d))
                                    .unwrap_or_default()
                            ),
                            Err(e) => println!("  {:<20} unreadable: {}", n, e),
                        }
                    }
                }
                WorkspaceAction::Save {
                    name,
                    hosts,
                    on_connect,
                } => {
                    let ws = workspace::Workspace {
                        name: name.clone(),
                        layout: workspace::Layout::Tabs,
                        description: None,
                        sessions: hosts
                            .iter()
                            .map(|h| workspace::WorkspaceSession {
                                host: h.clone(),
                                user: None,
                                port: None,
                                on_connect: on_connect.clone(),
                            })
                            .collect(),
                    };
                    let path = ws.save_in(&dir)?;
                    println!(
                        "Saved {} ({} sessions) to {}",
                        name,
                        ws.sessions.len(),
                        path.display()
                    );
                    if on_connect.is_none() {
                        println!();
                        println!(
                            "note: restoring gives fresh shells in $HOME. To restore actual work,"
                        );
                        println!(
                            "      add --on-connect 'tmux new -A -s essh' — ESSH does not provide"
                        );
                        println!("      server-side persistence and does not pretend to.");
                    }
                }
                WorkspaceAction::Show { name } => {
                    let ws = workspace::Workspace::load_from(&dir, &name)?;
                    println!("{}  ({:?} layout)", ws.name, ws.layout);
                    if let Some(d) = &ws.description {
                        println!("{}", d);
                    }
                    println!();
                    for s in &ws.sessions {
                        let target = match (&s.user, s.port) {
                            (Some(u), Some(p)) => format!("{}@{}:{}", u, s.host, p),
                            (Some(u), None) => format!("{}@{}", u, s.host),
                            (None, Some(p)) => format!("{}:{}", s.host, p),
                            (None, None) => s.host.clone(),
                        };
                        println!("  {}", target);
                        if let Some(c) = &s.on_connect {
                            println!("      on connect: {}", c);
                        }
                    }
                }
                WorkspaceAction::Remove { name } => {
                    if workspace::Workspace::delete_in(&dir, &name)? {
                        println!("Removed workspace {}", name);
                    } else {
                        println!("No workspace named {}", name);
                    }
                }
                WorkspaceAction::Open { name } => {
                    // Validate before entering the TUI so a typo fails fast
                    // and readably rather than on a blank screen.
                    let ws = workspace::Workspace::load_from(&dir, &name)?;
                    println!("Opening {} ({} sessions)…", ws.name, ws.sessions.len());
                    return run_tui_with_workspace(config, ws).await;
                }
            }
        }

        Commands::Bench => bench::run_all(),

        Commands::Why {
            target,
            port,
            timeout,
        } => {
            let t = build_diagnose_target(&target, port);
            let d = diagnose::diagnose(&t, std::time::Duration::from_secs(timeout)).await;
            if d.succeeded() {
                println!("{} reachable", t.alias);
                println!();
                for r in &d.rungs {
                    println!(
                        "{:<10} {}  {}",
                        r.label,
                        r.status.symbol(),
                        r.status.detail()
                    );
                }
            } else {
                print!("{}", d);
                std::process::exit(1);
            }
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
            ConfigAction::Ssh { path } => {
                let path = path.unwrap_or_else(sshconfig::default_path);
                print_ssh_config_check(&path);
            }
            ConfigAction::Resolve { host, path } => {
                let path = path.unwrap_or_else(sshconfig::default_path);
                print_ssh_config_resolution(&path, &host);
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
    handle_tofu(
        &db,
        &connect_config,
        &fingerprint,
        &status,
        app_config,
        audit,
        &session_id,
    )?;

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

async fn connect_and_shell_with_auth_candidates(
    hostname: String,
    port: u16,
    username: String,
    auth_candidates: Vec<AuthMethod>,
    app_config: &AppConfig,
    audit: &AuditLogger,
) -> anyhow::Result<()> {
    let mut last_auth_error = None;

    for auth in auth_candidates {
        let auth = match resolve_key_auth_method(auth, prompt_key_passphrase) {
            Ok(auth) => auth,
            Err(err) if should_try_next_auth_candidate(&err) => {
                last_auth_error = Some(anyhow::Error::new(err));
                continue;
            }
            Err(err) => return Err(err.into()),
        };
        let connect_config = ConnectConfig {
            hostname: hostname.clone(),
            port,
            username: username.clone(),
            auth,
            timeout: app_config.connect_timeout(),
        };

        match connect_and_shell(connect_config, app_config, audit).await {
            Ok(()) => return Ok(()),
            Err(err)
                if err
                    .downcast_ref::<SshError>()
                    .is_some_and(should_try_next_auth_candidate) =>
            {
                last_auth_error = Some(err);
            }
            Err(err) => return Err(err),
        }
    }

    Err(last_auth_error.unwrap_or_else(|| anyhow::anyhow!("No authentication methods available")))
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
    run_tui_inner(config, None).await
}

/// `essh workspace open <name>` — restore a saved set of sessions.
async fn run_tui_with_workspace(config: AppConfig, ws: workspace::Workspace) -> anyhow::Result<()> {
    run_tui_inner(config, Some(ws)).await
}

async fn run_tui_inner(
    mut config: AppConfig,
    pending_workspace: Option<workspace::Workspace>,
) -> anyhow::Result<()> {
    let mut app = App::new(config.session.max_concurrent);
    app.prefix_key = config.session.prefix_key.clone();
    app.theme = theme::by_name(&config.theme);

    load_hosts_into_app(&mut app, &config)?;
    load_launcher_candidates(&mut app, &config);

    // §2's fast path — launch → search → connect — starts at the launcher.
    // Esc from there drops to the dashboard, and `launcher = false` in
    // [general] restores v1's behaviour of opening straight into it.
    if config.general.launcher {
        app.view = AppView::Launcher;
    }
    // A workspace restore goes straight to the sessions; the launcher would
    // be in the way of something the user has already chosen.
    if pending_workspace.is_some() {
        app.view = AppView::Dashboard;
        app.pending_workspace = pending_workspace;
    }

    io::stdout().execute(EnterAlternateScreen)?;
    terminal::enable_raw_mode()?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = tui_main_loop(&mut terminal, &mut app, &mut config).await;

    terminal::disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    result
}

async fn tui_main_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    config: &mut AppConfig,
) -> anyhow::Result<()> {
    let mut events = EventHandler::new(Duration::from_millis(100));
    let audit = AuditLogger::default_logger();

    // Session runtime data, indexed same as session_manager.sessions
    let mut runtimes: Vec<Option<SessionRuntime>> = Vec::new();

    // Per-session channel receivers for remote output
    let mut session_output_rxs: Vec<Option<tokio::sync::mpsc::Receiver<Vec<u8>>>> = Vec::new();

    // Tick counter for periodic monitor collection
    let mut tick_count: u64 = 0;

    let mut tui_state = TuiState {
        reconnect_trackers: Vec::new(),
        notification_matcher: NotificationMatcher::new(&config.session.notification_patterns),
        fleet_prober: fleet::FleetProber::new(
            config.fleet.probe_interval,
            config.fleet.probe_timeout,
            config.fleet.latency_history_samples,
        ),
        fleet_probe_task: None,
        pending_connect: None,
    };

    loop {
        // Something else had the terminal — a password prompt, $EDITOR — so
        // ratatui's diff baseline is stale. Repaint everything or the screen
        // comes back with characters missing.
        if event::take_needs_full_redraw() {
            terminal.clear()?;
        }

        // Draw
        terminal.draw(|frame| {
            tui::render(frame, app);
        })?;

        // A connection requested last iteration. The "connecting" frame is now
        // on screen, so it is safe to block here.
        if let Some(host) = tui_state.pending_connect.take() {
            open_session(
                app,
                config,
                &audit,
                &host,
                &mut runtimes,
                &mut session_output_rxs,
            )
            .await?;
            continue;
        }

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
                                    if !tui_state.notification_matcher.is_empty() {
                                        let text = String::from_utf8_lossy(&data);
                                        if let Some(matched) =
                                            tui_state.notification_matcher.check(&text)
                                        {
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
                                        if let Some(session) =
                                            app.session_manager.sessions.get_mut(i)
                                        {
                                            session.state =
                                                SessionState::Reconnecting { attempt: 1, max };
                                        }
                                        while tui_state.reconnect_trackers.len() <= i {
                                            tui_state.reconnect_trackers.push(None);
                                        }
                                        tui_state.reconnect_trackers[i] =
                                            Some(ReconnectTracker::new(max));
                                    } else if let Some(session) =
                                        app.session_manager.sessions.get_mut(i)
                                    {
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
                    &mut tui_state,
                )
                .await?;
                if handled == KeyAction::Quit {
                    if let Some(task) = tui_state.fleet_probe_task.take() {
                        task.abort();
                    }
                    // Close all sessions
                    for rt in runtimes.iter_mut().flatten() {
                        rt.ssh_session.close().await.ok();
                    }
                    return Ok(());
                }
            }
            AppEvent::Tick => {
                tick_count += 1;
                app.hud.tick();

                // Begin a pending workspace restore. The work itself is
                // spread across ticks below so the UI keeps rendering.
                if let Some(ws) = app.pending_workspace.take() {
                    app.workspace_queue = ws.sessions.iter().cloned().collect();
                    app.workspace_report = Some(workspace::RestoreReport {
                        workspace: ws.name.clone(),
                        outcomes: Vec::new(),
                    });
                    app.set_status(format!(
                        "restoring {} — {} sessions",
                        ws.name,
                        ws.sessions.len()
                    ));
                }

                // One host per tick. Each is pre-probed with a short timeout
                // so an unreachable host costs seconds, not a TCP timeout,
                // and fails with a reason rather than a stall.
                if let Some(session) = app.workspace_queue.pop_front() {
                    let outcome = restore_one_session(
                        app,
                        config,
                        &audit,
                        &session,
                        &mut runtimes,
                        &mut session_output_rxs,
                    )
                    .await;

                    if let Some(report) = app.workspace_report.as_mut() {
                        report.outcomes.push((session.host.clone(), outcome));
                        let done = report.outcomes.len();
                        let total = done + app.workspace_queue.len();
                        let summary = report.summary();
                        let name = report.workspace.clone();
                        if app.workspace_queue.is_empty() {
                            for (host, why) in report.failed() {
                                tracing::warn!(workspace = %name, host = %host, reason = %why,
                                    "workspace session did not connect");
                            }
                            app.set_status(summary.clone());
                            if app.session_manager.has_sessions() {
                                app.view = AppView::Session;
                                // The shell has no status row, so the result
                                // is announced by the HUD instead.
                                app.hud.reset();
                                app.hud.on_change(
                                    tui::hud::Reason::Notice(summary),
                                    tui::hud::Vitals::default(),
                                );
                            }
                        } else {
                            app.set_status(format!("restoring {} — {} of {}", name, done, total));
                        }
                    }
                }

                if let Some(task) = tui_state.fleet_probe_task.take() {
                    if task.is_finished() {
                        if let Ok(results) = task.await {
                            tui_state.fleet_prober.record_probe_results(results);
                            for host in app.hosts.iter_mut() {
                                if let Some(state) =
                                    tui_state.fleet_prober.get_state(&host.hostname, host.port)
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
                    } else {
                        tui_state.fleet_probe_task = Some(task);
                    }
                }

                // Update diagnostics snapshots every 10 ticks (1s)
                if tick_count.is_multiple_of(10) {
                    for (i, rt_opt) in runtimes.iter().enumerate() {
                        if let Some(rt) = rt_opt {
                            let snap = rt.diagnostics.snapshot().await;
                            if let Some(slot) = app.session_diagnostics.get_mut(i) {
                                *slot = Some(snap);
                            }
                        }
                    }
                }

                // Publish whatever the background samplers have collected.
                //
                // Sampling itself runs in its own task per session — see
                // `spawn_metric_sampler`. Two things were wrong with doing it
                // here: `collect().await` runs a command on the remote host,
                // so every sample stalled the whole UI for as long as the
                // round trip took; and it only ran while the monitor was on
                // screen, so opening the monitor always began with "waiting
                // for first sample" and a visible pause. Copying out of the
                // shared snapshots is a memcpy and can happen every tick.
                if tick_count.is_multiple_of(5) {
                    if let Some(i) = app.session_manager.active_index {
                        if let Some(Some(rt)) = runtimes.get(i) {
                            if let Some(ref mon) = rt.monitor {
                                let metrics_arc = mon.metrics();
                                let cpu_h = mon.cpu_history();
                                let mem_h = mon.mem_history();
                                let rx_h = mon.net_rx_history();
                                let tx_h = mon.net_tx_history();

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

                // Collect divergence facets. These change on the timescale of a
                // package upgrade, not a tick, so a 600-tick (60s) refresh is
                // plenty — 40 hosts × 17 facets must not become a thundering
                // herd. Unlike metrics, this runs whatever view is showing,
                // because the Hosts and Fleet tabs are where divergence is read.
                //
                // A newly connected host is the exception: waiting up to a
                // minute before it has any facts makes the Hosts list say
                // "never probed" about a host you are looking at right now. So
                // every 30 ticks (3s) we also sweep sessions that have no facts
                // yet — a one-off per host, not a poll.
                let refresh_all = tick_count.is_multiple_of(600);
                if refresh_all || tick_count.is_multiple_of(30) {
                    let sessions: Vec<(usize, String, u16)> = app
                        .session_manager
                        .sessions
                        .iter()
                        .enumerate()
                        .filter(|(_, s)| matches!(s.state, SessionState::Active))
                        .map(|(i, s)| (i, s.hostname.clone(), s.port))
                        .filter(|(_, hostname, port)| {
                            refresh_all
                                || app
                                    .host_name_for(hostname, *port)
                                    .is_none_or(|n| !app.host_facts.contains_key(&n))
                        })
                        .collect();

                    for (i, hostname, port) in sessions {
                        let Some(Some(rt)) = runtimes.get(i) else {
                            continue;
                        };
                        // Reuse the platform the metrics collector detected if
                        // it has one. It usually will not: metrics only sweep
                        // while the monitor or split view is open, so gating
                        // divergence on that made facets collect *only* if you
                        // happened to visit the monitor first. Probe `uname`
                        // ourselves when it is unknown — once per host.
                        let platform = match &rt.monitor {
                            Some(mon) => mon.platform().await,
                            None => monitor::Platform::Undetected,
                        };
                        let platform = if platform == monitor::Platform::Undetected {
                            let raw = divergence::collect::probe_uname(&rt.ssh_session.handle)
                                .await
                                .unwrap_or_default();
                            monitor::Platform::from_uname(&raw)
                        } else {
                            platform
                        };
                        if !platform.is_supported() {
                            continue;
                        }
                        let Some(name) = app.host_name_for(&hostname, port) else {
                            continue;
                        };

                        let facts = divergence::collect::collect_facts(
                            &rt.ssh_session.handle,
                            &name,
                            &platform,
                            &config.divergence.config_paths,
                            &config.divergence.packages,
                        )
                        .await;
                        // Record what was collectable here, so the overlay
                        // reports the comparison actually attempted rather
                        // than the size of the facet table.
                        let (usable, total) = divergence::collect::collectable_count(
                            &platform,
                            &config.divergence.config_paths,
                            &config.divergence.packages,
                        );
                        let privileged = divergence::collect::privileged_count(
                            &platform,
                            &config.divergence.config_paths,
                            &config.divergence.packages,
                        );
                        app.host_coverage
                            .insert(name.clone(), (usable, total, privileged));
                        app.host_platforms.insert(name.clone(), platform);
                        app.host_facts.insert(name, facts);
                    }
                    app.recompute_divergence();

                    // Raise the HUD when the focused host's divergence
                    // changes. This is the only instrumentation allowed over
                    // a shell, and it states the reason in words.
                    if app.view == AppView::Session {
                        if let Some(idx) = app.session_manager.active_index {
                            if let Some(session) = app.session_manager.sessions.get(idx) {
                                if let Some(name) =
                                    app.host_name_for(&session.hostname, session.port)
                                {
                                    if let Some(text) = app.divergence_headline(&name) {
                                        let vitals = app
                                            .session_diagnostics
                                            .get(idx)
                                            .and_then(|d| d.as_ref())
                                            .map(|d| tui::hud::Vitals {
                                                rtt_ms: d.rtt_ms,
                                                down_bps: Some(d.throughput_down_bps),
                                                loss_pct: Some(d.packet_loss_pct),
                                            })
                                            .unwrap_or_default();
                                        app.hud.on_change(tui::hud::Reason::Diverged(text), vitals);
                                    }
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

                    let tracker = tui_state
                        .reconnect_trackers
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
                        tui_state.reconnect_trackers[i] = None;
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
                    let notice = format!(
                        "reconnecting to {} — attempt {} of {}",
                        connect_config.hostname, attempt, max
                    );
                    app.set_status(notice.clone());
                    // A link state change is exactly what the HUD is for:
                    // transient, over the shell, stating why in words.
                    app.hud
                        .on_change(tui::hud::Reason::Link(notice), tui::hud::Vitals::default());

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
                            tui_state.reconnect_trackers[i] = None;
                            app.set_status(format!("Reconnected to {}", connect_config.hostname));
                        }
                        Err(_) => {
                            // Will retry on next tick cycle
                        }
                    }
                }

                // Fleet health probes
                if config.fleet.probe_enabled
                    && tui_state.fleet_probe_task.is_none()
                    && tui_state.fleet_prober.should_probe()
                {
                    let hosts: Vec<(String, u16)> = app
                        .hosts
                        .iter()
                        .map(|h| (h.hostname.clone(), h.port))
                        .collect();
                    if !hosts.is_empty() {
                        let timeout = tui_state.fleet_prober.probe_timeout();
                        tui_state.fleet_prober.mark_probe_started();
                        tui_state.fleet_probe_task = Some(tokio::spawn(async move {
                            fleet::FleetProber::probe_hosts(hosts, timeout).await
                        }));
                    }
                }
            }
            AppEvent::Resize(w, h) => {
                // Forward PTY resize to all active sessions
                for rt in runtimes.iter().flatten() {
                    rt.channel_tx
                        .send(SessionInput::Resize {
                            cols: w as u32,
                            rows: h as u32,
                        })
                        .await
                        .ok();
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
    config: &mut AppConfig,
    audit: &AuditLogger,
    runtimes: &mut Vec<Option<SessionRuntime>>,
    output_rxs: &mut Vec<Option<tokio::sync::mpsc::Receiver<Vec<u8>>>>,
    tui_state: &mut TuiState,
) -> anyhow::Result<KeyAction> {
    if app.add_host_active {
        match key.code {
            KeyCode::Esc => {
                app.add_host_active = false;
                app.add_host_input.clear();
                app.add_host_error = None;
                app.add_host_original = None;
                app.set_status("Add host cancelled.".to_string());
            }
            KeyCode::Enter => {
                let input = app.add_host_input.trim().to_string();
                if input.is_empty() {
                    app.add_host_active = false;
                    app.add_host_input.clear();
                    app.add_host_error = None;
                    app.add_host_original = None;
                    app.set_status("Add host cancelled.".to_string());
                } else {
                    match parse_dashboard_host_input(&input) {
                        Ok(entry) => {
                            let save = save_host_dialog_entry(
                                config,
                                app.add_host_original
                                    .as_ref()
                                    .map(|(host, port)| (host.as_str(), *port)),
                                entry,
                            );
                            config.save()?;
                            load_hosts_into_app(app, config)?;
                            app.dashboard_tab = DashboardTab::Hosts;
                            select_host(app, &save.hostname, save.port);
                            app.add_host_active = false;
                            app.add_host_input.clear();
                            app.add_host_error = None;
                            app.add_host_original = None;
                            app.set_status(format!(
                                "{} {}:{}",
                                save.verb, save.hostname, save.port
                            ));
                        }
                        Err(err) => {
                            app.add_host_error = Some(err.clone());
                            app.set_status(format!("Add host failed: {}", err));
                        }
                    }
                }
            }
            KeyCode::Backspace => {
                app.add_host_input.pop();
                app.add_host_error = None;
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !has_meta_modifier(key.modifiers) =>
            {
                app.add_host_input.push(c);
                app.add_host_error = None;
            }
            _ => {}
        }
        return Ok(KeyAction::Handled);
    }

    // Command palette — intercept before everything else
    if app.command_palette.is_some() {
        return handle_palette_key(key, app, config, audit, runtimes, output_rxs).await;
    }

    // Ctrl+P opens the palette only where no shell is listening. In a session
    // it belongs to readline (previous history), and the hint strip promises
    // that every key we do not name goes to the shell. F10 is the binding
    // that works everywhere.
    if key.code == KeyCode::Char('p')
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && !shell_has_focus(app)
    {
        open_command_palette(app);
        return Ok(KeyAction::Handled);
    }

    // Divergence overlay — Esc or D closes it, everything else passes through
    if app.show_divergence {
        match key.code {
            KeyCode::Esc | KeyCode::Char('D') | KeyCode::Char('q') => {
                app.show_divergence = false;
                return Ok(KeyAction::Handled);
            }
            _ => {}
        }
    }

    // Help toggle — intercept before anything else
    if app.show_help {
        match key.code {
            KeyCode::Char('t') => cycle_theme(app, config),
            KeyCode::Char('?') | KeyCode::Esc => {
                app.show_help = false;
                app.help_scroll = 0;
            }
            // The reference is taller than a short terminal; without this the
            // keys past the fold are unreachable and invisible.
            KeyCode::Down | KeyCode::Char('j') => {
                app.help_scroll = app.help_scroll.saturating_add(1)
            }
            KeyCode::Up | KeyCode::Char('k') => app.help_scroll = app.help_scroll.saturating_sub(1),
            KeyCode::PageDown => app.help_scroll = app.help_scroll.saturating_add(10),
            KeyCode::PageUp => app.help_scroll = app.help_scroll.saturating_sub(10),
            KeyCode::Home => app.help_scroll = 0,
            _ => {}
        }
        return Ok(KeyAction::Handled);
    }
    if key.code == KeyCode::Char('?') && !has_meta_modifier(key.modifiers) {
        // Don't intercept '?' when in an active session (it should go to remote shell)
        if app.view != AppView::Session {
            app.show_help = true;
            return Ok(KeyAction::Handled);
        }
    }

    let shell_focused = shell_has_focus(app);

    // macOS Option-symbol shortcuts (¡™£ for session 1-3, and so on). These
    // are real characters a shell may legitimately want, so they are only
    // intercepted when no shell has focus.
    if !shell_focused {
        if let Some(idx) = plain_option_symbol_session_switch_index(&key.code) {
            // A different host: let its state raise the HUD afresh.
            app.hud.reset();
            if app.session_manager.switch_to(idx) {
                if let Some(session) = app.session_manager.sessions.get(idx) {
                    let label = session.label.clone();
                    app.notifications.retain(|n| n.session_label != label);
                }
                // Arriving on a host is when "this one is the odd one out"
                // is most worth knowing.
                announce_divergence(app, idx);
                app.view = AppView::Session;
            }
            return Ok(KeyAction::Handled);
        }
    }

    // ── Function keys: one press, no prefix ──────────────────────────────
    //
    // The prefix exists because a shell owns every Ctrl combination — `Ctrl+D`
    // is EOF, `Ctrl+M` *is* Enter, `Ctrl+C` interrupts — so claiming them
    // would break the terminal ESSH is supposed to be faithful to. Function
    // keys are the one range a shell does not use, so they can be direct.
    //
    // They are translated into the command-mode keys rather than duplicating
    // the handlers, so there is one implementation of "open the monitor".
    //
    // The known cost: a full-screen remote app that uses function keys — htop,
    // mc — will not see them. The prefix still reaches everything, and those
    // apps are rarer than the shell keys the alternative would have cost.
    let mut key = key;
    let mut from_function_key = false;
    if let Some(code) = function_key_command(&key) {
        if code == KeyCode::Char('\u{1}') {
            open_command_palette(app);
            return Ok(KeyAction::Handled);
        }
        if code == KeyCode::Char('?') {
            app.show_help = !app.show_help;
            if !app.show_help {
                app.help_scroll = 0;
            }
            return Ok(KeyAction::Handled);
        }
        key = crossterm::event::KeyEvent::new(code, KeyModifiers::NONE);
        from_function_key = true;
    }

    let is_alt = has_meta_modifier(key.modifiers);

    // ── Command mode ─────────────────────────────────────────────────────
    //
    // Terminal fidelity first: while a shell has focus, ESSH claims exactly
    // one key — the prefix — and everything else is the shell's. v1 claimed
    // twelve Alt combinations, including `Alt+f` and `Alt+d`, which are
    // readline word-motions, and `Alt+.`, which is yank-last-argument.
    //
    // Pressing the prefix twice sends the literal key through, so nothing is
    // permanently unreachable.
    let mut command_mode = from_function_key || (is_alt && !shell_focused);

    if app.prefix_pending {
        app.prefix_pending = false;
        if is_prefix_key(&key, config) {
            // Double tap: fall through and let the shell receive it.
            return handle_session_key(key, app, runtimes).await;
        }
        command_mode = true;
    } else if shell_focused && is_prefix_key(&key, config) {
        app.prefix_pending = true;
        // Deliberately no status message: the session footer already turns
        // into the prefix menu, and writing here would overwrite whatever
        // result the user is currently reading.
        return Ok(KeyAction::Handled);
    } else if shell_focused && is_alt {
        // Straight to the shell, ESC-prefixed by key_to_bytes.
        return handle_session_key(key, app, runtimes).await;
    }

    // Global keybindings
    if command_mode {
        if let Some(idx) = alt_session_switch_index(&key.code) {
            // Keep the pane highlight and the session receiving keystrokes in
            // agreement; otherwise the focused border points at one pane
            // while typing goes to another.
            if let Some(tree) = app.panes.as_mut() {
                tree.set_focus(idx);
            }
            // A different host: let its state raise the HUD afresh.
            app.hud.reset();
            if app.session_manager.switch_to(idx) {
                if let Some(session) = app.session_manager.sessions.get(idx) {
                    let label = session.label.clone();
                    app.notifications.retain(|n| n.session_label != label);
                }
                // Arriving on a host is when "this one is the odd one out"
                // is most worth knowing.
                announce_divergence(app, idx);
                app.view = AppView::Session;
            }
            return Ok(KeyAction::Handled);
        }

        match key.code {
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
            // Split the current pane with the *next* session, per §3.
            // Lower-case splits vertically (side by side), upper-case
            // horizontally (stacked) — the same shape tmux uses.
            KeyCode::Char('s') | KeyCode::Char('ß') => {
                split_current_pane(app, panes::SplitDirection::Vertical);
                return Ok(KeyAction::Handled);
            }
            KeyCode::Char('S') => {
                split_current_pane(app, panes::SplitDirection::Horizontal);
                return Ok(KeyAction::Handled);
            }
            // Move focus between panes.
            KeyCode::Char('o') => {
                if let Some(tree) = app.panes.as_mut() {
                    tree.focus_next();
                    let f = tree.focus();
                    app.session_manager.switch_to(f);
                }
                return Ok(KeyAction::Handled);
            }
            // Report how many panes are on screen, since with four live
            // terminals it is not always obvious.
            KeyCode::Char('?') if app.panes.as_ref().is_some_and(|t| !t.is_single()) => {
                if let Some(tree) = app.panes.as_ref() {
                    app.set_status(format!(
                        "{} panes on screen · ^A o next · ^A O previous · ^A [ ] resize",
                        tree.len()
                    ));
                }
                return Ok(KeyAction::Handled);
            }
            // The command menu, reachable through the prefix as well as F10.
            // The handoff binds it to ⌘K; `k` keeps that shape for anyone
            // whose terminal swallows function keys.
            KeyCode::Char('k') => {
                open_command_palette(app);
                return Ok(KeyAction::Handled);
            }
            // Open the launcher again to start another session.
            //
            // It used to be reachable only at startup, so after the first
            // connection the ssh_config hosts were gone for the rest of the
            // run — and with them any way to open a second session. Which
            // made the pane split look broken: it correctly refused, because
            // there was genuinely nothing to split into and no way to make
            // one.
            KeyCode::Char('n') => {
                app.launcher.query.clear();
                refresh_launcher(app);
                app.view = AppView::Launcher;
                return Ok(KeyAction::Handled);
            }
            // Toggle the terminal+monitor split, which is a different thing
            // from a session split and keeps its own key.
            KeyCode::Char('M') => {
                if app.session_manager.has_sessions() {
                    app.split_pane = !app.split_pane;
                }
                return Ok(KeyAction::Handled);
            }
            // `[` and `]` resize whichever split is in front: a session
            // split if one exists, otherwise the terminal/monitor split.
            KeyCode::Char('[') => {
                match app.panes.as_mut().filter(|t| !t.is_single()) {
                    Some(tree) => tree.resize_focused(-0.05),
                    None => {
                        if app.split_pane {
                            app.split_pane_pct = app.split_pane_pct.saturating_sub(5).max(20);
                        }
                    }
                }
                return Ok(KeyAction::Handled);
            }
            KeyCode::Char(']') => {
                match app.panes.as_mut().filter(|t| !t.is_single()) {
                    Some(tree) => tree.resize_focused(0.05),
                    None => {
                        if app.split_pane {
                            app.split_pane_pct = (app.split_pane_pct + 5).min(80);
                        }
                    }
                }
                return Ok(KeyAction::Handled);
            }
            // Focus the previous pane.
            KeyCode::Char('O') => {
                if let Some(tree) = app.panes.as_mut() {
                    tree.focus_prev();
                    let f = tree.focus();
                    app.session_manager.switch_to(f);
                }
                return Ok(KeyAction::Handled);
            }
            // Alt+d: detach (go back to dashboard) (∂ is Option+d on macOS)
            KeyCode::Char('d') | KeyCode::Char('∂') => {
                app.view = AppView::Dashboard;
                return Ok(KeyAction::Handled);
            }
            // Alt+t: cycle theme († is Option+t on macOS)
            KeyCode::Char('t') | KeyCode::Char('†') => {
                cycle_theme(app, config);
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
                                match list_remote_files(&rt.ssh_session.handle, &remote_path).await
                                {
                                    Ok(files) => {
                                        if let Some(ref mut fb) = app.file_browser {
                                            fb.remote_files = files;
                                        }
                                    }
                                    Err(e) => {
                                        if let Some(ref mut fb) = app.file_browser {
                                            fb.status_message =
                                                Some(format!("Remote listing failed: {}", e));
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
                    if idx < tui_state.reconnect_trackers.len() {
                        tui_state.reconnect_trackers.remove(idx);
                    }
                    app.session_manager.remove_session(idx);
                    app.remove_session_tracking(idx);

                    // Keep the pane tree in step: drop the pane, then shift
                    // every higher index down. Skipping the renumber would
                    // leave panes rendering a different session's terminal.
                    if let Some(tree) = app.panes.as_mut() {
                        if tree.contains(idx) && !tree.close(idx) {
                            app.panes = None;
                        }
                    }
                    if let Some(tree) = app.panes.as_mut() {
                        tree.reindex_after_removal(idx);
                    }

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
    // Catch them here so they work regardless of terminal emulator config —
    // but only when no shell has focus, since ƒ and ∂ are characters a user
    // may genuinely want to type.
    if !is_alt && !shell_focused {
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
            KeyCode::Char('†') => {
                // Option+t: cycle theme
                cycle_theme(app, config);
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

    // Any key other than a second `d` cancels a pending host deletion, so a
    // confirmation can never be answered by accident.
    if app.pending_delete.is_some() && key.code != KeyCode::Char('d') {
        app.pending_delete = None;
        app.set_status("Delete cancelled.".to_string());
    }

    // View-specific keybindings
    match app.view {
        AppView::Launcher => handle_launcher_key(key, app, tui_state),
        AppView::Dashboard => {
            handle_dashboard_key(key, app, config, audit, runtimes, output_rxs, tui_state).await
        }
        AppView::Session => handle_session_key(key, app, runtimes).await,
        AppView::Monitor => handle_monitor_key(key, app, config),
        AppView::PortForwarding => handle_portfwd_key(key, app, config),
        AppView::FileBrowser => handle_filebrowser_key(key, app, config, runtimes).await,
    }
}

async fn handle_filebrowser_key(
    key: crossterm::event::KeyEvent,
    app: &mut App,
    config: &mut AppConfig,
    runtimes: &mut [Option<SessionRuntime>],
) -> anyhow::Result<KeyAction> {
    let browser = match app.file_browser.as_mut() {
        Some(b) => b,
        None => return Ok(KeyAction::Handled),
    };

    match key.code {
        KeyCode::Char('t') => {
            cycle_theme(app, config);
        }
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
                                match list_remote_files(&rt.ssh_session.handle, &remote_path).await
                                {
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
        KeyCode::Backspace => match browser.focus {
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
        },
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
                            match upload_file(
                                &rt.ssh_session.handle,
                                &local_entry.path,
                                &remote_dest,
                            )
                            .await
                            {
                                Ok(_) => {
                                    if let Some(ref mut fb) = app.file_browser {
                                        if let Some(ref mut t) = fb.transfer {
                                            t.bytes_transferred = t.total_bytes;
                                            t.complete = true;
                                        }
                                        fb.status_message =
                                            Some(format!("Uploaded {}", local_entry.name));
                                        // Refresh remote
                                        let rp = fb.remote_path.clone();
                                        if let Ok(files) =
                                            list_remote_files(&rt.ssh_session.handle, &rp).await
                                        {
                                            fb.remote_files = files;
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
                            match download_file(&rt.ssh_session.handle, &remote_src, &local_dest)
                                .await
                            {
                                Ok(_) => {
                                    if let Some(ref mut fb) = app.file_browser {
                                        if let Some(ref mut t) = fb.transfer {
                                            t.bytes_transferred = t.total_bytes;
                                            t.complete = true;
                                        }
                                        fb.status_message =
                                            Some(format!("Downloaded {}", remote_entry.name));
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
                            match exec_remote_command(
                                &rt.ssh_session.handle,
                                &format!("mkdir -p '{}'", new_dir),
                            )
                            .await
                            {
                                Ok(_) => {
                                    if let Some(ref mut fb) = app.file_browser {
                                        fb.status_message = Some("Created new_folder".into());
                                        let rp = fb.remote_path.clone();
                                        if let Ok(files) =
                                            list_remote_files(&rt.ssh_session.handle, &rp).await
                                        {
                                            fb.remote_files = files;
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
        KeyCode::Delete => match browser.focus {
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
                                        if let Ok(files) =
                                            list_remote_files(&rt.ssh_session.handle, &rp).await
                                        {
                                            fb.remote_files = files;
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
        },
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
    config: &mut AppConfig,
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
                                    mgr.add_local(
                                        "127.0.0.1",
                                        bind_port,
                                        &target_host,
                                        target_port,
                                    );
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
        KeyCode::Char('t') => {
            cycle_theme(app, config);
        }
        KeyCode::Char('a') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
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
    config: &mut AppConfig,
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
                        // A different host: let its state raise the HUD afresh.
                        app.hud.reset();
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
    config: &mut AppConfig,
    audit: &AuditLogger,
    runtimes: &mut Vec<Option<SessionRuntime>>,
    output_rxs: &mut Vec<Option<tokio::sync::mpsc::Receiver<Vec<u8>>>>,
    tui_state: &mut TuiState,
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
                // Only connect to a host the filter actually matched.
                //
                // `select_first_filtered` leaves the selection untouched when
                // nothing matches, so connecting to "the selected host" here
                // would open whatever was highlighted *before* the search —
                // you type one hostname and land on another. For an SSH
                // client that is the worst possible failure: it succeeds,
                // silently, against the wrong machine.
                let matched = app.filtered_host_indices();
                if matched.is_empty() {
                    let q = app.search_query.clone();
                    app.set_status(format!("no host matches \"{q}\""));
                } else {
                    app.search_active = false;
                    if let Some(host) = app.selected_host().cloned() {
                        open_session(app, config, audit, &host, runtimes, output_rxs).await?;
                    }
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
        // Navigation follows the visible tab. It used to always drive the
        // host list, so on the Sessions tab the arrows moved a cursor nobody
        // could see, on a list nobody was looking at.
        KeyCode::Down | KeyCode::Char('j') => {
            if app.dashboard_tab == DashboardTab::Sessions {
                app.next_session_row();
            } else {
                app.next_host();
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.dashboard_tab == DashboardTab::Sessions {
                app.prev_session_row();
            } else {
                app.prev_host();
            }
        }
        KeyCode::Char('1') => app.dashboard_tab = DashboardTab::Sessions,
        KeyCode::Char('2') => app.dashboard_tab = DashboardTab::Hosts,
        KeyCode::Char('3') => app.dashboard_tab = DashboardTab::Fleet,
        KeyCode::Char('4') => app.dashboard_tab = DashboardTab::Config,
        KeyCode::Char('/') => {
            app.search_active = true;
            app.search_query.clear();
        }
        // The launcher is where the ssh_config hosts live; the dashboard only
        // knows hosts it has already seen.
        KeyCode::Char('n') => {
            app.launcher.query.clear();
            refresh_launcher(app);
            app.view = AppView::Launcher;
        }
        KeyCode::Char('e') if app.dashboard_tab == DashboardTab::Config => {
            match edit_config_from_tui(config) {
                Ok(()) => {
                    reload_app_config_state(app, config, tui_state)?;
                    app.set_status("Config reloaded.".to_string());
                }
                Err(err) => app.set_status(format!("Config edit failed: {}", err)),
            }
        }
        // Guarded on CONTROL: the command prefix is only intercepted while a
        // shell has focus, so in the dashboard `Ctrl+A` reached this arm and
        // opened the Add Host dialog.
        KeyCode::Char('a') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.add_host_active = true;
            app.add_host_input.clear();
            app.add_host_error = None;
            app.add_host_original = None;
        }
        // 'D' opens the divergence overlay for the selected host. Upper-case
        // so it does not collide with 'd' (delete host).
        KeyCode::Char('D')
            if matches!(app.dashboard_tab, DashboardTab::Hosts | DashboardTab::Fleet) =>
        {
            app.show_divergence = !app.show_divergence;
        }
        KeyCode::Char('e') if app.dashboard_tab == DashboardTab::Hosts => {
            if let Some(host) = app.selected_host().cloned() {
                app.add_host_active = true;
                app.add_host_input = format_host_dialog_input(&host);
                app.add_host_error = None;
                app.add_host_original = Some((host.hostname.clone(), host.port));
            } else {
                app.set_status("No host selected to edit.".to_string());
            }
        }
        KeyCode::Char('t') => cycle_theme(app, config),
        KeyCode::Char('r') => {
            load_hosts_into_app(app, config)?;
            app.set_status("Hosts refreshed.".to_string());
        }
        KeyCode::Char('d') => {
            // Deleting a host rewrites the user's config file and drops the
            // cached host key. One keystroke doing that with no confirmation
            // and no undo is how a configured host silently disappears —
            // which is exactly what happened during development.
            if let Some(host) = app.selected_host().cloned() {
                if app.pending_delete.as_deref() != Some(host.name.as_str()) {
                    app.pending_delete = Some(host.name.clone());
                    app.set_status(format!(
                        "Delete {} ({}:{}) from config and cache? Press d again to confirm, any other key to cancel.",
                        host.name, host.hostname, host.port
                    ));
                    return Ok(KeyAction::Handled);
                }
                app.pending_delete = None;

                let removed_from_config = remove_config_host(config, &host.hostname, host.port);
                if removed_from_config {
                    config.save()?;
                }

                let removed_from_cache = match CacheDb::open_default() {
                    Ok(db) => db.remove_host(&host.hostname, host.port)?,
                    Err(_) => false,
                };

                if removed_from_config || removed_from_cache {
                    load_hosts_into_app(app, config)?;
                    app.set_status(format!("Removed {}:{}.", host.hostname, host.port));
                } else {
                    app.set_status(format!(
                        "Host {}:{} was not found.",
                        host.hostname, host.port
                    ));
                }
            }
        }
        KeyCode::Enter => {
            // On the Sessions tab, Enter goes *back to* the highlighted
            // session rather than opening a new connection to a host that
            // the tab is not even showing.
            if app.dashboard_tab == DashboardTab::Sessions {
                app.clamp_session_row();
                let idx = app.selected_session;
                if app.session_manager.switch_to(idx) {
                    app.hud.reset();
                    app.view = AppView::Session;
                }
            } else if let Some(host) = app.selected_host().cloned() {
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
    runtimes: &mut [Option<SessionRuntime>],
) -> anyhow::Result<KeyAction> {
    // Everything reaching here belongs to the shell, Alt included —
    // key_to_bytes turns Alt into the ESC prefix the far end expects.
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
    config: &mut AppConfig,
) -> anyhow::Result<KeyAction> {
    match key.code {
        KeyCode::Char('t') => {
            cycle_theme(app, config);
        }
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

fn has_meta_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.intersects(KeyModifiers::ALT | KeyModifiers::META)
}

/// Whether a live remote shell currently owns the keyboard.
///
/// When it does, ESSH takes only its prefix key and forwards everything else.
/// In the dashboard, monitor and browsers there is no shell to steal from, so
/// the direct Alt bindings stay — they cost nothing there.
fn shell_has_focus(app: &App) -> bool {
    app.view == AppView::Session && app.session_manager.has_sessions()
}

/// Does this key event match the configured command prefix?
///
/// Defaults to `Ctrl+A`, the tmux/screen model. Every single-key prefix
/// collides with *something* in readline — Ctrl+A is beginning-of-line — which
/// is why the double-tap escape below exists: press it twice to send the real
/// key through.
fn is_prefix_key(key: &crossterm::event::KeyEvent, config: &AppConfig) -> bool {
    let spec = config.session.prefix_key.to_lowercase();
    let (want_ctrl, want_char) = match spec.strip_prefix("ctrl-") {
        Some(rest) => (true, rest.chars().next()),
        None => (false, spec.chars().next()),
    };
    let Some(want_char) = want_char else {
        return false;
    };
    let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    matches!(key.code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&want_char))
        && has_ctrl == want_ctrl
}

fn plain_option_symbol_session_switch_index(code: &KeyCode) -> Option<usize> {
    match code {
        KeyCode::Char('¡') => Some(0),
        KeyCode::Char('™') => Some(1),
        KeyCode::Char('£') => Some(2),
        KeyCode::Char('¢') => Some(3),
        KeyCode::Char('∞') => Some(4),
        KeyCode::Char('§') => Some(5),
        KeyCode::Char('¶') => Some(6),
        KeyCode::Char('•') => Some(7),
        KeyCode::Char('ª') => Some(8),
        _ => None,
    }
}

fn alt_session_switch_index(code: &KeyCode) -> Option<usize> {
    match code {
        KeyCode::Char('1' | '¡') => Some(0),
        KeyCode::Char('2' | '™') => Some(1),
        KeyCode::Char('3' | '£') => Some(2),
        KeyCode::Char('4' | '¢') => Some(3),
        KeyCode::Char('5' | '∞') => Some(4),
        KeyCode::Char('6' | '§') => Some(5),
        KeyCode::Char('7' | '¶') => Some(6),
        KeyCode::Char('8' | '•') => Some(7),
        KeyCode::Char('9' | 'ª') => Some(8),
        _ => None,
    }
}

fn cycle_theme(app: &mut App, config: &mut AppConfig) {
    let next = theme::next_theme_name(&config.theme).to_string();
    config.theme = next.clone();
    app.theme = theme::by_name(&next);

    if let Err(err) = config.save() {
        app.set_status(format!("Theme: {} (not saved: {})", next, err));
    } else {
        app.set_status(format!("Theme: {}", next));
    }
}

fn reload_app_config_state(
    app: &mut App,
    config: &AppConfig,
    tui_state: &mut TuiState,
) -> anyhow::Result<()> {
    app.theme = theme::by_name(&config.theme);
    load_hosts_into_app(app, config)?;
    tui_state.notification_matcher =
        NotificationMatcher::new(&config.session.notification_patterns);
    tui_state.fleet_prober = fleet::FleetProber::new(
        config.fleet.probe_interval,
        config.fleet.probe_timeout,
        config.fleet.latency_history_samples,
    );
    Ok(())
}

fn ssh_agent_available() -> bool {
    std::env::var("SSH_AUTH_SOCK").is_ok()
}

fn configured_auth_methods(
    host_key: Option<&str>,
    default_key: Option<&str>,
    has_ssh_agent: bool,
) -> anyhow::Result<Vec<AuthMethod>> {
    configured_auth_methods_for(None, host_key, default_key, has_ssh_agent)
}

/// Auth candidates for a host, honouring `IdentityFile` from `~/.ssh/config`.
///
/// Without the alias, a host defined the ordinary way —
/// `Host web-01 / IdentityFile ~/.ssh/id_web` — offers no key at all: ESSH
/// looked only at its own config and at `~/.ssh/id_*`, found neither, and
/// fell through to a password prompt for a host that has perfectly good key
/// auth. `IdentityFile` is the single most common thing in an ssh_config, so
/// ignoring it made ESSH unusable with an existing setup.
fn configured_auth_methods_for(
    alias: Option<&str>,
    host_key: Option<&str>,
    default_key: Option<&str>,
    has_ssh_agent: bool,
) -> anyhow::Result<Vec<AuthMethod>> {
    let mut identity_files = Vec::new();
    if let Some(alias) = alias {
        let cfg_path = sshconfig::default_path();
        if cfg_path.exists() {
            let local_user = std::env::var("USER").unwrap_or_default();
            let resolved = sshconfig::SshConfig::load(&cfg_path).resolve(alias, &local_user);
            for f in &resolved.identity_files {
                identity_files.push(PathBuf::from(shellexpand::tilde(f).to_string()));
            }
        }
    }

    Ok(auth_candidates_from_paths_with_identities(
        host_key,
        default_key,
        &identity_files,
        &cached_key_paths(),
        &default_ssh_key_paths(),
        has_ssh_agent,
    ))
}

// Kept as the no-IdentityFile form used by the CLI paths and the tests.
#[allow(dead_code)]
fn auth_candidates_from_paths(
    host_key: Option<&str>,
    default_key: Option<&str>,
    cached_key_paths: &[PathBuf],
    standard_key_paths: &[PathBuf],
    has_ssh_agent: bool,
) -> Vec<AuthMethod> {
    auth_candidates_from_paths_with_identities(
        host_key,
        default_key,
        &[],
        cached_key_paths,
        standard_key_paths,
        has_ssh_agent,
    )
}

/// As above, with `IdentityFile` entries tried after an explicit host key but
/// before the generic `~/.ssh/id_*` sweep — the order `ssh` itself uses.
fn auth_candidates_from_paths_with_identities(
    host_key: Option<&str>,
    default_key: Option<&str>,
    identity_files: &[PathBuf],
    cached_key_paths: &[PathBuf],
    standard_key_paths: &[PathBuf],
    has_ssh_agent: bool,
) -> Vec<AuthMethod> {
    let mut methods = Vec::new();
    let mut seen_paths = HashSet::new();

    if let Some(key) = host_key {
        add_key_auth_candidate(
            &mut methods,
            &mut seen_paths,
            PathBuf::from(shellexpand::tilde(key).to_string()),
        );
    }

    if let Some(key) = default_key {
        add_key_auth_candidate(
            &mut methods,
            &mut seen_paths,
            PathBuf::from(shellexpand::tilde(key).to_string()),
        );
    }

    for path in identity_files {
        add_key_auth_candidate(&mut methods, &mut seen_paths, path.clone());
    }

    for path in cached_key_paths {
        add_key_auth_candidate(&mut methods, &mut seen_paths, path.clone());
    }

    for path in standard_key_paths {
        add_key_auth_candidate(&mut methods, &mut seen_paths, path.clone());
    }

    if has_ssh_agent {
        methods.push(AuthMethod::Agent);
    }

    methods
}

fn add_key_auth_candidate(
    methods: &mut Vec<AuthMethod>,
    seen_paths: &mut HashSet<PathBuf>,
    path: PathBuf,
) {
    if !path.exists() || !seen_paths.insert(path.clone()) {
        return;
    }

    methods.push(AuthMethod::KeyFile {
        path,
        passphrase: None,
    });
}

fn cached_key_paths() -> Vec<PathBuf> {
    CacheDb::open_default()
        .ok()
        .and_then(|db| db.list_keys().ok())
        .map(|keys| {
            keys.into_iter()
                .map(|key| PathBuf::from(key.path))
                .collect()
        })
        .unwrap_or_default()
}

fn default_ssh_key_paths() -> Vec<PathBuf> {
    let Some(home_dir) = dirs::home_dir() else {
        return Vec::new();
    };

    let ssh_dir = home_dir.join(".ssh");
    [
        "id_ed25519",
        "id_rsa",
        "id_ecdsa",
        "id_dsa",
        "id_ed25519_sk",
        "id_ecdsa_sk",
    ]
    .into_iter()
    .map(|name| ssh_dir.join(name))
    .collect()
}

fn is_encrypted_key_error(err: &russh::keys::Error) -> bool {
    matches!(err, russh::keys::Error::KeyIsEncrypted)
}

fn prompt_key_passphrase(path: &Path) -> anyhow::Result<String> {
    prompt_password(&format!("Passphrase for {}: ", path.display()))
}

fn prompt_secret_from_tui(prompt: &str) -> anyhow::Result<String> {
    // Take stdin back from the event reader first. Without this the reader
    // thread consumes every keystroke, `prompt_password` never sees input,
    // and the app hangs on what looks like a blank screen — with no way out,
    // because Ctrl-C goes to the reader too.
    let _input = crate::event::pause_input();
    let _restore_tui = scopeguard::guard((), |_| {
        io::stdout().execute(EnterAlternateScreen).ok();
        terminal::enable_raw_mode().ok();
    });

    terminal::disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    prompt_password(prompt)
}

fn prompt_key_passphrase_from_tui(path: &Path) -> anyhow::Result<String> {
    prompt_secret_from_tui(&format!("Passphrase for {}: ", path.display()))
}

fn edit_config_from_tui(config: &mut AppConfig) -> anyhow::Result<()> {
    // Same contention as the password prompt: an editor cannot read a stdin
    // that the event reader is draining.
    let _input = crate::event::pause_input();
    let _restore_tui = scopeguard::guard((), |_| {
        io::stdout().execute(EnterAlternateScreen).ok();
        terminal::enable_raw_mode().ok();
    });

    terminal::disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    let path = AppConfig::data_dir().join("config.toml");
    if !path.exists() {
        config.save()?;
    }

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = Command::new(&editor).arg(&path).status()?;
    if !status.success() {
        anyhow::bail!("Editor '{}' exited with status {}", editor, status);
    }

    *config = AppConfig::load()?;
    Ok(())
}

fn resolve_key_auth_method<F>(
    auth: AuthMethod,
    prompt_passphrase: F,
) -> Result<AuthMethod, SshError>
where
    F: FnOnce(&Path) -> anyhow::Result<String>,
{
    match auth {
        AuthMethod::KeyFile {
            path,
            passphrase: None,
        } => match russh::keys::load_secret_key(&path, None) {
            Ok(_) => Ok(AuthMethod::KeyFile {
                path,
                passphrase: None,
            }),
            Err(err) if is_encrypted_key_error(&err) => {
                let passphrase = prompt_passphrase(&path)
                    .map_err(|e| SshError::Auth(format!("Failed to read key passphrase: {}", e)))?;
                russh::keys::load_secret_key(&path, Some(&passphrase)).map_err(SshError::Key)?;
                Ok(AuthMethod::KeyFile {
                    path,
                    passphrase: Some(passphrase),
                })
            }
            Err(err) => Err(SshError::Key(err)),
        },
        _ => Ok(auth),
    }
}

fn prompt_password_auth(username: &str, hostname: &str) -> anyhow::Result<AuthMethod> {
    let prompt = format!("{}@{}'s password: ", username, hostname);
    let password = prompt_password(&prompt)?;
    Ok(AuthMethod::Password(password))
}

fn prompt_password_auth_from_tui(
    username: &str,
    hostname: &str,
    tried: &[AuthMethod],
    reason: &str,
) -> anyhow::Result<AuthMethod> {
    // Say why we are asking. Dropping a bare password prompt on someone whose
    // key auth just failed gives them no way to tell a wrong username from a
    // wrong key from an unreachable host — and if they have no password, the
    // prompt is indistinguishable from a hang.
    let mut preamble = format!("\r\nKey authentication failed for {username}@{hostname}.\r\n");
    if !reason.is_empty() {
        preamble.push_str(&format!("  {reason}\r\n"));
    }
    let keys: Vec<String> = tried
        .iter()
        .filter_map(|m| match m {
            AuthMethod::KeyFile { path, .. } => Some(path.display().to_string()),
            AuthMethod::Agent => Some("ssh-agent".to_string()),
            _ => None,
        })
        .collect();
    if keys.is_empty() {
        preamble.push_str("  No key was offered: nothing in ssh-agent, no IdentityFile,\r\n");
        preamble.push_str("  and no key configured for this host.\r\n");
    } else {
        preamble.push_str(&format!("  Tried: {}\r\n", keys.join(", ")));
    }
    preamble.push_str("  Check the user and key for this host, or press Ctrl-C to cancel.\r\n\r\n");

    let prompt = format!("{preamble}{username}@{hostname}'s password: ");
    let password = prompt_secret_from_tui(&prompt)?;
    Ok(AuthMethod::Password(password))
}

fn should_try_next_auth_candidate(err: &SshError) -> bool {
    matches!(err, SshError::Auth(_) | SshError::Key(_))
}

async fn connect_session_with_auth_candidates(
    app: &mut App,
    config: &AppConfig,
    host: &HostDisplay,
    hostname: String,
    port: u16,
    username: String,
    auth_candidates: Vec<AuthMethod>,
) -> Result<(ConnectConfig, SshSession, String, Option<String>), SshError> {
    let mut last_auth_error = None;

    for auth in auth_candidates {
        let auth = match resolve_key_auth_method(auth, prompt_key_passphrase_from_tui) {
            Ok(auth) => auth,
            Err(err) if should_try_next_auth_candidate(&err) => {
                last_auth_error = Some(err);
                continue;
            }
            Err(err) => return Err(err),
        };

        let connect_config = ConnectConfig {
            hostname: hostname.clone(),
            port,
            username: username.clone(),
            auth,
            timeout: config.connect_timeout(),
        };

        match connect_session_for_host(app, config, host, &connect_config).await {
            Ok((ssh_session, fingerprint, banner)) => {
                return Ok((connect_config, ssh_session, fingerprint, banner));
            }
            Err(err) if should_try_next_auth_candidate(&err) => {
                last_auth_error = Some(err);
            }
            Err(err) => return Err(err),
        }
    }

    Err(last_auth_error
        .unwrap_or_else(|| SshError::Auth("No authentication methods available".to_string())))
}

async fn connect_session_for_host(
    app: &mut App,
    config: &AppConfig,
    host: &HostDisplay,
    connect_config: &ConnectConfig,
) -> Result<(SshSession, String, Option<String>), SshError> {
    let jump_host_name = config
        .hosts
        .iter()
        .find(|h| h.name == host.name || h.hostname == host.hostname)
        .and_then(|h| h.jump_host.as_deref())
        .filter(|j| !j.is_empty())
        .map(|s| s.to_string());

    if let Some(ref jump_name) = jump_host_name {
        let jump_entry = config
            .hosts
            .iter()
            .find(|h| h.name == *jump_name || h.hostname == *jump_name);
        let jump_hostname = jump_entry
            .map(|e| e.hostname.clone())
            .unwrap_or_else(|| jump_name.clone());
        let jump_port = jump_entry.map(|e| e.port).unwrap_or(22);
        let jump_user = jump_entry
            .and_then(|e| e.user.clone())
            .or_else(|| config.general.default_user.clone())
            .unwrap_or_else(whoami);
        let jump_auth = jump_entry
            .and_then(|e| e.key.as_ref())
            .map(|k| AuthMethod::KeyFile {
                path: shellexpand::tilde(k).to_string().into(),
                passphrase: None,
            })
            .unwrap_or_else(|| connect_config.auth.clone());
        let jump_auth = resolve_key_auth_method(jump_auth, prompt_key_passphrase_from_tui)?;

        let jump_config = ConnectConfig {
            hostname: jump_hostname,
            port: jump_port,
            username: jump_user,
            auth: jump_auth,
            timeout: config.connect_timeout(),
        };

        app.set_status(format!("Connecting via jump host {}...", jump_name));
        SshClient::connect_via_jump(&jump_config, connect_config).await
    } else {
        SshClient::connect(connect_config).await
    }
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

    // Look up per-host config entry for user/key overrides
    let host_entry = config
        .hosts
        .iter()
        .find(|e| e.hostname == host.hostname && e.port == host.port);

    let user = if !host.user.is_empty() {
        host.user.clone()
    } else if let Some(entry) = host_entry {
        entry
            .user
            .clone()
            .unwrap_or_else(|| config.general.default_user.clone().unwrap_or_else(whoami))
    } else {
        config.general.default_user.clone().unwrap_or_else(whoami)
    };

    let auth_candidates = configured_auth_methods_for(
        (!host.name.is_empty()).then_some(host.name.as_str()),
        host_entry.and_then(|entry| entry.key.as_deref()),
        config.general.default_key.as_deref(),
        ssh_agent_available(),
    )?;

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

    audit.log_connection_attempt(&session_id, &host.hostname, host.port, &user);

    app.view = AppView::Session;

    // Kept for the failure message: the candidate list is what makes
    // "key auth failed" actionable rather than mysterious.
    let tried_auth = auth_candidates.clone();
    let mut connect_result = connect_session_with_auth_candidates(
        app,
        config,
        host,
        host.hostname.clone(),
        host.port,
        user.clone(),
        auth_candidates,
    )
    .await;

    if matches!(connect_result, Err(ref err) if should_try_next_auth_candidate(err)) {
        let why = match &connect_result {
            Err(e) => e.to_string(),
            Ok(_) => String::new(),
        };
        match prompt_password_auth_from_tui(&user, &host.hostname, &tried_auth, &why) {
            Ok(auth) => {
                let password_connect_config = ConnectConfig {
                    hostname: host.hostname.clone(),
                    port: host.port,
                    username: user.clone(),
                    auth,
                    timeout: config.connect_timeout(),
                };
                connect_result =
                    connect_session_for_host(app, config, host, &password_connect_config)
                        .await
                        .map(|(ssh_session, fingerprint, banner)| {
                            (password_connect_config, ssh_session, fingerprint, banner)
                        });
            }
            Err(err) => {
                let reason = format!("Password prompt failed: {}", err);
                if let Some(session) = app.session_manager.sessions.get_mut(idx) {
                    session.state = SessionState::Disconnected {
                        reason: reason.clone(),
                    };
                }
                app.set_status(reason);
                return Ok(());
            }
        }
    }

    // Connect (directly or via jump host)
    match connect_result {
        Ok((connect_config, mut ssh_session, fingerprint, banner)) => {
            // TOFU check
            let db = CacheDb::open_default()?;
            let status =
                db.check_host_key(&connect_config.hostname, connect_config.port, &fingerprint)?;
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
                HostKeyStatus::Changed {
                    old_fingerprint,
                    old_last_seen,
                } => {
                    // Refuse. A changed host key is the signal that host key
                    // verification exists to raise, and silently re-trusting
                    // it — as this did — makes the whole mechanism
                    // decorative. OpenSSH refuses here too, and so do we:
                    // the old key must be removed deliberately, by a person
                    // who has confirmed why it changed.
                    audit.log_host_key_event(
                        &session_id,
                        &connect_config.hostname,
                        connect_config.port,
                        AuditEventType::HostKeyChanged,
                        &fingerprint,
                    );
                    ssh_session.close().await.ok();
                    app.set_status(format!(
                        "REFUSED: host key for {} changed (last seen {}). \
                         Known {}, offered {}. Confirm out of band, then \
                         `essh hosts remove {}` to accept the new key.",
                        connect_config.hostname,
                        crate::format::relative_time(&old_last_seen),
                        old_fingerprint,
                        fingerprint,
                        connect_config.hostname,
                    ));
                    tracing::error!(
                        host = %connect_config.hostname,
                        "{}",
                        diagnose::explain_host_key_change(
                            &old_fingerprint,
                            &fingerprint,
                            &connect_config.hostname
                        )
                    );
                    return Ok(());
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
                    match recording::SessionRecorder::new(
                        &cast_path,
                        cols as u32,
                        rows as u32,
                        Some(label.clone()),
                    ) {
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

            // Start sampling immediately, in the background, whether or not
            // the monitor is on screen. By the time the user presses F2 the
            // history is already there.
            if let Some(ref mon) = monitor {
                spawn_metric_sampler(mon.clone(), ssh_session.handle.clone());
            }

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

            // The session appearing is the success feedback. Leaving a
            // status here would park "Connected to web-01" over the key hints
            // for the first seconds of every session — exactly when someone
            // most needs to see how to reach a command.
            app.status_message = None;
            app.status_set_at = None;
            let _ = &label;
        }
        Err(e) => {
            // A session that never connected is not a session. Leaving it as
            // a tab gives a shell-less pane with no retry and no stated way
            // out — which is indistinguishable from a hang. Take it away and
            // put the user back where they can try again.
            app.session_manager.remove_session(idx);
            app.remove_session_tracking(idx);
            if idx < runtimes.len() {
                runtimes.remove(idx);
            }
            if idx < output_rxs.len() {
                output_rxs.remove(idx);
            }
            app.clamp_session_row();

            app.view = if app.session_manager.has_sessions() {
                AppView::Session
            } else {
                AppView::Launcher
            };
            app.launcher.query.clear();
            refresh_launcher(app);
            app.set_status(explain_connect_failure(
                &user,
                &host.hostname,
                &e.to_string(),
            ));
        }
    }

    Ok(())
}

/// Turn a connection failure into something the user can act on.
///
/// "Password is incorrect" is often a lie by omission: when *both* the key and
/// the password are refused for the same account, the account name is the
/// likelier culprit — the server rejects every credential for a user that
/// cannot log in, whatever the credential was.
fn explain_connect_failure(user: &str, hostname: &str, err: &str) -> String {
    let lower = err.to_lowercase();
    if lower.contains("auth") || lower.contains("password") || lower.contains("denied") {
        format!(
            "{user}@{hostname} refused every credential — check the username as well as the \
             password; a wrong user rejects a correct password"
        )
    } else {
        format!("could not connect to {user}@{hostname}: {err}")
    }
}

/// Open the command palette, populated with the current hosts and sessions.
fn open_command_palette(app: &mut App) {
    let mut palette = tui::command_palette::CommandPalette::new();
    palette.update(
        &app.hosts,
        &app.session_manager.sessions,
        app.session_manager.has_sessions(),
    );
    app.command_palette = Some(palette);
}

/// Sample a host's metrics continuously, off the event loop.
///
/// Each sample runs a command on the remote host. Awaiting that in the draw
/// loop stalls the entire UI for the round trip — noticeably on anything but
/// a LAN — and doing it only while the monitor is visible means the monitor
/// always opens empty and fills in a second later.
///
/// The collector is a bundle of `Arc`s, so the task writes into the same
/// snapshots the UI reads; the loop just copies them out.
///
/// The task ends when the session's channel dies, because `collect` then
/// fails every time — bounded by `MAX_CONSECUTIVE_FAILURES` so a flapping
/// link does not spin forever.
fn spawn_metric_sampler(
    collector: monitor::HostMetricsCollector,
    handle: std::sync::Arc<russh::client::Handle<ssh::ClientHandler>>,
) -> tokio::task::JoinHandle<()> {
    const INTERVAL: Duration = Duration::from_secs(2);
    const MAX_CONSECUTIVE_FAILURES: u32 = 5;

    tokio::spawn(async move {
        let mut failures = 0u32;
        loop {
            match collector.collect(&handle).await {
                Ok(()) => failures = 0,
                Err(_) => {
                    failures += 1;
                    if failures >= MAX_CONSECUTIVE_FAILURES {
                        return;
                    }
                }
            }
            tokio::time::sleep(INTERVAL).await;
        }
    })
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
    let output_events: Vec<&recording::CastEvent> =
        events.iter().filter(|e| e.event_type == "o").collect();

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

    // Alt is transmitted as an ESC prefix — the convention every readline,
    // vim and emacs on the far end expects. Without this, Alt+f (forward-word)
    // and Alt+d (kill-word) reach the remote as a bare `f` and `d`, which is
    // how v1 broke word motion for anyone who uses it.
    if has_meta_modifier(key.modifiers) {
        let mut stripped = key;
        stripped.modifiers.remove(KeyModifiers::ALT);
        stripped.modifiers.remove(KeyModifiers::META);
        let inner = key_to_bytes(stripped);
        if inner.is_empty() {
            return inner;
        }
        let mut out = vec![0x1b];
        out.extend_from_slice(&inner);
        return out;
    }

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
        let user = config.general.default_user.clone().unwrap_or_else(whoami);
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

fn parse_dashboard_host_input(input: &str) -> Result<config::HostEntry, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Host cannot be empty".to_string());
    }

    let (user, host_port) = match trimmed.split_once('@') {
        Some((user, rest)) if !user.is_empty() && !rest.is_empty() => {
            (Some(user.to_string()), rest)
        }
        Some(_) => return Err("Use user@host[:port] or host[:port]".to_string()),
        None => (None, trimmed),
    };

    let (hostname, port) = parse_host_and_port(host_port)?;

    Ok(config::HostEntry {
        name: hostname.clone(),
        hostname,
        port,
        user,
        key: None,
        tags: std::collections::HashMap::new(),
        jump_host: None,
        port_forwards: Vec::new(),
    })
}

fn parse_host_and_port(input: &str) -> Result<(String, u16), String> {
    if input.is_empty() {
        return Err("Host cannot be empty".to_string());
    }

    if let Some((host, port_str)) = input.rsplit_once(':') {
        if !host.is_empty() && !port_str.is_empty() {
            let port = port_str
                .parse::<u16>()
                .map_err(|_| format!("Invalid port '{}'. Use host[:port]", port_str))?;
            return Ok((host.to_string(), port));
        }
    }

    Ok((input.to_string(), 22))
}

struct HostSaveResult {
    verb: &'static str,
    hostname: String,
    port: u16,
}

fn save_host_dialog_entry(
    config: &mut AppConfig,
    original: Option<(&str, u16)>,
    entry: config::HostEntry,
) -> HostSaveResult {
    let hostname = entry.hostname.clone();
    let port = entry.port;

    let verb = if let Some((original_hostname, original_port)) = original {
        edit_config_host(config, original_hostname, original_port, entry);
        "Updated"
    } else if upsert_config_host(config, entry) {
        "Added"
    } else {
        "Updated"
    };

    HostSaveResult {
        verb,
        hostname,
        port,
    }
}

fn edit_config_host(
    config: &mut AppConfig,
    original_hostname: &str,
    original_port: u16,
    entry: config::HostEntry,
) {
    let original_idx = config
        .hosts
        .iter()
        .position(|host| host.hostname == original_hostname && host.port == original_port);

    if let Some(idx) = original_idx {
        let target_idx = config.hosts.iter().position(|host| {
            host.hostname == entry.hostname
                && host.port == entry.port
                && !(host.hostname == original_hostname && host.port == original_port)
        });

        if let Some(target_idx) = target_idx {
            config.hosts[target_idx].name = entry.name;
            config.hosts[target_idx].user = entry.user;
            config.hosts.remove(idx);
        } else {
            let existing = &mut config.hosts[idx];
            existing.name = entry.name;
            existing.hostname = entry.hostname;
            existing.port = entry.port;
            existing.user = entry.user;
        }
    } else {
        let _ = upsert_config_host(config, entry);
    }
}

fn upsert_config_host(config: &mut AppConfig, entry: config::HostEntry) -> bool {
    if let Some(existing) = config
        .hosts
        .iter_mut()
        .find(|host| host.hostname == entry.hostname && host.port == entry.port)
    {
        existing.name = entry.name;
        existing.user = entry.user;
        false
    } else {
        config.hosts.push(entry);
        true
    }
}

fn remove_config_host(config: &mut AppConfig, hostname: &str, port: u16) -> bool {
    let original_len = config.hosts.len();
    config
        .hosts
        .retain(|host| !(host.hostname == hostname && host.port == port));
    config.hosts.len() != original_len
}

fn format_host_dialog_input(host: &HostDisplay) -> String {
    let mut value = String::new();
    if !host.user.is_empty() {
        value.push_str(&host.user);
        value.push('@');
    }
    value.push_str(&host.hostname);
    if host.port != 22 {
        value.push(':');
        value.push_str(&host.port.to_string());
    }
    value
}

fn select_host(app: &mut App, hostname: &str, port: u16) {
    if let Some(idx) = app
        .hosts
        .iter()
        .position(|host| host.hostname == hostname && host.port == port)
    {
        app.selected_host = idx;
        app.table_state.select(Some(idx));
    }
}

fn load_hosts_into_app(app: &mut App, config: &AppConfig) -> anyhow::Result<()> {
    let mut displays = Vec::new();

    if let Ok(db) = CacheDb::open_default() {
        if let Ok(hosts) = db.list_hosts() {
            for h in hosts {
                let tags: Vec<String> =
                    h.tags.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
                // Merge config entry data (name, user, jump_host) for this cached host
                let config_entry = config
                    .hosts
                    .iter()
                    .find(|e| e.hostname == h.hostname && e.port == h.port);
                let name = config_entry
                    .map(|e| e.name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| h.hostname.clone());
                let user = config_entry
                    .and_then(|e| e.user.clone())
                    .unwrap_or_default();
                let jump_host = config_entry
                    .and_then(|e| e.jump_host.clone())
                    .filter(|j| !j.is_empty());
                // Merge tags from config if cache tags are empty. Keep the
                // structured pairs: chip layout and peer-set membership both
                // need key and value separately, not a pre-joined string.
                let tag_pairs: Vec<(String, String)> = if h.tags.is_empty() {
                    config_entry
                        .map(|e| crate::format::sorted_tags(&e.tags))
                        .unwrap_or_default()
                } else {
                    crate::format::sorted_tags(&h.tags)
                };
                let display_tags = if tags.is_empty() {
                    tag_pairs
                        .iter()
                        .map(|(k, v)| format!("{}={}", k, v))
                        .collect::<Vec<_>>()
                        .join(", ")
                } else {
                    tags.join(", ")
                };
                displays.push(HostDisplay {
                    name,
                    hostname: h.hostname,
                    port: h.port,
                    user,
                    status: HostStatus::NeverProbed,
                    last_seen: h.last_seen,
                    tags: display_tags,
                    tag_pairs,
                    latency_ms: None,
                    latency_history: Vec::new(),
                    jump_host,
                    diverge_count: None,
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
                status: HostStatus::NeverProbed,
                last_seen: String::new(),
                tags: tags.join(", "),
                tag_pairs: crate::format::sorted_tags(&entry.tags),
                latency_ms: None,
                latency_history: Vec::new(),
                jump_host: entry.jump_host.clone().filter(|j| !j.is_empty()),
                diverge_count: None,
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
        AuthMethod::KeyFile {
            path: expanded.into(),
            passphrase: None,
        }
    } else if std::env::var("SSH_AUTH_SOCK").is_ok() {
        AuthMethod::Agent
    } else {
        anyhow::bail!("No key configured for group and no ssh-agent available.");
    };
    let auth = resolve_key_auth_method(auth, prompt_key_passphrase)?;

    // Resolved once: `config` is a borrow and cannot move into the tasks, and
    // a Duration is Copy.
    let connect_timeout = config.connect_timeout();
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
                timeout: connect_timeout,
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
            russh::ChannelMsg::Eof
            | russh::ChannelMsg::Close
            | russh::ChannelMsg::ExitStatus { .. } => {
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

    channel
        .eof()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to send EOF: {}", e))?;

    // Wait for close
    let mut channel = channel;
    while let Some(msg) = channel.wait().await {
        match msg {
            russh::ChannelMsg::Eof
            | russh::ChannelMsg::Close
            | russh::ChannelMsg::ExitStatus { .. } => break,
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
            russh::ChannelMsg::Eof
            | russh::ChannelMsg::Close
            | russh::ChannelMsg::ExitStatus { .. } => break,
            _ => {}
        }
    }

    std::fs::write(local_path, &file_data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssh_key::rand_core::OsRng;
    use ssh_key::{Algorithm, LineEnding, PrivateKey};
    use tempfile::NamedTempFile;

    fn write_test_key(encrypted: bool) -> NamedTempFile {
        let file = NamedTempFile::new().expect("create temp key file");
        let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).expect("generate test key");
        let key = if encrypted {
            key.encrypt(&mut OsRng, "hunter2")
                .expect("encrypt test key")
        } else {
            key
        };
        std::fs::write(
            file.path(),
            key.to_openssh(LineEnding::LF)
                .expect("encode openssh key")
                .as_bytes(),
        )
        .expect("write test key");
        file
    }

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
    fn test_parse_dashboard_host_input_with_user_and_port() {
        let entry = parse_dashboard_host_input("deploy@web.example.com:2222").expect("parse");
        assert_eq!(entry.hostname, "web.example.com");
        assert_eq!(entry.port, 2222);
        assert_eq!(entry.user.as_deref(), Some("deploy"));
        assert_eq!(entry.name, "web.example.com");
    }

    #[test]
    fn test_parse_dashboard_host_input_defaults_port() {
        let entry = parse_dashboard_host_input("db.internal").expect("parse");
        assert_eq!(entry.hostname, "db.internal");
        assert_eq!(entry.port, 22);
        assert_eq!(entry.user, None);
    }

    #[test]
    fn test_alt_session_switch_index_supports_mac_option_number_symbols() {
        assert_eq!(alt_session_switch_index(&KeyCode::Char('1')), Some(0));
        assert_eq!(alt_session_switch_index(&KeyCode::Char('¡')), Some(0));
        assert_eq!(alt_session_switch_index(&KeyCode::Char('5')), Some(4));
        assert_eq!(alt_session_switch_index(&KeyCode::Char('∞')), Some(4));
        assert_eq!(alt_session_switch_index(&KeyCode::Char('9')), Some(8));
        assert_eq!(alt_session_switch_index(&KeyCode::Char('ª')), Some(8));
        assert_eq!(alt_session_switch_index(&KeyCode::Char('0')), None);
    }

    #[test]
    fn test_plain_option_symbol_session_switch_index_only_matches_mac_symbols() {
        assert_eq!(
            plain_option_symbol_session_switch_index(&KeyCode::Char('¡')),
            Some(0)
        );
        assert_eq!(
            plain_option_symbol_session_switch_index(&KeyCode::Char('•')),
            Some(7)
        );
        assert_eq!(
            plain_option_symbol_session_switch_index(&KeyCode::Char('1')),
            None
        );
    }

    #[test]
    fn alt_keys_reach_the_shell_as_an_esc_prefix() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        // Alt+f is readline forward-word. v1 swallowed it to open the file
        // browser, so the remote never saw it at all.
        let alt_f = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT);
        assert_eq!(key_to_bytes(alt_f), vec![0x1b, b'f']);

        // Alt+d is kill-word; v1 used it to detach.
        let alt_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT);
        assert_eq!(key_to_bytes(alt_d), vec![0x1b, b'd']);

        // Alt+. is yank-last-argument, the one people miss most.
        let alt_dot = KeyEvent::new(KeyCode::Char('.'), KeyModifiers::ALT);
        assert_eq!(key_to_bytes(alt_dot), vec![0x1b, b'.']);

        // macOS reports Option as META; it must behave identically.
        let meta_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::META);
        assert_eq!(key_to_bytes(meta_b), vec![0x1b, b'b']);
    }

    #[test]
    fn alt_arrows_keep_their_escape_sequence_under_the_prefix() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        // ESC + CSI D, which is what a terminal sends for Alt+Left.
        let alt_left = KeyEvent::new(KeyCode::Left, KeyModifiers::ALT);
        assert_eq!(key_to_bytes(alt_left), b"\x1b\x1b[D".to_vec());
    }

    #[test]
    fn unmodified_keys_are_unchanged_by_the_alt_path() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let plain = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE);
        assert_eq!(key_to_bytes(plain), vec![b'f']);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(key_to_bytes(ctrl_c), vec![0x03]);
    }

    #[test]
    fn prefix_key_parses_the_configured_spec() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut config = AppConfig::default();
        assert_eq!(config.session.prefix_key, "ctrl-a");

        let ctrl_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert!(is_prefix_key(&ctrl_a, &config));

        // A bare `a` is not the prefix; it is a letter someone is typing.
        let plain_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(!is_prefix_key(&plain_a, &config));

        // And Ctrl+B is not the prefix either, unless configured so.
        let ctrl_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert!(!is_prefix_key(&ctrl_b, &config));
        config.session.prefix_key = "ctrl-b".to_string();
        assert!(is_prefix_key(&ctrl_b, &config));
        assert!(!is_prefix_key(&ctrl_a, &config));
    }

    #[test]
    fn shell_focus_governs_whether_essh_claims_keys() {
        let mut app = App::new(4);
        // No sessions: the dashboard owns the keyboard.
        app.view = AppView::Session;
        assert!(
            !shell_has_focus(&app),
            "a Session view with no session is not a shell"
        );

        app.view = AppView::Dashboard;
        assert!(!shell_has_focus(&app));
    }

    #[test]
    fn test_has_meta_modifier_recognizes_alt_and_meta() {
        assert!(has_meta_modifier(KeyModifiers::ALT));
        assert!(has_meta_modifier(KeyModifiers::META));
        assert!(!has_meta_modifier(KeyModifiers::SHIFT));
    }

    #[test]
    fn test_upsert_config_host_updates_existing_entry() {
        let mut config = AppConfig::default();
        config.hosts.push(config::HostEntry {
            name: "old-name".to_string(),
            hostname: "web.example.com".to_string(),
            port: 22,
            user: Some("old-user".to_string()),
            key: Some("~/.ssh/id_ed25519".to_string()),
            tags: std::collections::HashMap::new(),
            jump_host: Some("bastion".to_string()),
            port_forwards: Vec::new(),
        });

        let added = upsert_config_host(
            &mut config,
            config::HostEntry {
                name: "web.example.com".to_string(),
                hostname: "web.example.com".to_string(),
                port: 22,
                user: Some("deploy".to_string()),
                key: None,
                tags: std::collections::HashMap::new(),
                jump_host: None,
                port_forwards: Vec::new(),
            },
        );

        assert!(!added);
        assert_eq!(config.hosts.len(), 1);
        assert_eq!(config.hosts[0].user.as_deref(), Some("deploy"));
        assert_eq!(config.hosts[0].key.as_deref(), Some("~/.ssh/id_ed25519"));
        assert_eq!(config.hosts[0].jump_host.as_deref(), Some("bastion"));
    }

    #[test]
    fn test_edit_config_host_preserves_metadata_when_endpoint_changes() {
        let mut tags = std::collections::HashMap::new();
        tags.insert("env".to_string(), "prod".to_string());

        let mut config = AppConfig::default();
        config.hosts.push(config::HostEntry {
            name: "web-old".to_string(),
            hostname: "web-old.example.com".to_string(),
            port: 22,
            user: Some("ops".to_string()),
            key: Some("~/.ssh/id_ed25519".to_string()),
            tags,
            jump_host: Some("bastion".to_string()),
            port_forwards: vec![config::PortForwardConfig {
                direction: "local".to_string(),
                bind_host: "127.0.0.1".to_string(),
                bind_port: 15432,
                target_host: "db.internal".to_string(),
                target_port: 5432,
            }],
        });

        edit_config_host(
            &mut config,
            "web-old.example.com",
            22,
            config::HostEntry {
                name: "web-new".to_string(),
                hostname: "web-new.example.com".to_string(),
                port: 2222,
                user: Some("deploy".to_string()),
                key: None,
                tags: std::collections::HashMap::new(),
                jump_host: None,
                port_forwards: Vec::new(),
            },
        );

        assert_eq!(config.hosts.len(), 1);
        let host = &config.hosts[0];
        assert_eq!(host.name, "web-new");
        assert_eq!(host.hostname, "web-new.example.com");
        assert_eq!(host.port, 2222);
        assert_eq!(host.user.as_deref(), Some("deploy"));
        assert_eq!(host.key.as_deref(), Some("~/.ssh/id_ed25519"));
        assert_eq!(host.tags.get("env").map(String::as_str), Some("prod"));
        assert_eq!(host.jump_host.as_deref(), Some("bastion"));
        assert_eq!(host.port_forwards.len(), 1);
        assert_eq!(host.port_forwards[0].target_host, "db.internal");
        assert_eq!(host.port_forwards[0].target_port, 5432);
    }

    #[test]
    fn test_remove_config_host_deletes_matching_entry_only() {
        let mut config = AppConfig::default();
        config.hosts.push(config::HostEntry {
            name: "web".to_string(),
            hostname: "web.example.com".to_string(),
            port: 22,
            user: Some("deploy".to_string()),
            key: None,
            tags: std::collections::HashMap::new(),
            jump_host: None,
            port_forwards: Vec::new(),
        });
        config.hosts.push(config::HostEntry {
            name: "db".to_string(),
            hostname: "db.example.com".to_string(),
            port: 5432,
            user: Some("postgres".to_string()),
            key: None,
            tags: std::collections::HashMap::new(),
            jump_host: None,
            port_forwards: Vec::new(),
        });

        assert!(remove_config_host(&mut config, "web.example.com", 22));
        assert_eq!(config.hosts.len(), 1);
        assert_eq!(config.hosts[0].hostname, "db.example.com");
        assert!(!remove_config_host(&mut config, "web.example.com", 22));
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
        let target = config
            .hosts
            .iter()
            .find(|h| h.name == "db-primary")
            .unwrap();
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

    #[test]
    fn test_auth_candidates_from_paths_prefers_host_key_default_key_cached_keys_then_agent() {
        let host_key = NamedTempFile::new().expect("host key temp file");
        let default_key = NamedTempFile::new().expect("default key temp file");
        let cached_key = NamedTempFile::new().expect("cached key temp file");
        let standard_key = NamedTempFile::new().expect("standard key temp file");

        let methods = auth_candidates_from_paths(
            Some(host_key.path().to_str().expect("host key path")),
            Some(default_key.path().to_str().expect("default key path")),
            &[cached_key.path().to_path_buf()],
            &[standard_key.path().to_path_buf()],
            true,
        );

        assert_eq!(methods.len(), 5);
        assert!(matches!(
            &methods[0],
            AuthMethod::KeyFile { path, .. } if path == host_key.path()
        ));
        assert!(matches!(
            &methods[1],
            AuthMethod::KeyFile { path, .. } if path == default_key.path()
        ));
        assert!(matches!(
            &methods[2],
            AuthMethod::KeyFile { path, .. } if path == cached_key.path()
        ));
        assert!(matches!(
            &methods[3],
            AuthMethod::KeyFile { path, .. } if path == standard_key.path()
        ));
        assert!(matches!(methods[4], AuthMethod::Agent));
    }

    #[test]
    fn test_auth_candidates_from_paths_deduplicates_and_skips_missing_keys() {
        let shared_key = NamedTempFile::new().expect("shared key temp file");

        let methods = auth_candidates_from_paths(
            Some(shared_key.path().to_str().expect("shared key path")),
            Some(shared_key.path().to_str().expect("shared key path")),
            &[
                shared_key.path().to_path_buf(),
                PathBuf::from("/tmp/essh-missing-key"),
            ],
            &[shared_key.path().to_path_buf()],
            false,
        );

        assert_eq!(methods.len(), 1);
        assert!(matches!(
            &methods[0],
            AuthMethod::KeyFile { path, .. } if path == shared_key.path()
        ));
    }

    #[test]
    fn test_resolve_key_auth_method_keeps_plain_key_unprompted() {
        let key_file = write_test_key(false);
        let auth = resolve_key_auth_method(
            AuthMethod::KeyFile {
                path: key_file.path().to_path_buf(),
                passphrase: None,
            },
            |_| panic!("plain key should not prompt"),
        )
        .expect("resolve plain key auth");

        match auth {
            AuthMethod::KeyFile { passphrase, .. } => assert!(passphrase.is_none()),
            _ => panic!("expected key auth"),
        }
    }

    #[test]
    fn test_resolve_key_auth_method_prompts_for_encrypted_key() {
        let key_file = write_test_key(true);
        let auth = resolve_key_auth_method(
            AuthMethod::KeyFile {
                path: key_file.path().to_path_buf(),
                passphrase: None,
            },
            |_| Ok("hunter2".to_string()),
        )
        .expect("resolve encrypted key auth");

        match auth {
            AuthMethod::KeyFile { passphrase, .. } => {
                assert_eq!(passphrase.as_deref(), Some("hunter2"));
            }
            _ => panic!("expected key auth"),
        }
    }

    #[test]
    fn test_should_try_next_auth_candidate_for_auth_and_key_errors_only() {
        assert!(should_try_next_auth_candidate(&SshError::Auth(
            "Authentication failed".to_string()
        )));
        assert!(should_try_next_auth_candidate(&SshError::Key(
            russh::keys::Error::KeyIsEncrypted,
        )));
        assert!(!should_try_next_auth_candidate(&SshError::HostKey(
            "mismatch".to_string()
        )));
    }
}

/// `essh config ssh` — what ESSH makes of the user's ssh_config.
///
/// The compatibility tier list from `sshconfig::support_for`, applied to the
/// user's actual file. A claim of "OpenSSH compatibility" that cannot be
/// inspected is a claim nobody can check.
fn print_ssh_config_check(path: &std::path::Path) {
    if !path.exists() {
        println!("No ssh_config at {}", path.display());
        return;
    }
    let cfg = sshconfig::SshConfig::load(path);

    println!("Parsed {}", path.display());
    for src in cfg.sources.iter().skip(1) {
        println!("  include: {}", src.display());
    }
    if !cfg.broken_includes.is_empty() {
        println!();
        println!("Includes that could not be read:");
        for (p, why) in &cfg.broken_includes {
            println!("  {} — {}", p, why);
        }
    }

    let aliases = cfg.aliases();
    println!();
    println!("{} host aliases", aliases.len());
    for a in aliases.iter().take(40) {
        println!("  {}", a);
    }
    if aliases.len() > 40 {
        println!("  … and {} more", aliases.len() - 40);
    }

    let caveats = cfg.global_caveats();
    println!();
    if caveats.is_empty() {
        println!("Every directive in this config is honoured natively.");
        return;
    }
    println!("Directives ESSH does not honour natively:");
    for (kw, support) in caveats {
        let note = match support {
            sshconfig::Support::ViaSystemSsh => "handled by delegating to the system ssh",
            sshconfig::Support::Unsupported => "parsed, but not acted on",
            sshconfig::Support::Full => unreachable!(),
        };
        println!("  {:<28} {}", kw, note);
    }
}

/// `essh config resolve <host>` — the `ssh -G` equivalent.
fn print_ssh_config_resolution(path: &std::path::Path, host: &str) {
    if !path.exists() {
        println!("No ssh_config at {}", path.display());
        return;
    }
    let cfg = sshconfig::SshConfig::load(path);
    let local_user = std::env::var("USER").unwrap_or_default();
    let r = cfg.resolve(host, &local_user);

    println!("alias            {}", r.alias);
    println!("hostname         {}", r.hostname);
    println!("port             {}", r.port);
    println!(
        "user             {}",
        r.user.clone().unwrap_or_else(|| local_user.clone())
    );
    for f in &r.identity_files {
        println!("identityfile     {}", f);
    }
    if r.identities_only {
        println!("identitiesonly   yes");
    }
    if let Some(j) = &r.proxy_jump {
        println!("proxyjump        {}", j);
    }
    if let Some(c) = &r.proxy_command {
        // Expanded, because the unexpanded form is not what actually runs.
        let ctx = sshconfig::tokens::TokenContext {
            hostname: r.hostname.clone(),
            original_host: r.alias.clone(),
            port: r.port,
            remote_user: r.user.clone().unwrap_or_else(|| local_user.clone()),
            local_user: local_user.clone(),
            home: dirs::home_dir().unwrap_or_default().display().to_string(),
            local_hostname: hostname_local(),
        };
        println!("proxycommand     {}", sshconfig::expand_tokens(c, &ctx));
    }
    for f in &r.local_forwards {
        println!("localforward     {}", f);
    }
    for f in &r.remote_forwards {
        println!("remoteforward    {}", f);
    }
    for f in &r.dynamic_forwards {
        println!("dynamicforward   {}", f);
    }
    if let Some(i) = r.server_alive_interval {
        println!("serveraliveinterval {}", i);
    }

    if let Some(summary) = r.caveat_summary() {
        println!();
        println!("note: {}", summary);
    }
    let unsupported: Vec<&(String, String, sshconfig::Support)> = r
        .caveats
        .iter()
        .filter(|(_, _, s)| *s == sshconfig::Support::Unsupported)
        .collect();
    if !unsupported.is_empty() {
        println!();
        println!("parsed but not acted on:");
        for (k, v, _) in unsupported {
            println!("  {} {}", k, v);
        }
    }
}

fn hostname_local() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Build a diagnosis target from ssh_config, so `essh why` probes the same
/// place a connect would actually go — including through a bastion.
fn build_diagnose_target(target: &str, port_override: Option<u16>) -> diagnose::Target {
    let (alias, user_port) = match target.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse::<u16>().ok()),
        _ => (target.to_string(), None),
    };
    // Strip any user@ prefix; it plays no part in reachability.
    let alias = alias.rsplit('@').next().unwrap_or(&alias).to_string();

    let cfg_path = sshconfig::default_path();
    let local_user = std::env::var("USER").unwrap_or_default();

    let resolved = if cfg_path.exists() {
        Some(sshconfig::SshConfig::load(&cfg_path).resolve(&alias, &local_user))
    } else {
        None
    };

    let (hostname, cfg_port, proxy_jump, proxy_command) = match &resolved {
        Some(r) => (
            r.hostname.clone(),
            r.port,
            r.proxy_jump.clone(),
            r.proxy_command.clone(),
        ),
        None => (alias.clone(), 22, None, None),
    };

    // ProxyJump can be `user@host:port` and can chain; the first hop is the
    // one we can probe from here.
    //
    // The value is usually *another alias* in the same config, not a
    // hostname — `ProxyJump bastion` where `Host bastion` sets HostName and
    // Port. Probing the literal string produces a DNS failure for a host that
    // was never meant to be resolved, which is precisely the unreadable
    // ProxyJump diagnostic this ladder exists to replace.
    let bastion = proxy_jump.and_then(|j| {
        let first = j.split(',').next().unwrap_or(&j).trim().to_string();
        if first.eq_ignore_ascii_case("none") || first.is_empty() {
            return None;
        }
        let hostpart = first.rsplit('@').next().unwrap_or(&first).to_string();
        let (name, explicit_port) = match hostpart.rsplit_once(':') {
            Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse::<u16>().ok()),
            _ => (hostpart, None),
        };

        // Resolve the hop through the config as well, so aliases work.
        match &resolved {
            Some(_) if cfg_path.exists() => {
                let hop = sshconfig::SshConfig::load(&cfg_path).resolve(&name, &local_user);
                Some(diagnose::Bastion {
                    alias: name,
                    hostname: hop.hostname,
                    port: explicit_port.unwrap_or(hop.port),
                })
            }
            _ => Some(diagnose::Bastion {
                hostname: name.clone(),
                alias: name,
                port: explicit_port.unwrap_or(22),
            }),
        }
    });

    diagnose::Target {
        alias,
        hostname,
        port: port_override.or(user_port).unwrap_or(cfg_port),
        bastion,
        proxy_command,
    }
}

/// Launcher key handling.
///
/// Typing filters, ↑/↓ select, Enter connects, Esc drops to the dashboard.
/// Everything printable goes into the query — the launcher is a search box
/// first, so single-letter commands would fight the thing it exists to do.
/// Records the intent to connect rather than doing it: the event loop makes
/// the connection after the next draw, so the user sees which host is being
/// dialled instead of a frozen screen.
fn handle_launcher_key(
    key: crossterm::event::KeyEvent,
    app: &mut App,
    tui_state: &mut TuiState,
) -> anyhow::Result<KeyAction> {
    match key.code {
        KeyCode::Esc => {
            app.view = AppView::Dashboard;
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Ok(KeyAction::Quit);
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl+D on an empty query quits, like a shell.
            if app.launcher.query.is_empty() {
                return Ok(KeyAction::Quit);
            }
        }
        KeyCode::Up => app.launcher.prev(),
        KeyCode::Down => app.launcher.next(),
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => app.launcher.prev(),
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => app.launcher.next(),
        KeyCode::Backspace => {
            app.launcher.query.pop();
            refresh_launcher(app);
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.launcher.query.clear();
            refresh_launcher(app);
        }
        KeyCode::Enter => {
            if let Some(m) = app.launcher.selected_match().cloned() {
                let host = host_display_for_candidate(&m.candidate);
                // Make sure the host list contains it, so the session and the
                // dashboard agree about what this host is called.
                if !app
                    .hosts
                    .iter()
                    .any(|h| h.hostname == host.hostname && h.port == host.port)
                {
                    let mut hosts = app.hosts.clone();
                    hosts.push(host.clone());
                    app.set_hosts(hosts);
                }
                select_host(app, &host.hostname, host.port);
                app.set_status(format!(
                    "Connecting to {}@{}:{}…",
                    host.user, host.hostname, host.port
                ));
                // Deferred so the status above is on screen before the
                // connect blocks the loop.
                tui_state.pending_connect = Some(host);
            }
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.launcher.query.push(c);
            refresh_launcher(app);
        }
        _ => {}
    }
    Ok(KeyAction::Handled)
}

/// Re-rank the launcher against the current query.
fn refresh_launcher(app: &mut App) {
    app.launcher.results = launcher::search(&app.candidates, &app.launcher.query);
    app.launcher.clamp();
}

fn host_display_for_candidate(c: &launcher::Candidate) -> HostDisplay {
    HostDisplay {
        name: c.alias.clone(),
        hostname: c.hostname.clone(),
        port: c.port,
        user: c.user.clone().unwrap_or_default(),
        status: HostStatus::NeverProbed,
        last_seen: String::new(),
        tags: c
            .tags
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join(", "),
        tag_pairs: c.tags.clone(),
        latency_ms: None,
        latency_history: Vec::new(),
        jump_host: None,
        diverge_count: None,
    }
}

/// Assemble launcher candidates from ssh_config, ESSH's config and the cache.
fn load_launcher_candidates(app: &mut App, config: &AppConfig) {
    let cfg_path = sshconfig::default_path();
    let ssh_cfg = if cfg_path.exists() {
        Some(sshconfig::SshConfig::load(&cfg_path))
    } else {
        None
    };
    let local_user = std::env::var("USER").unwrap_or_default();
    let cached = CacheDb::open_default()
        .and_then(|db| db.list_hosts())
        .unwrap_or_default();

    app.candidates = launcher::collect_candidates(
        ssh_cfg.as_ref(),
        &config.hosts,
        &cached,
        &local_user,
        chrono::Utc::now(),
    );

    app.launcher.empty_reason = if app.candidates.is_empty() {
        Some(format!(
            "Nothing in {}, in ESSH's config, or in the cache. \
             Add a Host block to your ssh_config and it will appear here.",
            cfg_path.display()
        ))
    } else {
        None
    };
    refresh_launcher(app);
}
/// Restore one workspace session.
///
/// The host is probed before connecting. That is not an optimisation: an
/// unreachable host would otherwise block until its TCP timeout, and because
/// this runs inside the event loop that stalls the entire UI — nothing
/// renders and no key is read. Probing first bounds the cost and produces a
/// better failure message than a timeout does.
async fn restore_one_session(
    app: &mut App,
    config: &AppConfig,
    audit: &AuditLogger,
    session: &workspace::WorkspaceSession,
    runtimes: &mut Vec<Option<SessionRuntime>>,
    output_rxs: &mut Vec<Option<tokio::sync::mpsc::Receiver<Vec<u8>>>>,
) -> workspace::SessionOutcome {
    // Resolve the alias the same way a manual connect would, so a workspace
    // entry means exactly what it means everywhere else.
    let candidate = app
        .candidates
        .iter()
        .find(|c| c.alias == session.host)
        .cloned();

    let mut host = match candidate {
        Some(c) => host_display_for_candidate(&c),
        None => HostDisplay {
            name: session.host.clone(),
            hostname: session.host.clone(),
            port: 22,
            user: String::new(),
            status: HostStatus::NeverProbed,
            last_seen: String::new(),
            tags: String::new(),
            tag_pairs: Vec::new(),
            latency_ms: None,
            latency_history: Vec::new(),
            jump_host: None,
            diverge_count: None,
        },
    };
    if let Some(u) = &session.user {
        host.user = u.clone();
    }
    if let Some(p) = session.port {
        host.port = p;
    }

    // Cheap reachability check first, against the host we actually resolved.
    //
    // Re-deriving the target from the alias would consult ssh_config alone
    // and fall back to treating the alias as a hostname — so a host defined
    // only in ESSH's own config would "fail DNS" for a name that was never
    // meant to be resolved.
    let mut target = build_diagnose_target(&session.host, Some(host.port));
    target.hostname = host.hostname.clone();
    let d = diagnose::diagnose(&target, std::time::Duration::from_secs(3)).await;
    if !d.succeeded() {
        return workspace::SessionOutcome::Failed(d.headline());
    }

    match open_session(app, config, audit, &host, runtimes, output_rxs).await {
        Ok(()) => {
            // `open_session` reports connection failures by setting a status
            // message and still returning `Ok`, so the Result alone would
            // record a dead session as connected. Check the session's actual
            // state instead.
            if let Some(idx) = app.session_manager.active_index {
                if let Some(SessionState::Disconnected { reason }) = app
                    .session_manager
                    .sessions
                    .get(idx)
                    .map(|s| s.state.clone())
                {
                    return workspace::SessionOutcome::Failed(reason);
                }
            }

            // The on-connect command is what makes restore restore *work*
            // rather than a fresh shell sitting in $HOME.
            if let Some(cmd) = &session.on_connect {
                if let Some(idx) = app.session_manager.active_index {
                    if let Some(Some(rt)) = runtimes.get(idx) {
                        let mut bytes = cmd.clone().into_bytes();
                        bytes.push(b'\r');
                        rt.channel_tx.send(SessionInput::Data(bytes)).await.ok();
                    }
                }
            }
            workspace::SessionOutcome::Connected
        }
        Err(e) => {
            // Reachable, but the session failed, so the fault is above the
            // transport — authentication, most likely.
            let raw = e.to_string();
            workspace::SessionOutcome::Failed(diagnose::classify_auth_failure(&raw, true).explain())
        }
    }
}

/// Split the focused pane, duplicating the current session's host.
///
/// §3 shows four different hosts on screen, so a split needs a session to put
/// in the new pane. Opening a second session to the same host is the
/// predictable default — it is what `tmux split-window` does — and the user
/// can switch that pane to another host afterwards.
fn split_current_pane(app: &mut App, direction: panes::SplitDirection) {
    let Some(active) = app.session_manager.active_index else {
        return;
    };
    // Seed the tree on first use, so a single session costs nothing.
    if app.panes.is_none() {
        app.panes = Some(panes::PaneTree::new(active));
    }
    // Only sessions not already on screen can fill a new pane.
    let shown: Vec<usize> = app.panes.as_ref().map(|t| t.sessions()).unwrap_or_default();
    let candidate = (0..app.session_manager.sessions.len()).find(|i| !shown.contains(i));

    match candidate {
        Some(next) => {
            if let Some(tree) = app.panes.as_mut() {
                tree.split(direction, next);
            }
            app.session_manager.switch_to(next);
        }
        None => {
            // Nothing to put in the new pane. Say so rather than splitting
            // into an empty region.
            app.set_status(
                "no other session to split into — open one first, then split".to_string(),
            );
        }
    }
}

/// Raise the HUD if this session's host differs from its peers.
///
/// Silence stays the correct output for a host at consensus — a HUD that
/// fires for agreement is just a status bar that forgot to be permanent.
fn announce_divergence(app: &mut App, session_idx: usize) {
    let Some(session) = app.session_manager.sessions.get(session_idx) else {
        return;
    };
    let (hostname, port) = (session.hostname.clone(), session.port);
    let Some(name) = app.host_name_for(&hostname, port) else {
        return;
    };
    let Some(text) = app.divergence_headline(&name) else {
        return;
    };
    let vitals = app
        .session_diagnostics
        .get(session_idx)
        .and_then(|d| d.as_ref())
        .map(|d| tui::hud::Vitals {
            rtt_ms: d.rtt_ms,
            down_bps: Some(d.throughput_down_bps),
            loss_pct: Some(d.packet_loss_pct),
        })
        .unwrap_or_default();
    app.hud.on_change(tui::hud::Reason::Diverged(text), vitals);
}

#[cfg(test)]
mod identity_file_auth_tests {
    use super::*;

    /// `IdentityFile` must be offered, and in ssh's own order: an explicit
    /// per-host key first, then IdentityFile, then the generic sweep.
    #[test]
    fn identity_files_are_offered_after_the_host_key_and_before_the_sweep() {
        let dir = std::env::temp_dir().join("essh-idfile-test");
        std::fs::create_dir_all(&dir).unwrap();
        let host_key = dir.join("host_key");
        let identity = dir.join("identity");
        let standard = dir.join("id_ed25519");
        for p in [&host_key, &identity, &standard] {
            std::fs::write(p, b"x").unwrap();
        }

        let methods = auth_candidates_from_paths_with_identities(
            Some(host_key.to_str().unwrap()),
            None,
            std::slice::from_ref(&identity),
            &[],
            std::slice::from_ref(&standard),
            false,
        );

        let paths: Vec<PathBuf> = methods
            .iter()
            .filter_map(|m| match m {
                AuthMethod::KeyFile { path, .. } => Some(path.clone()),
                _ => None,
            })
            .collect();

        assert_eq!(paths, vec![host_key, identity, standard]);
    }

    /// The regression this guards: a host with only an `IdentityFile` offered
    /// no key at all, so ESSH fell through to a password prompt for a host
    /// that authenticates fine by key.
    #[test]
    fn a_host_with_only_an_identity_file_still_offers_a_key() {
        let dir = std::env::temp_dir().join("essh-idfile-only");
        std::fs::create_dir_all(&dir).unwrap();
        let identity = dir.join("only_key");
        std::fs::write(&identity, b"x").unwrap();

        let methods = auth_candidates_from_paths_with_identities(
            None,
            None,
            std::slice::from_ref(&identity),
            &[],
            &[],
            false,
        );
        assert!(
            matches!(methods.first(), Some(AuthMethod::KeyFile { path, .. }) if *path == identity),
            "IdentityFile was not offered: {methods:?}"
        );
    }

    /// A missing IdentityFile must not become a candidate — offering a key
    /// that is not there wastes an auth round trip and muddies the error.
    #[test]
    fn a_missing_identity_file_is_skipped() {
        let methods = auth_candidates_from_paths_with_identities(
            None,
            None,
            &[PathBuf::from("/nonexistent/nope")],
            &[],
            &[],
            false,
        );
        assert!(methods.is_empty(), "{methods:?}");
    }
}

/// The command a function key stands for, if any.
///
/// Deliberately a short list. Every key claimed here is one a remote
/// full-screen application can no longer receive, so the set stops at the
/// commands people reach for repeatedly.
fn function_key_command(key: &crossterm::event::KeyEvent) -> Option<KeyCode> {
    if !key.modifiers.is_empty() {
        return None;
    }
    Some(match key.code {
        KeyCode::F(1) => KeyCode::Char('?'),      // help
        KeyCode::F(2) => KeyCode::Char('m'),      // host monitor
        KeyCode::F(3) => KeyCode::Char('f'),      // file browser
        KeyCode::F(4) => KeyCode::Char('p'),      // port forwards
        KeyCode::F(5) => KeyCode::Char('M'),      // terminal + monitor ("mini")
        KeyCode::F(6) => KeyCode::Char('d'),      // detach to dashboard
        KeyCode::F(7) => KeyCode::Left,           // previous session
        KeyCode::F(8) => KeyCode::Right,          // next session
        KeyCode::F(9) => KeyCode::Char('n'),      // new session
        KeyCode::F(10) => KeyCode::Char('\u{1}'), // command palette (sentinel)
        _ => return None,
    })
}

#[cfg(test)]
mod function_key_tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn the_function_keys_map_to_the_commands_the_hints_advertise() {
        assert_eq!(
            function_key_command(&k(KeyCode::F(1))),
            Some(KeyCode::Char('?'))
        );
        assert_eq!(
            function_key_command(&k(KeyCode::F(2))),
            Some(KeyCode::Char('m'))
        );
        assert_eq!(
            function_key_command(&k(KeyCode::F(3))),
            Some(KeyCode::Char('f'))
        );
        assert_eq!(
            function_key_command(&k(KeyCode::F(4))),
            Some(KeyCode::Char('p'))
        );
        // F5 is the terminal+monitor split, not the session split: the mini
        // monitor is the one people want next to a shell.
        assert_eq!(
            function_key_command(&k(KeyCode::F(5))),
            Some(KeyCode::Char('M'))
        );
        assert_eq!(
            function_key_command(&k(KeyCode::F(6))),
            Some(KeyCode::Char('d'))
        );
        // Session movement, so switching needs no prefix either.
        assert_eq!(function_key_command(&k(KeyCode::F(7))), Some(KeyCode::Left));
        assert_eq!(
            function_key_command(&k(KeyCode::F(8))),
            Some(KeyCode::Right)
        );
        assert_eq!(
            function_key_command(&k(KeyCode::F(9))),
            Some(KeyCode::Char('n'))
        );
        // F10 is the command palette; the sentinel is handled before the
        // char dispatch, so it never reaches a command handler.
        assert_eq!(
            function_key_command(&k(KeyCode::F(10))),
            Some(KeyCode::Char('\u{1}'))
        );
    }

    /// Unclaimed function keys must reach the remote application.
    #[test]
    fn function_keys_we_do_not_claim_are_left_alone() {
        for n in [11u8, 12] {
            assert_eq!(function_key_command(&k(KeyCode::F(n))), None, "F{n}");
        }
    }

    /// A modified function key belongs to the shell, not to us.
    #[test]
    fn modified_function_keys_are_not_commands() {
        let shifted = KeyEvent::new(KeyCode::F(2), KeyModifiers::SHIFT);
        assert_eq!(function_key_command(&shifted), None);
        let ctrl = KeyEvent::new(KeyCode::F(2), KeyModifiers::CONTROL);
        assert_eq!(function_key_command(&ctrl), None);
    }

    /// The keys a shell needs must never become commands. Claiming Ctrl+D
    /// would cost EOF; Ctrl+M is literally Enter.
    #[test]
    fn no_control_key_is_ever_claimed_as_a_command() {
        for c in ['d', 'm', 'c', 'w', 'p', 'a', 'z'] {
            let ev = KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
            assert_eq!(function_key_command(&ev), None, "Ctrl+{c}");
        }
    }
}

#[cfg(test)]
mod connect_failure_tests {
    use super::*;

    /// An auth failure must not blame the password alone.
    ///
    /// When the key and the password are both refused for the same account,
    /// the account name is the likelier cause — which is exactly the case
    /// that reads as "it says my password is wrong, but it isn't".
    #[test]
    fn an_auth_failure_names_the_user_as_a_suspect() {
        let msg = explain_connect_failure(
            "mattbot",
            "192.168.0.54",
            "Authentication error: password rejected",
        );
        assert!(msg.contains("mattbot@192.168.0.54"), "{msg}");
        assert!(
            msg.contains("username"),
            "the username is never mentioned: {msg}"
        );
    }

    /// A non-auth failure should say what actually happened rather than
    /// second-guessing the credentials.
    #[test]
    fn a_network_failure_is_not_reported_as_a_credential_problem() {
        let msg = explain_connect_failure("matt", "10.0.0.9", "Connection refused");
        assert!(msg.contains("Connection refused"), "{msg}");
        assert!(!msg.contains("username"), "{msg}");
    }
}
