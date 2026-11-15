//! Explicit project directory registry. It never stores project task content.

use std::fs;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};

use crate::queries::ProjectOverview;
use crate::store::{Result, Store};

const APPLICATION_ID: i64 = 0x44565247; // DVRG, distinct from project databases.

pub struct Registry {
    conn: Connection,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "availability", content = "data", rename_all = "snake_case")]
pub enum OverviewEntry {
    Available(ProjectOverview),
    Unavailable { root_path: String, reason: String },
}

#[derive(Debug, serde::Serialize)]
pub struct OverviewPage {
    pub entries: Vec<OverviewEntry>,
    /// Continue in canonical root-path order, not an implicit project selection.
    pub next_cursor: Option<String>,
}

impl Registry {
    /// Read an existing registry without initialization or journal-mode changes.
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open_with_flags(path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let application_id: i64 = conn.query_row("PRAGMA application_id", [], |row| row.get(0))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if application_id != APPLICATION_ID || version != 1 {
            return Err("not a supported devstate registry".into());
        }
        conn.pragma_update(None, "query_only", true)?;
        Ok(Self { conn })
    }

    /// The caller supplies a registry path explicitly. No implicit home-directory
    /// scan, directory discovery or "last project" setting exists.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let application_id: i64 = tx.query_row("PRAGMA application_id", [], |row| row.get(0))?;
        let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if application_id == 0 && version == 0 {
            let tables: i64 = tx.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [], |row| row.get(0),
            )?;
            if tables != 0 {
                return Err("registry path contains an unrelated database".into());
            }
            tx.execute_batch(
                "CREATE TABLE registered_projects (
                    root_path TEXT PRIMARY KEY CHECK (length(root_path) > 0),
                    project_id TEXT NOT NULL CHECK (length(project_id) BETWEEN 1 AND 64)
                 ) STRICT;"
            )?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.pragma_update(None, "user_version", 1)?;
        } else if application_id != APPLICATION_ID || version != 1 {
            return Err("not a supported devstate registry; refusing to alter it".into());
        }
        tx.commit()?;
        let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err("SQLite could not enable WAL for registry".into());
        }
        Ok(Self { conn })
    }

    /// Only an initialized, correctly bound project can be registered.
    /// Re-registering the same identity/path is harmless. A replaced DB needs
    /// explicit unregister/register instead of silently trusting a new identity.
    pub fn register(&self, root: impl AsRef<Path>) -> Result<()> {
        let project = Store::open_read_only(root)?.project()?;
        let affected = self.conn.execute(
            "INSERT INTO registered_projects (root_path, project_id) VALUES (?1, ?2)
             ON CONFLICT(root_path) DO UPDATE SET project_id = excluded.project_id
             WHERE registered_projects.project_id = excluded.project_id",
            params![project.root_path, project.id],
        )?;
        if affected != 1 {
            return Err("registered path now has a different identity; unregister it explicitly".into());
        }
        Ok(())
    }

    /// Use the registered path verbatim so missing/moved directories can also
    /// be removed. This deletes only a registry entry, never a project or DB.
    pub fn unregister(&self, registered_root: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM registered_projects WHERE root_path = ?1",
            params![registered_root],
        )? == 1)
    }

    /// Opt-in overview, at most 50 projects per call. Read each project's DB
    /// independently; no ATTACH, copied task content or global memory search.
    /// Project snapshots are individually consistent, not globally simultaneous.
    pub fn overview(&self, limit: u32, cursor: Option<&str>) -> Result<OverviewPage> {
        if !(1..=50).contains(&limit) {
            return Err("overview limit must be between 1 and 50".into());
        }
        let (sql, values) = match cursor {
            Some(cursor) => (
                "SELECT root_path, project_id FROM registered_projects
                 WHERE root_path > ?1 ORDER BY root_path LIMIT ?2",
                vec![rusqlite::types::Value::Text(cursor.to_owned()),
                     rusqlite::types::Value::Integer(i64::from(limit) + 1)],
            ),
            None => (
                "SELECT root_path, project_id FROM registered_projects
                 ORDER BY root_path LIMIT ?1",
                vec![rusqlite::types::Value::Integer(i64::from(limit) + 1)],
            ),
        };
        let mut statement = self.conn.prepare_cached(sql)?;
        let mut projects = statement.query_map(rusqlite::params_from_iter(values.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?.collect::<rusqlite::Result<Vec<_>>>()?;
        // Release the registry query before opening any project connections.
        drop(statement);
        let next_cursor = if projects.len() > limit as usize {
            projects.pop();
            projects.last().map(|(root, _)| root.clone())
        } else {
            None
        };
        let mut entries = Vec::with_capacity(projects.len());
        for (root_path, expected_id) in projects {
            let result = Store::open_read_only(&root_path)
                .and_then(|store| store.project_overview())
                .and_then(|overview| {
                    if overview.project.id != expected_id {
                        return Err("project identity changed; explicit registration is required".into());
                    }
                    Ok(overview)
                });
            entries.push(match result {
                Ok(overview) => OverviewEntry::Available(overview),
                Err(error) => OverviewEntry::Unavailable {
                    root_path,
                    reason: error.to_string().chars().take(500).collect(),
                },
            });
        }
        Ok(OverviewPage { entries, next_cursor })
    }
}
