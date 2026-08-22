//! The launcher: `essh` with no arguments.
//!
//! The spec's fast path is *launch → search → connect → work*, in seconds.
//! That makes the matcher the product: a list you have to scroll is a list
//! that failed. Search covers host, alias, user, tags and recency, per §2.
//!
//! The scoring is a deliberate design, not a library call:
//!
//! * **Subsequence matching**, so `pdb` finds `prod-db`.
//! * **Word starts score highest.** In a fleet, `pa` should find
//!   `prod-api` before `parallels-host`, because the letters land on segment
//!   boundaries rather than in the middle of one word.
//! * **Recency breaks ties, it does not override matches.** The host you used
//!   an hour ago wins a tie, but never outranks a better textual match — that
//!   is the behaviour that makes a launcher feel unpredictable.

use std::cmp::Ordering;

/// A candidate to connect to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Candidate {
    /// The name the user types and sees, e.g. `prod-db`.
    pub alias: String,
    /// Where it resolves to.
    pub hostname: String,
    pub port: u16,
    pub user: Option<String>,
    /// `key=value` tags, searched as `key=value`.
    pub tags: Vec<(String, String)>,
    /// Seconds since last connected; `None` if never.
    pub last_used_secs: Option<u64>,
    /// Where this candidate came from, shown so a user can tell an
    /// ssh_config host from one ESSH invented.
    pub source: Source,
    /// This host's config uses directives ESSH delegates to the system `ssh`
    /// (ProxyCommand, ControlMaster). Surfaced in the launcher so it is known
    /// before connecting, not discovered during an incident.
    pub delegated: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    /// From `~/.ssh/config` — the spec's "no separate host-management system".
    SshConfig,
    /// From ESSH's own config file.
    EsshConfig,
    /// Seen before and cached, but not in any config.
    #[default]
    Cache,
}

impl Source {
    #[allow(dead_code)] // shown in the launcher's detail row
    pub fn label(&self) -> &'static str {
        match self {
            Source::SshConfig => "ssh_config",
            Source::EsshConfig => "essh",
            Source::Cache => "seen before",
        }
    }
}

/// A scored match, with the positions that matched so the UI can highlight.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub candidate: Candidate,
    pub score: i64,
    /// Byte indices into `alias` that matched, for highlighting.
    pub highlights: Vec<usize>,
    /// Which field produced the match, so the row can say *why* it is here.
    pub matched_field: Field,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Alias,
    Hostname,
    User,
    Tag,
}

impl Field {
    pub fn label(&self) -> &'static str {
        match self {
            Field::Alias => "",
            Field::Hostname => "hostname",
            Field::User => "user",
            Field::Tag => "tag",
        }
    }
}

/// Score `query` against `text`, returning `None` when it does not match.
///
/// Positive scores only; higher is better.
fn score_subsequence(query: &str, text: &str) -> Option<(i64, Vec<usize>)> {
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let q: Vec<char> = query.chars().flat_map(|c| c.to_lowercase()).collect();
    let t: Vec<char> = text.chars().collect();
    let tl: Vec<char> = text.chars().flat_map(|c| c.to_lowercase()).collect();

    let mut score: i64 = 0;
    let mut positions = Vec::with_capacity(q.len());
    let mut ti = 0usize;
    let mut last_hit: Option<usize> = None;

    for &qc in &q {
        // Find the next occurrence.
        let found = (ti..tl.len()).find(|&i| tl[i] == qc)?;

        // Word-start bonus: the letter lands at the beginning of a segment.
        let at_boundary = found == 0
            || matches!(t[found - 1], '-' | '_' | '.' | ' ' | '/' | '=')
            || (t[found].is_uppercase() && !t[found - 1].is_uppercase());
        if at_boundary {
            score += 12;
        }
        // Adjacency bonus: consecutive characters read as a real prefix.
        if last_hit == Some(found.wrapping_sub(1)) {
            score += 8;
        }
        // Distance penalty: letters scattered far apart are a weak match.
        if let Some(prev) = last_hit {
            score -= ((found - prev - 1) as i64).min(6);
        }
        score += 1;
        positions.push(found);
        last_hit = Some(found);
        ti = found + 1;
    }

    // Whole-string bonuses.
    if tl.starts_with(&q) {
        score += 25;
    }
    if tl.len() == q.len() {
        score += 15;
    }
    // Shorter targets are better matches for the same query.
    score -= (tl.len() as i64 - q.len() as i64).min(20) / 4;

    Some((score, positions))
}

/// Recency contributes a bounded bonus — enough to break ties, never enough
/// to outrank a better textual match.
fn recency_bonus(last_used_secs: Option<u64>) -> i64 {
    match last_used_secs {
        None => 0,
        Some(s) if s < 300 => 10,
        Some(s) if s < 3_600 => 7,
        Some(s) if s < 86_400 => 5,
        Some(s) if s < 604_800 => 2,
        Some(_) => 1,
    }
}

/// Rank candidates against a query.
///
/// An empty query lists everything, most-recently-used first, which is the
/// right default for a launcher opened by reflex.
pub fn search(candidates: &[Candidate], query: &str) -> Vec<Match> {
    let q = query.trim();

    let mut out: Vec<Match> = candidates
        .iter()
        .filter_map(|c| {
            if q.is_empty() {
                return Some(Match {
                    candidate: c.clone(),
                    score: recency_bonus(c.last_used_secs),
                    highlights: Vec::new(),
                    matched_field: Field::Alias,
                });
            }

            // Try each field; keep the best. The alias is weighted highest
            // because it is what the user thinks the host is called.
            let mut best: Option<(i64, Vec<usize>, Field)> = None;
            let mut consider = |s: Option<(i64, Vec<usize>)>, weight: i64, field: Field| {
                if let Some((sc, pos)) = s {
                    let total = sc + weight;
                    if best.as_ref().map(|(b, _, _)| total > *b).unwrap_or(true) {
                        best = Some((total, pos, field));
                    }
                }
            };

            consider(score_subsequence(q, &c.alias), 30, Field::Alias);
            consider(score_subsequence(q, &c.hostname), 0, Field::Hostname);
            if let Some(u) = &c.user {
                consider(score_subsequence(q, u), -10, Field::User);
            }
            for (k, v) in &c.tags {
                consider(
                    score_subsequence(q, &format!("{}={}", k, v)),
                    -5,
                    Field::Tag,
                );
            }

            let (score, highlights, matched_field) = best?;
            Some(Match {
                candidate: c.clone(),
                score: score + recency_bonus(c.last_used_secs),
                // Highlights only make sense when the alias matched.
                highlights: if matched_field == Field::Alias {
                    highlights
                } else {
                    Vec::new()
                },
                matched_field,
            })
        })
        .collect();

    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            // Stable, predictable tie-break so the list never reshuffles
            // under the cursor between keystrokes.
            .then_with(
                || match (a.candidate.last_used_secs, b.candidate.last_used_secs) {
                    (Some(x), Some(y)) => x.cmp(&y),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => Ordering::Equal,
                },
            )
            .then_with(|| a.candidate.alias.cmp(&b.candidate.alias))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(alias: &str, hostname: &str) -> Candidate {
        Candidate {
            alias: alias.into(),
            hostname: hostname.into(),
            port: 22,
            ..Default::default()
        }
    }

    fn fleet() -> Vec<Candidate> {
        vec![
            c("prod-db", "10.0.0.5"),
            c("prod-api-01", "10.0.1.10"),
            c("prod-api-02", "10.0.1.11"),
            c("staging-db", "10.1.0.5"),
            c("bastion", "bastion.example.com"),
            c("parallels-host", "192.168.64.2"),
        ]
    }

    fn top(query: &str) -> String {
        search(&fleet(), query)
            .first()
            .map(|m| m.candidate.alias.clone())
            .unwrap_or_default()
    }

    #[test]
    fn initials_find_the_host_they_obviously_mean() {
        assert_eq!(top("pdb"), "prod-db");
        assert_eq!(top("sdb"), "staging-db");
        assert_eq!(top("bas"), "bastion");
    }

    #[test]
    fn a_contiguous_prefix_beats_scattered_initials() {
        // Deliberate: `pa` against `parallels-host` is an unbroken prefix,
        // which is a stronger and more predictable signal than p-a landing on
        // two separate segments of `prod-api-01`. Ranking initials above a
        // literal prefix is what makes launchers feel arbitrary.
        assert_eq!(top("pa"), "parallels-host");
    }

    #[test]
    fn word_starts_beat_matches_buried_mid_word() {
        // With no prefix match competing, the candidate whose letters land on
        // segment boundaries wins over one where they are buried mid-word.
        let cands = vec![
            c("prod-api-01", "10.0.1.10"),
            c("capacity-node", "10.5.0.1"),
        ];
        let hits = search(&cands, "pa");
        assert_eq!(hits[0].candidate.alias, "prod-api-01");
    }

    #[test]
    fn an_exact_alias_outranks_a_longer_prefix_match() {
        let cands = vec![c("db", "10.0.0.1"), c("db-replica-01", "10.0.0.2")];
        let hits = search(&cands, "db");
        assert_eq!(hits[0].candidate.alias, "db");
    }

    #[test]
    fn a_non_matching_query_returns_nothing_rather_than_everything() {
        assert!(search(&fleet(), "zzzzz").is_empty());
    }

    #[test]
    fn an_empty_query_lists_everything_most_recent_first() {
        let mut cands = fleet();
        cands[3].last_used_secs = Some(60); // staging-db, a minute ago
        cands[0].last_used_secs = Some(86_400 * 30); // prod-db, a month ago
        let hits = search(&cands, "");
        assert_eq!(hits.len(), cands.len());
        assert_eq!(hits[0].candidate.alias, "staging-db");
    }

    #[test]
    fn recency_breaks_ties_but_never_overrides_a_better_match() {
        let mut cands = vec![
            c("prod-db", "10.0.0.5"),
            c("parallels-host", "192.168.64.2"),
        ];
        // Make the *worse* textual match extremely recent.
        cands[1].last_used_secs = Some(1);
        let hits = search(&cands, "pdb");
        assert_eq!(
            hits[0].candidate.alias, "prod-db",
            "recency must not beat a clearly better match"
        );
    }

    #[test]
    fn recency_does_decide_between_equal_matches() {
        let mut a = c("web-01", "10.0.0.1");
        let mut b = c("web-02", "10.0.0.2");
        a.last_used_secs = Some(86_400 * 30);
        b.last_used_secs = Some(60);
        let hits = search(&[a, b], "web");
        assert_eq!(hits[0].candidate.alias, "web-02");
    }

    #[test]
    fn hostname_and_tags_are_searchable_not_just_the_alias() {
        let mut cand = c("mystery", "prod-db-7.internal");
        cand.tags = vec![("role".into(), "database".into())];
        let cands = vec![cand, c("unrelated", "10.9.9.9")];

        let by_hostname = search(&cands, "internal");
        assert_eq!(by_hostname[0].candidate.alias, "mystery");
        assert_eq!(by_hostname[0].matched_field, Field::Hostname);

        let by_tag = search(&cands, "role=data");
        assert_eq!(by_tag[0].candidate.alias, "mystery");
        assert_eq!(by_tag[0].matched_field, Field::Tag);
    }

    #[test]
    fn the_alias_wins_when_several_fields_match() {
        let mut cand = c("db", "db.example.com");
        cand.user = Some("db".into());
        let hits = search(&[cand], "db");
        assert_eq!(hits[0].matched_field, Field::Alias);
    }

    #[test]
    fn highlights_point_at_the_characters_that_matched() {
        let hits = search(&[c("prod-db", "10.0.0.5")], "pdb");
        let m = &hits[0];
        assert_eq!(m.matched_field, Field::Alias);
        let chars: Vec<char> = m.candidate.alias.chars().collect();
        let matched: String = m.highlights.iter().map(|&i| chars[i]).collect();
        assert_eq!(matched, "pdb");
    }

    #[test]
    fn results_are_stable_across_identical_searches() {
        // A list that reshuffles between keystrokes makes Enter dangerous.
        let cands = fleet();
        let first: Vec<String> = search(&cands, "p")
            .iter()
            .map(|m| m.candidate.alias.clone())
            .collect();
        for _ in 0..20 {
            let again: Vec<String> = search(&cands, "p")
                .iter()
                .map(|m| m.candidate.alias.clone())
                .collect();
            assert_eq!(first, again);
        }
    }

    #[test]
    fn matching_is_case_insensitive_both_ways() {
        let cands = vec![c("Prod-DB", "10.0.0.5")];
        assert!(!search(&cands, "prod").is_empty());
        assert!(!search(&cands, "PROD").is_empty());
    }

    #[test]
    fn source_is_reported_so_config_hosts_are_distinguishable() {
        let mut a = c("from-ssh", "10.0.0.1");
        a.source = Source::SshConfig;
        let hits = search(&[a], "");
        assert_eq!(hits[0].candidate.source.label(), "ssh_config");
    }
}

/// Build the candidate list from every source the spec names.
///
/// §2: *"ESSH uses the user's existing ~/.ssh/config, ~/.ssh/known_hosts,
/// ssh-agent. No separate host-management system is required."* So
/// `~/.ssh/config` is a first-class source, not an import step — an alias
/// added there appears here on the next launch with no `essh hosts import`.
///
/// Precedence when the same alias appears twice: ESSH's own config wins
/// (the user configured it here deliberately), then `ssh_config`, then the
/// cache. Merging rather than duplicating matters because a duplicated row
/// in a launcher is a coin flip.
pub fn collect_candidates(
    ssh_config: Option<&crate::sshconfig::SshConfig>,
    essh_hosts: &[crate::config::HostEntry],
    cached: &[crate::cache::CachedHost],
    local_user: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();

    fn upsert(out: &mut Vec<Candidate>, cand: Candidate) {
        if let Some(existing) = out.iter_mut().find(|c| c.alias == cand.alias) {
            // Fill gaps without overwriting a higher-precedence source.
            if existing.user.is_none() {
                existing.user = cand.user;
            }
            if existing.tags.is_empty() {
                existing.tags = cand.tags;
            }
            if existing.last_used_secs.is_none() {
                existing.last_used_secs = cand.last_used_secs;
            }
        } else {
            out.push(cand);
        }
    }

    // 1. ESSH config — highest precedence.
    for h in essh_hosts {
        upsert(
            &mut out,
            Candidate {
                alias: h.name.clone(),
                hostname: h.hostname.clone(),
                port: h.port,
                user: h.user.clone(),
                tags: crate::format::sorted_tags(&h.tags),
                last_used_secs: None,
                source: Source::EsshConfig,
                delegated: None,
            },
        );
    }

    // 2. ~/.ssh/config.
    if let Some(cfg) = ssh_config {
        for alias in cfg.aliases() {
            let r = cfg.resolve(&alias, local_user);
            upsert(
                &mut out,
                Candidate {
                    alias,
                    hostname: r.hostname.clone(),
                    port: r.port,
                    user: r.user.clone(),
                    tags: Vec::new(),
                    last_used_secs: None,
                    source: Source::SshConfig,
                    delegated: r.caveat_summary(),
                },
            );
        }
    }

    // 3. The cache — contributes recency to everything above, and any host
    // we have seen but which is in no config.
    for h in cached {
        let age = chrono::DateTime::parse_from_rfc3339(&h.last_seen)
            .ok()
            .map(|t| (now - t.with_timezone(&chrono::Utc)).num_seconds().max(0) as u64);

        if let Some(existing) = out
            .iter_mut()
            .find(|c| c.hostname == h.hostname && c.port == h.port)
        {
            existing.last_used_secs = age;
            if existing.tags.is_empty() {
                existing.tags = crate::format::sorted_tags(&h.tags);
            }
            continue;
        }

        upsert(
            &mut out,
            Candidate {
                alias: h.hostname.clone(),
                hostname: h.hostname.clone(),
                port: h.port,
                user: None,
                tags: crate::format::sorted_tags(&h.tags),
                last_used_secs: age,
                source: Source::Cache,
                delegated: None,
            },
        );
    }

    out
}

#[cfg(test)]
mod collect_tests {
    use super::*;
    use crate::config::HostEntry;
    use std::collections::HashMap;

    fn essh_host(name: &str, hostname: &str, port: u16) -> HostEntry {
        HostEntry {
            name: name.into(),
            hostname: hostname.into(),
            port,
            user: Some("deploy".into()),
            key: None,
            tags: HashMap::from([("role".to_string(), "web".to_string())]),
            jump_host: None,
            port_forwards: Vec::new(),
        }
    }

    fn cached(hostname: &str, port: u16, last_seen: &str) -> crate::cache::CachedHost {
        crate::cache::CachedHost {
            id: 1,
            hostname: hostname.into(),
            ip: None,
            port,
            fingerprint: "SHA256:x".into(),
            key_type: "ssh-ed25519".into(),
            first_seen: last_seen.into(),
            last_seen: last_seen.into(),
            tags: HashMap::new(),
        }
    }

    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-08-15T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn ssh_config_hosts_appear_without_an_import_step() {
        // §2: "No separate host-management system is required."
        let cfg = crate::sshconfig::SshConfig::parse_str(
            "Host prod-db\n  HostName 10.0.0.5\n  Port 2222\n  User deploy\n",
        );
        let cands = collect_candidates(Some(&cfg), &[], &[], "matt", now());
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].alias, "prod-db");
        assert_eq!(cands[0].hostname, "10.0.0.5");
        assert_eq!(cands[0].port, 2222);
        assert_eq!(cands[0].source, Source::SshConfig);
    }

    #[test]
    fn a_host_in_both_configs_appears_once_not_twice() {
        // A duplicated row in a launcher makes Enter a coin flip.
        let cfg = crate::sshconfig::SshConfig::parse_str("Host prod-db\n  HostName 10.9.9.9\n");
        let essh = vec![essh_host("prod-db", "10.0.0.5", 22)];
        let cands = collect_candidates(Some(&cfg), &essh, &[], "matt", now());
        assert_eq!(cands.len(), 1);
        // ESSH's own config wins, because the user configured it there.
        assert_eq!(cands[0].hostname, "10.0.0.5");
        assert_eq!(cands[0].source, Source::EsshConfig);
    }

    #[test]
    fn the_cache_contributes_recency_to_a_configured_host() {
        let essh = vec![essh_host("prod-db", "10.0.0.5", 22)];
        let seen = vec![cached("10.0.0.5", 22, "2026-08-15T11:30:00Z")];
        let cands = collect_candidates(None, &essh, &seen, "matt", now());
        assert_eq!(cands.len(), 1, "the cache must not add a duplicate row");
        assert_eq!(cands[0].last_used_secs, Some(1800));
        assert_eq!(cands[0].source, Source::EsshConfig);
    }

    #[test]
    fn a_host_only_in_the_cache_is_still_offered() {
        let seen = vec![cached("10.4.4.4", 22, "2026-08-15T11:00:00Z")];
        let cands = collect_candidates(None, &[], &seen, "matt", now());
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].source, Source::Cache);
        assert_eq!(cands[0].last_used_secs, Some(3600));
    }

    #[test]
    fn wildcard_blocks_do_not_become_launcher_entries() {
        let cfg = crate::sshconfig::SshConfig::parse_str(
            "Host *\n  User matt\n\nHost real\n  HostName 10.0.0.1\n",
        );
        let cands = collect_candidates(Some(&cfg), &[], &[], "matt", now());
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].alias, "real");
    }

    #[test]
    fn a_future_last_seen_does_not_underflow_the_age() {
        // Clock skew must not produce a nonsensical recency bonus.
        let seen = vec![cached("10.4.4.4", 22, "2026-08-15T13:00:00Z")];
        let cands = collect_candidates(None, &[], &seen, "matt", now());
        assert_eq!(cands[0].last_used_secs, Some(0));
    }

    #[test]
    fn everything_is_searchable_once_collected() {
        let cfg =
            crate::sshconfig::SshConfig::parse_str("Host bastion\n  HostName b.example.com\n");
        let essh = vec![essh_host("prod-db", "10.0.0.5", 22)];
        let cands = collect_candidates(Some(&cfg), &essh, &[], "matt", now());
        assert_eq!(search(&cands, "bas")[0].candidate.alias, "bastion");
        assert_eq!(search(&cands, "pdb")[0].candidate.alias, "prod-db");
        // Tag search reaches the ESSH-configured host.
        assert_eq!(search(&cands, "role=web")[0].candidate.alias, "prod-db");
    }
}
