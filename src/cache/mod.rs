use std::collections::HashMap;
use std::path::Path;

use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedHost {
    pub id: i64,
    pub hostname: String,
    pub ip: Option<String>,
    pub port: u16,
    pub fingerprint: String,
    pub key_type: String,
    pub first_seen: String,
    pub last_seen: String,
    pub tags: HashMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedKey {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub key_type: String,
    pub fingerprint: String,
    pub added_at: String,
}

#[derive(Debug)]
pub enum HostKeyStatus {
    Trusted,
    Changed {
        old_fingerprint: String,
        old_last_seen: String,
    },
    Unknown,
}

pub struct CacheDb {
    conn: Connection,
}

impl CacheDb {
    pub fn open(path: &Path) -> Result<Self, CacheError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.init_tables()?;
        Ok(db)
    }

    pub fn open_default() -> Result<Self, CacheError> {
        let mut path = dirs::home_dir().expect("cannot determine home directory");
        path.push(".essh");
        path.push("cache.db");
        Self::open(&path)
    }

    fn init_tables(&self) -> Result<(), CacheError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS known_hosts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                hostname TEXT NOT NULL,
                ip TEXT,
                port INTEGER NOT NULL DEFAULT 22,
                fingerprint TEXT NOT NULL,
                key_type TEXT NOT NULL,
                first_seen TEXT NOT NULL,
                last_seen TEXT NOT NULL,
                tags TEXT NOT NULL DEFAULT '{}',
                UNIQUE(hostname, port)
            );
            CREATE TABLE IF NOT EXISTS user_keys (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                path TEXT NOT NULL,
                key_type TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                added_at TEXT NOT NULL
            );",
        )?;
        Ok(())
    }

    pub fn check_host_key(
        &self,
        hostname: &str,
        port: u16,
        fingerprint: &str,
    ) -> Result<HostKeyStatus, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT fingerprint, last_seen FROM known_hosts WHERE hostname = ?1 AND port = ?2",
        )?;
        let result = stmt.query_row(params![hostname, port], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        });

        match result {
            Ok((stored_fp, last_seen)) => {
                if stored_fp == fingerprint {
                    Ok(HostKeyStatus::Trusted)
                } else if stored_fp == "unknown" || stored_fp == "imported" {
                    // Host was added manually or imported without a real fingerprint
                    Ok(HostKeyStatus::Unknown)
                } else {
                    Ok(HostKeyStatus::Changed {
                        old_fingerprint: stored_fp,
                        old_last_seen: last_seen,
                    })
                }
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(HostKeyStatus::Unknown),
            Err(e) => Err(CacheError::Database(e)),
        }
    }

    pub fn trust_host(
        &self,
        hostname: &str,
        ip: Option<&str>,
        port: u16,
        fingerprint: &str,
        key_type: &str,
    ) -> Result<(), CacheError> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO known_hosts (hostname, ip, port, fingerprint, key_type, first_seen, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(hostname, port) DO UPDATE SET
                 ip = excluded.ip,
                 fingerprint = excluded.fingerprint,
                 key_type = excluded.key_type,
                 last_seen = excluded.last_seen",
            params![hostname, ip, port, fingerprint, key_type, &now, &now],
        )?;
        Ok(())
    }

    pub fn update_last_seen(&self, hostname: &str, port: u16) -> Result<(), CacheError> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE known_hosts SET last_seen = ?1 WHERE hostname = ?2 AND port = ?3",
            params![&now, hostname, port],
        )?;
        Ok(())
    }

    pub fn list_hosts(&self) -> Result<Vec<CachedHost>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, hostname, ip, port, fingerprint, key_type, first_seen, last_seen, tags
             FROM known_hosts ORDER BY hostname, port",
        )?;
        let rows = stmt.query_map([], |row| {
            let tags_json: String = row.get(8)?;
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get::<_, i64>(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                tags_json,
            ))
        })?;

        let mut hosts = Vec::new();
        for row in rows {
            let (id, hostname, ip, port, fingerprint, key_type, first_seen, last_seen, tags_json) =
                row?;
            let tags: HashMap<String, String> =
                serde_json::from_str(&tags_json).map_err(CacheError::Json)?;
            hosts.push(CachedHost {
                id,
                hostname,
                ip,
                port: port as u16,
                fingerprint,
                key_type,
                first_seen,
                last_seen,
                tags,
            });
        }
        Ok(hosts)
    }

    pub fn remove_host(&self, hostname: &str, port: u16) -> Result<bool, CacheError> {
        let affected = self.conn.execute(
            "DELETE FROM known_hosts WHERE hostname = ?1 AND port = ?2",
            params![hostname, port],
        )?;
        Ok(affected > 0)
    }

    pub fn find_hosts_by_tag(&self, key: &str, value: &str) -> Result<Vec<CachedHost>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, hostname, ip, port, fingerprint, key_type, first_seen, last_seen, tags
             FROM known_hosts
             WHERE json_extract(tags, '$.' || ?1) = ?2
             ORDER BY hostname, port",
        )?;
        let rows = stmt.query_map(params![key, value], |row| {
            let tags_json: String = row.get(8)?;
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get::<_, i64>(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                tags_json,
            ))
        })?;

        let mut hosts = Vec::new();
        for row in rows {
            let (id, hostname, ip, port, fingerprint, key_type, first_seen, last_seen, tags_json) =
                row?;
            let tags: HashMap<String, String> =
                serde_json::from_str(&tags_json).map_err(CacheError::Json)?;
            hosts.push(CachedHost {
                id,
                hostname,
                ip,
                port: port as u16,
                fingerprint,
                key_type,
                first_seen,
                last_seen,
                tags,
            });
        }
        Ok(hosts)
    }

    pub fn set_host_tags(
        &self,
        hostname: &str,
        port: u16,
        tags: &HashMap<String, String>,
    ) -> Result<(), CacheError> {
        let tags_json = serde_json::to_string(tags)?;
        self.conn.execute(
            "UPDATE known_hosts SET tags = ?1 WHERE hostname = ?2 AND port = ?3",
            params![tags_json, hostname, port],
        )?;
        Ok(())
    }

    pub fn add_key(
        &self,
        name: &str,
        path: &str,
        key_type: &str,
        fingerprint: &str,
    ) -> Result<(), CacheError> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO user_keys (name, path, key_type, fingerprint, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(name) DO UPDATE SET
                 path = excluded.path,
                 key_type = excluded.key_type,
                 fingerprint = excluded.fingerprint,
                 added_at = excluded.added_at",
            params![name, path, key_type, fingerprint, &now],
        )?;
        Ok(())
    }

    pub fn list_keys(&self) -> Result<Vec<CachedKey>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, path, key_type, fingerprint, added_at
             FROM user_keys ORDER BY name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(CachedKey {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                key_type: row.get(3)?,
                fingerprint: row.get(4)?,
                added_at: row.get(5)?,
            })
        })?;

        rows.into_iter()
            .map(|r| r.map_err(CacheError::Database))
            .collect()
    }

    pub fn remove_key(&self, name: &str) -> Result<bool, CacheError> {
        let affected = self
            .conn
            .execute("DELETE FROM user_keys WHERE name = ?1", params![name])?;
        Ok(affected > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> CacheDb {
        let mut path = std::env::temp_dir();
        path.push(format!("essh_test_{}.db", uuid::Uuid::new_v4()));
        CacheDb::open(&path).unwrap()
    }

    #[test]
    fn test_open_creates_tables() {
        let db = test_db();
        let hosts = db.list_hosts().unwrap();
        assert_eq!(hosts.len(), 0);
        let keys = db.list_keys().unwrap();
        assert_eq!(keys.len(), 0);
    }

    #[test]
    fn test_trust_and_check_host_key() {
        let db = test_db();
        db.trust_host(
            "example.com",
            Some("1.2.3.4"),
            22,
            "sha256:abc123",
            "ed25519",
        )
        .unwrap();

        let status = db
            .check_host_key("example.com", 22, "sha256:abc123")
            .unwrap();
        assert!(matches!(status, HostKeyStatus::Trusted));
    }

    #[test]
    fn test_check_unknown_host() {
        let db = test_db();
        let status = db.check_host_key("unknown.host", 22, "sha256:xyz").unwrap();
        assert!(matches!(status, HostKeyStatus::Unknown));
    }

    #[test]
    fn test_host_key_changed() {
        let db = test_db();
        db.trust_host("example.com", None, 22, "sha256:old_fp", "rsa")
            .unwrap();

        let status = db
            .check_host_key("example.com", 22, "sha256:new_fp")
            .unwrap();
        match status {
            HostKeyStatus::Changed {
                old_fingerprint,
                old_last_seen,
            } => {
                assert_eq!(old_fingerprint, "sha256:old_fp");
                assert!(!old_last_seen.is_empty());
            }
            other => panic!("expected Changed, got {:?}", other),
        }
    }

    #[test]
    fn test_list_hosts() {
        let db = test_db();
        db.trust_host("alpha.com", None, 22, "fp1", "ed25519")
            .unwrap();
        db.trust_host("beta.com", Some("10.0.0.1"), 2222, "fp2", "rsa")
            .unwrap();
        db.trust_host("gamma.com", None, 22, "fp3", "ed25519")
            .unwrap();

        let hosts = db.list_hosts().unwrap();
        assert_eq!(hosts.len(), 3);
        assert_eq!(hosts[0].hostname, "alpha.com");
        assert_eq!(hosts[1].hostname, "beta.com");
        assert_eq!(hosts[1].port, 2222);
        assert_eq!(hosts[2].hostname, "gamma.com");
    }

    #[test]
    fn test_remove_host() {
        let db = test_db();
        db.trust_host("remove-me.com", None, 22, "fp", "rsa")
            .unwrap();
        assert_eq!(db.list_hosts().unwrap().len(), 1);

        let removed = db.remove_host("remove-me.com", 22).unwrap();
        assert!(removed);
        assert_eq!(db.list_hosts().unwrap().len(), 0);

        let removed_again = db.remove_host("remove-me.com", 22).unwrap();
        assert!(!removed_again);
    }

    #[test]
    fn test_update_last_seen() {
        let db = test_db();
        db.trust_host("seen.com", None, 22, "fp", "ed25519")
            .unwrap();

        let before = db.list_hosts().unwrap();
        let last_seen_before = before[0].last_seen.clone();

        std::thread::sleep(std::time::Duration::from_millis(50));
        db.update_last_seen("seen.com", 22).unwrap();

        let after = db.list_hosts().unwrap();
        let last_seen_after = &after[0].last_seen;
        assert_ne!(&last_seen_before, last_seen_after);
    }

    #[test]
    fn test_host_tags() {
        let db = test_db();
        db.trust_host("tagged.com", None, 22, "fp", "ed25519")
            .unwrap();

        let mut tags = HashMap::new();
        tags.insert("env".to_string(), "production".to_string());
        tags.insert("team".to_string(), "infra".to_string());
        db.set_host_tags("tagged.com", 22, &tags).unwrap();

        let found = db.find_hosts_by_tag("env", "production").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].hostname, "tagged.com");
        assert_eq!(found[0].tags.get("env").unwrap(), "production");

        let hosts = db.list_hosts().unwrap();
        assert_eq!(hosts[0].tags.len(), 2);
        assert_eq!(hosts[0].tags.get("team").unwrap(), "infra");
    }

    #[test]
    fn test_find_hosts_by_tag_no_match() {
        let db = test_db();
        db.trust_host("host.com", None, 22, "fp", "rsa").unwrap();

        let mut tags = HashMap::new();
        tags.insert("env".to_string(), "staging".to_string());
        db.set_host_tags("host.com", 22, &tags).unwrap();

        let found = db.find_hosts_by_tag("env", "production").unwrap();
        assert_eq!(found.len(), 0);

        let found = db.find_hosts_by_tag("nonexistent", "value").unwrap();
        assert_eq!(found.len(), 0);
    }

    #[test]
    fn test_add_key() {
        let db = test_db();
        db.add_key(
            "my-key",
            "/home/user/.ssh/id_ed25519",
            "ed25519",
            "sha256:keyprint",
        )
        .unwrap();

        let keys = db.list_keys().unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].name, "my-key");
        assert_eq!(keys[0].path, "/home/user/.ssh/id_ed25519");
        assert_eq!(keys[0].key_type, "ed25519");
        assert_eq!(keys[0].fingerprint, "sha256:keyprint");
    }

    #[test]
    fn test_remove_key() {
        let db = test_db();
        db.add_key("delete-me", "/tmp/key", "rsa", "fp").unwrap();
        assert_eq!(db.list_keys().unwrap().len(), 1);

        let removed = db.remove_key("delete-me").unwrap();
        assert!(removed);
        assert_eq!(db.list_keys().unwrap().len(), 0);

        let removed_again = db.remove_key("delete-me").unwrap();
        assert!(!removed_again);
    }

    #[test]
    fn test_add_key_upsert() {
        let db = test_db();
        db.add_key("dup-key", "/old/path", "rsa", "old_fp").unwrap();
        db.add_key("dup-key", "/new/path", "ed25519", "new_fp")
            .unwrap();

        let keys = db.list_keys().unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].path, "/new/path");
        assert_eq!(keys[0].key_type, "ed25519");
        assert_eq!(keys[0].fingerprint, "new_fp");
    }

    #[test]
    fn test_trust_host_upsert() {
        let db = test_db();
        db.trust_host("dup.com", Some("1.1.1.1"), 22, "fp1", "rsa")
            .unwrap();
        db.trust_host("dup.com", Some("2.2.2.2"), 22, "fp2", "ed25519")
            .unwrap();

        let hosts = db.list_hosts().unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].fingerprint, "fp2");
        assert_eq!(hosts[0].key_type, "ed25519");
        assert_eq!(hosts[0].ip.as_deref(), Some("2.2.2.2"));
    }
}
