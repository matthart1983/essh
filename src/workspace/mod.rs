//! Workspaces: a saved set of sessions and layout.
//!
//! §4 is explicit that ESSH *"does not pretend to provide server-side session
//! persistence"*, and that honesty is right. But it leaves the feature worth
//! less than it sounds: restoring a workspace gives you four fresh shells
//! sitting in `$HOME`, which is `ssh` four times in a loop.
//!
//! So each session may carry an `on_connect` command. Setting it to
//! `tmux new -A -s essh` makes restore actually restore *work*, using the
//! tool the spec already says to use for persistence. ESSH is not
//! reimplementing tmux; it is wiring it up.
//!
//! Two other things the spec does not say and a real implementation must:
//!
//! * **Restore is partial by default.** Eight hosts where three are down
//!   should give you five working sessions and a clear report, not an error.
//! * **Failures carry a diagnosis.** A workspace that will not restore is
//!   exactly when the ladder from [`crate::diagnose`] is most useful.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How the restored sessions are arranged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// One session per tab.
    #[default]
    Tabs,
    /// Side by side.
    VerticalSplit,
    /// Stacked.
    HorizontalSplit,
}

/// One session in a workspace.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSession {
    /// The alias to connect to — resolved through ssh_config like any other.
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Run on connect. The point of the feature: `tmux new -A -s essh`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_connect: Option<String>,
}

impl WorkspaceSession {
    // Constructed from disk in practice; kept for callers building one fresh.
    #[allow(dead_code)]
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            ..Default::default()
        }
    }
}

/// A named collection of sessions.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub name: String,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub sessions: Vec<WorkspaceSession>,
    /// Human note, e.g. "the prod incident set".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("no workspace named {0}")]
    NotFound(String),
    #[error("workspace names may not contain path separators: {0}")]
    UnsafeName(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not parse workspace: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("could not serialise workspace: {0}")]
    Serialize(#[from] toml::ser::Error),
}

/// Reject names that would escape the workspace directory.
///
/// A workspace name becomes a filename, so `../../.ssh/authorized_keys` must
/// not be one.
fn validate_name(name: &str) -> Result<(), WorkspaceError> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.starts_with('.')
    {
        return Err(WorkspaceError::UnsafeName(name.to_string()));
    }
    Ok(())
}

impl Workspace {
    pub fn path_in(dir: &Path, name: &str) -> PathBuf {
        dir.join(format!("{}.toml", name))
    }

    pub fn save_in(&self, dir: &Path) -> Result<PathBuf, WorkspaceError> {
        validate_name(&self.name)?;
        std::fs::create_dir_all(dir)?;
        let path = Self::path_in(dir, &self.name);
        std::fs::write(&path, toml::to_string_pretty(self)?)?;
        Ok(path)
    }

    pub fn load_from(dir: &Path, name: &str) -> Result<Self, WorkspaceError> {
        validate_name(name)?;
        let path = Self::path_in(dir, name);
        if !path.exists() {
            return Err(WorkspaceError::NotFound(name.to_string()));
        }
        Ok(toml::from_str(&std::fs::read_to_string(path)?)?)
    }

    pub fn delete_in(dir: &Path, name: &str) -> Result<bool, WorkspaceError> {
        validate_name(name)?;
        let path = Self::path_in(dir, name);
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(path)?;
        Ok(true)
    }

    /// Every workspace in `dir`, by name, sorted.
    pub fn list_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = match std::fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
                .filter_map(|e| {
                    e.path()
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string())
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        names
    }

    /// The directory workspaces live in.
    pub fn default_dir() -> PathBuf {
        let mut p = dirs::home_dir().unwrap_or_default();
        p.push(".essh");
        p.push("workspaces");
        p
    }
}

/// What happened to one session during a restore.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionOutcome {
    Connected,
    /// Failed, with the first rung of the diagnosis that explains why.
    Failed(String),
}

/// The result of restoring a workspace.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RestoreReport {
    pub workspace: String,
    pub outcomes: Vec<(String, SessionOutcome)>,
}

impl RestoreReport {
    pub fn connected(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|(_, o)| *o == SessionOutcome::Connected)
            .count()
    }

    pub fn failed(&self) -> Vec<(&str, &str)> {
        self.outcomes
            .iter()
            .filter_map(|(h, o)| match o {
                SessionOutcome::Failed(why) => Some((h.as_str(), why.as_str())),
                _ => None,
            })
            .collect()
    }

    /// A summary that states the partial result plainly.
    ///
    /// "Restored 5 of 8" is the honest headline for a fleet with three hosts
    /// down; reporting either success or failure alone would be wrong.
    pub fn summary(&self) -> String {
        let total = self.outcomes.len();
        let ok = self.connected();
        if total == 0 {
            return format!("{} has no sessions", self.workspace);
        }
        if ok == total {
            return format!("restored {} ({} sessions)", self.workspace, total);
        }
        let failed = self.failed();
        let names: Vec<&str> = failed.iter().map(|(h, _)| *h).collect();
        format!(
            "restored {} of {} sessions in {} — {} did not connect",
            ok,
            total,
            self.workspace,
            names.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Workspace {
        Workspace {
            name: "production".into(),
            layout: Layout::Tabs,
            description: Some("the prod incident set".into()),
            sessions: vec![
                WorkspaceSession::new("bastion"),
                WorkspaceSession {
                    host: "prod-api-01".into(),
                    user: Some("deploy".into()),
                    port: Some(2222),
                    on_connect: Some("tmux new -A -s essh".into()),
                },
                WorkspaceSession::new("prod-db"),
            ],
        }
    }

    #[test]
    fn a_workspace_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let ws = sample();
        ws.save_in(dir.path()).unwrap();

        let back = Workspace::load_from(dir.path(), "production").unwrap();
        assert_eq!(back, ws);
        assert_eq!(back.sessions.len(), 3);
        assert_eq!(
            back.sessions[1].on_connect.as_deref(),
            Some("tmux new -A -s essh")
        );
    }

    #[test]
    fn the_saved_file_is_readable_toml_a_human_can_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = sample().save_in(dir.path()).unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("name = \"production\""));
        assert!(text.contains("host = \"bastion\""));
        // Absent optionals must not appear as empty noise.
        assert!(!text.contains("on_connect = \"\""));
    }

    #[test]
    fn listing_returns_names_in_a_stable_order() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["staging", "production", "dev"] {
            Workspace {
                name: n.into(),
                ..Default::default()
            }
            .save_in(dir.path())
            .unwrap();
        }
        assert_eq!(
            Workspace::list_in(dir.path()),
            vec!["dev", "production", "staging"]
        );
    }

    #[test]
    fn a_missing_workspace_is_named_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        match Workspace::load_from(dir.path(), "nope") {
            Err(WorkspaceError::NotFound(n)) => assert_eq!(n, "nope"),
            other => panic!("expected NotFound, got {:?}", other),
        }
    }

    #[test]
    fn a_workspace_name_cannot_escape_its_directory() {
        // The name becomes a filename, so this is a path-traversal surface.
        let dir = tempfile::tempdir().unwrap();
        for bad in ["../evil", "a/b", "..", ".hidden", ""] {
            let ws = Workspace {
                name: bad.into(),
                ..Default::default()
            };
            assert!(
                matches!(ws.save_in(dir.path()), Err(WorkspaceError::UnsafeName(_))),
                "{:?} should have been rejected",
                bad
            );
            assert!(matches!(
                Workspace::load_from(dir.path(), bad),
                Err(WorkspaceError::UnsafeName(_))
            ));
        }
    }

    #[test]
    fn deleting_reports_whether_anything_was_there() {
        let dir = tempfile::tempdir().unwrap();
        sample().save_in(dir.path()).unwrap();
        assert!(Workspace::delete_in(dir.path(), "production").unwrap());
        assert!(!Workspace::delete_in(dir.path(), "production").unwrap());
    }

    #[test]
    fn a_partial_restore_says_so_rather_than_claiming_success() {
        let report = RestoreReport {
            workspace: "production".into(),
            outcomes: vec![
                ("bastion".into(), SessionOutcome::Connected),
                ("prod-api-01".into(), SessionOutcome::Connected),
                (
                    "prod-db".into(),
                    SessionOutcome::Failed("TCP:22: timed out after 5s".into()),
                ),
            ],
        };
        assert_eq!(report.connected(), 2);
        let s = report.summary();
        assert!(s.contains("2 of 3"), "{}", s);
        assert!(s.contains("prod-db"), "{}", s);
        assert_eq!(
            report.failed(),
            vec![("prod-db", "TCP:22: timed out after 5s")]
        );
    }

    #[test]
    fn a_fully_successful_restore_does_not_mention_failures() {
        let report = RestoreReport {
            workspace: "production".into(),
            outcomes: vec![("a".into(), SessionOutcome::Connected)],
        };
        let s = report.summary();
        assert!(s.contains("restored production"));
        assert!(!s.contains("did not connect"), "{}", s);
    }

    #[test]
    fn an_empty_workspace_is_described_not_reported_as_restored() {
        let report = RestoreReport {
            workspace: "empty".into(),
            outcomes: vec![],
        };
        assert!(report.summary().contains("no sessions"));
    }

    #[test]
    fn layout_survives_a_round_trip_and_defaults_to_tabs() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = sample();
        ws.layout = Layout::VerticalSplit;
        ws.save_in(dir.path()).unwrap();
        assert_eq!(
            Workspace::load_from(dir.path(), "production")
                .unwrap()
                .layout,
            Layout::VerticalSplit
        );

        // A file written by hand with no layout key still loads.
        std::fs::write(
            Workspace::path_in(dir.path(), "manual"),
            "name = \"manual\"\n[[sessions]]\nhost = \"a\"\n",
        )
        .unwrap();
        let manual = Workspace::load_from(dir.path(), "manual").unwrap();
        assert_eq!(manual.layout, Layout::Tabs);
        assert_eq!(manual.sessions.len(), 1);
    }
}
