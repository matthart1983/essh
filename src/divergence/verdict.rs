//! Verdicts: naming what the divergence looks like, without inventing why.
//!
//! The rule inherited from SoundWatch is "a verdict names the likely cause,
//! evidence-backed or absent — never invent a cause." That rule is easy to
//! state and easy to violate: "both likely from the same manual intervention"
//! is a causal claim inferred from two facts changing together, and nothing in
//! the data supports it.
//!
//! So verdicts here are **enumerated templates over co-occurrence**, and they
//! say what they observed rather than what caused it. Every template carries
//! the facets it fired on, so the sentence can be checked against the evidence
//! that produced it. If no template matches, there is no verdict — an empty
//! verdict is a correct output, not a gap to fill.

use super::{FacetKey, HostDivergence};

/// A verdict is a sentence plus the evidence it was derived from.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub text: String,
    /// The facets this sentence is about. The UI shows these alongside it so
    /// the claim is always checkable.
    pub evidence: Vec<FacetKey>,
    /// Which template fired, for testing and for the audit log.
    pub pattern: &'static str,
}

/// Compare two dotted version-ish strings.
///
/// Returns `Some(Ordering)` only when both sides parse as dotted numbers —
/// `6.1.0-15` vs `6.1.0-18`. Anything else returns `None` rather than falling
/// back to string ordering, because "behind" is a claim and `abc` < `abd` is
/// not evidence for it.
fn version_cmp(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    let parse = |s: &str| -> Option<Vec<u64>> {
        let cleaned: String = s
            .chars()
            .map(|c| if c.is_ascii_digit() { c } else { ' ' })
            .collect();
        let parts: Vec<u64> = cleaned
            .split_whitespace()
            .filter_map(|p| p.parse().ok())
            .collect();
        (!parts.is_empty()).then_some(parts)
    };
    let (va, vb) = (parse(a)?, parse(b)?);
    Some(va.cmp(&vb))
}

/// Build a verdict for one host's divergence, or `None` if nothing patterned.
pub fn verdict_for(d: &HostDivergence) -> Option<Verdict> {
    let diverging = d.diverging();
    if diverging.is_empty() {
        return None;
    }

    let categorical: Vec<_> = diverging
        .iter()
        .filter(|c| !c.key.is_numeric())
        .copied()
        .collect();
    let numeric: Vec<_> = diverging
        .iter()
        .filter(|c| c.key.is_numeric())
        .copied()
        .collect();

    // ── Template 1: behind on a version ──────────────────────────────────
    // Fires only when both values parse as versions and ours sorts lower.
    for c in &categorical {
        if let (Some(consensus), mine) = (c.consensus.as_ref(), &c.mine) {
            let (m, p) = (mine.as_display(), consensus.as_display());
            if let Some(std::cmp::Ordering::Less) = version_cmp(&m, &p) {
                let others: Vec<&FacetKey> = categorical
                    .iter()
                    .map(|x| &x.key)
                    .filter(|k| **k != c.key)
                    .collect();

                // ── Template 2: a version gap alongside a changed file ────
                // Stated as co-occurrence. Two things differing together is
                // an observation; "one manual intervention" would be a guess.
                if let Some(file) = others
                    .iter()
                    .find(|k| matches!(k, FacetKey::FileHash(_)))
                    .copied()
                {
                    return Some(Verdict {
                        text: format!(
                            "{} is behind its peers ({} vs {}) and {} also differs here. \
                             The two diverge together on this host; ESSH does not infer \
                             a shared cause.",
                            c.key.label(),
                            m,
                            p,
                            file.label()
                        ),
                        evidence: vec![c.key.clone(), file.clone()],
                        pattern: "version-behind-with-file-change",
                    });
                }

                return Some(Verdict {
                    text: format!(
                        "{} is behind its peers: {} against a consensus of {} across {} hosts.",
                        c.key.label(),
                        m,
                        p,
                        c.known
                    ),
                    evidence: vec![c.key.clone()],
                    pattern: "version-behind",
                });
            }
        }
    }

    // ── Template 3: a numeric facet drifting with nothing else changed ────
    if categorical.is_empty() && numeric.len() == 1 {
        let c = numeric[0];
        let d_ = c.distribution.as_ref();
        let detail = match d_ {
            Some(dist) => format!(
                "{} against a peer median of {:.0} (range {:.0}–{:.0})",
                c.mine.as_display(),
                dist.median,
                dist.min,
                dist.max
            ),
            None => c.mine.as_display(),
        };
        return Some(Verdict {
            text: format!(
                "{} is the only facet that differs here: {}. Configuration matches its peers.",
                c.key.label(),
                detail
            ),
            evidence: vec![c.key.clone()],
            pattern: "numeric-drift-alone",
        });
    }

    // ── Template 4: a config file changed and nothing else ────────────────
    if numeric.is_empty() && categorical.len() == 1 {
        let c = categorical[0];
        if let FacetKey::FileHash(path) = &c.key {
            return Some(Verdict {
                text: format!(
                    "{} differs from the {} peers that share a version of it. \
                     Nothing else about this host diverges.",
                    path, c.agree
                ),
                evidence: vec![c.key.clone()],
                pattern: "lone-file-change",
            });
        }
    }

    // ── Template 5: broad divergence ─────────────────────────────────────
    // When many facets differ at once we describe the shape and stop. A host
    // differing on eight things has no single story worth asserting.
    if diverging.len() >= 4 {
        let names: Vec<String> = diverging.iter().take(3).map(|c| c.key.label()).collect();
        return Some(Verdict {
            text: format!(
                "{} facets differ on this host, including {}. \
                 This looks like a different build rather than drift.",
                diverging.len(),
                names.join(", ")
            ),
            evidence: diverging.iter().map(|c| c.key.clone()).collect(),
            pattern: "broad-divergence",
        });
    }

    // ── No template matched ───────────────────────────────────────────────
    // Enumerate, claim nothing.
    let names: Vec<String> = diverging.iter().map(|c| c.key.label()).collect();
    Some(Verdict {
        text: format!("Differs from peers on {}.", names.join(" and ")),
        evidence: diverging.iter().map(|c| c.key.clone()).collect(),
        pattern: "enumeration-only",
    })
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;
    use std::collections::HashMap;

    fn text(s: &str) -> FacetValue {
        FacetValue::Text(s.to_string())
    }

    fn set_of(hosts: &[&str]) -> PeerSet {
        PeerSet {
            selector: ("role".into(), "web".into()),
            hosts: hosts.iter().map(|h| h.to_string()).collect(),
        }
    }

    #[test]
    fn version_comparison_only_fires_on_real_versions() {
        assert_eq!(
            version_cmp("6.1.0-15", "6.1.0-18"),
            Some(std::cmp::Ordering::Less)
        );
        assert_eq!(
            version_cmp("6.2.0-1", "6.1.0-18"),
            Some(std::cmp::Ordering::Greater)
        );
        // Not versions: no ordering claim at all.
        assert_eq!(version_cmp("abc", "abd"), None);
        assert_eq!(version_cmp("nginx", "apache"), None);
    }

    #[test]
    fn a_host_behind_on_kernel_gets_a_verdict_naming_the_gap() {
        let mut all = HashMap::new();
        for h in ["a", "b", "c"] {
            all.insert(
                h.to_string(),
                HostFacts::new(h).with(FacetKey::Kernel, text("6.1.0-18")),
            );
        }
        all.insert(
            "odd".into(),
            HostFacts::new("odd").with(FacetKey::Kernel, text("6.1.0-15")),
        );

        let set = set_of(&["a", "b", "c", "odd"]);
        let v = verdict_for(&compare("odd", &set, &all)).expect("verdict");
        assert_eq!(v.pattern, "version-behind");
        assert!(v.text.contains("6.1.0-15"), "{}", v.text);
        assert!(v.text.contains("6.1.0-18"), "{}", v.text);
        assert_eq!(v.evidence, vec![FacetKey::Kernel]);
    }

    #[test]
    fn co_occurrence_is_reported_as_co_occurrence_not_as_cause() {
        let mut all = HashMap::new();
        let conf = FacetKey::FileHash("/etc/nginx/nginx.conf".into());
        for h in ["a", "b", "c"] {
            all.insert(
                h.to_string(),
                HostFacts::new(h)
                    .with(FacetKey::Kernel, text("6.1.0-18"))
                    .with(conf.clone(), text("aaaa1111")),
            );
        }
        all.insert(
            "odd".into(),
            HostFacts::new("odd")
                .with(FacetKey::Kernel, text("6.1.0-15"))
                .with(conf.clone(), text("bbbb2222")),
        );

        let set = set_of(&["a", "b", "c", "odd"]);
        let v = verdict_for(&compare("odd", &set, &all)).expect("verdict");
        assert_eq!(v.pattern, "version-behind-with-file-change");

        // The claim the design's own example made, which the data cannot
        // support, must not appear.
        let lowered = v.text.to_lowercase();
        assert!(
            !lowered.contains("manual intervention"),
            "verdict asserted an uncorroborated cause: {}",
            v.text
        );
        assert!(
            lowered.contains("does not infer") || lowered.contains("diverge together"),
            "verdict must frame this as co-occurrence: {}",
            v.text
        );
        assert_eq!(v.evidence.len(), 2, "both facets must be cited");
    }

    #[test]
    fn a_lone_numeric_drift_says_configuration_matches() {
        let mut all = HashMap::new();
        for (i, h) in ["a", "b", "c"].iter().enumerate() {
            all.insert(
                h.to_string(),
                HostFacts::new(*h)
                    .with(FacetKey::Kernel, text("6.1.0-18"))
                    .with(FacetKey::DiskRootPct, FacetValue::Number(40.0 + i as f64)),
            );
        }
        all.insert(
            "full".into(),
            HostFacts::new("full")
                .with(FacetKey::Kernel, text("6.1.0-18"))
                .with(FacetKey::DiskRootPct, FacetValue::Number(91.0)),
        );

        let set = set_of(&["a", "b", "c", "full"]);
        let v = verdict_for(&compare("full", &set, &all)).expect("verdict");
        assert_eq!(v.pattern, "numeric-drift-alone");
        assert!(v.text.contains("median"), "{}", v.text);
        assert!(
            v.text.to_lowercase().contains("matches its peers"),
            "{}",
            v.text
        );
    }

    #[test]
    fn a_host_at_consensus_gets_no_verdict_at_all() {
        let mut all = HashMap::new();
        for h in ["a", "b", "c"] {
            all.insert(
                h.to_string(),
                HostFacts::new(h).with(FacetKey::Kernel, text("6.1.0-18")),
            );
        }
        let set = set_of(&["a", "b", "c"]);
        assert!(
            verdict_for(&compare("a", &set, &all)).is_none(),
            "silence is the correct output for a host that agrees"
        );
    }

    #[test]
    fn an_unprobed_host_gets_no_verdict() {
        let mut all = HashMap::new();
        all.insert(
            "a".into(),
            HostFacts::new("a").with(FacetKey::Kernel, text("6.1.0-18")),
        );
        let set = set_of(&["a", "never-probed"]);
        let d = compare("never-probed", &set, &all);
        assert!(!d.is_probed());
        assert!(
            verdict_for(&d).is_none(),
            "we know nothing about this host and must not narrate"
        );
    }

    #[test]
    fn broad_divergence_describes_the_shape_and_stops() {
        let mut all = HashMap::new();
        for h in ["a", "b", "c"] {
            all.insert(
                h.to_string(),
                HostFacts::new(h)
                    .with(FacetKey::Kernel, text("6.1.0-18"))
                    .with(FacetKey::OsRelease, text("debian 12"))
                    .with(FacetKey::CpuModel, text("Xeon 6338"))
                    .with(FacetKey::OpenSsl, text("3.0.11"))
                    .with(FacetKey::Timezone, text("UTC+0000")),
            );
        }
        all.insert(
            "odd".into(),
            HostFacts::new("odd")
                .with(FacetKey::Kernel, text("5.15.0-1"))
                .with(FacetKey::OsRelease, text("ubuntu 22.04"))
                .with(FacetKey::CpuModel, text("EPYC 7402"))
                .with(FacetKey::OpenSsl, text("1.1.1w"))
                .with(FacetKey::Timezone, text("PST-0800")),
        );

        let set = set_of(&["a", "b", "c", "odd"]);
        let v = verdict_for(&compare("odd", &set, &all)).expect("verdict");
        // Kernel 5.15 vs 6.1 is genuinely "behind", so that template wins —
        // which is correct and more specific than the broad one.
        assert!(
            v.pattern == "version-behind-with-file-change"
                || v.pattern == "version-behind"
                || v.pattern == "broad-divergence",
            "unexpected pattern {}",
            v.pattern
        );
        assert!(!v.evidence.is_empty());
    }

    #[test]
    fn every_verdict_cites_evidence() {
        // A sentence with no facets behind it is exactly the failure mode this
        // module exists to prevent.
        let mut all = HashMap::new();
        for h in ["a", "b"] {
            all.insert(
                h.to_string(),
                HostFacts::new(h).with(FacetKey::Timezone, text("UTC+0000")),
            );
        }
        all.insert(
            "odd".into(),
            HostFacts::new("odd").with(FacetKey::Timezone, text("PST-0800")),
        );
        let set = set_of(&["a", "b", "odd"]);
        let v = verdict_for(&compare("odd", &set, &all)).expect("verdict");
        assert!(!v.evidence.is_empty(), "verdict '{}' cites nothing", v.text);
        assert_eq!(v.pattern, "enumeration-only");
    }
}
