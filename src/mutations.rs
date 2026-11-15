//! Task state transitions. Every write validates binding inside an immediate
//! transaction; stale revisions fail without partial task/focus changes.

use std::path::{Component, Path};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::params;
use uuid::Uuid;

use crate::fingerprint::SCOPE;
use crate::store::{Result, Store};

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewTask {
    pub title: String,
    #[serde(default = "task_kind")]
    pub kind: String,
    pub spec_path: Option<String>,
}
fn task_kind() -> String { "task".into() }
fn active() -> String { "active".into() }
fn implementing() -> String { "implement".into() }

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verification {
    pub status: String,
    /// Actual command/result or the explicit rationale for not_required.
    pub notes: String,
    /// Source fingerprint captured BEFORE an externally authorized check.
    pub before: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub id: String,
    pub expected_revision: i64,
    pub summary: String,
    pub next_action: String,
    #[serde(default = "active")]
    pub status: String,
    #[serde(default = "implementing")]
    pub phase: String,
    #[serde(default)]
    pub blockers: String,
    pub notes: Option<String>,
    pub verification: Option<Verification>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloseTask {
    pub id: String,
    pub expected_revision: i64,
    pub expected_project_revision: i64,
    pub summary: String,
    pub status: String,
    /// Reaffirmed for this completion; never silently reuse an old exemption.
    pub not_required_reason: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct WriteResult {
    pub id: String,
    pub revision: i64,
    pub project_revision: i64,
}

impl Store {
    pub fn create_task(&mut self, task: NewTask) -> Result<WriteResult> {
        if let Some(path) = &task.spec_path {
            let root = self.project()?.root_path;
            let relative = Path::new(path);
            if path.trim().is_empty() || !relative.components().all(|part| matches!(part, Component::Normal(_))) {
                return Err("spec_path must be a nonempty repository-relative path without ..".into());
            }
            let mut full = std::path::PathBuf::from(root);
            for part in relative.components() {
                full.push(part);
                match std::fs::symlink_metadata(&full) {
                    Ok(meta) if meta.file_type().is_symlink() => return Err("spec_path cannot traverse symlinks".into()),
                    Ok(_) => {},
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                    Err(error) => return Err(error.into()),
                }
            }
        }
        self.write_project(|tx, project| {
            let id = Uuid::new_v4().to_string();
            tx.execute("INSERT INTO tasks (id, title, kind, spec_path) VALUES (?1, ?2, ?3, ?4)",
                params![id, task.title, task.kind, task.spec_path])?;
            Ok(WriteResult { id, revision: 1, project_revision: project.revision })
        })
    }

    /// Change selection only. Never reset another task or silently activate it.
    pub fn set_focus(&mut self, id: Option<&str>, expected_project_revision: i64) -> Result<crate::store::Project> {
        let now = now_ms()?;
        self.write_project(|tx, mut project| {
            if project.revision != expected_project_revision { return Err("project revision conflict; resume again".into()); }
            if let Some(id) = id {
                let status: String = tx.query_row("SELECT status FROM tasks WHERE id = ?1", params![id], |row| row.get(0))?;
                if matches!(status.as_str(), "done" | "cancelled") { return Err("cannot focus a closed task".into()); }
            }
            if tx.execute("UPDATE project SET focused_task_id = ?1, revision = revision + 1,
                updated_at = max(?2, updated_at) WHERE singleton = 1 AND revision = ?3",
                params![id, now, expected_project_revision])? != 1 {
                return Err("project revision conflict".into());
            }
            project.focused_task_id = id.map(str::to_owned);
            project.revision += 1;
            Ok(project)
        })
    }

    pub fn checkpoint(&mut self, checkpoint: Checkpoint) -> Result<WriteResult> {
        if !matches!(checkpoint.status.as_str(), "todo" | "active" | "blocked") {
            return Err("checkpoint status must be todo, active or blocked".into());
        }
        // Capture only when recording actual verification, outside the DB lock.
        let mut snapshot = None;
        if let Some(verification) = &checkpoint.verification {
            if verification.notes.trim().is_empty() { return Err("verification notes are required".into()); }
            match verification.status.as_str() {
                "passed" | "failed" => {
                    let current = self.capture_source()?;
                    if verification.before.as_deref() != Some(&current.fingerprint) {
                        return Err("source differs from before-check fingerprint; do not attach this result".into());
                    }
                    snapshot = Some(current);
                }
                "not_required" if verification.before.is_none() => {},
                _ => return Err("verification must be passed/failed with before, or not_required with rationale".into()),
            }
        }
        let now = now_ms()?;
        self.write_project(|tx, project| {
            let affected = tx.execute(
                "UPDATE tasks SET status = ?1, phase = ?2, summary = ?3, next_action = ?4,
                 blockers = ?5, notes = coalesce(?6, notes),
                 started_at = CASE WHEN ?1 IN ('active','blocked') THEN coalesce(started_at, max(?7, updated_at)) ELSE started_at END,
                 updated_at = max(?7, updated_at), revision = revision + 1
                 WHERE id = ?8 AND revision = ?9 AND status NOT IN ('done','cancelled')",
                params![checkpoint.status, checkpoint.phase, checkpoint.summary, checkpoint.next_action,
                    checkpoint.blockers, checkpoint.notes, now, checkpoint.id, checkpoint.expected_revision],
            )?;
            if affected != 1 { return Err("task revision conflict, missing or closed task; read task again".into()); }
            if let Some(verification) = &checkpoint.verification {
                let scope = snapshot.as_ref().map(|snapshot| snapshot.scope);
                let fingerprint = snapshot.as_ref().map(|snapshot| snapshot.fingerprint.as_str());
                tx.execute("UPDATE tasks SET verification_status = ?1, verification_notes = ?2,
                    source_scope = ?3, source_fingerprint = ?4, verified_fingerprint = ?4,
                    verified_at = CASE WHEN ?1 = 'not_required' THEN NULL ELSE updated_at END WHERE id = ?5",
                    params![verification.status, verification.notes, scope, fingerprint, checkpoint.id])?;
            }
            Ok(WriteResult { id: checkpoint.id, revision: checkpoint.expected_revision + 1, project_revision: project.revision })
        })
    }

    pub fn close_task(&mut self, close: CloseTask) -> Result<WriteResult> {
        if !matches!(close.status.as_str(), "done" | "cancelled") {
            return Err("close status must be done or cancelled".into());
        }
        let reason = close.not_required_reason.as_deref();
        if reason.is_some_and(|reason| reason.trim().is_empty()) { return Err("not_required reason cannot be empty".into()); }
        let snapshot = if close.status == "done" && reason.is_none() {
            let task = self.get_task(&close.id)?.ok_or("task not found")?;
            if task.checkpoint.revision != close.expected_revision { return Err("task revision conflict".into()); }
            let current = self.capture_source()?;
            if task.checkpoint.verification_status != "passed" || task.source_scope.as_deref() != Some(SCOPE)
                || task.verified_fingerprint.as_deref() != Some(current.fingerprint.as_str()) {
                return Err("completion needs a current passed result or an explicit not_required reason".into());
            }
            Some(current)
        } else { None };
        let now = now_ms()?;
        self.write_project(|tx, project| {
            if project.revision != close.expected_project_revision { return Err("project revision conflict; resume again".into()); }
            let changed = tx.execute(
                "UPDATE tasks SET status = ?1, summary = ?2, next_action = '', blockers = '',
                 phase = CASE WHEN ?1 = 'done' THEN 'deliver' ELSE phase END,
                 started_at = CASE WHEN ?1 = 'done' THEN coalesce(started_at, max(?3, updated_at)) ELSE started_at END,
                 closed_at = max(?3, updated_at), updated_at = max(?3, updated_at), revision = revision + 1,
                 verification_status = CASE WHEN ?4 IS NOT NULL THEN 'not_required' ELSE verification_status END,
                 verification_notes = coalesce(?4, verification_notes),
                 verified_at = CASE WHEN ?4 IS NOT NULL THEN NULL ELSE verified_at END,
                 verified_fingerprint = CASE WHEN ?4 IS NOT NULL THEN NULL ELSE verified_fingerprint END,
                 source_scope = CASE WHEN ?5 IS NOT NULL THEN ?6 ELSE source_scope END,
                 source_fingerprint = coalesce(?5, source_fingerprint)
                 WHERE id = ?7 AND revision = ?8 AND status NOT IN ('done','cancelled')",
                params![close.status, close.summary, now, reason,
                    snapshot.as_ref().map(|snapshot| snapshot.fingerprint.as_str()),
                    snapshot.as_ref().map(|snapshot| snapshot.scope), close.id, close.expected_revision],
            )?;
            if changed != 1 { return Err("task revision conflict, missing or closed task".into()); }
            let focused = project.focused_task_id.as_deref() == Some(&close.id);
            if focused {
                if tx.execute("UPDATE project SET focused_task_id = NULL, revision = revision + 1,
                    updated_at = max(?1, updated_at) WHERE singleton = 1 AND revision = ?2",
                    params![now, close.expected_project_revision])? != 1 { return Err("project revision conflict".into()); }
            }
            Ok(WriteResult { id: close.id, revision: close.expected_revision + 1,
                project_revision: project.revision + i64::from(focused) })
        })
    }
}

fn now_ms() -> Result<i64> {
    Ok(i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?)
}
