use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const SCHEMA_VERSION: i64 = 1;
const SCHEMA: &str = include_str!("../schema.sql");

#[derive(Debug, serde::Serialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub focused_task_id: Option<String>,
    pub revision: i64,
}

/// A connection bound to one explicit, canonical working-directory root.
/// Raw SQL access stays private so callers cannot attach other project DBs.
pub struct Store {
    conn: Connection,
    root: PathBuf,
}

impl Store {
    pub(crate) fn write_project<T>(
        &mut self,
        write: impl FnOnce(&rusqlite::Transaction<'_>, Project) -> Result<T>,
    ) -> Result<T> {
        let tx = rusqlite::Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let project = self.project()?;
        let result = write(&tx, project)?;
        tx.commit()?;
        Ok(result)
    }

    /// Capture source identity under this store's explicitly bound root.
    pub fn capture_source(&self) -> Result<crate::fingerprint::Snapshot> {
        self.project()?;
        crate::fingerprint::capture(&self.root)
    }

    /// Validate binding and read tasks in the same SQLite snapshot.
    pub(crate) fn read_project<T>(
        &self,
        read: impl FnOnce(&Connection, Project) -> Result<T>,
    ) -> Result<T> {
        let tx = self.conn.unchecked_transaction()?;
        let project = self.project()?;
        let result = read(&tx, project)?;
        tx.commit()?;
        Ok(result)
    }

    /// Initialize once; existing database files are never overwritten.
    /// Schema and project identity commit together, or both roll back.
    pub fn init(root: impl AsRef<Path>, name: &str) -> Result<Self> {
        let root = canonical_root(root.as_ref())?;
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 120 {
            return Err("project name must contain 1..=120 characters".into());
        }

        let state_dir = root.join(".devstate");
        match fs::create_dir(&state_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        require_directory(&state_dir)?;

        let db_path = state_dir.join("state.db");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        // Atomic reservation also rejects existing symlinks. On init failure,
        // leave the file for explicit recovery; never delete a possibly live DB.
        drop(options.open(&db_path)?);

        let mut conn = open_existing(&db_path)?;
        configure_connection(&conn)?;
        let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err("SQLite could not enable WAL mode".into());
        }

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(SCHEMA)?;
        tx.execute(
            "INSERT INTO project (singleton, project_id, name, root_path)
             VALUES (1, ?1, ?2, ?3)",
            params![
                Uuid::new_v4().to_string(),
                name,
                root.to_str().ok_or("project root must have a UTF-8 path")?,
            ],
        )?;
        tx.commit()?;
        Ok(Self { conn, root })
    }

    /// Open only an initialized DB under this root. Never create or rebind it.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Self::open_bound(root.as_ref(), false)
    }

    /// Explicit overview reads never acquire a writable project connection.
    pub fn open_read_only(root: impl AsRef<Path>) -> Result<Self> {
        Self::open_bound(root.as_ref(), true)
    }

    fn open_bound(root: &Path, read_only: bool) -> Result<Self> {
        let root = canonical_root(root.as_ref())?;
        let state_dir = root.join(".devstate");
        require_directory(&state_dir)?;
        let db_path = state_dir.join("state.db");
        let metadata = fs::symlink_metadata(&db_path)?;
        if !metadata.file_type().is_file() {
            return Err("state.db must be a regular file, not a symlink".into());
        }

        let flags = if read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        };
        let conn = Connection::open_with_flags(&db_path, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported schema version {version}; expected {SCHEMA_VERSION}; no changes made"
            ).into());
        }

        let store = Self { conn, root };
        store.project()?;
        configure_connection(&store.conn)?;
        if read_only {
            store.conn.pragma_update(None, "query_only", true)?;
        }
        let mode: String = store.conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err("expected a WAL database; refusing to change its journal mode".into());
        }
        Ok(store)
    }

    /// Read project identity and verify the binding rather than returning a
    /// cached project that another process could have rebound.
    pub fn project(&self) -> Result<Project> {
        let project = self.conn.query_row(
            "SELECT project_id, name, root_path, focused_task_id, revision
             FROM project WHERE singleton = 1",
            [],
            |row| Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                root_path: row.get(2)?,
                focused_task_id: row.get(3)?,
                revision: row.get(4)?,
            }),
        )?;
        if Some(project.root_path.as_str()) != self.root.to_str() {
            return Err("project root mismatch; explicit rebinding is required".into());
        }
        Ok(project)
    }
}

fn canonical_root(root: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(root)?;
    if !root.is_dir() || root.to_str().is_none() {
        return Err("project root must be an existing directory with a UTF-8 path".into());
    }
    Ok(root)
}

fn require_directory(path: &Path) -> Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(".devstate must be a directory, not a symlink".into());
    }
    Ok(())
}

fn open_existing(path: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

fn configure_connection(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    Ok(())
}
