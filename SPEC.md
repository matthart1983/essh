# ESSH — Enterprise SSH Client

## 1. Overview

A terminal-based SSH client built for operations teams managing server fleets. ESSH combines enterprise connection management with real-time remote host diagnostics (CPU, memory, disk, network — like a built-in `htop`), concurrent multi-session support with seamless switching, and a Netwatch-inspired TUI aesthetic featuring performance histograms, sparklines, and color-coded health indicators.

---

## 2. Goals

- **Real-time host diagnostics**: Surface CPU, memory, disk, load, and process information from remote hosts as live-updating dashboards with sparkline histories and health indicators — not just SSH connection metrics
- **Concurrent sessions**: Run multiple SSH sessions simultaneously with instant tab-switching, split-pane views, and per-session diagnostics
- **Netwatch-inspired aesthetic**: Clean, information-dense TUI with performance histograms, latency heatmaps, sparkline bandwidth graphs, and color-coded status indicators
- **Zero-friction connections**: Auto-discover and cache hosts and keys so engineers connect once and never re-configure
- **Enterprise-grade security**: Support hardware tokens, certificate authorities, key rotation policies, and audit logging
- **Fleet management**: Manage hundreds of hosts with tagging, grouping, and bulk operations

---

## 3. Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                         TUI Layer                            │
│  (ratatui + crossterm)                                       │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌───────────────┐   │
│  │ Session  │ │ Host     │ │ Fleet    │ │ Config        │   │
│  │ Tabs     │ │ Monitor  │ │ Browser  │ │ Editor        │   │
│  └──────────┘ └──────────┘ └──────────┘ └───────────────┘   │
├──────────────────────────────────────────────────────────────┤
│                       Core Engine                            │
│  ┌──────────────┐ ┌───────────────┐ ┌────────────────────┐  │
│  │ Session      │ │ Host Metrics  │ │ Host/Key Cache     │  │
│  │ Manager      │ │ Collector     │ │ (SQLite)           │  │
│  │ (concurrent) │ │ (remote htop) │ │                    │  │
│  └──────────────┘ └───────────────┘ └────────────────────┘  │
│  ┌──────────────┐ ┌───────────────┐ ┌────────────────────┐  │
│  │ Connection   │ │ Auth          │ │ Audit Logger       │  │
│  │ Diagnostics  │ │ Provider      │ │                    │  │
│  └──────────────┘ └───────────────┘ └────────────────────┘  │
├──────────────────────────────────────────────────────────────┤
│                Transport (russh — pure Rust SSH)              │
└──────────────────────────────────────────────────────────────┘
```

---

## 4. Core Features

### 4.1 Real-Time Host Diagnostics (Remote htop)

Each active session runs a background metrics collector over the SSH channel. Metrics are gathered by executing lightweight commands on the remote host (`/proc` reads on Linux, `sysctl`/`vm_stat` on macOS) via a dedicated SSH channel — no agent installation required.

#### Collected Metrics

| Metric | Source (Linux) | Source (macOS) | Update |
|---|---|---|---|
| **CPU usage** | `/proc/stat` (per-core and aggregate) | `sysctl hw.ncpu` + `top -l 1` | 1s |
| **Memory** | `/proc/meminfo` (total, used, available, buffers, cached, swap) | `vm_stat` + `sysctl hw.memsize` | 2s |
| **Load average** | `/proc/loadavg` (1m, 5m, 15m) | `sysctl vm.loadavg` | 5s |
| **Disk usage** | `df -P` (mount, size, used, avail, %) | `df -P` | 10s |
| **Disk I/O** | `/proc/diskstats` (read/write bytes per second) | `iostat -d` | 2s |
| **Network I/O** | `/proc/net/dev` (RX/TX bytes per interface) | `netstat -ib` | 2s |
| **Top processes** | `/proc/<pid>/stat` + `/proc/<pid>/status` (top 10 by CPU, top 10 by MEM) | `ps aux --sort=-%cpu` | 2s |
| **Uptime** | `/proc/uptime` | `sysctl kern.boottime` | 10s |

#### Performance History

Each metric maintains a rolling 60-sample history buffer for sparkline rendering:
- CPU: 60 × 1s = 1 minute of CPU history
- Memory: 60 × 2s = 2 minutes of memory history
- Network I/O: 60 × 2s = 2 minutes of bandwidth history

#### Host Monitor Data Model

```rust
pub struct HostMetrics {
    pub cpu_percent: f64,              // aggregate CPU usage
    pub cpu_per_core: Vec<f64>,        // per-core percentages
    pub mem_total_kb: u64,
    pub mem_used_kb: u64,
    pub mem_available_kb: u64,
    pub mem_swap_total_kb: u64,
    pub mem_swap_used_kb: u64,
    pub load_1m: f64,
    pub load_5m: f64,
    pub load_15m: f64,
    pub disks: Vec<DiskInfo>,
    pub disk_read_bps: f64,
    pub disk_write_bps: f64,
    pub net_rx_bps: f64,
    pub net_tx_bps: f64,
    pub top_procs_cpu: Vec<ProcessInfo>,
    pub top_procs_mem: Vec<ProcessInfo>,
    pub uptime_secs: u64,
    pub os_info: String,
}

pub struct DiskInfo {
    pub mount: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub use_pct: f64,
}

pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_pct: f64,
    pub mem_pct: f64,
    pub mem_rss_kb: u64,
    pub state: String,
}
```

> **Note:** Sparkline history buffers (CPU, memory, network) are stored in separate `MetricHistory` structs in the `monitor::history` module, not inside `HostMetrics`.

### 4.2 Concurrent Session Management

ESSH supports multiple simultaneous SSH sessions, each running in its own tab with independent terminal state, diagnostics, and host metrics.

#### Session Model

```rust
pub struct Session {
    pub id: String,
    pub label: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub state: SessionState,
    pub terminal: VirtualTerminal,   // vt100-backed PTY state
    pub created_at: Instant,
    pub has_new_output: bool,
}

pub enum SessionState {
    Connecting,
    Active,
    Suspended,     // backgrounded, still connected
    Reconnecting { attempt: u32, max: u32 },
    Disconnected { reason: String },
}
```

> **Note:** Connection diagnostics and host metrics are managed separately in the `App` struct, not stored inside `Session`. This keeps session state lightweight.

#### Session Lifecycle

1. **Open**: `Enter` on a host or `essh connect <host>` opens a new session tab
2. **Switch**: `Alt+1`–`Alt+9` to jump to session by index, `Alt+←/→` to cycle, `Alt+Tab` for last-used
3. **Rename**: `Alt+r` to rename the active session tab
4. **Detach**: `Alt+d` to suspend (keep connection alive, return to dashboard)
5. **Close**: `Alt+w` to disconnect and close the tab
6. **Reconnect**: Automatic on network interruption with exponential backoff

#### Session Limits

- Max 9 concurrent sessions (matches `Alt+1`–`Alt+9` keybindings)
- Each session maintains its own scrollback buffer (configurable, default 10,000 lines)
- Suspended sessions continue receiving data into scrollback

### 4.3 Connection Diagnostics

Real-time SSH connection health metrics, displayed as a persistent status bar on every session tab.

| Metric | Source | Update |
|---|---|---|
| **RTT / Latency** | SSH keepalive round-trip timing | 1s |
| **Throughput** | Bytes sent/received per second | 1s |
| **Packet loss** | Keepalive miss ratio | 5s |
| **Cipher suite** | Negotiated algorithms (kex, cipher, MAC, compression) | On connect |
| **Auth method** | publickey / password / certificate | On connect |
| **Session uptime** | Wall-clock duration | 1s |
| **Channel count** | Active channels (shell, forwarded ports, SCP/SFTP) | On change |
| **Rekey status** | Data transferred since last rekey; threshold warning | 10s |
| **Connection quality** | Composite score as color-coded indicator (●) | 5s |

**Diagnostic log**: All metrics written to `~/.essh/sessions/<session-id>.jsonl`.

### 4.4 Host & Key Cache

| Capability | Details |
|---|---|
| **Host discovery** | Manual add, SSH config import (`~/.ssh/config`), LDAP/AD lookup, cloud APIs (AWS EC2, GCP, Azure), DNS SRV |
| **Fingerprint cache** | SQLite at `~/.essh/cache.db`; hostname, IP, port, host key fingerprint (SHA-256), first/last-seen |
| **Key management** | Index user private keys; map keys → hosts/groups; ED25519, RSA (≥3072-bit), ECDSA |
| **TOFU policy** | `strict` (reject), `prompt` (ask), `auto` (accept and cache) |
| **Key rotation detection** | Alert on host key change with fingerprint diff and accept/reject options |
| **Certificate authority** | OpenSSH CA-signed host and user certificates; pin trusted CA public keys |
| **Cache expiry** | Configurable TTL per host/group; stale entries flagged in host browser |

### 4.5 Authentication

| Method | Details |
|---|---|
| **Public key** | ED25519, RSA, ECDSA; agent forwarding; `ssh-agent` integration |
| **Certificate** | OpenSSH user certificates with CA pinning |
| **Password** | Prompted; never stored on disk |
| **MFA / 2FA** | Keyboard-interactive for TOTP/FIDO2 challenge-response |
| **Hardware tokens** | PKCS#11 / FIDO2 (e.g., YubiKey) via `ssh-agent` or direct |
| **SSO / OIDC** | Plugin-based: exchange OIDC token for short-lived SSH certificate |

### 4.6 Audit & Compliance

- **Structured audit log**: JSON at `~/.essh/audit.log` — connection attempts, auth results, host key events, session lifecycle
- **Syslog / SIEM export**: Forward via syslog (RFC 5424) or webhook
- **Session recording**: Opt-in terminal I/O capture (asciicast format) for replay
- **Policy engine**: Org-wide rules via `/etc/essh/policy.toml` (min key size, allowed ciphers, required MFA, max session duration)

### 4.7 Fleet Management

- **Tagging**: Arbitrary key-value tags (e.g., `env:prod`, `team:platform`)
- **Groups**: Logical groups with inherited connection defaults
- **Search & filter**: Full-text and tag-based search in host browser
- **Bulk operations**: Run a command across a group (parallel fan-out, streamed output)
- **Health checks**: Periodic background connectivity probes; reachable/unreachable status

---

## 5. UI Design

### 5.1 Design Language (Netwatch-Inspired)

The TUI draws directly from Netwatch's aesthetic:

- **Sparkline histograms** (`▁▂▃▄▅▆▇█`) for all time-series data (CPU, memory, network bandwidth, latency)
- **Color-coded health indicators** (`●` green/yellow/red) for connection quality and host health
- **Performance bars** for disk usage, CPU per-core, and memory utilisation
- **Tab bar** with numbered hotkeys across the top (`[1] Sessions [2] Monitor [3] Hosts ...`)
- **Persistent status footer** with context-sensitive keybindings
- **DarkGray borders** with Cyan accents for labels and Yellow for active/selected elements
- **Information-dense panels** — multiple metrics visible at a glance without scrolling

### 5.2 Main Views

#### Dashboard (default — no active session)

```
┌─ ESSH ─────────────────────────────────────────── 15:04:32 ─┐
│ [1] Sessions  [2] Hosts  [3] Fleet  [4] Config         [?]  │
├──────────────────────────────────────────────────────────────┤
│ ACTIVE SESSIONS                                              │
│  #  Label          Host              Status    Uptime        │
│  1  bastion-east   bastion.us-east   ● Active  2h 14m        │
│  2  db-primary     db01.internal     ● Active  45m           │
│  3  web-staging    web.staging.corp  ● Recon.  —             │
├──────────────────────────────────────────────────────────────┤
│ FLEET HEALTH                                                 │
│  Online: 42  │  Offline: 3  │  Unknown: 7  │  Total: 52     │
│  ████████████████████████████████████░░░░ 81%                │
├──────────────────────────────────────────────────────────────┤
│ RECENT CONNECTIONS                                           │
│  bastion-east   2m ago    db-primary   45m ago               │
│  web-staging    1h ago    cache-01     3h ago                 │
├──────────────────────────────────────────────────────────────┤
│ Enter:Connect  Alt+1-9:Session  a:Add  /:Search  q:Quit     │
└──────────────────────────────────────────────────────────────┘
```

#### Session View (active SSH session)

```
┌─ ESSH ── [1] bastion-east  [2] db-primary  [3] web-staging ─┐
│ ┌──────────────────────────────────────────────────────────┐ │
│ │ matt@bastion:~$ uptime                                   │ │
│ │  15:04:32 up 42 days, 3:17, 2 users, load avg: 0.42 ... │ │
│ │ matt@bastion:~$ █                                        │ │
│ │                                                          │ │
│ │                                                          │ │
│ │                                                          │ │
│ ├──────────────────────────────────────────────────────────┤ │
│ │ RTT:2.1ms ↑1.2KB/s ↓3.4KB/s Loss:0.0% ●Good Up:2h14m  │ │
│ └──────────────────────────────────────────────────────────┘ │
│ Alt+←→:Switch  Alt+m:Monitor  Alt+d:Detach  Alt+w:Close     │
└──────────────────────────────────────────────────────────────┘
```

#### Host Monitor Overlay (Alt+m — Netwatch-style diagnostics)

```
┌─ ESSH ── [1] bastion-east ── Host Monitor ───── 15:04:32 ─┐
├────────────────────────────────────────────────────────────┤
│ CPU  34.2%  ▁▂▃▅▆█▇▅▃▂▁▂▃▅▇█▇▅▃▂▁▁▂▃▅▆█▇▅▃▂▁▂▃▅▇█▇▅▃▂  │
│ ■■■■■■■■■■■■■■■░░░░░░░░░░░░░░░░░░░░░░░░░░ 34%            │
│ Core 0: ████████░░░ 72%   Core 1: ████░░░░░░░ 38%         │
│ Core 2: ██░░░░░░░░░ 18%   Core 3: ██████░░░░░ 52%         │
├────────────────────────────────────────────────────────────┤
│ MEM  6.2 / 16.0 GB (38%)  Swap: 0.1 / 4.0 GB              │
│ ▁▁▂▂▃▃▃▃▃▃▃▃▃▃▃▃▃▂▂▂▂▂▂▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃▃  │
│ ■■■■■■■■■■■■■■■■░░░░░░░░░░░░░░░░░░░░░░░░░ 38%            │
├────────────────────────────────────────────────────────────┤
│ LOAD  0.42  0.38  0.35    UPTIME  42d 3h 17m               │
├────────────────────────────────────────────────────────────┤
│ DISK                  Used     Avail    Use%               │
│ /                     24.1 GB  75.9 GB  ████░░░░░░ 24%     │
│ /data                 412 GB   88 GB    █████████░ 82%     │
├────────────────────────────────────────────────────────────┤
│ NET I/O   RX ▁▂▃▅▆█▇▅▃▂ 12.4 MB/s  TX ▁▁▂▂▃▃▂▂ 1.2 MB/s │
├────────────────────────────────────────────────────────────┤
│ TOP PROCESSES (by CPU)                                     │
│  PID    Name          CPU%   MEM%    RSS                   │
│  1842   postgres      28.3   12.1   1.9 GB                │
│  2103   node          14.7    8.4   1.3 GB                 │
│  891    nginx          3.2    0.8   128 MB                 │
│  1      systemd        0.1    0.3    48 MB                 │
├────────────────────────────────────────────────────────────┤
│ SSH: RTT 2.1ms  ●Good  cipher:chacha20  auth:publickey     │
├────────────────────────────────────────────────────────────┤
│ Esc:Terminal  s:Sort(cpu/mem)  p:Pause  r:Refresh          │
└────────────────────────────────────────────────────────────┘
```

#### Host Browser

```
┌─ ESSH ── Hosts ─────────────────────────────────────────────┐
│ [1] Sessions  [2] Hosts  [3] Fleet  [4] Config         [?]  │
├──────────────────────────────────────────────────────────────┤
│ HOSTS (52)                    Filter: env:prod               │
│  Name              Hostname              Status   Tags       │
│  bastion-east      bastion.us-east.corp  ● Online  prod,east │
│  db-primary        db01.internal.corp    ● Online  prod,db   │
│  web-staging       web.staging.corp      ● Offline staging   │
│  cache-01          redis.internal.corp   ○ Unknown prod      │
├──────────────────────────────────────────────────────────────┤
│ Enter:Connect  /:Filter  a:Add  i:Import  d:Delete  q:Quit  │
└──────────────────────────────────────────────────────────────┘
```

### 5.3 Session Tab Bar

The session tab bar appears whenever ≥1 session is active. It uses Netwatch's numbered-tab pattern:

```
[1] bastion-east  [2] db-primary  [3] web-staging
```

- **Active tab**: Yellow bold text
- **Suspended tab**: DarkGray text
- **Reconnecting tab**: Red text with blinking indicator
- **New activity on background tab**: Cyan underline

### 5.4 Color Palette

| Element | Color | Usage |
|---|---|---|
| App title, labels | Cyan | Header, section labels |
| Active/selected | Yellow, bold | Active tab, selected row |
| Healthy / Good | Green | Online hosts, good quality, low CPU |
| Warning | Yellow | Fair quality, moderate CPU/mem |
| Critical / Error | Red | Offline, poor quality, high CPU/mem |
| Inactive / muted | DarkGray | Borders, secondary text |
| Data values | White | Metric values, table content |
| Sparkline fill | Cyan (low) → Yellow (mid) → Red (high) | Performance histograms |

### 5.5 Performance Histogram Rendering

Following Netwatch's sparkline pattern, all time-series metrics render as Unicode block characters:

```
▁▂▃▄▅▆▇█
```

Scaling: Values are normalized to the range 0–7 and mapped to the corresponding block character. The sparkline width adapts to available terminal width.

Color thresholds for CPU/Memory sparklines:
- 0–50%: Green
- 50–80%: Yellow
- 80–100%: Red

Disk usage bars use the same color thresholds.

---

## 6. Configuration

### 6.1 File Layout

```
~/.essh/
├── config.toml            # User configuration
├── cache.db               # SQLite host/key cache
├── known_cas/             # Trusted CA public keys
├── audit.log              # Local audit log
├── sessions/              # Per-session diagnostic logs
│   └── <session-id>.jsonl
├── recordings/            # Terminal session recordings
│   └── <session-id>.cast
└── plugins/               # Installed plugins
```

### 6.2 Example `config.toml`

```toml
[general]
default_user = "matt"
default_key = "~/.ssh/id_ed25519"
tofu_policy = "prompt"          # strict | prompt | auto
cache_ttl = "30d"
log_level = "info"

[diagnostics]
enabled = true
display = "status_bar"         # status_bar | overlay | hidden
keepalive_interval = 15

[host_monitor]
enabled = true
cpu_interval = 1               # seconds
memory_interval = 2
process_count = 10             # top N processes to show
history_samples = 60           # sparkline depth

[session]
max_concurrent = 9
auto_reconnect = true
reconnect_max_retries = 5
multiplex = true
recording = false
scrollback_lines = 10000

[security]
min_key_bits = 3072
allowed_ciphers = ["chacha20-poly1305@openssh.com", "aes256-gcm@openssh.com"]
allowed_kex = ["curve25519-sha256", "curve25519-sha256@libssh.org"]
allowed_macs = ["hmac-sha2-256-etm@openssh.com", "hmac-sha2-512-etm@openssh.com"]
require_mfa_groups = ["prod-*"]

[audit]
enabled = true
syslog_target = "udp://siem.corp.example.com:514"

[[hosts]]
name = "bastion-us-east"
hostname = "bastion.us-east-1.corp.example.com"
port = 22
user = "ops"
key = "~/.ssh/id_ed25519_ops"
tags = { env = "prod", region = "us-east-1", role = "bastion" }
jump_host = ""

[[hosts]]
name = "db-primary"
hostname = "db01.internal.corp.example.com"
port = 22
user = "dba"
tags = { env = "prod", role = "database" }
jump_host = "bastion-us-east"

[[host_groups]]
name = "prod-databases"
match_tags = { env = "prod", role = "database" }
defaults = { user = "dba", key = "~/.ssh/id_ed25519_dba" }
```

---

## 7. CLI Interface

```
essh                                # Launch TUI dashboard
essh connect <host>                 # Connect to a cached host by name
essh connect <user>@<hostname>      # Ad-hoc connection (auto-cache)
essh hosts list [--tag key=val]     # List cached hosts
essh hosts add <hostname> [opts]    # Add host to cache
essh hosts import <ssh_config>      # Import from SSH config file
essh hosts discover --provider aws  # Auto-discover from cloud API
essh hosts health [--group <name>]  # Run connectivity health checks
essh keys list                      # List cached keys
essh keys add <path>                # Add key to cache
essh keys rotate <host>             # Trigger host key re-verification
essh session list                   # List active and saved sessions
essh session replay <id>            # Replay a recorded session
essh diag <session-id>              # Show diagnostics for a past session
essh run <group> -- <command>       # Execute command across host group
essh config edit                    # Open config in $EDITOR
essh audit tail                     # Stream audit log
essh plugin install <name>          # Install a plugin
```

---

## 8. Keyboard Controls

### Global (all views)

| Key | Action |
|---|---|
| `Alt+1`–`Alt+9` | Switch to session tab N |
| `Alt+←` / `Alt+→` | Cycle to previous / next session |
| `Alt+Tab` | Switch to last-used session |
| `Alt+s` | Toggle split-pane view (terminal + monitor side-by-side) |
| `Alt+[` / `Alt+]` | Adjust split-pane width (5% steps, 20–80% range) |
| `Alt+m` | Toggle host monitor overlay on active session |
| `Alt+f` | Toggle file browser (upload/download) |
| `Alt+p` | Toggle port forwarding manager |
| `Alt+d` | Detach (suspend) active session |
| `Alt+w` | Close active session |
| `Alt+h` | Toggle help overlay |
| `Alt+r` | Rename active session tab |
| `q` | Quit (from dashboard) / no-op in session |
| `?` | Help overlay (from Dashboard / Monitor views; passes through in session) |

### Dashboard

| Key | Action |
|---|---|
| `↑` `↓` | Navigate host list |
| `Enter` | Connect to selected host (opens new session tab) |
| `a` | Add host |
| `d` | Delete host |
| `/` | Filter hosts |
| `r` | Refresh host health |
| `1`–`4` | Switch dashboard tab (Sessions / Hosts / Fleet / Config) |

### Session Terminal

| Key | Action |
|---|---|
| All input | Forwarded to remote shell |
| `Alt+m` | Toggle host monitor overlay |

### Host Monitor Overlay

| Key | Action |
|---|---|
| `Esc` | Return to terminal |
| `s` | Toggle sort: by CPU / by memory |
| `p` | Pause / resume metric collection |
| `r` | Force refresh |
| `↑` `↓` | Scroll process list |

---

## 9. Technology Stack

| Component | Choice | Rationale |
|---|---|---|
| Language | **Rust** | Memory safety, performance, single binary |
| SSH library | `russh` (pure Rust) | No C dependency; full protocol control for diagnostics and multi-channel |
| TUI framework | `ratatui` + `crossterm` | Mature, flexible — same stack as Netwatch for consistent aesthetic |
| Terminal emulation | `vt100` crate | Parse remote terminal output for virtual PTY per session |
| Database | `SQLite` via `rusqlite` | Embedded, zero-config host/key cache |
| Serialization | `serde` + TOML/JSON | Config and log formats |
| Async runtime | `tokio` | Async SSH, concurrent sessions, background metric collection |
| Plugin system | *(future work)* | Sandboxed extensibility for auth providers and discovery backends |

---

## 10. Project Structure

```
essh/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── SPEC.md
├── LICENSE
├── .gitignore
├── essh.sh
└── src/
    ├── main.rs                 # Entry point, CLI dispatch, TUI event loop, session management
    ├── event.rs                # Keyboard/tick/resize event handling
    ├── ssh/
    │   └── mod.rs              # SSH connection, authentication, shell channel
    ├── session/
    │   ├── mod.rs              # Session state, VirtualTerminal (vt100-backed)
    │   └── manager.rs          # Concurrent session lifecycle management
    ├── diagnostics/
    │   └── mod.rs              # Connection diagnostics engine (RTT, throughput, quality)
    ├── monitor/
    │   ├── mod.rs              # HostMetrics, DiskInfo, ProcessInfo data models
    │   ├── collector.rs        # Remote host metric collection via SSH exec
    │   ├── parser.rs           # Parse /proc/stat, meminfo, loadavg, df, net/dev, ps
    │   └── history.rs          # Rolling sample buffers for sparklines
    ├── cache/
    │   └── mod.rs              # SQLite host/key cache, TOFU, tagging
    ├── config/
    │   └── mod.rs              # TOML config parsing and defaults
    ├── audit/
    │   └── mod.rs              # Structured JSON audit logging
    ├── tui/
    │   ├── mod.rs              # App state, render dispatch, view management
    │   ├── dashboard.rs        # Dashboard view (sessions, hosts, fleet, config tabs)
    │   ├── session_view.rs     # Terminal rendering, tab bar, status bar, footer
    │   ├── host_monitor.rs     # Host monitor overlay (htop-style diagnostics)
    │   ├── help.rs             # Help overlay with keybinding reference
    │   └── widgets.rs          # Sparklines, bar gauges, format helpers
    └── cli/
        └── mod.rs              # CLI argument definitions (clap derive)
```

---

## 11. Security Considerations

- Private keys are **never** written to the cache database; only public key fingerprints and metadata are stored
- Host metrics are collected via SSH exec channels — no persistent agent on remote hosts
- All cached host fingerprints are integrity-checked with HMAC using a local device key
- Audit logs are append-only; tampering is detectable via chained hashes
- Plugin sandboxing *(future work)* will prevent untrusted plugins from accessing filesystem or network
- Memory holding passwords or key material is zeroed after use (`zeroize` crate)
- Remote metric commands are hardcoded read-only operations (no shell injection surface)

---

## 12. Milestones

| Phase | Scope | Status |
|---|---|---|
| **M1 — Core SSH** | SSH connect via `russh`, host/key cache (SQLite), TOFU, basic TUI shell with single session | ✅ Complete |
| **M2 — Session Manager** | Concurrent sessions, tab bar, `Alt+N` switching, virtual terminal per session, session lifecycle | ✅ Complete |
| **M3 — Connection Diagnostics** | RTT, throughput, packet loss, cipher info, quality score, status bar, diagnostic logs | ✅ Complete |
| **M4 — Host Monitor** | Remote metric collection via SSH exec, CPU/mem/disk/net/process parsing, sparkline history buffers | ✅ Complete |
| **M5 — Monitor UI** | Host monitor overlay with Netwatch-style sparklines, histograms, per-core CPU, process table, color-coded health | ✅ Complete |
| **M6 — Dashboard & Fleet** | Dashboard view, fleet health summary, host browser with search/filter, health checks | ✅ Complete |
| **M7 — Enterprise Auth** | Certificate auth, PKCS#11/FIDO2, SSO/OIDC plugin, MFA enforcement | 🔲 Future |
| **M8 — Audit & Compliance** | Structured audit log, syslog export, session recording, policy engine | 🔲 Future |
| **M9 — Cloud Discovery** | AWS/GCP/Azure host discovery plugins, SSH config import, DNS SRV | 🔲 Future |
| **M10 — Polish & Plugins** | Auto-reconnect, multiplexing, bulk `run`, plugin system, packaging (Homebrew, deb, rpm) | 🔲 Future |

---

## 13. Planned Enhancements

### 13.1 SSH Agent Forwarding

Wire up the existing `AuthMethod::Agent` variant to discover keys from the local `ssh-agent` via `SSH_AUTH_SOCK`. Use agent-held keys as an automatic authentication fallback (before prompting for password). Support forwarding the agent channel to the remote host so multi-hop connections (e.g., bastion → internal server) work without copying private keys onto intermediate machines.

### 13.2 Host Search & Filter ✅

**Implemented.** Press `/` in the dashboard to activate a live filter bar. Characters narrow the host list in real-time, matching against name, hostname, user, tags, and status (case-insensitive). `↑`/`↓`/`j`/`k` navigate within filtered results. `Enter` connects to the selected match. `Esc` cancels and clears the filter. The Hosts tab title shows `(matched/total)` when a filter is active.

### 13.3 Auto-Reconnect ✅

**Implemented.** On channel EOF or unexpected disconnect, sessions automatically retry with exponential backoff (2 s, 4 s, 8 s, 16 s, capped at 30 s). Controlled by `session.auto_reconnect` (default true) and `session.reconnect_max_retries` (default 5) from config. The tab bar shows `● Recon. 2/5` with red styling during reconnection. On success, the session resumes as Active with scrollback preserved (VirtualTerminal state is never reset). On exhaustion, transitions to `Disconnected` with reason. The `ReconnectTracker` manages per-session backoff state; cleanup on `Alt+w` close.

### 13.4 Session Recording & Replay ✅

**Implemented.** When `session.recording = true` in config, all terminal I/O is recorded to asciicast v2 files at `~/.essh/recordings/<session-id>.cast`. Both output (remote → terminal) and input (user → remote) events are captured with sub-millisecond timestamps. Replay via `essh session replay <id>` plays back with accurate timing, capped at 2 s max delay per event. Controls: `Space` = pause/resume, `+`/`-` = speed (0.25×–16×), `q` = quit. `essh session list` shows available recordings. Recording is also active during reconnect sessions. The `SessionRecorder` is `Arc`-shared with the channel I/O task for lock-free concurrent writes.

### 13.5 Split-Pane View ✅

**Implemented.** Press `Alt+s` in session view to split the area horizontally — terminal on the left, host monitor on the right — as an alternative to the full-screen overlay toggle (`Alt+m`). Uses ratatui's horizontal `Layout` to divide the session area. Pane width is adjustable with `Alt+[` (shrink terminal, 5% steps) and `Alt+]` (grow terminal, 5% steps), clamped to 20–80% range. Default terminal pane is 60%. The split-pane state is per-application (applies to the active session). Help overlay and session footer updated with new keybindings.

### 13.6 Jump Host / ProxyJump Support ✅

**Implemented.** The `[[hosts]]` config `jump_host` field is now wired up. When connecting to a host with `jump_host` set, ESSH first connects to the jump host, then opens a `direct-tcpip` channel to forward TCP to the target host. A new SSH handshake runs over this forwarded channel via a custom `ChannelStream` (implements `AsyncRead` + `AsyncWrite` backed by mpsc channels). The session status bar shows the hop path as `user@target:port via jump_host`. Jump host authentication uses the jump host's configured key, falling back to the target's auth method. Empty `jump_host` strings are ignored.

### 13.7 SCP/SFTP File Transfer ✅

**Implemented.** Press `Alt+f` to open a two-pane file browser over the active session. Left pane shows local files, right pane shows remote files listed via SSH exec (`ls -la`). `Tab` switches pane focus (active pane highlighted with Yellow border). Navigation: `↑`/`↓` to browse, `Enter` to enter directories, `Backspace` to go up. Operations: `u` to upload selected local file (via `cat >` over SSH exec channel), `d` to download selected remote file (via `cat` over SSH exec channel), `m` to create remote directory, `Delete` to remove remote file. Transfer progress shown with a bar gauge at the bottom. File sizes formatted with human-readable units. `Esc` closes the browser. Uses the Netwatch aesthetic with Cyan borders, Yellow active selection, and DarkGray styling.

### 13.8 Port Forwarding Manager ✅

**Implemented.** Supports local (`-L`) TCP port forwards toggled live via `Alt+p`, which opens a forwarding manager panel. The panel shows active forwards in a table (Direction, Bind, Target, Status) with Netwatch styling. Press `a` to add a forward using the format `L:bind_port:target_host:target_port`, `d` to delete, `Esc` to close. Active forwards are shown in the session status bar (e.g., `Fwd:L:8080→80`). Forward lifecycle is tied to the session. Local forwarding works by binding a local TCP listener and proxying connections through SSH `channel_open_direct_tcpip` channels. Per-host port forwards can also be configured in `[[hosts]]` entries via `port_forwards` array with `direction`, `bind_host`, `bind_port`, `target_host`, `target_port` fields.

### 13.9 Background Activity Notifications ✅

**Implemented.** The existing cyan-underline new-output indicator is extended with regex-based notification matching. When a background session receives output matching a configurable pattern, a yellow `!` indicator appears next to the session tab. Patterns are configured globally via `session.notification_patterns` (array of regex strings, e.g. `["ERROR", "build complete", "OOM"]`). Notifications are automatically dismissed when switching to the affected session. The `notify` module provides `NotificationMatcher` with graceful handling of invalid regex patterns. TUI-only notifications (no desktop notification crate dependency).

### 13.10 Live Fleet Health Dashboard ✅

**Implemented.** The Fleet tab now runs periodic background TCP probes against all hosts, updating `●Online` / `●Offline` status in real-time. Each host shows colour-coded latency (green < 50 ms, yellow < 200 ms, red ≥ 200 ms) and a 16-column sparkline history. The summary bar shows fleet-wide availability percentage with a colour-coded gauge. Configurable via `[fleet]` in config: `probe_interval` (default 60 s), `probe_timeout` (default 5 s), `probe_enabled` (default true), `latency_history_samples` (default 30). Probes run concurrently via `tokio::spawn` to avoid blocking the event loop.

---

## 14. Open Questions

1. ~~Should host metrics collection use a dedicated SSH channel or multiplex over the shell channel?~~ **Resolved:** Uses dedicated SSH exec channels per metric collection cycle.
2. ~~Should the virtual terminal emulator support full alternate screen (`vim`, `htop` on remote)?~~ **Resolved:** Yes — `vt100::Parser` provides full alternate screen support.
3. ~~Should we support split-pane views (terminal + monitor side-by-side) in addition to the overlay toggle?~~ **Planned:** See §13.5.
4. Plugin system architecture — sandboxing vs. ecosystem reach tradeoff? *(deferred to M10)*
5. Should we support Windows or Linux/macOS only?
