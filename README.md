<p align="center">

```
                         ███████╗███████╗███████╗██╗  ██╗
                         ██╔════╝██╔════╝██╔════╝██║  ██║
                         █████╗  ███████╗███████╗███████║
                         ██╔══╝  ╚════██║╚════██║██╔══██║
                         ███████╗███████║███████║██║  ██║
                         ╚══════╝╚══════╝╚══════╝╚═╝  ╚═╝
                           Enhanced SSH for people with fleets
```

</p>

<p align="center">
  <a href="https://crates.io/crates/essh"><img src="https://img.shields.io/crates/v/essh.svg" alt="crates.io"></a>
  <a href="https://github.com/matthart1983/essh/actions/workflows/ci.yml"><img src="https://github.com/matthart1983/essh/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/matthart1983/essh/blob/main/LICENSE"><img src="https://img.shields.io/crates/l/essh.svg" alt="License: MIT"></a>
  <img src="https://img.shields.io/badge/macOS-supported-111111?logo=apple&logoColor=white" alt="macOS">
  <img src="https://img.shields.io/badge/Linux-supported-FCC624?logo=linux&logoColor=black" alt="Linux">
</p>

<p align="center">
  A pure-Rust SSH client with a dense, Netwatch-style TUI. Multiple sessions,
  live host metrics, and fleet-wide divergence — without leaving the terminal.
</p>

![ESSH Demo](https://raw.githubusercontent.com/matthart1983/essh/main/docs/media/essh-demo.gif)

## Install

```bash
cargo install essh
```

Or grab a binary for macOS or Linux, arm64 or x86_64, from
[Releases](https://github.com/matthart1983/essh/releases/latest).

From source:

```bash
git clone https://github.com/matthart1983/essh && cd essh
cargo build --release        # ./target/release/essh
```

## Start

```bash
essh                          # launcher: type to search, Enter to connect
essh connect deploy@web-01    # straight to a host
essh workspace open prod      # restore a saved set of sessions
essh run web -- uptime        # fan a command across a tagged group
```

Hosts come from `~/.ssh/config` — including `IdentityFile`, `ProxyJump` and
`Match` — so there is nothing to import before you start.

## Keys

Everything common is one keypress. The shell keeps every `Ctrl` combination,
so `Ctrl+D` still means EOF and `Ctrl+C` still interrupts.

| | | | |
|---|---|---|---|
| `F1` help | `F2` monitor | `F3` files | `F4` port forwards |
| `F5` mini monitor | `F6` detach | `F7`/`F8` prev/next session | `F9` new session |
| `F10` command menu | | | |

**On macOS**, that row is brightness and volume out of the box — `F10` mutes.
Either hold `fn` with them, or turn on *System Settings → Keyboard → Use F1, F2,
etc. as standard function keys* once. The prefix route below needs neither and
works everywhere.

For anything else, press `Ctrl+A`, let go, then the key — `s` split, `w` close,
`t` theme, `1`–`9` jump to session, `[`/`]` resize. The strip along the bottom
of a session lists the keys that apply right now, so there is nothing to
memorise. Change the prefix with `prefix_key` under `[session]`.

`Option`/`Alt` combinations work on the dashboard and the other screens, but
**not while a shell has focus** — there essh claims exactly one key, the prefix,
and forwards everything else, so `Alt+f` stays readline's forward-word and
`Alt+.` stays yank-last-argument. In a session it is function keys or the
prefix.

## What it does

| | |
|---|---|
| **Multiple sessions** | Tabs and splits in one window, with per-session scrollback and reconnect. |
| **Live host monitor** | CPU, memory, disk, network and top processes, sampled in the background so the view is warm when you open it. No agent — plain SSH exec channels. |
| **Divergence** | Compares each host against its peer set and tells you *which* host differs, on *which* facet, and why. |
| **File transfer** | Two-pane local/remote browser over SFTP. |
| **Port forwarding** | Add, inspect and remove forwards without dropping the session. |
| **Audit and replay** | Structured JSON audit log; sessions record to asciicast for replay. |

### Divergence

Forty web servers, thirty-nine fine, and one with a different kernel or a
hand-edited `nginx.conf`. A column of green dots structurally cannot show you
that.

ESSH groups hosts into peer sets by tag, collects the same facts from each,
and scores every host against the group's consensus:

```
100.0% of 16 facet-checks agree
0 facets diverge across 0 hosts.
2 hosts have never been probed, so their facets are unknown rather than in agreement.
```

The Fleet screen names the outlier and the facets behind the claim, so the
verdict is checkable rather than asserted. Facts it cannot collect are
reported as uncollected — never as agreement.

## Themes

Eight, cycled with `t` (and `Alt+t` inside a session, where `t` belongs to the
shell), persisted to the config:

**dark** (default) · **terminal** · **light** · **solarized** · **dracula** · **nord** · **ocean** · **sky**

`terminal` pins no colours of its own — every slot resolves to an ANSI entry and
foreground and background use your terminal's own, so pywal, matugen or a
terminal profile carries straight through. `sky` and `light` paint their own
ground; `ocean` uses your terminal's with a fixed palette on top.

Or jump straight to one from the command menu — `F10`, or `Ctrl+P` outside a
session — and type its name.

```bash
essh --theme terminal        # also accepts: system, ansi
essh --theme dracula hosts list
```

The flag applies for one run and is not written back, so it's a way to try one
without committing. `t` in the TUI changes it and saves.

## Configuration

State lives in `~/.essh/`: `config.toml`, `cache.db`, `audit.log`,
`sessions/`, `recordings/`, `known_cas/`.

```bash
essh config init      # write a default config
essh config edit      # open it in $EDITOR
essh config resolve web-01   # show how ssh_config resolves a host
```

Host keys are verified and cached, with a configurable TOFU policy
(`strict`, `prompt`, `auto`). Ciphers and KEX algorithms can be restricted.

Full configuration and architecture reference: [SPEC.md](SPEC.md).

## CLI

```bash
essh hosts list | add | remove | import | health
essh keys list | add | remove
essh workspace list | open | save | show | remove
essh session list | replay <id>
essh diag <session-id>        # diagnostics for a past session
essh why <host>               # explain why a host will not connect
essh audit tail --lines 20
essh bench                    # published performance numbers
```

## Development

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
```

CI runs all three on every push.

`tests/tui_harness.rs` drives the real binary on a PTY and asserts on a parsed
terminal screen with a deadline, because the failures that matter in a TUI are
the absence of a frame rather than a wrong one. The tests that need SSH skip
themselves when no host is reachable.

## Contributing

Fork, branch, make the change, run the three checks above, open a PR.

## License

MIT. See [LICENSE](LICENSE).
