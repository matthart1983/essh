
<p align="center">

```
                         ███████╗███████╗███████╗██╗  ██╗
                         ██╔════╝██╔════╝██╔════╝██║  ██║
                         █████╗  ███████╗███████╗███████║
                         ██╔══╝  ╚════██║╚════██║██╔══██║
                         ███████╗███████║███████║██║  ██║
                         ╚══════╝╚══════╝╚══════╝╚═╝  ╚═╝
                          Enterprise SSH Client for the Terminal
```

</p>

<p align="center">
  <a href="https://crates.io/crates/essh"><img src="https://img.shields.io/crates/v/essh.svg" alt="crates.io"></a>
  <a href="https://github.com/matthart1983/essh/actions"><img src="https://img.shields.io/github/actions/workflow/status/matthart1983/essh/ci.yml?branch=main" alt="Build Status"></a>
  <a href="https://github.com/matthart1983/essh/blob/main/LICENSE"><img src="https://img.shields.io/crates/l/essh.svg" alt="License: MIT"></a>
</p>

<p align="center">
  <b>A pure-Rust SSH client with a rich TUI — concurrent sessions, real-time host monitoring, fleet management, and connection diagnostics. All from your terminal.</b>
</p>

---

## ✨ Feature Highlights

| Feature | Description |
|---|---|
| **Pure-Rust SSH** | Built on [russh](https://github.com/warp-tech/russh) — no libssh/OpenSSH dependency. Public key, password, & SSH agent auth with TOFU host key verification. |
| **Concurrent Sessions** | Up to 9 simultaneous SSH sessions with instant `Alt+1-9` switching, `Alt+←/→` cycling, and `Alt+Tab` last-used recall. |
| **Virtual Terminal** | Full ANSI escape sequence rendering via `vt100::Parser` — colors, cursor positioning, and alternate screen all work correctly. |
| **Host Monitor** | Real-time remote htop: CPU sparklines, memory bars, disk usage, network I/O, and top processes — all collected over SSH exec channels (no agent required). |
| **Connection Diagnostics** | Live RTT, throughput (↑/↓), packet loss, and a 5-tier connection quality score per session. JSONL diagnostic logs for replay. |
| **Fleet Management** | Tag-based host groups, bulk command execution with parallel fan-out, and SSH config import. |
| **Live Fleet Health** | Background TCP probes with per-host latency sparklines and colour-coded status (green/yellow/red). Configurable probe interval. |
| **Host Search & Filter** | Press `/` to live-filter hosts by name, hostname, tags, or status. Navigate filtered results with `↑`/`↓`. |
| **Auto-Reconnect** | Exponential backoff reconnection on disconnect (2s → 30s cap). Scrollback preserved across reconnects. Tab bar shows `● Recon. 2/5`. |
| **Session Recording** | Record terminal I/O to asciicast v2 files. Replay with `essh session replay <id>` — pause, speed control (0.25×–16×), and quit. |
| **Audit Logging** | Structured JSON audit trail — connection attempts, auth results, host key events, session lifecycle. |
| **TUI Dashboard** | 4-tab dashboard (Sessions, Hosts, Fleet, Config) with a Netwatch-inspired Cyan/Yellow/DarkGray aesthetic. |

---

## 🖥️ TUI Preview

### Dashboard — Hosts Tab

```
┌──────────────────────────────────────────────────────────────────────┐
│ ESSH │ [1] Sessions  [2] Hosts  [3] Fleet  [4] Config  │ ?:Help │ 14:32:07 │
├──────────────────────────────────────────────────────────────────────┤
│ Hosts (4)                                                            │
│                                                                      │
│  Name            Hostname             Port  User     Status          │
│ ─────────────────────────────────────────────────────────────────── │
│» web-prod-1      10.0.1.10            22    deploy   ● Online        │
│  web-prod-2      10.0.1.11            22    deploy   ● Online        │
│  db-primary      10.0.2.10            22    dba      ● Online        │
│  staging-1       10.0.3.5             2222  matt     ○ Unknown       │
│                                                                      │
├──────────────────────────────────────────────────────────────────────┤
│ Enter:Connect  Alt+1-9:Session  a:Add  /:Search  r:Refresh  q:Quit │
└──────────────────────────────────────────────────────────────────────┘
```

### Session View — Active Terminal

```
┌──────────────────────────────────────────────────────────────────────┐
│ ESSH ── [1] web-prod-1  [2] db-primary  [3] staging-1 ── 14:33:42 │
├──────────────────────────────────────────────────────────────────────┤
│ deploy@web-prod-1:~$ htop                                          │
│                                                                      │
│  (full terminal output — colors, cursor, alternate screen)          │
│                                                                      │
│                                                                      │
│                                                                      │
├──────────────────────────────────────────────────────────────────────┤
│ RTT:12.3ms  ↑1.2KB/s  ↓48.5KB/s  Loss:0.0%  ●Excellent  Up:1h24m │
├──────────────────────────────────────────────────────────────────────┤
│ Alt+←→:Switch  Alt+m:Monitor  Alt+d:Detach  Alt+w:Close  Alt+h:Help│
└──────────────────────────────────────────────────────────────────────┘
```

### Host Monitor — Remote htop (`Alt+m`)

```
┌──────────────────────────────────────────────────────────────────────┐
│ ESSH ── [1] web-prod-1  [2] db-primary ────────────── 14:34:18     │
├──────────────────────────────────────────────────────────────────────┤
│ CPU   23.4%  ▁▂▃▃▂▅▃▂▁▂▃▄▅▆▅▃▂▁▂▃▃▂▁▁▂▃▅▆▅▃▂▂▃▃▂▁▂▃▃▂          │
│       ████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░ 23%                  │
│       Core 0: ██████░░░░░  28%    Core 1: █████░░░░░░  19%          │
│       Core 2: ███████░░░░  31%    Core 3: █████░░░░░░  15%          │
│──────────────────────────────────────────────────────────────────── │
│ MEM   3.2G / 8.0G (40%)  Swap: 0B / 2.0G                          │
│       ▁▁▂▂▂▂▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃                  │
│       ████████████████░░░░░░░░░░░░░░░░░░░░░░░░ 40%                  │
│──────────────────────────────────────────────────────────────────── │
│ LOAD  0.82  0.64  0.55    UPTIME  14d 6h 32m                      │
│──────────────────────────────────────────────────────────────────── │
│ DISK  /               12.4G      7.6G ██████████ 62%                │
│       /data           84.2G    115.8G █████░░░░░ 42%                │
│──────────────────────────────────────────────────────────────────── │
│ NET   RX ▁▂▃▂▁▂▃▅▃▂▁ 48.5KB/s   TX ▁▁▁▂▁▁▁▂▁▁▁ 1.2KB/s          │
│──────────────────────────────────────────────────────────────────── │
│ Top Processes (by CPU)                                              │
│  PID     Name                      CPU%    MEM%    RSS              │
│  1842    node                       8.2     3.1    256M             │
│  2104    nginx                      4.1     0.8     64M             │
│  3921    postgres                   3.7     5.2    420M             │
│  1203    containerd                 2.1     1.4    112M             │
├──────────────────────────────────────────────────────────────────────┤
│ Esc:Terminal  s:Sort(→mem)  p:Pause  r:Refresh  ↑↓:Scroll          │
└──────────────────────────────────────────────────────────────────────┘
```

---

## 📦 Installation

### From crates.io

```bash
cargo install essh
```

### From Source

```bash
git clone https://github.com/matthart1983/essh.git
cd essh
cargo build --release
# Binary is at ./target/release/essh
```

---

## 🚀 Quick Start

```bash
# Launch the TUI dashboard
essh

# Direct connect to a host
essh connect user@hostname

# Connect with a specific key
essh connect user@hostname -i ~/.ssh/id_ed25519

# Connect with password authentication
essh connect user@hostname --password

# Import hosts from your SSH config
essh hosts import

# Run a command across a host group
essh run web-servers -- uptime
```

On first launch, ESSH creates `~/.essh/` with a default `config.toml`. Navigate the Hosts tab, select a host, and press `Enter` to connect.

---

## ⚙️ Configuration

ESSH uses a TOML config file at `~/.essh/config.toml`. Initialize or edit it:

```bash
essh config init    # Create default config
essh config edit    # Open in $EDITOR
essh config show    # Print current config
```

### Example Configuration

```toml
[general]
default_user = "deploy"
default_key = "~/.ssh/id_ed25519"
tofu_policy = "prompt"       # strict | prompt | auto
cache_ttl = "30d"
log_level = "info"

[diagnostics]
enabled = true
display = "status_bar"       # status_bar | overlay | hidden
export_format = "jsonl"
keepalive_interval = 15

[host_monitor]
enabled = true
cpu_interval = 1
memory_interval = 2
process_count = 10
history_samples = 60

[session]
auto_reconnect = true
reconnect_max_retries = 5
max_concurrent = 9
scrollback_lines = 10000
recording = false            # Set true to record sessions as asciicast v2

[fleet]
probe_enabled = true
probe_interval = 60          # Seconds between TCP health probes
probe_timeout = 5            # Seconds before probe timeout
latency_history_samples = 30 # Sparkline data points per host

[security]
min_key_bits = 3072
allowed_ciphers = [
    "chacha20-poly1305@openssh.com",
    "aes256-gcm@openssh.com",
    "aes128-gcm@openssh.com",
]
allowed_kex = [
    "curve25519-sha256",
    "curve25519-sha256@libssh.org",
]

[audit]
enabled = true

# Define hosts
[[hosts]]
name = "web-prod-1"
hostname = "10.0.1.10"
port = 22
user = "deploy"
key = "~/.ssh/deploy_key"

[hosts.tags]
env = "production"
role = "web"

[[hosts]]
name = "db-primary"
hostname = "10.0.2.10"
user = "dba"

[hosts.tags]
env = "production"
role = "database"

# Define host groups (matched by tags)
[[host_groups]]
name = "web-servers"

[host_groups.match_tags]
role = "web"

[host_groups.defaults]
user = "deploy"
key = "~/.ssh/deploy_key"
```

### Data Directory Structure

```
~/.essh/
├── config.toml          # Main configuration
├── cache.db             # SQLite host key & host cache
├── audit.log            # Structured JSON audit log
├── sessions/            # Per-session diagnostic logs (JSONL)
├── recordings/          # Session recordings (if enabled)
└── known_cas/           # Trusted CA certificates
```

---

## ⌨️ Keyboard Reference

### Global (all views)

| Key | Action |
|---|---|
| `?` / `Alt+h` | Toggle help overlay |
| `Alt+1` – `Alt+9` | Jump to session N |
| `Alt+←` / `Alt+→` | Cycle to previous / next session |
| `Alt+Tab` | Switch to last-used session |
| `Alt+m` | Toggle host monitor |
| `Alt+d` | Detach to dashboard |
| `Alt+w` | Close active session |
| `Alt+r` | Rename active session |

### Dashboard

| Key | Action |
|---|---|
| `1` – `4` | Switch tab (Sessions / Hosts / Fleet / Config) |
| `j` / `k` / `↑` / `↓` | Navigate host list |
| `Enter` | Connect to selected host |
| `a` | Add host |
| `/` | Live search/filter hosts |
| `r` | Refresh host list |
| `d` | Delete selected host |
| `q` / `Ctrl+c` | Quit |

### Session Terminal

| Key | Action |
|---|---|
| *(all keys)* | Forwarded directly to the remote shell |

### Host Monitor

| Key | Action |
|---|---|
| `Esc` | Return to terminal view |
| `s` | Toggle sort (CPU ↔ Memory) |
| `p` | Pause metric collection |
| `r` | Force refresh |
| `↑` / `↓` | Scroll process list |

---

## 📋 CLI Reference

```
essh                                  Launch TUI dashboard
essh connect <user@host>              Direct SSH connection
  -p, --port <PORT>                     Port (default: 22)
  -i, --identity <FILE>                 Private key file
  --password                            Use password auth

essh hosts list [--tag key=value]     List cached hosts
essh hosts add <hostname>             Add host to cache
  -p, --port <PORT>                     Port (default: 22)
  -n, --name <NAME>                     Display name
  -u, --user <USER>                     Username
  --tag <key=value>                     Tag (repeatable)
essh hosts remove <hostname>          Remove host from cache
essh hosts import [path]              Import from SSH config
essh hosts health [--group <name>]    Run TCP health checks

essh keys list                        List cached keys
essh keys add <path> [-n name]        Add a private key
essh keys remove <name>               Remove a key

essh session list                     List session recordings
essh session replay <id>              Replay recorded session (asciicast)
                                        Space:pause  +/-:speed  q:quit

essh diag <session_id>                Show session diagnostics

essh run <group> -- <command>         Execute across a host group

essh config init                      Initialize default config
essh config edit                      Open config in $EDITOR
essh config show                      Print current config

essh audit tail [-l lines]            Show recent audit entries
```

---

## 🔐 Security

- **TOFU Host Key Verification** — Three policies: `strict` (reject unknown), `prompt` (ask user), `auto` (trust on first use). Host key fingerprints are cached in SQLite.
- **Configurable Cipher Suites** — Restrict allowed ciphers, key exchange algorithms, and MACs.
- **Minimum Key Strength** — Enforce minimum key bit length (default: 3072).
- **Structured Audit Trail** — Every connection attempt, auth result, and host key event is logged as structured JSON.

---

## 🏗️ Architecture & Tech Stack

| Layer | Crate | Purpose |
|---|---|---|
| SSH Protocol | `russh`, `russh-keys` | Pure-Rust SSH2 client implementation |
| TUI Framework | `ratatui`, `crossterm` | Terminal UI rendering and input |
| Terminal Emulation | `vt100` | Virtual terminal with ANSI escape parsing |
| Database | `rusqlite` (bundled) | Host key cache, host/key management |
| Async Runtime | `tokio` | Async I/O, task spawning, timers |
| CLI | `clap` (derive) | Command-line argument parsing |
| Serialization | `serde`, `serde_json`, `toml` | Config, audit logs, diagnostics |
| Crypto | `sha2`, `base64` | Fingerprint hashing |
| Utilities | `chrono`, `uuid`, `dirs`, `thiserror`, `anyhow` | Time, IDs, paths, errors |

### Module Structure

```
src/
├── main.rs              # Entry point, TUI event loop, CLI dispatch, auto-reconnect
├── cli/                 # Clap command definitions
├── config/              # TOML config parsing & defaults (incl. [fleet] section)
├── ssh/                 # russh client, auth (key/password/agent), host key verification
├── session/             # Session state, VirtualTerminal (vt100), SessionManager
├── tui/                 # TUI views
│   ├── dashboard.rs     # Dashboard with 4 tabs, host search/filter bar
│   ├── session_view.rs  # Terminal renderer, tab bar, status bar
│   ├── host_monitor.rs  # CPU/MEM/Disk/Net/Process panels
│   ├── help.rs          # Help overlay popup
│   └── widgets.rs       # Sparklines, bar gauges, formatters
├── fleet/               # Live fleet health — background TCP probes, latency tracking
├── recording/           # Session recording (asciicast v2) & replay
├── monitor/             # Remote host metric collection
│   ├── collector.rs     # SSH exec-based metric gathering
│   ├── parser.rs        # /proc & command output parsing
│   └── history.rs       # Sparkline ring buffers
├── diagnostics/         # Connection quality engine, JSONL logging
├── cache/               # SQLite host key & host/key CRUD
├── audit/               # Structured JSON audit logger
└── event.rs             # Async event handler (keys, ticks, SSH data)
```

---

## 🤝 Contributing

Contributions are welcome! Please:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/my-feature`)
3. Make your changes with tests (`cargo test`)
4. Ensure formatting (`cargo fmt`) and lints pass (`cargo clippy`)
5. Open a pull request

### Building & Testing

```bash
cargo build              # Debug build
cargo build --release    # Release build
cargo test               # Run all tests (94 tests)
cargo clippy             # Lint checks
cargo fmt --check        # Format check
```

---

## 📄 License

MIT — see [LICENSE](LICENSE) for details.

---

<p align="center">
  <sub>Built with 🦀 Rust — inspired by the Netwatch aesthetic</sub>
</p>
