//! Divergence: how does this host differ from its peers?
//!
//! A flat list of green dots cannot answer the question an operator with forty
//! web servers actually has — *are these machines the same?* Divergence
//! answers it by collecting the same facts from every host in a peer set,
//! taking the modal value as consensus, and scoring each host by how far it
//! sits from that consensus.
//!
//! Three rules run through the whole module:
//!
//! 1. **Severity is derived, never guessed.** Categorical facets score
//!    `1.0 - agree/total`; numeric facets score by distance from the peer
//!    median. There is no hand-tuned weighting table.
//! 2. **Unprobed is not diverging.** A host we have no facts for is excluded
//!    from every denominator and reported separately. Conflating the two is
//!    what made v1's `Offline: 8` meaningless.
//! 3. **A missing fact is a stated fact.** If a collector cannot read
//!    something — no permission, no such command — the facet records why, and
//!    that host is left out of the consensus for that facet rather than
//!    counted as disagreeing.

pub mod collect;
pub mod verdict;

use std::collections::HashMap;

pub use verdict::{verdict_for, Verdict};

/// Which fact about a host is being compared.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FacetKey {
    Kernel,
    OsRelease,
    CpuModel,
    CpuCount,
    MemTotal,
    OpenSsl,
    Timezone,
    NtpSync,
    ListeningPorts,
    SystemdUnits,
    SshHostKeyAlgo,
    DiskRootPct,
    UptimeDays,
    LoadPerCore,
    PkgVersion(String),
    FileHash(String),
}

impl FacetKey {
    pub fn label(&self) -> String {
        match self {
            FacetKey::Kernel => "kernel".into(),
            FacetKey::OsRelease => "os release".into(),
            FacetKey::CpuModel => "cpu model".into(),
            FacetKey::CpuCount => "cpu count".into(),
            FacetKey::MemTotal => "mem total".into(),
            FacetKey::OpenSsl => "openssl".into(),
            FacetKey::Timezone => "timezone".into(),
            FacetKey::NtpSync => "ntp sync".into(),
            FacetKey::ListeningPorts => "listening ports".into(),
            FacetKey::SystemdUnits => "systemd units".into(),
            FacetKey::SshHostKeyAlgo => "ssh host key algo".into(),
            FacetKey::DiskRootPct => "disk /".into(),
            FacetKey::UptimeDays => "uptime".into(),
            FacetKey::LoadPerCore => "load per core".into(),
            FacetKey::PkgVersion(p) => format!("pkg {}", p),
            FacetKey::FileHash(p) => p.clone(),
        }
    }

    /// Numeric facets are compared by distance from the median; categorical
    /// ones by whether you match the modal value.
    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            FacetKey::DiskRootPct
                | FacetKey::UptimeDays
                | FacetKey::LoadPerCore
                | FacetKey::CpuCount
                | FacetKey::MemTotal
        )
    }
}

/// One host's answer for one facet.
#[derive(Clone, Debug, PartialEq)]
pub enum FacetValue {
    Text(String),
    Number(f64),
    /// The collector ran but could not produce a value, and says why. This
    /// host is excluded from this facet's consensus — it is not evidence of
    /// agreement or of disagreement.
    Missing(String),
}

impl FacetValue {
    pub fn as_display(&self) -> String {
        match self {
            FacetValue::Text(s) => s.clone(),
            FacetValue::Number(n) => {
                if (n.fract()).abs() < f64::EPSILON {
                    format!("{}", *n as i64)
                } else {
                    format!("{:.1}", n)
                }
            }
            FacetValue::Missing(reason) => format!("— {}", reason),
        }
    }

    pub fn is_known(&self) -> bool {
        !matches!(self, FacetValue::Missing(_))
    }

    fn as_number(&self) -> Option<f64> {
        match self {
            FacetValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// The key a categorical comparison groups on.
    fn group_key(&self) -> Option<String> {
        match self {
            FacetValue::Text(s) => Some(s.clone()),
            FacetValue::Number(n) => Some(format!("{}", n)),
            FacetValue::Missing(_) => None,
        }
    }
}

/// Everything collected from one host in one sweep.
#[derive(Clone, Debug, Default)]
pub struct HostFacts {
    /// Read by the live tests and by callers that keep facts keyed elsewhere.
    #[allow(dead_code)]
    pub host: String,
    pub facets: HashMap<FacetKey, FacetValue>,
}

impl HostFacts {
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            facets: HashMap::new(),
        }
    }

    /// Builder used by tests and by callers assembling facts by hand.
    #[allow(dead_code)]
    pub fn with(mut self, key: FacetKey, value: FacetValue) -> Self {
        self.facets.insert(key, value);
        self
    }
}

/// A named group of hosts to compare against each other.
#[derive(Clone, Debug)]
pub struct PeerSet {
    /// The tag that defines membership, e.g. `("role", "web")`.
    pub selector: (String, String),
    pub hosts: Vec<String>,
}

impl PeerSet {
    pub fn label(&self) -> String {
        format!("{}={}", self.selector.0, self.selector.1)
    }
}

/// How one host compares to its peers on one facet.
#[derive(Clone, Debug)]
pub struct FacetComparison {
    pub key: FacetKey,
    pub mine: FacetValue,
    /// The modal value across peers that had a reading.
    pub consensus: Option<FacetValue>,
    /// How many peers share the consensus value.
    pub agree: usize,
    /// How many peers had a reading at all.
    pub known: usize,
    /// 0.0 = at consensus, 1.0 = alone. Used for ranking.
    pub severity: f64,
    /// Whether this host is genuinely the odd one out on this facet.
    ///
    /// Separate from `severity` because the two questions are different. On a
    /// numeric facet every host has some distance from the median — a fleet
    /// whose disks range evenly from 40% to 79% has forty non-zero severities
    /// and zero outliers. Ranking wants the distance; flagging wants the
    /// outlier test.
    pub is_outlier: bool,
    /// For numeric facets: the peer median and this host's percentile.
    pub distribution: Option<Distribution>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Distribution {
    pub median: f64,
    pub percentile: f64,
    pub min: f64,
    pub max: f64,
}

impl FacetComparison {
    /// Whether this comparison is worth a row. Agreement collapses.
    pub fn diverges(&self) -> bool {
        self.is_outlier
    }

    /// A one-line summary that carries the peer context, because `disk 84%` is
    /// not a finding and `84% · median 62% · you are p100` is.
    pub fn summary(&self) -> String {
        match (&self.distribution, &self.consensus) {
            (Some(d), _) => format!(
                "{} · median {:.0} · you are p{:.0}",
                self.mine.as_display(),
                d.median,
                d.percentile
            ),
            (None, Some(c)) if c != &self.mine => format!(
                "{} · {} of {} peers have {}",
                self.mine.as_display(),
                self.agree,
                self.known,
                c.as_display()
            ),
            _ => self.mine.as_display(),
        }
    }
}

/// The full comparison of one host against its peer set.
#[derive(Clone, Debug)]
pub struct HostDivergence {
    pub host: String,
    pub peer_set: String,
    pub comparisons: Vec<FacetComparison>,
    /// Facets where every host with a reading agreed.
    pub identical: Vec<FacetKey>,
    /// Peers we have no facts for — excluded from every denominator.
    pub unprobed_peers: Vec<String>,
}

impl HostDivergence {
    /// The facets on which this host differs, worst first.
    pub fn diverging(&self) -> Vec<&FacetComparison> {
        let mut v: Vec<&FacetComparison> =
            self.comparisons.iter().filter(|c| c.diverges()).collect();
        v.sort_by(|a, b| {
            b.severity
                .partial_cmp(&a.severity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }

    /// A single 0–N score for sorting a host list: how many facets differ.
    pub fn score(&self) -> usize {
        self.diverging().len()
    }

    /// Whether this host has any facts at all. A host with none is *unprobed*,
    /// which is a different thing from a host that agrees with everyone.
    pub fn is_probed(&self) -> bool {
        self.comparisons.iter().any(|c| c.mine.is_known())
    }
}

/// Compare one host against its peer set.
///
/// `all` must contain the facts for every host in `peer_set` that has any;
/// hosts in the set but absent from `all` are reported as unprobed.
pub fn compare(host: &str, peer_set: &PeerSet, all: &HashMap<String, HostFacts>) -> HostDivergence {
    let unprobed_peers: Vec<String> = peer_set
        .hosts
        .iter()
        .filter(|h| !all.contains_key(*h))
        .cloned()
        .collect();

    let mine = all.get(host);
    let mut comparisons = Vec::new();
    let mut identical = Vec::new();

    // Every facet key anyone in the set reported.
    let mut keys: Vec<FacetKey> = peer_set
        .hosts
        .iter()
        .filter_map(|h| all.get(h))
        .flat_map(|f| f.facets.keys().cloned())
        .collect();
    keys.sort();
    keys.dedup();

    for key in keys {
        // Peer readings, excluding hosts whose collector could not answer.
        let readings: Vec<(&String, &FacetValue)> = peer_set
            .hosts
            .iter()
            .filter_map(|h| all.get(h).map(|f| (h, f)))
            .filter_map(|(h, f)| f.facets.get(&key).map(|v| (h, v)))
            .filter(|(_, v)| v.is_known())
            .collect();

        let known = readings.len();
        let my_value = mine
            .and_then(|f| f.facets.get(&key))
            .cloned()
            .unwrap_or_else(|| FacetValue::Missing("not collected".into()));

        if known == 0 {
            continue;
        }

        let scored = if key.is_numeric() {
            score_numeric(&my_value, &readings)
        } else {
            score_categorical(&my_value, &readings)
        };

        // Everyone who answered gave the same answer.
        if scored.agree == known && scored.severity == 0.0 {
            identical.push(key.clone());
            continue;
        }

        comparisons.push(FacetComparison {
            key,
            mine: my_value,
            consensus: scored.consensus,
            agree: scored.agree,
            known,
            severity: scored.severity,
            is_outlier: scored.is_outlier,
            distribution: scored.distribution,
        });
    }

    HostDivergence {
        host: host.to_string(),
        peer_set: peer_set.label(),
        comparisons,
        identical,
        unprobed_peers,
    }
}

struct Scored {
    consensus: Option<FacetValue>,
    agree: usize,
    severity: f64,
    is_outlier: bool,
    distribution: Option<Distribution>,
}

/// Categorical severity: `1.0 - (hosts sharing my value / hosts with a value)`.
/// Being the only host with a value scores 1.0 — you are alone.
///
/// A categorical facet flags as an outlier when this host does not hold the
/// modal value. Matching the majority is agreement even when the majority is
/// not unanimous.
fn score_categorical(mine: &FacetValue, readings: &[(&String, &FacetValue)]) -> Scored {
    let mut counts: HashMap<String, (usize, FacetValue)> = HashMap::new();
    for (_, v) in readings {
        if let Some(k) = v.group_key() {
            let e = counts.entry(k).or_insert((0, (*v).clone()));
            e.0 += 1;
        }
    }

    // Modal value. Ties break on the value itself so the answer is stable
    // across runs rather than depending on hash order.
    let consensus = counts
        .iter()
        .max_by(|a, b| a.1 .0.cmp(&b.1 .0).then_with(|| b.0.cmp(a.0)))
        .map(|(_, (_, v))| v.clone());

    let total = readings.len();
    let mine_count = mine
        .group_key()
        .and_then(|k| counts.get(&k).map(|(n, _)| *n))
        .unwrap_or(0);

    let severity = if total == 0 || mine_count == 0 {
        // We have no reading of our own; that is not divergence, it is absence.
        0.0
    } else {
        1.0 - (mine_count as f64 / total as f64)
    };

    let agree = consensus
        .as_ref()
        .and_then(|c| c.group_key())
        .and_then(|k| counts.get(&k).map(|(n, _)| *n))
        .unwrap_or(0);

    let is_outlier = mine.is_known()
        && consensus
            .as_ref()
            .map(|c| c.group_key() != mine.group_key())
            .unwrap_or(false);

    Scored {
        consensus,
        agree,
        severity,
        is_outlier,
        distribution: None,
    }
}

/// Numeric severity: distance from the peer median, normalised by the spread.
///
/// A host at the median scores 0. A host at either extreme of a spread set
/// scores near 1. When every peer holds the same number the spread is zero and
/// nobody diverges, which is the correct answer rather than a division by it.
///
/// **Outlier status is a separate question from severity**, and it uses the
/// Tukey fence — outside `[Q1 - 1.5·IQR, Q3 + 1.5·IQR]`. Without it, a fleet
/// whose disks run evenly from 40% to 79% reports all forty hosts as
/// diverging, which is both true and useless. The fence is the conventional
/// robust definition, so there is no tuned constant to defend.
fn score_numeric(mine: &FacetValue, readings: &[(&String, &FacetValue)]) -> Scored {
    let mut values: Vec<f64> = readings.iter().filter_map(|(_, v)| v.as_number()).collect();
    if values.is_empty() {
        return Scored {
            consensus: None,
            agree: 0,
            severity: 0.0,
            is_outlier: false,
            distribution: None,
        };
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let median = percentile_of(&values, 50.0);
    let min = values[0];
    let max = values[values.len() - 1];

    let my_num = match mine.as_number() {
        Some(n) => n,
        None => {
            return Scored {
                consensus: Some(FacetValue::Number(median)),
                agree: 0,
                severity: 0.0,
                is_outlier: false,
                distribution: None,
            }
        }
    };

    let spread = max - min;
    let severity = if spread <= f64::EPSILON {
        0.0
    } else {
        ((my_num - median).abs() / spread).clamp(0.0, 1.0)
    };

    // Tukey fence. Needs at least four readings to mean anything; below that
    // there is no distribution to be an outlier in.
    let is_outlier = if values.len() < 4 {
        false
    } else {
        let q1 = percentile_of(&values, 25.0);
        let q3 = percentile_of(&values, 75.0);
        let iqr = q3 - q1;
        if iqr <= f64::EPSILON {
            // Everyone agrees except possibly you: any difference is an outlier.
            (my_num - median).abs() > f64::EPSILON
        } else {
            my_num < q1 - 1.5 * iqr || my_num > q3 + 1.5 * iqr
        }
    };

    let below = values.iter().filter(|v| **v < my_num).count() as f64;
    let percentile = below / values.len() as f64 * 100.0;

    let agree = values
        .iter()
        .filter(|v| (**v - my_num).abs() < f64::EPSILON)
        .count();

    Scored {
        consensus: Some(FacetValue::Number(median)),
        agree,
        severity,
        is_outlier,
        distribution: Some(Distribution {
            median,
            percentile,
            min,
            max,
        }),
    }
}

fn percentile_of(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let rank = (p / 100.0) * (sorted.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        sorted[lo] + (sorted[hi] - sorted[lo]) * (rank - lo as f64)
    }
}

/// Fleet-wide agreement: what fraction of all facet-checks agree.
///
/// This replaces v1's `Online: 2 │ Offline: 8 │ 20%`, which counted pings.
/// The denominator is checks that produced a reading — unprobed hosts do not
/// silently drag the number down.
#[derive(Clone, Debug, PartialEq)]
pub struct Consensus {
    pub agreeing_checks: usize,
    pub total_checks: usize,
    pub diverging_facets: usize,
    pub diverging_hosts: usize,
    pub unprobed_hosts: usize,
}

impl Consensus {
    pub fn percent(&self) -> f64 {
        if self.total_checks == 0 {
            return 0.0;
        }
        self.agreeing_checks as f64 / self.total_checks as f64 * 100.0
    }
}

/// Summarise a peer set.
pub fn consensus(peer_set: &PeerSet, all: &HashMap<String, HostFacts>) -> Consensus {
    let mut agreeing = 0usize;
    let mut total = 0usize;
    let mut diverging_hosts = 0usize;
    let mut diverging_facets: Vec<FacetKey> = Vec::new();
    let mut unprobed = 0usize;

    for host in &peer_set.hosts {
        if !all.contains_key(host) {
            unprobed += 1;
            continue;
        }
        let d = compare(host, peer_set, all);
        let div = d.score();
        if div > 0 {
            diverging_hosts += 1;
        }
        for c in d.diverging() {
            diverging_facets.push(c.key.clone());
        }
        total += d.comparisons.len() + d.identical.len();
        agreeing += d.identical.len() + (d.comparisons.len() - div);
    }

    diverging_facets.sort();
    diverging_facets.dedup();

    Consensus {
        agreeing_checks: agreeing,
        total_checks: total,
        diverging_facets: diverging_facets.len(),
        diverging_hosts,
        unprobed_hosts: unprobed,
    }
}

/// Derive peer sets from host tags.
///
/// A tag shared by fewer than two hosts defines no comparison, so it is not a
/// peer set. Sets are ordered largest first, and `role` keys sort ahead of
/// other keys at equal size because that is the grouping operators mean when
/// they ask "are these the same?".
///
/// This is deliberately explicit rather than inferred: automatic peer-set
/// discovery is magical when it is right and baffling when it is wrong, and a
/// wrong peer set makes every number downstream wrong too.
pub fn peer_sets_from_tags(hosts: &[(String, Vec<(String, String)>)]) -> Vec<PeerSet> {
    let mut groups: HashMap<(String, String), Vec<String>> = HashMap::new();
    for (host, tags) in hosts {
        for (k, v) in tags {
            if v.is_empty() {
                continue;
            }
            groups
                .entry((k.clone(), v.clone()))
                .or_default()
                .push(host.clone());
        }
    }

    let mut sets: Vec<PeerSet> = groups
        .into_iter()
        .filter(|(_, members)| members.len() >= 2)
        .map(|(selector, mut hosts)| {
            hosts.sort();
            PeerSet { selector, hosts }
        })
        .collect();

    sets.sort_by(|a, b| {
        b.hosts
            .len()
            .cmp(&a.hosts.len())
            .then_with(|| (a.selector.0 != "role").cmp(&(b.selector.0 != "role")))
            .then_with(|| a.selector.cmp(&b.selector))
    });
    sets
}

/// A peer set's headline: does it agree, and if not, who breaks it.
#[derive(Clone, Debug)]
pub struct GroupSummary {
    pub label: String,
    pub host_count: usize,
    pub at_consensus: usize,
    pub unprobed: usize,
    /// Hosts that break consensus, and the facets they break it on.
    pub breakers: Vec<(String, Vec<String>)>,
}

impl GroupSummary {
    /// The line the GROUPS panel shows. This is where a list of green dots is
    /// replaced by an actual statement.
    pub fn note(&self) -> String {
        if self.host_count == self.unprobed {
            return format!(
                "{} hosts, none probed — nothing to compare",
                self.host_count
            );
        }
        if self.breakers.is_empty() {
            let mut s = format!("{} / {} full consensus", self.at_consensus, self.probed());
            if self.unprobed > 0 {
                s.push_str(&format!(" · {} never probed", self.unprobed));
            }
            return s;
        }
        let (host, facets) = &self.breakers[0];

        // Name at most three facets. Listing six runs off the edge of the
        // panel and gets clipped mid-path, which is the same failure as
        // truncating `region=us-east-1` to `region=us-ea` — the clipped text
        // reads as the whole value.
        const MAX_FACETS: usize = 3;
        let shown: Vec<&str> = facets.iter().take(MAX_FACETS).map(|s| s.as_str()).collect();
        let mut list = shown.join(" + ");
        if facets.len() > MAX_FACETS {
            list.push_str(&format!(" +{} more", facets.len() - MAX_FACETS));
        }

        let extra = if self.breakers.len() > 1 {
            format!(", and {} other hosts", self.breakers.len() - 1)
        } else {
            String::new()
        };
        format!("{} breaks consensus on {}{}", host, list, extra)
    }

    pub fn probed(&self) -> usize {
        self.host_count - self.unprobed
    }
}

/// Summarise every peer set for the GROUPS panel.
pub fn summarise_groups(sets: &[PeerSet], all: &HashMap<String, HostFacts>) -> Vec<GroupSummary> {
    sets.iter()
        .map(|set| {
            let mut breakers = Vec::new();
            let mut at_consensus = 0;
            let mut unprobed = 0;

            for host in &set.hosts {
                if !all.contains_key(host) {
                    unprobed += 1;
                    continue;
                }
                let d = compare(host, set, all);
                let diverging = d.diverging();
                if diverging.is_empty() {
                    at_consensus += 1;
                } else {
                    let facets: Vec<String> = diverging.iter().map(|c| c.key.label()).collect();
                    breakers.push((host.clone(), facets));
                }
            }

            // Worst first, so the panel leads with the biggest problem.
            breakers.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));

            GroupSummary {
                label: set.label(),
                host_count: set.hosts.len(),
                at_consensus,
                unprobed,
                breakers,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end divergence over real SSH.
    ///
    /// Collects facts from a live host twice, perturbs one copy the way a real
    /// drifted host would be, and asserts the engine names the right one.
    /// Run with:
    ///
    /// ```sh
    /// ESSH_LIVE_SSH=localhost ESSH_LIVE_KEY=~/.ssh/id_ed25519 \
    ///   cargo test --bin essh live_divergence -- --ignored --nocapture
    /// ```
    #[ignore = "needs a reachable sshd; set ESSH_LIVE_SSH=<host>"]
    #[tokio::test]
    async fn live_divergence_finds_the_drifted_host() {
        let host = match std::env::var("ESSH_LIVE_SSH") {
            Ok(h) if !h.is_empty() => h,
            _ => return,
        };
        let auth = match std::env::var("ESSH_LIVE_KEY") {
            Ok(p) if !p.is_empty() => crate::ssh::AuthMethod::KeyFile {
                path: std::path::PathBuf::from(p),
                passphrase: None,
            },
            _ => crate::ssh::AuthMethod::Agent,
        };
        let cfg = crate::ssh::ConnectConfig {
            hostname: host.clone(),
            port: 22,
            username: std::env::var("USER").unwrap_or_else(|_| "root".into()),
            auth,
            timeout: std::time::Duration::from_secs(5),
        };
        let (session, _, _) = crate::ssh::SshClient::connect(&cfg)
            .await
            .expect("connect to the live host");

        let platform = crate::monitor::Platform::from_uname(
            &crate::divergence::collect::probe_uname(&session.handle)
                .await
                .unwrap_or_default(),
        );

        let base = collect::collect_facts(
            &session.handle,
            "web-01",
            &platform,
            &["/etc/ssh/sshd_config".to_string()],
            &[],
        )
        .await;

        // Four hosts built from the same real machine, so every facet is
        // genuine. Then move one host's kernel back a release, which is the
        // canonical drift case.
        let mut all = HashMap::new();
        for name in ["web-01", "web-02", "web-03"] {
            let mut f = base.clone();
            f.host = name.to_string();
            all.insert(name.to_string(), f);
        }
        let mut drifted = base.clone();
        drifted.host = "web-04".into();
        let real_kernel = drifted
            .facets
            .get(&FacetKey::Kernel)
            .expect("kernel collected from the live host")
            .as_display();
        drifted
            .facets
            .insert(FacetKey::Kernel, FacetValue::Text("1.0.0-old".into()));
        all.insert("web-04".into(), drifted);

        let set = PeerSet {
            selector: ("role".into(), "web".into()),
            hosts: vec![
                "web-01".into(),
                "web-02".into(),
                "web-03".into(),
                "web-04".into(),
            ],
        };

        // The three matching hosts must be silent.
        for h in ["web-01", "web-02", "web-03"] {
            let d = compare(h, &set, &all);
            assert_eq!(d.score(), 0, "{} should agree with its peers", h);
        }

        // The drifted one must be named, on the kernel facet, alone.
        let d = compare("web-04", &set, &all);
        assert_eq!(d.score(), 1, "exactly one facet should differ");
        let k = d.diverging()[0];
        assert_eq!(k.key, FacetKey::Kernel);
        assert_eq!(k.agree, 3);
        assert_eq!(k.known, 4);
        assert!((k.severity - 0.75).abs() < 1e-9, "got {}", k.severity);

        let v = verdict_for(&d).expect("a verdict");
        eprintln!("verdict [{}]: {}", v.pattern, v.text);
        assert!(
            v.text.contains(&real_kernel),
            "verdict must cite the peers' kernel"
        );

        // And the group summary must name the host, not just count it.
        let g = &summarise_groups(std::slice::from_ref(&set), &all)[0];
        eprintln!("group note: {}", g.note());
        assert_eq!(g.at_consensus, 3);
        assert_eq!(g.breakers.len(), 1);
        assert_eq!(g.breakers[0].0, "web-04");

        let c = consensus(&set, &all);
        eprintln!(
            "consensus: {:.1}% of {} checks agree, {} facets diverge across {} hosts",
            c.percent(),
            c.total_checks,
            c.diverging_facets,
            c.diverging_hosts
        );
        assert_eq!(c.diverging_hosts, 1);
        assert!(c.percent() > 90.0 && c.percent() < 100.0);
    }

    fn text(s: &str) -> FacetValue {
        FacetValue::Text(s.to_string())
    }

    /// 39 hosts on one kernel, 1 on another — the scenario the feature exists
    /// for.
    fn forty_web_servers() -> (PeerSet, HashMap<String, HostFacts>) {
        let mut all = HashMap::new();
        let mut hosts = Vec::new();
        for i in 0..40 {
            let name = format!("web-{:02}", i);
            let kernel = if i == 7 { "6.1.0-15" } else { "6.1.0-18" };
            // Disks cluster in the 40s, as a real fleet's would, with one host
            // genuinely full. Normal variation must not read as divergence.
            let disk = if i == 39 { 95.0 } else { 40.0 + (i % 6) as f64 };
            all.insert(
                name.clone(),
                HostFacts::new(&name)
                    .with(FacetKey::Kernel, text(kernel))
                    .with(FacetKey::OsRelease, text("debian 12"))
                    .with(FacetKey::DiskRootPct, FacetValue::Number(disk)),
            );
            hosts.push(name);
        }
        (
            PeerSet {
                selector: ("role".into(), "web".into()),
                hosts,
            },
            all,
        )
    }

    #[test]
    fn the_odd_host_out_is_found_and_the_other_39_are_quiet() {
        let (set, all) = forty_web_servers();

        let odd = compare("web-07", &set, &all);
        let kernel = odd
            .diverging()
            .into_iter()
            .find(|c| c.key == FacetKey::Kernel)
            .expect("web-07 must diverge on kernel");
        // Alone against 39 others: severity is 1 - 1/40.
        assert!((kernel.severity - (1.0 - 1.0 / 40.0)).abs() < 1e-9);
        assert_eq!(kernel.agree, 39);
        assert_eq!(kernel.known, 40);

        let normal = compare("web-08", &set, &all);
        assert!(
            !normal.diverging().iter().any(|c| c.key == FacetKey::Kernel),
            "a host at consensus must not be flagged"
        );
    }

    #[test]
    fn agreement_collapses_instead_of_taking_a_row() {
        let (set, all) = forty_web_servers();
        let d = compare("web-08", &set, &all);
        // os release is identical everywhere, so it never becomes a comparison.
        assert!(d.identical.contains(&FacetKey::OsRelease));
        assert!(!d.comparisons.iter().any(|c| c.key == FacetKey::OsRelease));
    }

    #[test]
    fn numeric_facets_report_the_peer_distribution_not_just_a_number() {
        let (set, all) = forty_web_servers();
        // web-39 has the fullest disk in the set.
        let d = compare("web-39", &set, &all);
        let disk = d
            .comparisons
            .iter()
            .find(|c| c.key == FacetKey::DiskRootPct)
            .expect("disk comparison");
        let dist = disk.distribution.as_ref().expect("distribution");
        assert!((dist.min - 40.0).abs() < 1e-9);
        assert!((dist.max - 95.0).abs() < 1e-9);
        assert!((dist.percentile - 97.5).abs() < 0.1);
        assert!(
            disk.is_outlier,
            "95% against a fleet in the 40s is an outlier"
        );
        // The summary carries the context that makes the number a finding.
        assert!(disk.summary().contains("median"), "{}", disk.summary());
        assert!(disk.summary().contains("p98"), "{}", disk.summary());
    }

    #[test]
    fn ordinary_numeric_variation_is_not_divergence() {
        // The bug this test exists for: with `severity > 0` as the flag, a
        // fleet whose disks vary normally reported every host as diverging.
        let (set, all) = forty_web_servers();
        let ordinary = compare("web-12", &set, &all);
        assert!(
            !ordinary
                .diverging()
                .iter()
                .any(|c| c.key == FacetKey::DiskRootPct),
            "a host inside the normal spread must not be flagged"
        );
    }

    #[test]
    fn a_host_at_the_median_does_not_diverge_numerically() {
        let mut all = HashMap::new();
        for (i, name) in ["a", "b", "c"].iter().enumerate() {
            all.insert(
                name.to_string(),
                HostFacts::new(*name)
                    .with(FacetKey::DiskRootPct, FacetValue::Number(50.0 + i as f64)),
            );
        }
        let set = PeerSet {
            selector: ("role".into(), "db".into()),
            hosts: vec!["a".into(), "b".into(), "c".into()],
        };
        let d = compare("b", &set, &all); // b = 51, the median
        let disk = d
            .comparisons
            .iter()
            .find(|c| c.key == FacetKey::DiskRootPct);
        assert!(
            disk.map(|c| c.severity).unwrap_or(0.0) < 1e-9,
            "the median host must not be flagged"
        );
    }

    #[test]
    fn identical_numbers_across_the_set_produce_no_divergence() {
        // Guards the zero-spread division.
        let mut all = HashMap::new();
        for name in ["a", "b", "c"] {
            all.insert(
                name.to_string(),
                HostFacts::new(name).with(FacetKey::CpuCount, FacetValue::Number(8.0)),
            );
        }
        let set = PeerSet {
            selector: ("role".into(), "web".into()),
            hosts: vec!["a".into(), "b".into(), "c".into()],
        };
        let d = compare("a", &set, &all);
        assert!(d.identical.contains(&FacetKey::CpuCount));
        assert_eq!(d.score(), 0);
    }

    #[test]
    fn an_unprobed_host_is_not_a_diverging_host() {
        let (mut set, mut all) = forty_web_servers();
        set.hosts.push("web-99".into());
        all.remove("web-07"); // now unprobed rather than odd

        let d = compare("web-99", &set, &all);
        assert!(!d.is_probed(), "no facts means unprobed");
        assert_eq!(d.score(), 0, "unprobed must not read as divergence");
        assert!(d.unprobed_peers.contains(&"web-99".to_string()));
        assert!(d.unprobed_peers.contains(&"web-07".to_string()));

        let c = consensus(&set, &all);
        assert_eq!(c.unprobed_hosts, 2);
        // web-07 (the kernel outlier) is gone; only the full disk remains.
        assert_eq!(c.diverging_hosts, 1);
    }

    #[test]
    fn a_host_that_could_not_answer_is_excluded_from_the_denominator() {
        let mut all = HashMap::new();
        all.insert(
            "a".into(),
            HostFacts::new("a").with(FacetKey::FileHash("/etc/nginx.conf".into()), text("aaa")),
        );
        all.insert(
            "b".into(),
            HostFacts::new("b").with(FacetKey::FileHash("/etc/nginx.conf".into()), text("aaa")),
        );
        all.insert(
            "c".into(),
            HostFacts::new("c").with(
                FacetKey::FileHash("/etc/nginx.conf".into()),
                FacetValue::Missing("permission denied".into()),
            ),
        );
        let set = PeerSet {
            selector: ("role".into(), "web".into()),
            hosts: vec!["a".into(), "b".into(), "c".into()],
        };

        // a and b agree; c could not read the file. c must not make them look
        // like a 2-of-3 majority.
        let d = compare("a", &set, &all);
        assert!(d
            .identical
            .contains(&FacetKey::FileHash("/etc/nginx.conf".into())));

        // And c itself is not "diverging" — it is unreadable, and says so.
        let dc = compare("c", &set, &all);
        let cmp = dc
            .comparisons
            .iter()
            .find(|x| matches!(x.key, FacetKey::FileHash(_)));
        assert!(
            cmp.map(|c| c.severity).unwrap_or(0.0) < 1e-9,
            "unreadable is not disagreement"
        );
    }

    #[test]
    fn consensus_counts_checks_not_pings() {
        let (set, all) = forty_web_servers();
        let c = consensus(&set, &all);
        // web-07 on kernel, web-39 on disk. Nobody else.
        assert_eq!(c.diverging_hosts, 2);
        assert!(c.total_checks > 40, "every host contributes several checks");
        assert!(
            c.percent() > 80.0 && c.percent() < 100.0,
            "got {}",
            c.percent()
        );
    }

    #[test]
    fn consensus_is_stable_across_runs() {
        // Modal selection must not depend on HashMap iteration order.
        let (set, all) = forty_web_servers();
        let first = compare("web-07", &set, &all);
        for _ in 0..20 {
            let again = compare("web-07", &set, &all);
            assert_eq!(first.score(), again.score());
            let a = first.diverging()[0].consensus.clone();
            let b = again.diverging()[0].consensus.clone();
            assert_eq!(a, b);
        }
    }

    #[test]
    fn a_tag_held_by_one_host_is_not_a_peer_set() {
        let hosts = vec![
            (
                "a".to_string(),
                vec![
                    ("role".to_string(), "web".to_string()),
                    ("env".to_string(), "prod".to_string()),
                ],
            ),
            (
                "b".to_string(),
                vec![
                    ("role".to_string(), "web".to_string()),
                    ("env".to_string(), "prod".to_string()),
                ],
            ),
            (
                "c".to_string(),
                vec![("role".to_string(), "bastion".to_string())],
            ),
        ];
        let sets = peer_sets_from_tags(&hosts);
        assert!(
            !sets.iter().any(|s| s.label() == "role=bastion"),
            "a lone host has no peers to diverge from"
        );
        assert_eq!(sets.len(), 2, "role=web and env=prod");
        // Ties on size put role first, because that is the grouping operators mean.
        assert_eq!(sets[0].selector.0, "role");
    }

    #[test]
    fn peer_sets_are_ordered_largest_first_and_are_stable() {
        let mk = |n: &str, role: &str| {
            (
                n.to_string(),
                vec![
                    ("role".to_string(), role.to_string()),
                    ("env".to_string(), "prod".to_string()),
                ],
            )
        };
        let hosts = vec![mk("a", "web"), mk("b", "web"), mk("c", "db"), mk("d", "db")];
        let first = peer_sets_from_tags(&hosts);
        assert_eq!(first[0].label(), "env=prod", "4 hosts beats 2");
        for _ in 0..10 {
            let again = peer_sets_from_tags(&hosts);
            let a: Vec<String> = first.iter().map(|s| s.label()).collect();
            let b: Vec<String> = again.iter().map(|s| s.label()).collect();
            assert_eq!(a, b, "peer set order must not depend on hash iteration");
        }
    }

    #[test]
    fn group_summary_names_the_host_that_breaks_consensus() {
        let (set, all) = forty_web_servers();
        let summaries = summarise_groups(&[set], &all);
        let g = &summaries[0];
        assert_eq!(g.host_count, 40);
        assert_eq!(g.unprobed, 0);
        // web-07 (kernel) and web-39 (disk) break it; nobody else.
        assert_eq!(g.breakers.len(), 2);
        assert_eq!(g.at_consensus, 38);
        assert!(g.note().contains("breaks consensus"), "{}", g.note());
    }

    #[test]
    fn a_group_note_caps_the_facet_list_instead_of_being_clipped() {
        // Real case from the demo fleet: web-03 diverged on six facets and the
        // note ran off the panel, ending mid-path at "/etc/s".
        let g = GroupSummary {
            label: "role=web".into(),
            host_count: 3,
            at_consensus: 2,
            unprobed: 0,
            breakers: vec![(
                "web-03".into(),
                vec![
                    "os release".into(),
                    "openssl".into(),
                    "pkg nginx".into(),
                    "pkg openssh-server".into(),
                    "/etc/nginx/nginx.conf".into(),
                    "/etc/ssh/sshd_config".into(),
                ],
            )],
        };
        let note = g.note();
        assert!(note.contains("+3 more"), "{}", note);
        assert!(note.len() < 90, "note is {} chars: {}", note.len(), note);
        assert!(note.starts_with("web-03 breaks consensus on"), "{}", note);
    }

    #[test]
    fn a_group_nobody_probed_says_so_rather_than_claiming_consensus() {
        let set = PeerSet {
            selector: ("role".into(), "web".into()),
            hosts: vec!["a".into(), "b".into()],
        };
        let empty = HashMap::new();
        let g = &summarise_groups(&[set], &empty)[0];
        assert_eq!(g.unprobed, 2);
        assert!(
            g.note().contains("nothing to compare"),
            "an unprobed group must not read as agreement: {}",
            g.note()
        );
    }

    #[test]
    fn a_group_in_full_agreement_says_so_quietly() {
        let mut all = HashMap::new();
        for h in ["a", "b", "c"] {
            all.insert(
                h.to_string(),
                HostFacts::new(h).with(FacetKey::Kernel, text("6.1.0-18")),
            );
        }
        let set = PeerSet {
            selector: ("role".into(), "web".into()),
            hosts: vec!["a".into(), "b".into(), "c".into()],
        };
        let g = &summarise_groups(&[set], &all)[0];
        assert_eq!(g.breakers.len(), 0);
        assert!(g.note().contains("full consensus"), "{}", g.note());
    }

    #[test]
    fn a_two_host_split_names_exactly_one_side() {
        // With 2 hosts disagreeing 1-1 each sits at severity 0.5, but only one
        // can hold the consensus. Which one is decided deterministically, so
        // the report does not flicker between runs.
        let mut all = HashMap::new();
        all.insert(
            "a".into(),
            HostFacts::new("a").with(FacetKey::Kernel, text("6.1")),
        );
        all.insert(
            "b".into(),
            HostFacts::new("b").with(FacetKey::Kernel, text("6.2")),
        );
        let set = PeerSet {
            selector: ("role".into(), "web".into()),
            hosts: vec!["a".into(), "b".into()],
        };

        let da = compare("a", &set, &all);
        let db = compare("b", &set, &all);

        for d in [&da, &db] {
            let k = d
                .comparisons
                .iter()
                .find(|c| c.key == FacetKey::Kernel)
                .expect("kernel comparison");
            assert!((k.severity - 0.5).abs() < 1e-9, "got {}", k.severity);
        }

        assert_eq!(
            da.score() + db.score(),
            1,
            "exactly one side of a 1-1 split is the outlier"
        );
    }
}
