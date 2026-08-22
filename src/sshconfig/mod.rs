//! A real `ssh_config` parser and resolver.
//!
//! The spec's principle is *"ESSH should adapt to the user's SSH environment,
//! not force users into a new one."* That is a much larger promise than the
//! ten bullets under §6 suggest. Reaching production hosts in practice means
//! `Include`, `Match`, wildcard and negated `Host` patterns, `ProxyCommand`
//! (not only `ProxyJump`), `ControlMaster`, `IdentityFile` and percent-token
//! expansion.
//!
//! Two deliberate positions:
//!
//! * **Unsupported keywords are reported, not ignored.** Every resolution
//!   carries the directives we saw and did not honour, so the UI can say
//!   "this host uses ProxyCommand, which ESSH runs via the system ssh" rather
//!   than silently connecting differently from what the file asked for. See
//!   [`Support`].
//! * **`Match exec` is never run during enumeration.** Listing hosts must not
//!   execute arbitrary commands out of a config file. It is evaluated only on
//!   an explicit connect, and even then only when the caller opts in.

pub mod tokens;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub use tokens::expand_tokens;

/// How well ESSH honours a directive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// Honoured natively.
    Full,
    /// Understood, but handled by delegating to the system `ssh` binary.
    ViaSystemSsh,
    /// Parsed and reported, but not acted on.
    Unsupported,
}

/// The compatibility tier list, exposed so the product can be honest about it.
///
/// v1 claimed "prioritise compatibility with normal OpenSSH workflows" and
/// implemented ten directives. Publishing the real table is the difference
/// between a sentiment and a contract.
pub fn support_for(keyword: &str) -> Support {
    match keyword.to_ascii_lowercase().as_str() {
        // Fully honoured by the native client.
        "hostname" | "port" | "user" | "identityfile" | "identitiesonly" | "proxyjump"
        | "serveraliveinterval" | "serveralivecountmax" | "connecttimeout" | "requesttty"
        | "stricthostkeychecking" | "userknownhostsfile" | "forwardagent" | "identityagent"
        | "localforward" | "remoteforward" | "dynamicforward" | "setenv" | "sendenv"
        | "compression" | "addkeystoagent" | "include" | "match" | "host" => Support::Full,

        // Understood, delegated. ProxyCommand is the important one: `nc`,
        // `cloudflared access`, `aws ssm start-session` and `gcloud compute
        // ssh` are how a great many production hosts are actually reached.
        "proxycommand" | "controlmaster" | "controlpath" | "controlpersist"
        | "certificatefile" | "pkcs11provider" | "securitykeyprovider" | "gssapiauthentication"
        | "remotecommand" => Support::ViaSystemSsh,

        // Parsed and surfaced, but not acted on.
        _ => Support::Unsupported,
    }
}

/// One `Host` or `Match` block.
#[derive(Clone, Debug)]
struct Block {
    selector: Selector,
    options: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
enum Selector {
    Host(Vec<Pattern>),
    Match(Vec<Criterion>),
}

#[derive(Clone, Debug, PartialEq)]
struct Pattern {
    glob: String,
    negated: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Criterion {
    All,
    Final,
    Canonical,
    Host(Vec<Pattern>),
    User(Vec<Pattern>),
    /// Never evaluated during enumeration; see the module docs.
    Exec(String),
}

/// A parsed `ssh_config`, in file order.
#[derive(Clone, Debug, Default)]
pub struct SshConfig {
    blocks: Vec<Block>,
    /// Files that were read, in the order they were included.
    pub sources: Vec<PathBuf>,
    /// Includes that could not be read, with the reason.
    pub broken_includes: Vec<(String, String)>,
}

/// Everything ESSH resolved for one target.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedHost {
    pub alias: String,
    pub hostname: String,
    pub port: u16,
    pub user: Option<String>,
    pub identity_files: Vec<String>,
    pub identities_only: bool,
    pub identity_agent: Option<String>,
    pub proxy_jump: Option<String>,
    pub proxy_command: Option<String>,
    pub request_tty: Option<String>,
    pub remote_command: Option<String>,
    pub strict_host_key_checking: Option<String>,
    pub user_known_hosts_files: Vec<String>,
    pub server_alive_interval: Option<u64>,
    pub connect_timeout: Option<u64>,
    pub forward_agent: bool,
    pub set_env: Vec<(String, String)>,
    pub local_forwards: Vec<String>,
    pub remote_forwards: Vec<String>,
    pub dynamic_forwards: Vec<String>,
    pub control_master: Option<String>,
    pub control_path: Option<String>,
    pub certificate_files: Vec<String>,
    /// Directives seen for this host that ESSH does not honour natively,
    /// as `(keyword, value, support)`.
    pub caveats: Vec<(String, String, Support)>,
}

impl ResolvedHost {
    /// Whether reaching this host needs the system `ssh` binary.
    ///
    /// `caveat_summary` is what the UI shows; this is the predicate callers
    /// branch on when choosing a transport.
    #[allow(dead_code)]
    pub fn needs_system_ssh(&self) -> bool {
        self.caveats
            .iter()
            .any(|(_, _, s)| *s == Support::ViaSystemSsh)
    }

    /// One line for the UI naming why, or `None` when we can connect natively.
    pub fn caveat_summary(&self) -> Option<String> {
        let mut delegated: Vec<&str> = self
            .caveats
            .iter()
            .filter(|(_, _, s)| *s == Support::ViaSystemSsh)
            .map(|(k, _, _)| k.as_str())
            .collect();
        delegated.sort_unstable();
        delegated.dedup();
        if delegated.is_empty() {
            return None;
        }
        Some(format!(
            "uses {} — ESSH connects via the system ssh for this host",
            delegated.join(", ")
        ))
    }
}

fn split_patterns(value: &str) -> Vec<Pattern> {
    value
        .split_whitespace()
        .map(|p| {
            if let Some(rest) = p.strip_prefix('!') {
                Pattern {
                    glob: rest.to_string(),
                    negated: true,
                }
            } else {
                Pattern {
                    glob: p.to_string(),
                    negated: false,
                }
            }
        })
        .collect()
}

/// OpenSSH glob matching: `*` any run, `?` one character. Case-insensitive.
fn glob_matches(pattern: &str, text: &str) -> bool {
    fn inner(p: &[u8], t: &[u8]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some(b'*') => {
                // Try consuming nothing, then one character at a time.
                (0..=t.len()).any(|i| inner(&p[1..], &t[i..]))
            }
            Some(b'?') => !t.is_empty() && inner(&p[1..], &t[1..]),
            Some(c) => {
                !t.is_empty()
                    && t[0].eq_ignore_ascii_case(c)
                    && inner(&p[1..], &t[1..])
            }
        }
    }
    inner(pattern.as_bytes(), text.as_bytes())
}

/// A pattern list matches when something matches and nothing negated does.
fn patterns_match(patterns: &[Pattern], text: &str) -> bool {
    let mut positive = false;
    for p in patterns {
        if glob_matches(&p.glob, text) {
            if p.negated {
                return false;
            }
            positive = true;
        }
    }
    positive
}

impl SshConfig {
    /// Parse a config file, expanding `Include` directives.
    pub fn load(path: &Path) -> Self {
        let mut cfg = SshConfig::default();
        let mut seen = HashSet::new();
        cfg.load_into(path, &mut seen, 0);
        cfg
    }

    /// Parse from a string rather than a file.
    #[allow(dead_code)] // used by tests and by callers holding config in memory
    pub fn parse_str(text: &str) -> Self {
        let mut cfg = SshConfig::default();
        cfg.absorb(text, Path::new("<memory>"), &mut HashSet::new(), 0);
        cfg
    }

    fn load_into(&mut self, path: &Path, seen: &mut HashSet<PathBuf>, depth: usize) {
        // OpenSSH caps include depth; a config that includes itself must not
        // hang the launcher.
        if depth > 16 {
            self.broken_includes.push((
                path.display().to_string(),
                "include nesting too deep".to_string(),
            ));
            return;
        }
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if !seen.insert(canonical.clone()) {
            return;
        }
        match std::fs::read_to_string(path) {
            Ok(text) => {
                self.sources.push(path.to_path_buf());
                self.absorb(&text, path, seen, depth);
            }
            Err(e) => {
                self.broken_includes
                    .push((path.display().to_string(), e.to_string()));
            }
        }
    }

    fn absorb(&mut self, text: &str, origin: &Path, seen: &mut HashSet<PathBuf>, depth: usize) {
        let base = origin.parent().map(|p| p.to_path_buf()).unwrap_or_default();

        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // `Key value` or `Key=value`.
            let (key, value) = match line.find(|c: char| c.is_whitespace() || c == '=') {
                Some(i) => (
                    line[..i].trim(),
                    line[i..].trim_start_matches(['=', ' ', '\t']).trim(),
                ),
                None => (line, ""),
            };
            let lowered = key.to_ascii_lowercase();

            match lowered.as_str() {
                "host" => self.blocks.push(Block {
                    selector: Selector::Host(split_patterns(value)),
                    options: Vec::new(),
                }),
                "match" => self.blocks.push(Block {
                    selector: Selector::Match(parse_match(value)),
                    options: Vec::new(),
                }),
                "include" => {
                    for token in value.split_whitespace() {
                        self.expand_include(token, &base, seen, depth);
                    }
                }
                _ => {
                    if let Some(block) = self.blocks.last_mut() {
                        block.options.push((lowered, value.to_string()));
                    } else {
                        // Options before any Host block apply globally, which
                        // OpenSSH models as an implicit `Host *`.
                        self.blocks.push(Block {
                            selector: Selector::Host(vec![Pattern {
                                glob: "*".into(),
                                negated: false,
                            }]),
                            options: vec![(lowered, value.to_string())],
                        });
                    }
                }
            }
        }
    }

    fn expand_include(
        &mut self,
        token: &str,
        base: &Path,
        seen: &mut HashSet<PathBuf>,
        depth: usize,
    ) {
        let expanded = shellexpand::tilde(token).to_string();
        let candidate = PathBuf::from(&expanded);
        // Relative includes resolve against the including file's directory —
        // OpenSSH resolves them against ~/.ssh for the user config.
        let path = if candidate.is_absolute() {
            candidate
        } else {
            base.join(candidate)
        };

        let pattern = path.to_string_lossy().to_string();
        if pattern.contains('*') || pattern.contains('?') {
            match glob_paths(&pattern) {
                Ok(paths) if !paths.is_empty() => {
                    for p in paths {
                        self.load_into(&p, seen, depth + 1);
                    }
                }
                Ok(_) => { /* a glob matching nothing is not an error */ }
                Err(e) => self.broken_includes.push((pattern, e)),
            }
        } else {
            self.load_into(&path, seen, depth + 1);
        }
    }

    /// Resolve every directive that applies to `alias`.
    ///
    /// OpenSSH semantics: the *first* value obtained for a keyword wins, so
    /// earlier blocks take precedence. List-valued keywords accumulate.
    pub fn resolve(&self, alias: &str, local_user: &str) -> ResolvedHost {
        let mut out = ResolvedHost {
            alias: alias.to_string(),
            hostname: alias.to_string(),
            port: 22,
            ..Default::default()
        };
        let mut seen_scalar: HashSet<String> = HashSet::new();

        // Two passes so `Match user` can see a User set by an earlier Host
        // block, which is how OpenSSH behaves in practice.
        let provisional_user = self.first_value(alias, local_user, "user", None);
        let effective_user = provisional_user
            .clone()
            .unwrap_or_else(|| local_user.to_string());

        for block in &self.blocks {
            if !self.block_applies(block, alias, &effective_user) {
                continue;
            }
            for (key, value) in &block.options {
                apply_option(&mut out, key, value, &mut seen_scalar);
            }
        }

        if out.user.is_none() {
            out.user = provisional_user;
        }
        out
    }

    fn first_value(
        &self,
        alias: &str,
        local_user: &str,
        keyword: &str,
        _default: Option<&str>,
    ) -> Option<String> {
        for block in &self.blocks {
            if !self.block_applies(block, alias, local_user) {
                continue;
            }
            for (k, v) in &block.options {
                if k == keyword {
                    return Some(v.clone());
                }
            }
        }
        None
    }

    fn block_applies(&self, block: &Block, alias: &str, user: &str) -> bool {
        match &block.selector {
            Selector::Host(patterns) => patterns_match(patterns, alias),
            Selector::Match(criteria) => criteria.iter().all(|c| match c {
                Criterion::All | Criterion::Final => true,
                Criterion::Canonical => false,
                Criterion::Host(p) => patterns_match(p, alias),
                Criterion::User(p) => patterns_match(p, user),
                // Never executed while enumerating or resolving for display.
                Criterion::Exec(_) => false,
            }),
        }
    }

    /// Concrete host aliases, for the launcher.
    ///
    /// Wildcard blocks are settings, not hosts — `Host *` is not something a
    /// user can connect to, and listing it as a target is noise.
    pub fn aliases(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for block in &self.blocks {
            if let Selector::Host(patterns) = &block.selector {
                for p in patterns {
                    if p.negated || p.glob.contains('*') || p.glob.contains('?') {
                        continue;
                    }
                    if !out.iter().any(|a| a == &p.glob) {
                        out.push(p.glob.clone());
                    }
                }
            }
        }
        out
    }

    /// Directives present anywhere in the file that ESSH does not honour.
    pub fn global_caveats(&self) -> Vec<(String, Support)> {
        let mut out: Vec<(String, Support)> = Vec::new();
        for block in &self.blocks {
            for (k, _) in &block.options {
                let s = support_for(k);
                if s != Support::Full && !out.iter().any(|(name, _)| name == k) {
                    out.push((k.clone(), s));
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

fn parse_match(value: &str) -> Vec<Criterion> {
    let mut out = Vec::new();
    let mut it = value.split_whitespace().peekable();
    while let Some(tok) = it.next() {
        match tok.to_ascii_lowercase().as_str() {
            "all" => out.push(Criterion::All),
            "final" => out.push(Criterion::Final),
            "canonical" => out.push(Criterion::Canonical),
            "host" | "originalhost" => {
                if let Some(v) = it.next() {
                    out.push(Criterion::Host(split_patterns(v)));
                }
            }
            "user" | "localuser" => {
                if let Some(v) = it.next() {
                    out.push(Criterion::User(split_patterns(v)));
                }
            }
            "exec" => {
                let rest: Vec<&str> = it.by_ref().collect();
                out.push(Criterion::Exec(rest.join(" ")));
            }
            _ => {}
        }
    }
    out
}

fn apply_option(
    out: &mut ResolvedHost,
    key: &str,
    value: &str,
    seen_scalar: &mut HashSet<String>,
) {
    // Scalars: first value wins, per OpenSSH.
    let mut once = |set: &mut dyn FnMut()| {
        if seen_scalar.insert(key.to_string()) {
            set();
        }
    };

    match key {
        "hostname" => once(&mut || out.hostname = value.to_string()),
        "port" => once(&mut || {
            if let Ok(p) = value.parse() {
                out.port = p;
            }
        }),
        "user" => once(&mut || out.user = Some(value.to_string())),
        "proxyjump" => once(&mut || out.proxy_jump = Some(value.to_string())),
        "proxycommand" => once(&mut || out.proxy_command = Some(value.to_string())),
        "requesttty" => once(&mut || out.request_tty = Some(value.to_string())),
        "remotecommand" => once(&mut || out.remote_command = Some(value.to_string())),
        "identityagent" => once(&mut || out.identity_agent = Some(value.to_string())),
        "stricthostkeychecking" => {
            once(&mut || out.strict_host_key_checking = Some(value.to_string()))
        }
        "controlmaster" => once(&mut || out.control_master = Some(value.to_string())),
        "controlpath" => once(&mut || out.control_path = Some(value.to_string())),
        "identitiesonly" => once(&mut || out.identities_only = is_yes(value)),
        "forwardagent" => once(&mut || out.forward_agent = is_yes(value)),
        "serveraliveinterval" => once(&mut || out.server_alive_interval = value.parse().ok()),
        "connecttimeout" => once(&mut || out.connect_timeout = value.parse().ok()),

        // Lists accumulate across blocks.
        "identityfile" => out.identity_files.push(value.to_string()),
        "certificatefile" => out.certificate_files.push(value.to_string()),
        "userknownhostsfile" => out
            .user_known_hosts_files
            .extend(value.split_whitespace().map(|s| s.to_string())),
        "localforward" => out.local_forwards.push(value.to_string()),
        "remoteforward" => out.remote_forwards.push(value.to_string()),
        "dynamicforward" => out.dynamic_forwards.push(value.to_string()),
        "setenv" | "sendenv" => {
            for pair in value.split_whitespace() {
                if let Some((k, v)) = pair.split_once('=') {
                    out.set_env.push((k.to_string(), v.to_string()));
                }
            }
        }
        _ => {}
    }

    // Record anything we do not honour natively, once per keyword.
    let support = support_for(key);
    if support != Support::Full && !out.caveats.iter().any(|(k, _, _)| k == key) {
        out.caveats
            .push((key.to_string(), value.to_string(), support));
    }
}

fn is_yes(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "yes" | "true" | "on")
}

/// Minimal glob for `Include` paths — only the final component may be a glob,
/// which is what OpenSSH itself supports.
fn glob_paths(pattern: &str) -> Result<Vec<PathBuf>, String> {
    let path = Path::new(pattern);
    let dir = path.parent().ok_or("include pattern has no directory")?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("include pattern has no filename")?;

    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| glob_matches(name, n))
                .unwrap_or(false)
        })
        .map(|e| e.path())
        .collect();
    // Deterministic order: OpenSSH reads glob matches sorted.
    out.sort();
    Ok(out)
}

/// The user's config path, honouring `$HOME`.
pub fn default_path() -> PathBuf {
    let mut p = dirs::home_dir().unwrap_or_default();
    p.push(".ssh");
    p.push("config");
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_semantics_match_openssh() {
        assert!(glob_matches("*", "anything"));
        assert!(glob_matches("web-*", "web-01"));
        assert!(glob_matches("web-?", "web-1"));
        assert!(!glob_matches("web-?", "web-01"));
        assert!(glob_matches("*.example.com", "a.b.example.com"));
        assert!(!glob_matches("*.example.com", "example.com"));
        // Case-insensitive, like hostnames.
        assert!(glob_matches("WEB-*", "web-01"));
    }

    #[test]
    fn negated_patterns_exclude() {
        // `Host web-* !web-99` must not match web-99.
        let pats = split_patterns("web-* !web-99");
        assert!(patterns_match(&pats, "web-01"));
        assert!(!patterns_match(&pats, "web-99"));
    }

    #[test]
    fn first_value_wins_for_scalars() {
        // OpenSSH takes the first obtained value, so the specific block above
        // the wildcard block is the one that counts.
        let cfg = SshConfig::parse_str(
            "Host prod-db\n  HostName 10.0.0.5\n  Port 2222\n\nHost *\n  Port 22\n  User fallback\n",
        );
        let r = cfg.resolve("prod-db", "matt");
        assert_eq!(r.hostname, "10.0.0.5");
        assert_eq!(r.port, 2222, "the later Host * must not override");
        assert_eq!(r.user.as_deref(), Some("fallback"));
    }

    #[test]
    fn options_before_any_host_block_apply_globally() {
        let cfg = SshConfig::parse_str("ServerAliveInterval 30\n\nHost a\n  HostName 10.0.0.1\n");
        let r = cfg.resolve("a", "matt");
        assert_eq!(r.server_alive_interval, Some(30));
        assert_eq!(r.hostname, "10.0.0.1");
    }

    #[test]
    fn identity_files_accumulate_rather_than_overwrite() {
        let cfg = SshConfig::parse_str(
            "Host a\n  IdentityFile ~/.ssh/id_a\n\nHost *\n  IdentityFile ~/.ssh/id_default\n",
        );
        let r = cfg.resolve("a", "matt");
        assert_eq!(r.identity_files.len(), 2);
        assert_eq!(r.identity_files[0], "~/.ssh/id_a");
    }

    #[test]
    fn equals_separated_directives_parse() {
        // `Port=2222` is legal ssh_config and the old importer dropped it.
        let cfg = SshConfig::parse_str("Host a\n  HostName=10.0.0.9\n  Port=2222\n");
        let r = cfg.resolve("a", "matt");
        assert_eq!(r.hostname, "10.0.0.9");
        assert_eq!(r.port, 2222);
    }

    #[test]
    fn match_host_and_user_are_honoured() {
        let cfg = SshConfig::parse_str(
            "Match host bastion-* user deploy\n  ProxyJump none\n  Port 2200\n",
        );
        let hit = cfg.resolve("bastion-01", "deploy");
        assert_eq!(hit.port, 2200);
        // Wrong user: the block must not apply.
        let miss = cfg.resolve("bastion-01", "someone-else");
        assert_eq!(miss.port, 22);
    }

    #[test]
    fn match_user_sees_a_user_set_by_an_earlier_host_block() {
        let cfg = SshConfig::parse_str(
            "Host prod\n  User deploy\n\nMatch user deploy\n  IdentityFile ~/.ssh/id_deploy\n",
        );
        let r = cfg.resolve("prod", "matt");
        assert!(
            r.identity_files.iter().any(|f| f.contains("id_deploy")),
            "Match user should see User from the Host block: {:?}",
            r.identity_files
        );
    }

    #[test]
    fn match_exec_is_never_evaluated() {
        // Listing hosts must not run commands out of a config file.
        let cfg = SshConfig::parse_str("Match exec \"rm -rf /\"\n  Port 9999\n");
        let r = cfg.resolve("anything", "matt");
        assert_eq!(r.port, 22, "Match exec must not apply, and must not run");
    }

    #[test]
    fn aliases_exclude_wildcard_blocks() {
        let cfg = SshConfig::parse_str(
            "Host prod-db prod-api\n  User deploy\n\nHost *.internal\n  User x\n\nHost *\n  User y\n",
        );
        let aliases = cfg.aliases();
        assert_eq!(aliases, vec!["prod-db", "prod-api"]);
        assert!(
            !aliases.iter().any(|a| a.contains('*')),
            "`Host *` is settings, not a target"
        );
    }

    #[test]
    fn proxycommand_is_reported_as_delegated_not_silently_dropped() {
        let cfg = SshConfig::parse_str(
            "Host prod\n  HostName 10.0.0.5\n  ProxyCommand cloudflared access ssh --hostname %h\n",
        );
        let r = cfg.resolve("prod", "matt");
        assert_eq!(
            r.proxy_command.as_deref(),
            Some("cloudflared access ssh --hostname %h")
        );
        assert!(r.needs_system_ssh());
        let summary = r.caveat_summary().expect("a caveat summary");
        assert!(summary.contains("proxycommand"), "{}", summary);
        assert!(summary.contains("system ssh"), "{}", summary);
    }

    #[test]
    fn a_host_with_no_special_directives_has_no_caveats() {
        let cfg = SshConfig::parse_str("Host plain\n  HostName 10.0.0.1\n  User deploy\n");
        let r = cfg.resolve("plain", "matt");
        assert!(!r.needs_system_ssh());
        assert!(r.caveat_summary().is_none());
    }

    #[test]
    fn controlmaster_is_flagged_because_users_notice_its_absence() {
        let cfg = SshConfig::parse_str(
            "Host prod\n  ControlMaster auto\n  ControlPath ~/.ssh/cm-%r@%h:%p\n",
        );
        let r = cfg.resolve("prod", "matt");
        assert_eq!(r.control_master.as_deref(), Some("auto"));
        assert!(r.needs_system_ssh());
    }

    #[test]
    fn unknown_directives_are_surfaced_rather_than_ignored() {
        let cfg = SshConfig::parse_str("Host a\n  KexAlgorithms curve25519-sha256\n");
        let r = cfg.resolve("a", "matt");
        assert!(r
            .caveats
            .iter()
            .any(|(k, _, s)| k == "kexalgorithms" && *s == Support::Unsupported));
    }

    #[test]
    fn forwards_accumulate() {
        let cfg = SshConfig::parse_str(
            "Host a\n  LocalForward 8080 localhost:80\n  LocalForward 5432 db:5432\n  DynamicForward 1080\n",
        );
        let r = cfg.resolve("a", "matt");
        assert_eq!(r.local_forwards.len(), 2);
        assert_eq!(r.dynamic_forwards, vec!["1080"]);
    }

    #[test]
    fn setenv_pairs_are_split() {
        let cfg = SshConfig::parse_str("Host a\n  SetEnv FOO=bar BAZ=qux\n");
        let r = cfg.resolve("a", "matt");
        assert_eq!(
            r.set_env,
            vec![
                ("FOO".to_string(), "bar".to_string()),
                ("BAZ".to_string(), "qux".to_string())
            ]
        );
    }

    #[test]
    fn include_pulls_in_another_file_and_resolves_across_both() {
        let dir = tempfile::tempdir().unwrap();
        let inc = dir.path().join("work.conf");
        std::fs::write(&inc, "Host work-db\n  HostName 10.9.9.9\n  Port 2299\n").unwrap();
        let main = dir.path().join("config");
        std::fs::write(
            &main,
            format!("Include {}\n\nHost *\n  User matt\n", inc.display()),
        )
        .unwrap();

        let cfg = SshConfig::load(&main);
        assert_eq!(cfg.sources.len(), 2);
        assert!(cfg.broken_includes.is_empty());

        let r = cfg.resolve("work-db", "matt");
        assert_eq!(r.hostname, "10.9.9.9");
        assert_eq!(r.port, 2299);
        assert_eq!(r.user.as_deref(), Some("matt"));
        assert!(cfg.aliases().contains(&"work-db".to_string()));
    }

    #[test]
    fn include_globs_expand_in_a_stable_order() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("conf.d");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("10-a.conf"), "Host a\n  Port 2001\n").unwrap();
        std::fs::write(sub.join("20-b.conf"), "Host b\n  Port 2002\n").unwrap();
        let main = dir.path().join("config");
        std::fs::write(&main, "Include conf.d/*.conf\n").unwrap();

        let cfg = SshConfig::load(&main);
        assert_eq!(cfg.resolve("a", "matt").port, 2001);
        assert_eq!(cfg.resolve("b", "matt").port, 2002);
        let mut names = cfg.aliases();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn a_self_including_config_terminates() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("config");
        std::fs::write(&main, format!("Include {}\nHost a\n  Port 22\n", main.display())).unwrap();
        // The guarantee under test is simply that this returns.
        let cfg = SshConfig::load(&main);
        assert_eq!(cfg.sources.len(), 1);
    }

    #[test]
    fn a_missing_include_is_reported_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("config");
        std::fs::write(&main, "Include /nonexistent/nope.conf\nHost a\n  Port 2222\n").unwrap();
        let cfg = SshConfig::load(&main);
        assert_eq!(cfg.broken_includes.len(), 1);
        // The rest of the file still parses.
        assert_eq!(cfg.resolve("a", "matt").port, 2222);
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let cfg = SshConfig::parse_str("# a comment\n\n  # indented\nHost a\n  Port 2222\n");
        assert_eq!(cfg.resolve("a", "matt").port, 2222);
    }

    /// Differential test against OpenSSH itself.
    ///
    /// `ssh -G <host>` prints OpenSSH's own resolution, so for every alias in
    /// the user's real config we can assert that ESSH agrees. This is the
    /// compatibility claim made executable — far stronger than any fixture,
    /// because it runs against whatever config the user actually has.
    ///
    /// Opt-in because it needs `ssh` and a real config:
    /// `cargo test --bin essh differential -- --ignored --nocapture`
    #[ignore = "needs the ssh binary and a real ~/.ssh/config"]
    #[test]
    fn differential_against_ssh_dash_g() {
        let path = default_path();
        if !path.exists() {
            eprintln!("no ~/.ssh/config; nothing to compare");
            return;
        }
        let cfg = SshConfig::load(&path);
        let local_user = std::env::var("USER").unwrap_or_default();

        let aliases = cfg.aliases();
        if aliases.is_empty() {
            eprintln!("no concrete aliases to compare");
            return;
        }

        let mut compared = 0usize;
        for alias in &aliases {
            let out = match std::process::Command::new("ssh").arg("-G").arg(alias).output() {
                Ok(o) if o.status.success() => o,
                _ => continue,
            };
            let text = String::from_utf8_lossy(&out.stdout);

            let field = |name: &str| -> Option<String> {
                text.lines()
                    .find(|l| {
                        l.split_whitespace().next().map(|k| k.eq_ignore_ascii_case(name))
                            == Some(true)
                    })
                    .and_then(|l| l.split_once(char::is_whitespace))
                    .map(|(_, v)| v.trim().to_string())
            };

            let mine = cfg.resolve(alias, &local_user);
            compared += 1;

            if let Some(h) = field("hostname") {
                assert_eq!(
                    mine.hostname, h,
                    "hostname disagrees with `ssh -G {}`",
                    alias
                );
            }
            if let Some(p) = field("port").and_then(|v| v.parse::<u16>().ok()) {
                assert_eq!(mine.port, p, "port disagrees with `ssh -G {}`", alias);
            }
            if let Some(u) = field("user") {
                let ours = mine.user.clone().unwrap_or_else(|| local_user.clone());
                assert_eq!(ours, u, "user disagrees with `ssh -G {}`", alias);
            }
            // ProxyCommand is compared post-expansion on both sides, since
            // that is what actually runs.
            if let Some(pc) = field("proxycommand") {
                let ctx = tokens::TokenContext {
                    hostname: mine.hostname.clone(),
                    original_host: alias.clone(),
                    port: mine.port,
                    remote_user: mine.user.clone().unwrap_or_else(|| local_user.clone()),
                    local_user: local_user.clone(),
                    home: dirs::home_dir().unwrap_or_default().display().to_string(),
                    local_hostname: String::new(),
                };
                let ours = mine
                    .proxy_command
                    .as_ref()
                    .map(|c| expand_tokens(c, &ctx))
                    .unwrap_or_default();
                assert_eq!(ours, pc, "proxycommand disagrees for {}", alias);
            }
            eprintln!("  ✓ {} agrees with ssh -G", alias);
        }
        eprintln!("compared {} aliases against ssh -G", compared);
        assert!(compared > 0, "no alias could be compared");
    }

    #[test]
    fn support_table_is_explicit_about_the_three_tiers() {
        assert_eq!(support_for("HostName"), Support::Full);
        assert_eq!(support_for("ProxyJump"), Support::Full);
        assert_eq!(support_for("ProxyCommand"), Support::ViaSystemSsh);
        assert_eq!(support_for("ControlMaster"), Support::ViaSystemSsh);
        assert_eq!(support_for("Ciphers"), Support::Unsupported);
    }
}
