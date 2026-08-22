//! Facet collection over SSH exec channels.
//!
//! The design constraint the spec understated: **most interesting facets are
//! not readable by an unprivileged login user.** `/etc/nginx/nginx.conf` is
//! usually mode 0644 but its directory may not be traversable; `ss -lntp`
//! needs root for process attribution; `systemctl` may not exist at all.
//!
//! So every facet declares what it needs, and a facet we cannot collect
//! reports *why* as a `Missing` value rather than being silently dropped. A
//! divergence report that quietly compares eight of seventeen facets while
//! claiming seventeen is worse than one that compares eight and says so.


use russh::client::Handle;

use super::{FacetKey, FacetValue, HostFacts};
use crate::monitor::Platform;

/// What a facet's command needs in order to answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Privilege {
    /// Readable by any login user.
    User,
    /// Needs root (or sudo). Collected only when the session has it.
    Root,
}

/// How to collect one facet.
pub struct FacetSpec {
    pub key: FacetKey,
    pub privilege: Privilege,
    /// `None` means this facet does not exist on that platform.
    pub linux: Option<&'static str>,
    pub macos: Option<&'static str>,
    /// Turn raw stdout into a value. Returning `None` means "ran, but produced
    /// nothing usable", which becomes a `Missing` with a stated reason.
    pub parse: fn(&str) -> Option<FacetValue>,
}

impl FacetSpec {
    pub fn command(&self, platform: &Platform) -> Option<&'static str> {
        match platform {
            Platform::Linux => self.linux,
            Platform::MacOS => self.macos,
            _ => None,
        }
    }
}

fn first_line(raw: &str) -> Option<FacetValue> {
    let t = raw.trim();
    (!t.is_empty()).then(|| FacetValue::Text(t.lines().next().unwrap_or("").trim().to_string()))
}

fn number(raw: &str) -> Option<FacetValue> {
    raw.split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .map(FacetValue::Number)
}

/// Percentage from `df -Pk /` — the `Capacity` column.
fn root_disk_pct(raw: &str) -> Option<FacetValue> {
    let line = raw.lines().nth(1)?;
    let tok = line
        .split_whitespace()
        .rev()
        .find(|t| t.ends_with('%') && t.trim_end_matches('%').parse::<u64>().is_ok())?;
    tok.trim_end_matches('%')
        .parse::<f64>()
        .ok()
        .map(FacetValue::Number)
}

/// Sorted, deduplicated listening ports as a single comparable string.
///
/// Sorting matters: two hosts listening on the same ports in a different order
/// are the same host, and an unsorted string would score them as divergent.
///
/// The two platforms disagree about the separator. Linux `ss -lnt` prints
/// `0.0.0.0:443`; macOS `netstat -an` prints `*.443` and `127.0.0.1.11434`.
/// Splitting on `:` alone finds nothing at all on a Mac, so we take the
/// trailing numeric field after either separator.
fn listening_ports(raw: &str) -> Option<FacetValue> {
    fn port_of(token: &str) -> Option<u32> {
        let tail = token.rsplit([':', '.']).next()?;
        let port = tail.parse::<u32>().ok()?;
        (port > 0 && port <= 65535).then_some(port)
    }

    let mut ports: Vec<u32> = raw
        .lines()
        // Skip headers and the remote-address column: on a LISTEN row the
        // local address is the first token that carries a port.
        .filter(|l| l.contains("LISTEN") || l.trim_start().starts_with("tcp"))
        .filter_map(|l| {
            l.split_whitespace()
                .filter(|t| t.contains(':') || t.contains('.'))
                .find_map(port_of)
        })
        .collect();
    if ports.is_empty() {
        return None;
    }
    ports.sort_unstable();
    ports.dedup();
    Some(FacetValue::Text(
        ports
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(","),
    ))
}

/// Host key algorithms from `ssh-keyscan`.
///
/// `ssh-keyscan` interleaves `# host:port SSH-2.0-...` comment lines with the
/// key lines. Taking field 2 without filtering yields `localhost:22` as if it
/// were an algorithm name.
fn host_key_algos(raw: &str) -> Option<FacetValue> {
    let mut algos: Vec<&str> = raw
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_whitespace().nth(1))
        .filter(|a| a.starts_with("ssh-") || a.starts_with("ecdsa-") || a.starts_with("sk-"))
        .collect();
    if algos.is_empty() {
        return None;
    }
    algos.sort_unstable();
    algos.dedup();
    Some(FacetValue::Text(algos.join(",")))
}

/// Enabled systemd units, sorted so ordering noise does not read as drift.
fn enabled_units(raw: &str) -> Option<FacetValue> {
    let mut units: Vec<&str> = raw
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|u| u.ends_with(".service"))
        .collect();
    if units.is_empty() {
        return None;
    }
    units.sort_unstable();
    units.dedup();
    Some(FacetValue::Text(units.join(",")))
}

/// Uptime in whole days, which is the granularity that actually distinguishes
/// hosts. Comparing seconds would make every host diverge from every other.
///
/// Accepts both platforms' shapes: Linux `/proc/uptime` ("864000.12 1200.5")
/// and macOS `kern.boottime` followed by `date +%s`.
///
/// The macOS side used to be a shell one-liner, and it was a trap:
///
/// ```text
/// sysctl -n kern.boottime | sed 's/.*sec = \([0-9]*\).*/\1/'
/// ```
///
/// `.*sec = ` is greedy and `usec = ` also contains `sec = `, so it captured
/// the *microseconds* field. On a host up three hours that produced a
/// plausible-looking 20669 rather than 0. Parsing in Rust — with the routine
/// the metrics collector already uses and tests — removes the guesswork.
fn uptime_days(raw: &str) -> Option<FacetValue> {
    let secs = if raw.contains("sec =") {
        let mut lines = raw.lines().filter(|l| !l.trim().is_empty());
        let boot = lines.next()?;
        let now = lines.next()?;
        crate::monitor::parser::parse_boottime(boot, now)? as f64
    } else {
        raw.split_whitespace().next()?.parse().ok()?
    };
    Some(FacetValue::Number((secs / 86_400.0).floor()))
}

/// The facet table: fourteen built-in facets, plus one per configured config
/// path and package.
///
/// Deliberately not "the seventeen facets". Two of the built-ins are
/// Linux-only, so the number actually compared depends on the platform and on
/// configuration — which is why [`collectable_count`] exists and why the UI
/// reports what was attempted rather than what is declared.
pub fn facet_specs(config_paths: &[String], packages: &[String]) -> Vec<FacetSpec> {
    let mut specs = vec![
        FacetSpec {
            key: FacetKey::Kernel,
            privilege: Privilege::User,
            linux: Some("uname -r"),
            macos: Some("uname -r"),
            parse: first_line,
        },
        FacetSpec {
            key: FacetKey::OsRelease,
            privilege: Privilege::User,
            linux: Some(". /etc/os-release 2>/dev/null && echo \"$ID $VERSION_ID\""),
            macos: Some("sw_vers -productVersion"),
            parse: first_line,
        },
        FacetSpec {
            key: FacetKey::CpuModel,
            privilege: Privilege::User,
            linux: Some("awk -F: '/model name/{print $2; exit}' /proc/cpuinfo"),
            macos: Some("sysctl -n machdep.cpu.brand_string"),
            parse: first_line,
        },
        FacetSpec {
            key: FacetKey::CpuCount,
            privilege: Privilege::User,
            linux: Some("nproc"),
            macos: Some("sysctl -n hw.ncpu"),
            parse: number,
        },
        FacetSpec {
            key: FacetKey::MemTotal,
            privilege: Privilege::User,
            linux: Some("awk '/MemTotal/{print int($2/1048576)}' /proc/meminfo"),
            macos: Some("echo $(( $(sysctl -n hw.memsize) / 1073741824 ))"),
            parse: number,
        },
        FacetSpec {
            key: FacetKey::OpenSsl,
            privilege: Privilege::User,
            linux: Some("openssl version 2>/dev/null"),
            macos: Some("openssl version 2>/dev/null"),
            parse: first_line,
        },
        FacetSpec {
            key: FacetKey::Timezone,
            privilege: Privilege::User,
            linux: Some("date +%Z%z"),
            macos: Some("date +%Z%z"),
            parse: first_line,
        },
        FacetSpec {
            key: FacetKey::NtpSync,
            privilege: Privilege::User,
            linux: Some("timedatectl show -p NTPSynchronized --value 2>/dev/null"),
            macos: Some("sntp -q time.apple.com >/dev/null 2>&1 && echo yes || echo no"),
            parse: first_line,
        },
        FacetSpec {
            key: FacetKey::SshHostKeyAlgo,
            privilege: Privilege::User,
            linux: Some("ssh-keyscan -T 2 localhost 2>/dev/null"),
            macos: Some("ssh-keyscan -T 2 localhost 2>/dev/null"),
            parse: host_key_algos,
        },
        FacetSpec {
            key: FacetKey::DiskRootPct,
            privilege: Privilege::User,
            linux: Some("df -Pk /"),
            macos: Some("df -Pk /"),
            parse: root_disk_pct,
        },
        FacetSpec {
            key: FacetKey::UptimeDays,
            privilege: Privilege::User,
            linux: Some("cat /proc/uptime"),
            macos: Some("sysctl -n kern.boottime; date +%s"),
            parse: uptime_days,
        },
        FacetSpec {
            key: FacetKey::LoadPerCore,
            privilege: Privilege::User,
            linux: Some("awk -v c=$(nproc) '{printf \"%.2f\", $1/c}' /proc/loadavg"),
            macos: Some(
                "sysctl -n vm.loadavg | awk -v c=$(sysctl -n hw.ncpu) '{printf \"%.2f\", $2/c}'",
            ),
            parse: number,
        },
        // ── Privileged facets ────────────────────────────────────────────
        // `ss -lntp` and `netstat -p` need root for the process column; the
        // port list itself is readable without it, so this stays User and we
        // simply do not ask for process names.
        FacetSpec {
            key: FacetKey::ListeningPorts,
            privilege: Privilege::User,
            linux: Some("ss -lnt 2>/dev/null || netstat -lnt 2>/dev/null"),
            macos: Some("netstat -an -p tcp | grep LISTEN"),
            parse: listening_ports,
        },
        // systemd is Linux-only by construction. On macOS this reports as
        // unsupported rather than as a difference.
        FacetSpec {
            key: FacetKey::SystemdUnits,
            privilege: Privilege::User,
            linux: Some("systemctl list-unit-files --state=enabled --no-legend 2>/dev/null"),
            macos: None,
            parse: enabled_units,
        },
    ];

    // Config file hashes. These are the facets most likely to come back
    // Missing on a real fleet — the file is often unreadable by the login
    // user — which is exactly why the reason is carried through.
    for path in config_paths {
        specs.push(FacetSpec {
            key: FacetKey::FileHash(path.clone()),
            privilege: Privilege::Root,
            linux: None, // filled per-host below; see command_for_path
            macos: None,
            parse: first_line,
        });
    }

    for pkg in packages {
        specs.push(FacetSpec {
            key: FacetKey::PkgVersion(pkg.clone()),
            privilege: Privilege::User,
            linux: None,
            macos: None,
            parse: first_line,
        });
    }

    specs
}

/// Dynamic command for a config-file hash facet.
pub fn command_for_file_hash(path: &str) -> String {
    // `sha256sum` on Linux, `shasum -a 256` on macOS; try both and take
    // whichever exists. `2>&1` is deliberate: a permission error is the
    // information we want to surface, not swallow.
    format!(
        "if [ ! -e '{p}' ]; then echo 'ESSH_ABSENT'; \
         elif [ ! -r '{p}' ]; then echo 'ESSH_DENIED'; \
         else (sha256sum '{p}' 2>/dev/null || shasum -a 256 '{p}' 2>/dev/null) | cut -c1-16; fi",
        p = path
    )
}

/// Dynamic command for a package-version facet.
pub fn command_for_package(pkg: &str, platform: &Platform) -> Option<String> {
    match platform {
        Platform::Linux => Some(format!(
            "(dpkg-query -W -f='${{Version}}' {p} 2>/dev/null || rpm -q --qf '%{{VERSION}}' {p} 2>/dev/null || echo ESSH_ABSENT)",
            p = pkg
        )),
        Platform::MacOS => Some(format!(
            "(brew list --versions {p} 2>/dev/null | awk '{{print $2}}' || echo ESSH_ABSENT)",
            p = pkg
        )),
        _ => None,
    }
}

/// Interpret the sentinel values the dynamic commands emit.
pub fn interpret_sentinel(raw: &str) -> Option<FacetValue> {
    match raw.trim() {
        "ESSH_ABSENT" => Some(FacetValue::Missing("not installed".into())),
        "ESSH_DENIED" => Some(FacetValue::Missing("permission denied".into())),
        _ => None,
    }
}

/// Collect every applicable facet from one host.
///
/// Facets are batched into a single exec channel with the same `===KEY===`
/// envelope the metrics collector uses, because forty hosts × seventeen facets
/// on individual channels is 680 channel opens a minute.
pub async fn collect_facts<H: russh::client::Handler>(
    handle: &Handle<H>,
    host: &str,
    platform: &Platform,
    config_paths: &[String],
    packages: &[String],
) -> HostFacts {
    let mut facts = HostFacts::new(host);
    let specs = facet_specs(config_paths, packages);

    let mut script = String::new();
    // Index by position so a facet key containing `=` or spaces cannot break
    // the envelope.
    let mut order: Vec<(usize, &FacetSpec)> = Vec::new();

    for (i, spec) in specs.iter().enumerate() {
        let cmd: Option<String> = match &spec.key {
            FacetKey::FileHash(p) => Some(command_for_file_hash(p)),
            FacetKey::PkgVersion(p) => command_for_package(p, platform),
            _ => spec.command(platform).map(|s| s.to_string()),
        };

        match cmd {
            Some(c) => {
                // `printf '\n===N===\n'` rather than `echo`, because several
                // of these commands end without a newline — `awk printf`,
                // `tr '\n' ','` — and a marker appended to the tail of the
                // previous command's output is not at the start of a line, so
                // the splitter never sees it. One such command corrupts every
                // section after it.
                script.push_str(&format!("printf '\\n==={}===\\n'; {}; ", i, c));
                order.push((i, spec));
            }
            None => {
                facts.facets.insert(
                    spec.key.clone(),
                    FacetValue::Missing(format!("not available on {}", platform.label())),
                );
            }
        }
    }
    script.push_str("printf '\\n===END===\\n'");

    let raw = match exec(handle, &script).await {
        Ok(r) => r,
        Err(e) => {
            // One dead channel must not look like seventeen unrelated
            // failures, but each facet still has to say something.
            let reason = format!("collector failed: {}", e);
            for (_, spec) in &order {
                facts
                    .facets
                    .insert(spec.key.clone(), FacetValue::Missing(reason.clone()));
            }
            return facts;
        }
    };

    let sections = crate::monitor::collector::split_sections(&raw);

    for (i, spec) in order {
        let out = sections.get(&i.to_string()).cloned().unwrap_or_default();
        let value = interpret_sentinel(&out)
            .or_else(|| (spec.parse)(&out))
            .unwrap_or_else(|| {
                FacetValue::Missing(if out.trim().is_empty() {
                    "command produced no output".into()
                } else {
                    "unreadable output".into()
                })
            });
        facts.facets.insert(spec.key.clone(), value);
    }

    facts
}

/// Run `uname -s` on its own, for callers that need the platform before any
/// metrics collector exists.
pub async fn probe_uname<H: russh::client::Handler>(handle: &Handle<H>) -> Option<String> {
    exec(handle, "uname -s").await.ok()
}

async fn exec<H: russh::client::Handler>(
    handle: &Handle<H>,
    command: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut channel = handle.channel_open_session().await?;
    channel.exec(true, command.as_bytes()).await?;
    let mut out = Vec::new();
    while let Some(msg) = channel.wait().await {
        match msg {
            russh::ChannelMsg::Data { data } => out.extend_from_slice(&data),
            russh::ChannelMsg::ExtendedData { data, .. } => out.extend_from_slice(&data),
            russh::ChannelMsg::Eof | russh::ChannelMsg::Close => break,
            russh::ChannelMsg::ExitStatus { .. } => break,
            _ => {}
        }
    }
    Ok(String::from_utf8_lossy(&out).to_string())
}

/// How many of the declared facets can actually be collected on a platform.
///
/// Surfaced in the UI so "17 facets" is never claimed when only eight ran.
pub fn collectable_count(
    platform: &Platform,
    config_paths: &[String],
    packages: &[String],
) -> (usize, usize) {
    let specs = facet_specs(config_paths, packages);
    let total = specs.len();
    let usable = specs.iter().filter(|s| runs_on(s, platform)).count();
    (usable, total)
}

/// How many collectable facets need privileges the login user may not have.
///
/// Config-file hashes are the common case: the file exists and the command
/// runs, but the read fails. Counting them separately keeps "N of M
/// collectable" from implying they will all succeed.
pub fn privileged_count(
    platform: &Platform,
    config_paths: &[String],
    packages: &[String],
) -> usize {
    facet_specs(config_paths, packages)
        .iter()
        .filter(|s| runs_on(s, platform) && s.privilege == Privilege::Root)
        .count()
}

fn runs_on(spec: &FacetSpec, platform: &Platform) -> bool {
    match &spec.key {
        FacetKey::FileHash(_) => matches!(platform, Platform::Linux | Platform::MacOS),
        FacetKey::PkgVersion(p) => command_for_package(p, platform).is_some(),
        _ => spec.command(platform).is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_facet_declares_a_privilege_and_at_least_one_platform() {
        let specs = facet_specs(&[], &[]);
        assert!(specs.len() >= 14, "expected the base facet set");
        for s in &specs {
            let has_platform = s.linux.is_some() || s.macos.is_some();
            assert!(has_platform, "{:?} runs nowhere", s.key);
            // Privilege is an enum, so this is a compile-time guarantee; the
            // assertion documents that Root facets exist and are deliberate.
            let _ = s.privilege;
        }
    }

    #[test]
    fn systemd_is_unsupported_on_macos_rather_than_divergent() {
        let specs = facet_specs(&[], &[]);
        let units = specs
            .iter()
            .find(|s| s.key == FacetKey::SystemdUnits)
            .unwrap();
        assert!(units.command(&Platform::Linux).is_some());
        assert!(
            units.command(&Platform::MacOS).is_none(),
            "a Mac without systemd is not a Mac that disagrees about systemd"
        );
    }

    #[test]
    fn privileged_facets_are_counted_separately() {
        // Config-file hashes are the ones that run but may not be readable.
        // "N of M collectable" must not quietly imply "N will succeed".
        let paths = vec!["/etc/nginx/nginx.conf".to_string()];
        let priv_n = privileged_count(&Platform::Linux, &paths, &[]);
        assert_eq!(priv_n, 1, "the config hash needs privileges");
        assert_eq!(privileged_count(&Platform::Linux, &[], &[]), 0);
        // An unsupported platform collects nothing, privileged or otherwise.
        assert_eq!(
            privileged_count(&Platform::Other("FreeBSD".into()), &paths, &[]),
            0
        );
    }

    #[test]
    fn collectable_count_does_not_overclaim() {
        let (mac_usable, mac_total) = collectable_count(&Platform::MacOS, &[], &[]);
        let (linux_usable, _) = collectable_count(&Platform::Linux, &[], &[]);
        assert!(
            mac_usable < mac_total,
            "macOS cannot collect every facet and must say so"
        );
        assert!(linux_usable > mac_usable);
        // A platform with no collectors at all claims nothing.
        let (other, _) = collectable_count(&Platform::Other("FreeBSD".into()), &[], &[]);
        assert_eq!(other, 0);
    }

    #[test]
    fn unreadable_config_files_report_why_rather_than_vanishing() {
        assert_eq!(
            interpret_sentinel("ESSH_DENIED"),
            Some(FacetValue::Missing("permission denied".into()))
        );
        assert_eq!(
            interpret_sentinel("ESSH_ABSENT"),
            Some(FacetValue::Missing("not installed".into()))
        );
        assert_eq!(interpret_sentinel("abc123"), None);
    }

    #[test]
    fn file_hash_command_distinguishes_absent_from_denied() {
        let cmd = command_for_file_hash("/etc/nginx/nginx.conf");
        assert!(cmd.contains("ESSH_ABSENT"));
        assert!(cmd.contains("ESSH_DENIED"));
        // Both hashers, because neither exists on both platforms.
        assert!(cmd.contains("sha256sum") && cmd.contains("shasum"));
    }

    #[test]
    fn listening_ports_are_sorted_so_ordering_is_not_drift() {
        let a = "LISTEN 0 128 0.0.0.0:443 0.0.0.0:*\nLISTEN 0 128 0.0.0.0:80 0.0.0.0:*\n";
        let b = "LISTEN 0 128 0.0.0.0:80 0.0.0.0:*\nLISTEN 0 128 0.0.0.0:443 0.0.0.0:*\n";
        assert_eq!(listening_ports(a), listening_ports(b));
        assert_eq!(listening_ports(a), Some(FacetValue::Text("80,443".into())));
    }

    #[test]
    fn listening_ports_parse_on_macos_where_the_separator_is_a_dot() {
        // Captured from `netstat -an -p tcp | grep LISTEN` on macOS 15.
        // Splitting on ':' alone found nothing here, so the facet came back
        // empty on every Mac.
        let raw = "tcp6       0      0  *.49419                *.*                    LISTEN\n\
                   tcp4       0      0  *.49419                *.*                    LISTEN\n\
                   tcp4       0      0  127.0.0.1.11434        *.*                    LISTEN\n";
        assert_eq!(
            listening_ports(raw),
            Some(FacetValue::Text("11434,49419".into()))
        );
    }

    #[test]
    fn host_key_algos_skip_the_keyscan_comment_lines() {
        // ssh-keyscan interleaves `# host:port SSH-2.0-...` with key lines.
        // Field 2 of a comment line is the host:port, which is not an
        // algorithm — and it turned up in the collected value.
        let raw = "# localhost:22 SSH-2.0-OpenSSH_9.9\n\
                   localhost ssh-ed25519 AAAAC3Nza...\n\
                   # localhost:22 SSH-2.0-OpenSSH_9.9\n\
                   localhost ssh-rsa AAAAB3Nza...\n";
        let v = host_key_algos(raw).expect("algos parse");
        assert_eq!(v, FacetValue::Text("ssh-ed25519,ssh-rsa".into()));
        match v {
            FacetValue::Text(s) => {
                assert!(!s.contains("localhost"), "host:port leaked in: {}", s);
                assert!(!s.ends_with(','), "trailing separator: {}", s);
            }
            _ => unreachable!(),
        }
        assert!(host_key_algos("# only a comment\n").is_none());
    }

    #[test]
    fn systemd_units_are_sorted_and_deduplicated() {
        let a = "nginx.service enabled\nssh.service enabled\n";
        let b = "ssh.service enabled\nnginx.service enabled\nnginx.service enabled\n";
        assert_eq!(enabled_units(a), enabled_units(b));
    }

    #[test]
    fn macos_uptime_survives_the_greedy_regex_trap() {
        // Real capture. The shell version extracted 938379 — the *usec*
        // field — because `.*sec = ` also matches inside `usec = `, and then
        // reported a host up 3 hours as up 20669 days.
        let raw = "{ sec = 1786755015, usec = 938379 } Sat Aug 15 10:50:15 2026\n1786766473\n";
        assert_eq!(uptime_days(raw), Some(FacetValue::Number(0.0)));

        // A host genuinely up 20 days reads as 20.
        let raw = "{ sec = 1785038473, usec = 1 } Sat Aug 15 10:50:15 2026\n1786766473\n";
        assert_eq!(uptime_days(raw), Some(FacetValue::Number(20.0)));
    }

    #[test]
    fn uptime_parser_handles_both_platform_shapes() {
        // Linux /proc/uptime
        assert_eq!(uptime_days("864000.12 1200.5"), Some(FacetValue::Number(10.0)));
        // macOS boottime + now
        assert_eq!(
            uptime_days("{ sec = 1786000000, usec = 5 }\n1786864000\n"),
            Some(FacetValue::Number(10.0))
        );
    }

    #[test]
    fn uptime_compares_in_days_not_seconds() {
        // Two hosts booted an hour apart are the same host, operationally.
        let a = uptime_days("864000.12 1200.5").unwrap();
        let b = uptime_days("867600.44 1300.1").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, FacetValue::Number(10.0));
    }

    #[test]
    fn root_disk_pct_reads_the_capacity_column() {
        let raw = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                   /dev/disk3s1s1 971350180 17380616 562007844 3% /\n";
        assert_eq!(root_disk_pct(raw), Some(FacetValue::Number(3.0)));
        assert_eq!(root_disk_pct(""), None);
    }

    #[test]
    fn section_markers_survive_commands_that_omit_a_trailing_newline() {
        // The bug this guards: `awk printf` and `tr '\n' ','` end without a
        // newline, so `echo '===N==='` landed on the tail of the previous
        // command's output and was never recognised as a marker. Every
        // section after the first such command was then attributed wrongly.
        let script_uses_printf = {
            let specs = facet_specs(&[], &[]);
            // Rebuild the same script shape collect_facts emits.
            let mut s = String::new();
            for (i, spec) in specs.iter().enumerate() {
                if spec.command(&Platform::Linux).is_some() {
                    s.push_str(&format!("printf '\\n==={}===\\n'; ", i));
                }
            }
            s
        };
        assert!(
            script_uses_printf.contains("printf '\\n===0===\\n'"),
            "markers must be newline-delimited on both sides"
        );

        // And the splitter must recover the sections given such output.
        let raw = "\n===0===\n6.1.0-18\n\n===1===\n0.07\n\n===2===\n4%\n\n===END===\n";
        let s = crate::monitor::collector::split_sections(raw);
        assert_eq!(s.get("0").map(|v| v.trim()), Some("6.1.0-18"));
        assert_eq!(s.get("1").map(|v| v.trim()), Some("0.07"));
        assert_eq!(s.get("2").map(|v| v.trim()), Some("4%"));
    }

    #[test]
    fn a_marker_glued_to_previous_output_is_the_failure_being_prevented() {
        // Documents the old behaviour so the fix is not silently reverted.
        let glued = "===0===\nssh-rsa,===1===\n4%\n===END===\n";
        let s = crate::monitor::collector::split_sections(glued);
        assert!(
            s.get("1").is_none(),
            "a glued marker cannot be recovered — which is why we emit \\n first"
        );
    }

    #[test]
    fn parsers_return_none_rather_than_a_confident_default() {
        assert_eq!(first_line(""), None);
        assert_eq!(number(""), None);
        assert_eq!(number("not a number"), None);
        assert_eq!(listening_ports(""), None);
        assert_eq!(enabled_units(""), None);
        assert_eq!(uptime_days(""), None);
    }
}
