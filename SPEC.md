# ESSH — Enhanced SSH Client

## 1. Overview

A terminal-based SSH client built for operations teams managing server fleets. ESSH combines enterprise connection management with real-time remote host diagnostics (CPU, memory, disk, network — like a built-in `htop`), concurrent multi-session support with seamless switching, and a Netwatch-inspired TUI aesthetic featuring performance histograms, sparklines, and color-coded health indicators.

The local ESSH application currently supports macOS and Linux builds only.

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
| `Ctrl+p` | Command palette (fuzzy finder for hosts, sessions, views) |
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
    │   └── mod.rs              # SSH connection, auth, shell channel, jump host (ProxyJump)
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
    ├── fleet/
    │   └── mod.rs              # Live fleet health — background TCP probes, latency tracking
    ├── recording/
    │   └── mod.rs              # Session recording (asciicast v2) & replay
    ├── filetransfer/
    │   └── mod.rs              # Two-pane file browser, upload/download via SSH exec
    ├── portfwd/
    │   └── mod.rs              # Port forwarding manager, local TCP proxy via direct-tcpip
    ├── notify/
    │   └── mod.rs              # Background activity notification matching (regex)
    ├── tui/
    │   ├── mod.rs              # App state, render dispatch, view management
    │   ├── dashboard.rs        # Dashboard view (sessions, hosts, fleet, config tabs)
    │   ├── session_view.rs     # Terminal rendering, tab bar, status bar, footer
    │   ├── host_monitor.rs     # Host monitor overlay (htop-style diagnostics)
    │   ├── filebrowser_view.rs # Two-pane file browser UI
    │   ├── portfwd_view.rs     # Port forwarding manager panel
    │   ├── help.rs             # Help overlay with keybinding reference
    │   ├── command_palette.rs  # Fuzzy-matched command palette overlay (Ctrl+P)
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

### 13.11 Command Palette ✅

**Implemented.** Press `Ctrl+p` from any view to open a fuzzy-matched command palette overlay. The palette provides instant access to hosts, active sessions, views, dashboard tabs, and actions. Type to filter entries — multi-word queries match independently with prefix and word-boundary bonuses for more intuitive ranking. Navigate with `↑`/`↓`, execute with `Enter`, dismiss with `Esc`. Available entry categories: **Hosts** (connect to a cached host), **Sessions** (switch to an active session), **Views** (Dashboard, Monitor, Port Forwarding, File Browser), **Dashboard Tabs** (Sessions, Hosts, Fleet, Config), **Actions** (Toggle Split Pane, Toggle Help). The palette renders as a centered overlay with Netwatch-style Cyan/Yellow/DarkGray theming, showing up to 12 matched results with category labels.

---

## 14. Open Questions

1. ~~Should host metrics collection use a dedicated SSH channel or multiplex over the shell channel?~~ **Resolved:** Uses dedicated SSH exec channels per metric collection cycle.
2. ~~Should the virtual terminal emulator support full alternate screen (`vim`, `htop` on remote)?~~ **Resolved:** Yes — `vt100::Parser` provides full alternate screen support.
3. ~~Should we support split-pane views (terminal + monitor side-by-side) in addition to the overlay toggle?~~ **Resolved:** Implemented in §13.5 — `Alt+s` toggles split-pane with adjustable width.
4. Plugin system architecture — sandboxing vs. ecosystem reach tradeoff? *(deferred to M10)*
5. ~~Should we support Windows or Linux/macOS only?~~ **Resolved:** ESSH currently supports local builds on macOS and Linux only.

---

# Addendum — collector correctness, honest empty states, divergence

Added on branch `feat/n0-collector-divergence`. This supersedes the parts of
the document above that describe the host monitor and the fleet view.

## 1. The monitor was reporting zeros (fixed)

§4.1 above promised macOS metrics via "`sysctl`/`vm_stat`". That was never
implemented. `HostMetricsCollector` issued `cat /proc/stat`, `/proc/meminfo`,
`/proc/loadavg`, `/proc/net/dev` and `/proc/uptime` at every host regardless of
platform. On a Mac every section came back empty, every parser returned its
`unwrap_or(0)` default, and the UI drew `CPU 0.0%`, `MEM 0 B / 0 B`,
`LOAD 0.00 0.00 0.00` and flat sparklines — beside a terminal printing the real
figures.

Two things were wrong: no macOS collectors, and **no way to distinguish a zero
reading from a missing one**.

### Three-state metrics

Every metric group is now `Pending`, `Collected`, `Uncollected { reason,
attempts }` or `Unsupported { reason }`, and `Default` is `Pending`. A freshly
constructed `HostMetrics` claims nothing even though every numeric field is 0.
Accessors return `Option`, so a caller cannot read a number that was never
collected without ignoring the type.

The UI renders a group's reason in words, and draws no bar and no sparkline:

```
CPU   uncollected · neither `iostat` nor `top` returned a CPU sample 4×
NET   waiting for first sample
```

Rates are `Pending` on the first sweep rather than `0 B/s`: there is no previous
counter to difference against, so there is genuinely no reading yet.

### macOS collectors

`uname -s` is probed once per session and cached. Per platform:

| Group | Linux | macOS |
|---|---|---|
| CPU | `/proc/stat` delta | `iostat -c 2 -w 1`, falling back to `top -l 2` |
| Memory | `/proc/meminfo` | `vm_stat` + `hw.memsize` + `vm.swapusage` |
| Load | `/proc/loadavg` | `sysctl -n vm.loadavg` |
| Uptime | `/proc/uptime` | `kern.boottime` + remote `date +%s` |
| Network | `/proc/net/dev` | `netstat -ib`, `<Link#N>` rows only |
| Disk | `df -Pk` | `df -Pk` |
| Processes | `ps aux --sort=-%cpu` | `ps axo … -r` |

Notes on the traps, each of which has a regression test:

- `df -P` alone leaves the block size implementation-defined and macOS answers
  in 512-byte blocks. The old code multiplied by 1024 unconditionally, so every
  macOS filesystem read at twice its real size. Pinned to `df -Pk`.
- `iostat` prints a since-boot row first and the interval row second; taking the
  wrong one is a permanent, silent overstatement.
- `vm_stat` has both `Pages stored in compressor` and `Pages occupied by
  compressor`. The first is larger and wrong for a footprint.
- `netstat -ib` prints one row per interface *per address family*, all carrying
  identical counters. Summing every row double-counts the machine.
- macOS reports an aggregate CPU only; per-core would need a Mach call
  unavailable over an exec channel. That is `Unsupported`, not zero.

### Testing

Golden fixtures captured from a running macOS host, plus two opt-in live tests
that run the real command set:

```sh
cargo test --bin essh macos_command_set                # local shell
ESSH_LIVE_SSH=host ESSH_LIVE_KEY=~/.ssh/id_ed25519 \
  cargo test --bin essh live_ssh -- --ignored          # real SSH
```

The live test asserts non-zero, in-range values and fails if any group is
uncollected. Its absence is why the original bug shipped: every prior test fed
the parsers hand-written Linux strings.

## 2. Display honesty

- Timestamps are relative (`2d ago`), never
  `2026-03-02T09:40:05.401277+00:00` in a list view.
- Paths truncate from the **left** — the tail identifies. `region=us-east-1`
  no longer becomes `region=us-ea`.
- Tags render as chips, whole or not at all, with `+N` for the remainder.
  Clipping mid-value produced `env=`, which reads as an empty value rather than
  a hidden one.
- Filesystems are filtered to user data: `/System/Volumes/*` except `Data`,
  no App Translocation mounts, nothing under 1 GiB unless over 90% full,
  ordered fullest first, with a count of what was hidden.
- `HostStatus::Unknown` is now `NeverProbed` and renders as `never probed`.
  `○ Unknown` read as a failed check rather than an absent one.
- No dashes in laid-out columns. An unreachable host shows `no reply`; an
  unprobed one shows `never probed`.

## 3. Terminal fidelity

v1 bound twelve `Alt` combinations. `Alt+f` and `Alt+b` are readline
word-motion, `Alt+d` is kill-word, `Alt+.` is yank-last-argument — all
swallowed, and `key_to_bytes` dropped Alt keys entirely rather than sending
them on.

Now: while a shell has focus ESSH claims exactly one key, a configurable
prefix defaulting to `Ctrl+A`, and forwards everything else. Alt is transmitted
as the ESC prefix the far end expects. Pressing the prefix twice sends the
literal key. Outside a session there is no shell to steal from, so the direct
`Alt` bindings remain.

## 4. Divergence

The headline feature: *how does this host differ from its peers?*

Peer sets are derived from tags; a tag held by fewer than two hosts defines no
comparison and is not a peer set. Facts are collected over batched SSH exec
channels every 60s.

**Severity is derived.** Categorical facets score
`1 - (hosts sharing my value / hosts with a value)`. Numeric facets score by
normalised distance from the peer median.

**Flagging is separate from ranking.** A numeric facet is an outlier only
outside the Tukey fence (`Q1 - 1.5·IQR`, `Q3 + 1.5·IQR`). Without this, a fleet
whose disks run evenly from 40% to 79% reports all forty hosts as diverging —
true, and useless.

**Unprobed is not diverging.** Hosts with no facts are excluded from every
denominator and reported separately.

**A missing fact is a stated fact.** Facets declare a platform and a privilege.
`systemd units` is Linux-only and reports `unsupported` on macOS rather than as
a difference. Config-file hashes distinguish `not installed` from
`permission denied`. The UI reports facets *collectable on this platform*, and
how many need privileges, so the count shown is what was attempted.

**Verdicts are enumerated templates over co-occurrence.** They name what was
observed and cite the facets behind it; they never assert a cause the data
cannot support. A host at consensus gets no verdict, and an unprobed host gets
none either — silence is a correct output.

Facets: kernel · os release · cpu model · cpu count · mem total · openssl ·
timezone · ntp sync · ssh host key algo · disk / · uptime · load per core ·
listening ports · systemd units (Linux) · plus one per configured config-file
path and package.

Live end-to-end test:

```sh
ESSH_LIVE_SSH=host ESSH_LIVE_KEY=~/.ssh/id_ed25519 \
  cargo test --bin essh live_divergence -- --ignored --nocapture
```

## 5. Not done

- **TLS certificate expiry** is listed as a facet in the design but has no
  collector, because it needs to know which certificate. The enum variant was
  removed rather than left as a claim without an implementation.
- **Privilege escalation.** Facets declare `Privilege::Root` and the count is
  surfaced, but ESSH never attempts sudo. Unreadable files report why.
- **Peer-set inference from facts** rather than tags. Tags are user-maintained
  and often are not; inferring peers from the facts themselves is circular with
  collection and needs design.
- **The GPU renderer.** The design proposed replacing ratatui + crossterm with
  wgpu and an own VT core. Everything above ships on the existing stack; that
  decision is deferred until divergence has earned it.

---

# Addendum 2 — building out the ESSH 2.0 spec

Built on `feat/n0-collector-divergence`. This covers §2 (launcher), §3
(session splits), §4 (workspaces), §5 (connection failure diagnosis), §6
(ssh_config compatibility) and §9 (benchmarks).

## §6 — ssh_config, for real

`src/sshconfig/` parses the user's actual config: `Include` (with globs, a
recursion guard and reported failures), `Host` patterns with wildcards and
negation, `Match host/user/all/final`, `Key=value` form, and OpenSSH's
first-value-wins precedence. Percent tokens (`%h %p %r %u %n %d %l %L`)
expand where they matter — `ProxyCommand` and `ControlPath` are unusable
without it.

**`Match exec` is never evaluated.** Listing hosts must not run arbitrary
commands out of a config file.

**The compatibility claim is a table, not a sentiment.** Every directive is
Full, ViaSystemSsh or Unsupported, and the resolution carries the ones it did
not honour so the UI can say so:

```
$ essh config ssh
Directives ESSH does not honour natively:
  proxycommand                 handled by delegating to the system ssh
  proxyusefdpass               parsed, but not acted on
```

`essh config resolve <host>` is shaped like `ssh -G` on purpose: the two can
be diffed. There is a differential test that does exactly that against every
alias in the user's real config:

```sh
cargo test --bin essh differential -- --ignored --nocapture
```

Verified byte-identical to `ssh -G` on hostname, port, user, identityfile and
expanded proxycommand.

## §2 — the launcher

`essh` with no arguments opens a search over every host ESSH knows: aliases
from `~/.ssh/config` (no import step — §2's "no separate host-management
system" is now literally true), ESSH's own config, and the cache.

The matcher is the product, so its behaviour is specified by tests rather
than by feel: subsequence matching (`pdb` → `prod-db`), word-start bonuses, a
contiguous prefix beating scattered initials, and recency that breaks ties
but never outranks a better textual match. Results are stable across
identical searches, because a list that reshuffles between keystrokes makes
Enter dangerous.

Hosts reached via `ProxyCommand`/`ControlMaster` are marked *via system ssh*
in the list, so that is known before connecting rather than mid-incident.

## §5 — why a host will not connect

`essh why <host>` runs a probe ladder and stops at the first failure:

```
Could not connect to prod-db

Config    ✓  prod-db → 10.0.0.5:22
Bastion   ✓  bastion → 127.0.0.1:2201 in 0ms
DNS       ✓  10.0.0.5 (literal address)
TCP:22    ✗  timed out after 2s
SSH          not probed
```

Four states, not two: `Ok`, `Failed`, `Skipped` (no bastion configured) and
`NotProbed` — which renders blank, never as a tick. A ladder that shows ✓ for
something it never tested is worse than no ladder.

The `SSH` rung reads the server's identification string, which separates "the
port is open" from "sshd is listening" — the port-forward-pointing-at-the-
wrong-service case.

`ProxyJump` targets are resolved through the config, because they are usually
aliases; probing the literal string produced a DNS failure for a name that
was never meant to be resolved.

Authentication failures have a taxonomy, because the three publickey cases
share one OpenSSH message and need three different fixes: *no key offered*,
*key rejected*, and *algorithm refused* — the `ssh-rsa` deprecation, where
the key is fine and the server will not accept its signature algorithm.
Anything unrecognised is carried verbatim rather than forced into a bucket.

## §4 — workspaces

`essh workspace save|list|show|open|remove`. A subcommand rather than a bare
argument, so a host genuinely named `workspace` stays reachable.

Each session may carry an `on_connect` command. `tmux new -A -s essh` is what
makes restore restore *work* rather than four fresh shells in `$HOME` — using
the tool §4 already says to use for persistence, rather than reimplementing
it. Saving without one prints that caveat.

**Restore is partial by default and reports honestly:** "restored 2 of 3
sessions in production — prod-db did not connect", with each failure carrying
its diagnosis from the ladder above.

Restore happens one host per tick with a bounded pre-probe. Doing it in a
single pass froze the event loop — nothing rendered, no key was read, and one
unreachable host hung the UI until its TCP timeout.

## §3 — session splits

`src/panes/` is a layout tree producing the §3 diagram: four live sessions,
two over two. `^A s` splits vertically, `^A S` horizontally, `^A o` moves
focus. Each pane's terminal is resized to its own rectangle, so the remote's
idea of the window matches what is drawn.

Two invariants under test: closing a pane promotes its sibling rather than
leaving an empty region, and session indices are renumbered when a session is
removed — without that, a pane renders a different session's terminal, which
looks exactly like data corruption.

v1's "split" showed one terminal beside the *monitor*; that still exists on
`^A M`.

## §9 — benchmarks

`essh bench` measures things that can fail, which the spec's own targets
mostly cannot:

```
  ssh_config parse           0.186 ms   12922 bytes                    ✓ (target 10ms)
  launcher search            0.595 ms   500 hosts, per keystroke       ✓ (target 16ms)
  VT parse throughput      124.452 MiB/s coloured log output
  divergence consensus       0.619 ms   40 hosts × 4 facets            ✓ (target 50ms)
```

It also prints what it does *not* measure — added keystroke latency versus
plain `ssh`, sustained throughput over a live channel, memory at 30 sessions
— because those need a real host and a local number for them would be
precise and meaningless.

## Bugs found while building this

Each has a regression test.

- **The TUI silently re-trusted changed host keys.** `HostKeyStatus::Changed`
  called `trust_host` with a comment reading "auto-accept in TUI mode for
  now". A changed host key is the signal host key verification exists to
  raise. It now refuses the connection and says why.
- **Deleting a host had no confirmation.** One keystroke rewrote the config
  and dropped the cached key, with no undo. It ate a host during development.
- **`Ctrl+A` in the dashboard opened the Add Host dialog**, because the
  command prefix is only intercepted while a shell has focus and the `a` arm
  had no modifier guard.
- **Status messages never rendered.** The footer was a fixed three rows, so
  `set_status` output was laid out past the bottom edge — every error and
  result message in the app.
- **Divergence facets collected only if you visited the monitor first**,
  because they reused the platform the metrics collector detected and metrics
  only sweep while the monitor is open.
- **A newly connected host waited up to 60s** before showing any facts.
- **Container roots vanished** from the monitor: `parse_df` skipped `overlay`
  as a pseudo-filesystem, leaving a bind-mounted `/etc/hosts` as the only
  "disk".
- **ESSH reported itself** in its own process list.
- **The workspace restore recorded failed sessions as connected**, because
  `open_session` reports failure by setting a status and still returning
  `Ok`.

## Still not built

- **The wgpu renderer** (handoff N1–N4). Everything above runs on ratatui.
- **`ControlMaster`/`ProxyCommand` transports.** They are parsed, reported
  and marked *via system ssh*, but ESSH does not yet shell out to `ssh` for
  them — so those hosts are identified rather than reached.
- **Layout persistence in workspaces.** `Layout` round-trips through the file
  but restore currently opens sessions as tabs.
- **`essh` connecting by bare alias** (`essh prod-db`) still goes through
  `essh connect prod-db`.


---

# Addendum 3 — implementing the design handoff

The first two addenda built *product* scope out of the ESSH 2.0 spec. They did
not implement `source/ESSH 2.0.html`, which is the design. This one does.

The gap was not cosmetic. The handoff's central rule is a **chrome rule**, and
v1 — and my first pass — violated it directly.

## The chrome rule

> Dashboards are dashboards. Hosts and Fleet keep a tab strip, because that is
> where you navigate. **The shell gets nothing** — no tab strip, no status
> row, no borders, no per-pane titles, even in split.

The session view now renders the terminal and nothing else. It previously
spent four rows on a tab bar, a status line and a keybind footer before a
single line of remote output. Splits lost their per-pane borders and titles
too; a focused pane is marked by a one-column bar, which costs no row.

Instrumentation over a shell is now transient or on-demand only:

* **`src/tui/hud.rs`** — appears on a *change*, states why in words, carries
  the numbers it actually has, and fades after ~4s. It is drawn over the
  terminal's last row rather than being given a row, so the shell never
  reflows when it comes and goes. Raised by divergence changes, reconnects
  and workspace restores; `Esc` dismisses it.
* Overlays (divergence, palette) are floating cards on a dimmed scrim, not
  bordered boxes — *"no box-drawing needed when you can composite."*

## The design system

`src/design/mod.rs` holds every value from the handoff, so screens read from
one place rather than respelling colours:

```
bg #0c1418   fg #c8d4d9   dim #6d8189   faint #42555d   rule #1c282e
green #5cd989  cyan #5fdcff  amber #f0c060  red #ff7878  violet #b8a8e8
RAMP_DIV  #2c3a42 → #4a5a60 → #8a8f6a → #f0c060 → #ff7878
```

**Magnitude ramps run cool → bright: high means busy, not bad.** v1's
`pct_color` — `<50% green / <80% yellow / >80% red` — is deleted, not
rewired. Only genuinely bounded-bad values (disk fullness, cert expiry) keep
green → amber → red, and only on meters.

**`RAMP_DIV` measures agreement, not magnitude.** Consensus is faint; red
means *you are alone*.

Also transcribed: the `.box` idiom (title on the rule, annotation opposite —
no rows wasted), faint tracked column headers with a cyan `DIVERGE`,
right-aligned tabular numerics, the current row tinted with a cyan inset bar,
tags as chips, `⏎ connect` footers with cyan keys, and italic-faint empty
states.

## What is approximated, and why

The design was drawn for a GPU renderer. Two things do not translate, and are
marked rather than faked:

* **Vector sparklines become braille.** The handoff's own graph-primitives
  section carries the braille invariants forward "where braille is used".
  Levels are absolute with a derived ceiling, colour is by value on one-row
  graphs, and an empty series draws nothing rather than a flat line.
* **The 3px peer ribbon is not drawn.** A terminal has no sub-cell row, and
  spending a whole row on it would violate the chrome rule it exists to
  satisfy.

## Screens

| # | Screen | State |
|---|--------|-------|
| 1 | Hosts | Rebuilt: box idiom, chips, cyan DIVERGE, selection bar, GROUPS panel, boxes sized to content |
| 2 | Fleet | Rebuilt: facet consensus headline, per-facet agreement meters, hosts-by-divergence, agreement collapsed to one line, reachability demoted |
| 3 | Session terminal | Rebuilt: zero chrome + HUD |
| 4 | Host Monitor | Rebuilt: three headline boxes carrying peer medians, DISK beside VS PEERS, PROCESSES, footer |
| 5 | Split | Rebuilt: shell keeps zero chrome, narrow pane drops to essentials, single hairline divider |
| 6 | Command palette | Rebuilt: scrim + floating card; **emoji replaced with single-width glyphs** — they are double-width in a terminal and shear the grid |

## Still not matching

- **Fonts.** The handoff specifies JetBrains Mono / Berkeley Mono / SF Mono
  and real CSS-grade typography — 9.5px tracked labels, 19–26px headline
  numerals. A terminal has one cell size, so hierarchy is carried by colour
  and weight alone. Headline numerals are bold white; they are not larger.
- **Per-facet meters in the Fleet consensus box** are a single column, where
  the design has a labelled list of six.
- **Sessions and Config tabs** are still v1. The handoff deliberately did not
  redesign them ("no v1 screenshots were provided, and inventing them would
  repeat the mistake this revision corrects"), so neither did I.
