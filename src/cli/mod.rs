use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "essh", version, about = "Enhanced SSH Client")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Connect to a host
    Connect {
        /// Host name or user@hostname
        target: String,

        /// Port number
        #[arg(short, long, default_value_t = 22)]
        port: u16,

        /// Identity file (private key)
        #[arg(short = 'i', long)]
        identity: Option<PathBuf>,

        /// Use password authentication
        #[arg(long)]
        password: bool,
    },

    /// Manage cached hosts
    Hosts {
        #[command(subcommand)]
        action: HostsAction,
    },

    /// Manage SSH keys
    Keys {
        #[command(subcommand)]
        action: KeysAction,
    },

    /// Manage session profiles
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },

    /// Show diagnostics for a past session
    Diag {
        /// Session ID
        session_id: String,
    },

    /// Save, restore and list workspaces
    ///
    /// §4's `essh workspace production` shape is a subcommand, not a bare
    /// argument, so a host genuinely named `workspace` stays reachable as
    /// `essh connect workspace`.
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },

    /// Run the performance benchmarks
    Bench,

    /// Explain why a host will not connect
    ///
    /// Runs a probe ladder — config, bastion, DNS, TCP, SSH banner — and
    /// stops at the first rung that fails. Rungs it never reached are shown
    /// as unprobed rather than as passes.
    Why {
        /// Host alias or hostname
        target: String,

        /// Port (overrides ssh_config)
        #[arg(short, long)]
        port: Option<u16>,

        /// Seconds to wait per probe
        #[arg(short, long, default_value_t = 5)]
        timeout: u64,
    },

    /// Execute a command across a host group
    Run {
        /// Group name
        group: String,

        /// Command to execute (everything after --)
        #[arg(last = true)]
        command: Vec<String>,
    },

    /// Open configuration in $EDITOR
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Stream or view audit log
    Audit {
        #[command(subcommand)]
        action: AuditAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum HostsAction {
    /// List all cached hosts
    List {
        /// Filter by tag (key=value)
        #[arg(long)]
        tag: Option<String>,
    },

    /// Add a host to the cache
    Add {
        /// Hostname or IP
        hostname: String,

        /// Port
        #[arg(short, long, default_value_t = 22)]
        port: u16,

        /// Display name
        #[arg(short, long)]
        name: Option<String>,

        /// User
        #[arg(short, long)]
        user: Option<String>,

        /// Tags (key=value, can repeat)
        #[arg(long)]
        tag: Vec<String>,
    },

    /// Remove a host from the cache
    Remove {
        /// Hostname
        hostname: String,

        /// Port
        #[arg(short, long, default_value_t = 22)]
        port: u16,
    },

    /// Import hosts from SSH config file
    Import {
        /// Path to SSH config (default: ~/.ssh/config)
        path: Option<PathBuf>,
    },

    /// Run connectivity health checks
    Health {
        /// Group name
        #[arg(long)]
        group: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum KeysAction {
    /// List cached keys
    List,

    /// Add a key to the cache
    Add {
        /// Path to the private key file
        path: PathBuf,

        /// Friendly name
        #[arg(short, long)]
        name: Option<String>,
    },

    /// Remove a key from the cache
    Remove {
        /// Key name
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum SessionAction {
    /// List saved session profiles
    List,

    /// Replay a recorded session
    Replay {
        /// Session ID
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Open config in $EDITOR
    Edit,

    /// Show current config
    Show,

    /// Initialize default config
    Init,

    /// Parse ~/.ssh/config and report what ESSH will and will not honour
    Ssh {
        /// Config file (defaults to ~/.ssh/config)
        #[arg(short, long)]
        path: Option<PathBuf>,
    },

    /// Show every ssh_config directive that applies to a host, like `ssh -G`
    ///
    /// Shaped deliberately like `ssh -G` so the two can be diffed: a user
    /// should be able to see where ESSH and OpenSSH disagree before an
    /// outage, not during one.
    Resolve {
        /// Host alias
        host: String,

        /// Config file (defaults to ~/.ssh/config)
        #[arg(short, long)]
        path: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum AuditAction {
    /// Show recent audit log entries
    Tail {
        /// Number of entries to show
        #[arg(short, long, default_value_t = 20)]
        lines: usize,
    },
}

#[derive(Subcommand, Debug)]
pub enum WorkspaceAction {
    /// List saved workspaces
    List,

    /// Restore a workspace, connecting each session
    Open {
        /// Workspace name
        name: String,
    },

    /// Save a set of hosts as a workspace
    Save {
        /// Workspace name
        name: String,

        /// Hosts to include
        #[arg(required = true)]
        hosts: Vec<String>,

        /// Command to run on each host after connecting, e.g.
        /// "tmux new -A -s essh" so restoring restores actual work
        #[arg(long)]
        on_connect: Option<String>,
    },

    /// Show what a workspace contains
    Show {
        /// Workspace name
        name: String,
    },

    /// Delete a workspace
    Remove {
        /// Workspace name
        name: String,
    },
}
