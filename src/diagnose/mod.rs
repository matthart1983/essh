//! Why did this connection fail?
//!
//! OpenSSH's failure output is famously unreadable, and worst exactly where it
//! matters most — through a `ProxyJump` chain, against a server that has
//! disabled `ssh-rsa`, or when a host key has changed. The spec asks for a
//! ladder:
//!
//! ```text
//! Could not connect to prod-db
//!
//! DNS       ✓
//! Bastion   ✓
//! TCP:22    timeout
//! ```
//!
//! Two rules the spec states and this module enforces:
//!
//! * **Never invent diagnostics.** Every rung is `Ok`, `Failed`, `Skipped` or
//!   `NotProbed`, and the last two render differently from a tick or a cross.
//!   A ladder that shows ✓ for something it never tested is worse than no
//!   ladder, because it directs attention away from the actual fault.
//! * **Name the auth failure.** "Permission denied (publickey)" is where most
//!   real time is lost. The taxonomy below distinguishes *no key offered*
//!   from *key rejected* from *algorithm refused*, which are three different
//!   problems with three different fixes.

use std::fmt;
use std::net::ToSocketAddrs;
use std::time::{Duration, Instant};

/// One rung of the ladder.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Probed and fine. Carries what we learned.
    Ok(String),
    /// Probed and failed, in words a human can act on.
    Failed(String),
    /// Deliberately not applicable — e.g. no bastion is configured.
    Skipped(String),
    /// Not reached, because an earlier rung failed. Explicitly *not* a pass.
    NotProbed,
}

impl Status {
    pub fn symbol(&self) -> &'static str {
        match self {
            Status::Ok(_) => "✓",
            Status::Failed(_) => "✗",
            Status::Skipped(_) => "·",
            // Deliberately blank rather than any mark that could read as a
            // verdict.
            Status::NotProbed => " ",
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Status::Ok(d) | Status::Failed(d) | Status::Skipped(d) => d,
            Status::NotProbed => "not probed",
        }
    }

    pub fn is_failure(&self) -> bool {
        matches!(self, Status::Failed(_))
    }
}

/// A rung, in the order it is attempted.
#[derive(Clone, Debug, PartialEq)]
pub struct Rung {
    pub label: String,
    pub status: Status,
}

/// The whole ladder for one connection attempt.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Diagnosis {
    pub target: String,
    pub rungs: Vec<Rung>,
}

impl Diagnosis {
    fn push(&mut self, label: &str, status: Status) {
        self.rungs.push(Rung {
            label: label.to_string(),
            status,
        });
    }

    /// The first rung that failed, which is the one to act on.
    pub fn first_failure(&self) -> Option<&Rung> {
        self.rungs.iter().find(|r| r.status.is_failure())
    }

    /// Did everything we attempted succeed?
    pub fn succeeded(&self) -> bool {
        self.first_failure().is_none()
    }

    /// A one-line summary for a status bar.
    pub fn headline(&self) -> String {
        match self.first_failure() {
            Some(r) => format!("{}: {}", r.label, r.status.detail()),
            None => "all checks passed".to_string(),
        }
    }
}

impl fmt::Display for Diagnosis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Could not connect to {}", self.target)?;
        writeln!(f)?;
        let width = self
            .rungs
            .iter()
            .map(|r| r.label.chars().count())
            .max()
            .unwrap_or(8)
            .max(8);
        for r in &self.rungs {
            writeln!(
                f,
                "{:<width$}  {}  {}",
                r.label,
                r.status.symbol(),
                r.status.detail(),
                width = width
            )?;
        }
        Ok(())
    }
}

/// What we know about an authentication failure.
///
/// The three publickey cases below fail with the same OpenSSH message and
/// need entirely different fixes, which is why they are separated here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthFailure {
    /// No key was offered at all — nothing in the agent, no IdentityFile.
    NoCredentialOffered,
    /// A key was offered and the server rejected it.
    KeyRejected,
    /// The server refused the key's *algorithm*, the classic `ssh-rsa`
    /// deprecation failure, which looks identical to a rejected key.
    AlgorithmRefused(String),
    /// A certificate was offered but is expired or not yet valid.
    CertificateInvalid(String),
    /// Keyboard-interactive or MFA did not complete.
    InteractiveIncomplete,
    /// Password authentication failed.
    PasswordRejected,
    /// The server permitted none of the methods we can perform.
    NoSharedMethod { offered: Vec<String> },
    /// Something else; carries the raw text rather than guessing.
    Other(String),
}

impl AuthFailure {
    /// What to tell the user, and what to try.
    pub fn explain(&self) -> String {
        match self {
            AuthFailure::NoCredentialOffered => {
                "no key was offered — ssh-agent is empty and no IdentityFile matched".into()
            }
            AuthFailure::KeyRejected => {
                "the key was offered and rejected — it is probably not in authorized_keys".into()
            }
            AuthFailure::AlgorithmRefused(algo) => format!(
                "the server refused the key's algorithm ({}) — the key itself may be fine",
                algo
            ),
            AuthFailure::CertificateInvalid(why) => {
                format!("the certificate was rejected: {}", why)
            }
            AuthFailure::InteractiveIncomplete => {
                "keyboard-interactive did not complete — an MFA prompt may have timed out".into()
            }
            AuthFailure::PasswordRejected => "the password was rejected".into(),
            AuthFailure::NoSharedMethod { offered } => format!(
                "the server accepts only {} — none of which ESSH offered",
                offered.join(", ")
            ),
            AuthFailure::Other(raw) => raw.clone(),
        }
    }
}

/// Classify an authentication error.
///
/// Deliberately conservative: anything unrecognised becomes `Other` carrying
/// the original text, rather than being forced into a category it might not
/// belong to.
pub fn classify_auth_failure(raw: &str, offered_a_key: bool) -> AuthFailure {
    let l = raw.to_ascii_lowercase();

    if l.contains("no keys found") || l.contains("no identities") || l.contains("agent is empty") {
        return AuthFailure::NoCredentialOffered;
    }
    // Algorithm refusal — check before the generic publickey case, since the
    // server's message contains both.
    for algo in ["ssh-rsa", "ssh-dss", "rsa-sha2-256", "rsa-sha2-512"] {
        if l.contains(algo)
            && (l.contains("not in pubkeyacceptedalgorithms")
                || l.contains("no mutual signature")
                || l.contains("unsupported public key algorithm")
                || l.contains("refused"))
        {
            return AuthFailure::AlgorithmRefused(algo.to_string());
        }
    }
    if l.contains("certificate") && (l.contains("expired") || l.contains("not yet valid")) {
        return AuthFailure::CertificateInvalid(raw.trim().to_string());
    }

    // The server's method list must be read before the per-method keywords
    // below. "can continue with: publickey, keyboard-interactive" *names*
    // keyboard-interactive as available — it is not evidence that an MFA
    // prompt was attempted and timed out, which is what the keyword check
    // would otherwise conclude.
    if let Some(rest) = l.split("can continue with:").nth(1) {
        let methods: Vec<String> = rest
            .split(',')
            .map(|s| s.trim().trim_end_matches(')').trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !methods.is_empty() {
            return AuthFailure::NoSharedMethod { offered: methods };
        }
    }

    if l.contains("keyboard-interactive") || l.contains("mfa") || l.contains("two-factor") {
        return AuthFailure::InteractiveIncomplete;
    }
    if l.contains("password") {
        return AuthFailure::PasswordRejected;
    }
    if l.contains("publickey") || l.contains("public key") || l.contains("auth") {
        return if offered_a_key {
            AuthFailure::KeyRejected
        } else {
            AuthFailure::NoCredentialOffered
        };
    }
    AuthFailure::Other(raw.trim().to_string())
}

/// Classify a host-key problem. This one is security-relevant, so the wording
/// stays blunt and the remedy is never made to look routine.
pub fn explain_host_key_change(old_fp: &str, new_fp: &str, host: &str) -> String {
    format!(
        "the host key for {} changed.\n  \
         known: {}\n  \
         offered: {}\n\
         This is what a machine-in-the-middle looks like. It is also what a \
         rebuilt host looks like. Confirm out of band which one this is before \
         removing the old key.",
        host, old_fp, new_fp
    )
}

/// A jump host, carrying both the alias the user wrote and where it resolved
/// to. Showing only the address loses the name they typed; showing only the
/// name hides where it actually pointed.
#[derive(Clone, Debug)]
pub struct Bastion {
    pub alias: String,
    pub hostname: String,
    pub port: u16,
}

/// What to probe.
#[derive(Clone, Debug)]
pub struct Target {
    pub alias: String,
    pub hostname: String,
    pub port: u16,
    /// The first `ProxyJump` hop, if one is configured.
    pub bastion: Option<Bastion>,
    /// Set when the config uses a ProxyCommand, which we cannot probe through.
    pub proxy_command: Option<String>,
}

/// Run the ladder.
///
/// Each rung is attempted only if the previous one passed; the rest are left
/// `NotProbed`, which renders blank rather than as a pass.
pub async fn diagnose(target: &Target, timeout: Duration) -> Diagnosis {
    let mut d = Diagnosis {
        target: target.alias.clone(),
        rungs: Vec::new(),
    };

    // A ProxyCommand means the path to the host is an opaque external
    // program. We cannot meaningfully probe DNS or TCP ourselves, and
    // pretending otherwise would produce a confident, wrong ladder.
    if let Some(cmd) = &target.proxy_command {
        d.push(
            "Config",
            Status::Ok(format!("{} → {}", target.alias, target.hostname)),
        );
        d.push(
            "ProxyCommand",
            Status::Skipped(format!(
                "reached through `{}` — ESSH cannot probe inside it",
                cmd.split_whitespace().next().unwrap_or(cmd)
            )),
        );
        d.push("DNS", Status::NotProbed);
        d.push(&format!("TCP:{}", target.port), Status::NotProbed);
        return d;
    }

    d.push(
        "Config",
        Status::Ok(format!(
            "{} → {}:{}",
            target.alias, target.hostname, target.port
        )),
    );

    // ── Bastion first: if you cannot reach the jump host, nothing about the
    // target itself can be established, and saying so is the whole point.
    match &target.bastion {
        Some(b) => {
            let shown = if b.alias == b.hostname {
                format!("{}:{}", b.hostname, b.port)
            } else {
                format!("{} → {}:{}", b.alias, b.hostname, b.port)
            };
            match resolve_host(&b.hostname, timeout).await {
                Ok(_) => match tcp_probe(&format!("{}:{}", b.hostname, b.port), timeout).await {
                    Ok(rtt) => d.push(
                        "Bastion",
                        Status::Ok(format!("{} in {}ms", shown, rtt.as_millis())),
                    ),
                    Err(e) => {
                        d.push("Bastion", Status::Failed(format!("{} — {}", shown, e)));
                        d.push("DNS", Status::NotProbed);
                        d.push(&format!("TCP:{}", target.port), Status::NotProbed);
                        return d;
                    }
                },
                Err(e) => {
                    d.push("Bastion", Status::Failed(format!("{} — {}", shown, e)));
                    d.push("DNS", Status::NotProbed);
                    d.push(&format!("TCP:{}", target.port), Status::NotProbed);
                    return d;
                }
            }
        }
        None => d.push("Bastion", Status::Skipped("no ProxyJump configured".into())),
    }

    // ── DNS
    let addr = match resolve_host(&target.hostname, timeout).await {
        Ok(a) => {
            d.push("DNS", Status::Ok(a.clone()));
            a
        }
        Err(e) => {
            d.push("DNS", Status::Failed(e));
            d.push(&format!("TCP:{}", target.port), Status::NotProbed);
            d.push("SSH", Status::NotProbed);
            return d;
        }
    };

    // ── TCP
    let label = format!("TCP:{}", target.port);
    match tcp_probe(&format!("{}:{}", target.hostname, target.port), timeout).await {
        Ok(rtt) => d.push(
            &label,
            Status::Ok(format!("{} in {}ms", addr, rtt.as_millis())),
        ),
        Err(e) => {
            d.push(&label, Status::Failed(e));
            d.push("SSH", Status::NotProbed);
            return d;
        }
    }

    // ── SSH banner: distinguishes "something is listening" from "sshd is
    // listening", which is the difference between a port-forward pointing at
    // the wrong service and a real SSH problem.
    match banner_probe(&format!("{}:{}", target.hostname, target.port), timeout).await {
        Ok(banner) => d.push("SSH", Status::Ok(banner)),
        Err(e) => d.push("SSH", Status::Failed(e)),
    }

    d
}

async fn resolve_host(host: &str, timeout: Duration) -> Result<String, String> {
    // A literal address needs no lookup; saying "resolved" would be a lie.
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Ok(format!("{} (literal address)", host));
    }
    let h = host.to_string();
    let lookup = tokio::task::spawn_blocking(move || {
        (h.as_str(), 0u16)
            .to_socket_addrs()
            .map(|it| it.map(|a| a.ip().to_string()).collect::<Vec<_>>())
    });

    match tokio::time::timeout(timeout, lookup).await {
        Err(_) => Err(format!("timed out after {}s", timeout.as_secs())),
        Ok(Err(e)) => Err(format!("lookup task failed: {}", e)),
        Ok(Ok(Err(e))) => {
            let msg = e.to_string();
            if msg.contains("not known") || msg.contains("No address") || msg.contains("failure") {
                Err(format!("no such host ({})", msg))
            } else {
                Err(msg)
            }
        }
        Ok(Ok(Ok(addrs))) if addrs.is_empty() => Err("resolved to no addresses".into()),
        Ok(Ok(Ok(addrs))) => Ok(addrs.join(", ")),
    }
}

async fn tcp_probe(addr: &str, timeout: Duration) -> Result<Duration, String> {
    let start = Instant::now();
    match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(addr)).await {
        Err(_) => Err(format!("timed out after {}s", timeout.as_secs())),
        Ok(Err(e)) => Err(describe_io(&e)),
        Ok(Ok(_)) => Ok(start.elapsed()),
    }
}

/// Read the SSH identification string the server sends on connect.
async fn banner_probe(addr: &str, timeout: Duration) -> Result<String, String> {
    use tokio::io::AsyncReadExt;

    let fut = async {
        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .map_err(|e| describe_io(&e))?;
        let mut buf = [0u8; 255];
        let n = stream.read(&mut buf).await.map_err(|e| describe_io(&e))?;
        Ok::<String, String>(String::from_utf8_lossy(&buf[..n]).trim().to_string())
    };

    match tokio::time::timeout(timeout, fut).await {
        Err(_) => Err("connected, but no SSH banner arrived".into()),
        Ok(Err(e)) => Err(e),
        Ok(Ok(banner)) if banner.starts_with("SSH-") => {
            Ok(banner.lines().next().unwrap_or(&banner).to_string())
        }
        Ok(Ok(banner)) if banner.is_empty() => {
            Err("connected, but the server said nothing — is this port really sshd?".into())
        }
        Ok(Ok(other)) => Err(format!(
            "something is listening, but it is not sshd (said {:?})",
            other.chars().take(24).collect::<String>()
        )),
    }
}

fn describe_io(e: &std::io::Error) -> String {
    use std::io::ErrorKind;
    match e.kind() {
        ErrorKind::ConnectionRefused => "connection refused — nothing is listening".to_string(),
        ErrorKind::TimedOut => "timed out".to_string(),
        ErrorKind::HostUnreachable => "host unreachable".to_string(),
        ErrorKind::NetworkUnreachable => "network unreachable — check routing or VPN".to_string(),
        ErrorKind::PermissionDenied => {
            "permission denied — a local firewall may be blocking".to_string()
        }
        _ => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unprobed_rungs_never_render_as_a_pass() {
        // The core honesty rule: a rung we never reached must not show a tick.
        let s = Status::NotProbed;
        assert_eq!(s.symbol(), " ");
        assert_ne!(s.symbol(), "✓");
        assert!(!s.is_failure());
        assert_eq!(s.detail(), "not probed");
    }

    #[test]
    fn skipped_is_distinct_from_both_pass_and_fail() {
        let s = Status::Skipped("no ProxyJump configured".into());
        assert_eq!(s.symbol(), "·");
        assert!(!s.is_failure());
    }

    #[test]
    fn the_ladder_names_the_first_failure() {
        let d = Diagnosis {
            target: "prod-db".into(),
            rungs: vec![
                Rung {
                    label: "DNS".into(),
                    status: Status::Ok("10.0.0.5".into()),
                },
                Rung {
                    label: "Bastion".into(),
                    status: Status::Ok("ok".into()),
                },
                Rung {
                    label: "TCP:22".into(),
                    status: Status::Failed("timed out after 5s".into()),
                },
                Rung {
                    label: "SSH".into(),
                    status: Status::NotProbed,
                },
            ],
        };
        assert!(!d.succeeded());
        assert_eq!(d.first_failure().unwrap().label, "TCP:22");
        assert!(d.headline().contains("TCP:22"));
        assert!(d.headline().contains("timed out"));

        let rendered = d.to_string();
        assert!(rendered.contains("Could not connect to prod-db"));
        assert!(rendered.contains("✓"));
        assert!(rendered.contains("✗"));
        // The unprobed SSH rung must not have acquired a tick.
        let ssh_line = rendered.lines().find(|l| l.starts_with("SSH")).unwrap();
        assert!(!ssh_line.contains('✓'), "{}", ssh_line);
        assert!(ssh_line.contains("not probed"), "{}", ssh_line);
    }

    #[test]
    fn auth_taxonomy_separates_the_three_publickey_cases() {
        // Same OpenSSH message, three different fixes.
        assert_eq!(
            classify_auth_failure("Permission denied (publickey)", true),
            AuthFailure::KeyRejected
        );
        assert_eq!(
            classify_auth_failure("Permission denied (publickey)", false),
            AuthFailure::NoCredentialOffered
        );
        assert_eq!(
            classify_auth_failure("No keys found in ssh-agent", false),
            AuthFailure::NoCredentialOffered
        );
    }

    #[test]
    fn the_ssh_rsa_deprecation_is_named_rather_than_read_as_a_bad_key() {
        // This is the failure that wastes the most time: the key is fine, the
        // server just will not accept its signature algorithm.
        let f = classify_auth_failure(
            "send_pubkey_test: no mutual signature algorithm for ssh-rsa",
            true,
        );
        assert_eq!(f, AuthFailure::AlgorithmRefused("ssh-rsa".into()));
        assert!(
            f.explain().contains("key itself may be fine"),
            "{}",
            f.explain()
        );
    }

    #[test]
    fn expired_certificates_are_distinguished_from_rejected_keys() {
        let f = classify_auth_failure("certificate has expired", true);
        assert!(matches!(f, AuthFailure::CertificateInvalid(_)));
    }

    #[test]
    fn the_servers_offered_methods_are_reported_when_known() {
        let f = classify_auth_failure(
            "Permission denied (can continue with: publickey, keyboard-interactive)",
            false,
        );
        match f {
            AuthFailure::NoSharedMethod { offered } => {
                assert!(offered.iter().any(|m| m.contains("keyboard-interactive")));
            }
            other => panic!("expected NoSharedMethod, got {:?}", other),
        }
    }

    #[test]
    fn an_unrecognised_error_is_carried_verbatim_not_forced_into_a_bucket() {
        let f = classify_auth_failure("kex_exchange_identification: banana", true);
        assert_eq!(
            f,
            AuthFailure::Other("kex_exchange_identification: banana".into())
        );
    }

    #[test]
    fn a_changed_host_key_is_never_made_to_sound_routine() {
        let msg = explain_host_key_change("SHA256:aaa", "SHA256:bbb", "prod-db");
        assert!(msg.contains("machine-in-the-middle"));
        assert!(msg.contains("Confirm out of band"));
        // It must not tell the user how to delete the old key in the same
        // breath as telling them it changed.
        assert!(!msg.contains("ssh-keygen -R"));
    }

    #[tokio::test]
    async fn a_proxycommand_host_reports_that_it_cannot_be_probed() {
        // Claiming DNS ✓ for a host reached through cloudflared would be
        // inventing a diagnostic.
        let t = Target {
            alias: "prod".into(),
            hostname: "prod.internal".into(),
            port: 22,
            bastion: None,
            proxy_command: Some("cloudflared access ssh --hostname %h".into()),
        };
        let d = diagnose(&t, Duration::from_millis(200)).await;
        let dns = d.rungs.iter().find(|r| r.label == "DNS").unwrap();
        assert_eq!(dns.status, Status::NotProbed);
        let pc = d.rungs.iter().find(|r| r.label == "ProxyCommand").unwrap();
        assert!(matches!(pc.status, Status::Skipped(_)));
        assert!(pc.status.detail().contains("cloudflared"));
    }

    #[tokio::test]
    async fn a_literal_address_is_not_described_as_resolved() {
        let out = resolve_host("127.0.0.1", Duration::from_secs(1))
            .await
            .unwrap();
        assert!(out.contains("literal"), "{}", out);
    }

    #[tokio::test]
    async fn a_nonexistent_host_fails_at_dns_and_stops_there() {
        let t = Target {
            alias: "nope".into(),
            hostname: "this-host-does-not-exist.invalid".into(),
            port: 22,
            bastion: None,
            proxy_command: None,
        };
        let d = diagnose(&t, Duration::from_secs(3)).await;
        assert_eq!(d.first_failure().map(|r| r.label.as_str()), Some("DNS"));
        // Everything downstream must be NotProbed, not failed and not passed.
        let tcp = d.rungs.iter().find(|r| r.label.starts_with("TCP")).unwrap();
        assert_eq!(tcp.status, Status::NotProbed);
    }

    #[tokio::test]
    async fn a_closed_port_is_named_as_refused_not_as_a_timeout() {
        // Port 1 on loopback: refused immediately.
        let t = Target {
            alias: "local".into(),
            hostname: "127.0.0.1".into(),
            port: 1,
            bastion: None,
            proxy_command: None,
        };
        let d = diagnose(&t, Duration::from_secs(2)).await;
        let tcp = d.rungs.iter().find(|r| r.label == "TCP:1").unwrap();
        assert!(tcp.status.is_failure());
        assert!(
            tcp.status.detail().contains("refused") || tcp.status.detail().contains("unreachable"),
            "got {:?}",
            tcp.status
        );
        // DNS came first and passed, so it stays a tick.
        let dns = d.rungs.iter().find(|r| r.label == "DNS").unwrap();
        assert!(matches!(dns.status, Status::Ok(_)));
    }

    #[tokio::test]
    async fn a_bastion_failure_stops_the_ladder_before_the_target() {
        let t = Target {
            alias: "prod-db".into(),
            hostname: "10.255.255.1".into(),
            port: 22,
            bastion: Some(Bastion {
                alias: "jump".into(),
                hostname: "this-bastion-does-not-exist.invalid".into(),
                port: 22,
            }),
            proxy_command: None,
        };
        let d = diagnose(&t, Duration::from_secs(3)).await;
        assert_eq!(d.first_failure().map(|r| r.label.as_str()), Some("Bastion"));
        // The target's own DNS is unknowable from here and must say so.
        let dns = d.rungs.iter().find(|r| r.label == "DNS").unwrap();
        assert_eq!(dns.status, Status::NotProbed);
    }

    #[tokio::test]
    async fn no_bastion_is_skipped_rather_than_passed() {
        let t = Target {
            alias: "local".into(),
            hostname: "127.0.0.1".into(),
            port: 1,
            bastion: None,
            proxy_command: None,
        };
        let d = diagnose(&t, Duration::from_secs(2)).await;
        let b = d.rungs.iter().find(|r| r.label == "Bastion").unwrap();
        assert!(matches!(b.status, Status::Skipped(_)));
        assert_eq!(b.status.symbol(), "·");
    }
}
