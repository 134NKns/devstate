//! Read-only task queries. Response rendering/token budgets belong to CLI/MCP.

use rusqlite::{OptionalExtension, Row, params, params_from_iter, types::Value};

use crate::store::{Project, Result, Store};

#[derive(Debug, serde::Serialize)]
pub struct Checkpoint {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub spec_path: Option<String>,
    pub status: String,
    pub phase: String,
    pub summary: String,
    pub next_action: String,
    pub blockers: String,
    pub verification_status: String,
    /// current/stale/unknown/not_applicable for the scoped source only.
    pub verification_freshness: &'static str,
    pub revision: i64,
    pub updated_at: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct Resume {
    pub project: Project,
    /// None means no explicit focus; never guess from other tasks.
    pub task: Option<Checkpoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<ResumeFallback>,
}

/// Discovery context only; returning candidates never changes focus.
#[derive(Debug, serde::Serialize)]
pub struct ResumeFallback {
    pub reason: &'static str,
    pub candidates: Vec<ResumeCandidate>,
    pub has_more: bool,
    pub latest_task: Option<ResumeCandidate>,
}

#[derive(Debug, serde::Serialize)]
pub struct ResumeCandidate {
    pub id: String,
    pub title: String,
    pub status: String,
    pub revision: i64,
    pub summary: String,
    pub next_action: String,
    pub blockers: String,
    pub truncated: bool,
}

#[derive(Debug, serde::Serialize)]
pub struct TaskDetails {
    pub checkpoint: Checkpoint,
    pub notes: String,
    pub git_branch: Option<String>,
    pub git_head: Option<String>,
    pub source_scope: Option<String>,
    pub source_fingerprint: Option<String>,
    pub verification_notes: String,
    pub verified_fingerprint: Option<String>,
    pub verified_at: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub usage_status: String,
    /// Stored JSON array; deserialize at the presentation boundary.
    #[serde(rename = "models_used", serialize_with = "serialize_models")]
    pub models_used_json: String,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub closed_at: Option<i64>,
}

impl TaskDetails {
    /// Wall time including pauses/offline time, not active effort/model latency.
    /// Unknown start or a backwards wall clock returns None.
    pub fn elapsed_ms(&self, now_ms: i64) -> Option<i64> {
        self.closed_at.unwrap_or(now_ms)
            .checked_sub(self.started_at?)
            .filter(|elapsed| *elapsed >= 0)
    }
}

#[derive(Debug, serde::Serialize)]
pub struct TaskListItem {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub phase: String,
    pub revision: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskCursor {
    pub project_id: String,
    pub status_filter: Option<String>,
    pub kind_filter: Option<String>,
    pub updated_at: i64,
    pub id: String,
}

#[derive(Debug, serde::Serialize)]
pub struct TaskPage {
    pub items: Vec<TaskListItem>,
    pub next_cursor: Option<TaskCursor>,
}

#[derive(Debug, serde::Serialize)]
pub struct SearchResults {
    pub items: Vec<TaskListItem>,
    pub truncated: bool,
}

#[derive(Debug, serde::Serialize)]
pub struct TaskCounts {
    pub todo: i64,
    pub active: i64,
    pub blocked: i64,
    pub done: i64,
    pub cancelled: i64,
    pub open_bugs: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct OverviewTask {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub summary: String,
    pub next_action: String,
    pub truncated: bool,
    pub updated_at: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct ProjectOverview {
    pub project: Project,
    pub focused_task: Option<OverviewTask>,
    pub latest_task: Option<OverviewTask>,
    pub counts: TaskCounts,
}

const CHECKPOINT_COLUMNS: &str =
    "id, title, kind, spec_path, status, phase, summary, next_action, blockers,
     verification_status, source_scope, verified_fingerprint, revision, updated_at";

impl Store {
    /// Latest work across statuses, including completed tasks.
    pub fn recent_tasks(&self, limit: u32) -> Result<TaskPage> {
        self.list_tasks(None, limit, None)
    }

    /// Literal substring search over title/summary/notes, newest first.
    /// SQLite lower() folds ASCII only; Thai matching is literal, not stemming.
    /// '%' and '_' have no wildcard meaning. No FTS or unbounded results.
    pub fn search_tasks(&self, text: &str, limit: u32) -> Result<SearchResults> {
        let text = text.trim();
        if text.is_empty() || text.chars().count() > 200 {
            return Err("search text must contain 1..=200 characters".into());
        }
        if !(1..=50).contains(&limit) {
            return Err("search limit must be between 1 and 50".into());
        }
        self.read_project(|conn, _project| {
            let mut statement = conn.prepare_cached(
                "SELECT id, title, kind, status, phase, revision, updated_at
                 FROM tasks WHERE instr(lower(title), lower(?1)) > 0
                 OR instr(lower(summary), lower(?1)) > 0
                 OR instr(lower(notes), lower(?1)) > 0
                 ORDER BY updated_at DESC, id DESC LIMIT ?2"
            )?;
            let mut items = statement.query_map(
                params![text, i64::from(limit) + 1], list_item_from_row,
            )?.collect::<rusqlite::Result<Vec<_>>>()?;
            let truncated = items.len() > limit as usize;
            if truncated {
                items.pop();
            }
            Ok(SearchResults { items, truncated })
        })
    }

    /// Compact explicit cross-project view; no notes, evidence or usage payloads.
    /// Focus, latest task and counts are read from the same per-project snapshot.
    pub fn project_overview(&self) -> Result<ProjectOverview> {
        self.read_project(|conn, project| {
            let columns = "id, title, kind, status, substr(summary, 1, 240) AS summary,
                substr(next_action, 1, 200) AS next_action,
                (length(summary) > 240 OR length(next_action) > 200) AS truncated,
                updated_at";
            let focused_task = match project.focused_task_id.as_deref() {
                None => None,
                Some(id) => {
                    let sql = format!("SELECT {columns} FROM tasks WHERE id = ?1");
                    let task = conn.query_row(&sql, params![id], overview_task_from_row)
                        .optional()?
                        .ok_or("focused task is missing; database state is inconsistent")?;
                    if matches!(task.status.as_str(), "done" | "cancelled") {
                        return Err("focus points to a closed task".into());
                    }
                    Some(task)
                }
            };
            let sql = format!(
                "SELECT {columns} FROM tasks ORDER BY updated_at DESC, id DESC LIMIT 1"
            );
            let latest_task = conn.query_row(&sql, [], overview_task_from_row).optional()?;
            let counts = conn.query_row(
                "SELECT count(*) FILTER (WHERE status = 'todo') AS todo,
                    count(*) FILTER (WHERE status = 'active') AS active,
                    count(*) FILTER (WHERE status = 'blocked') AS blocked,
                    count(*) FILTER (WHERE status = 'done') AS done,
                    count(*) FILTER (WHERE status = 'cancelled') AS cancelled,
                    count(*) FILTER (WHERE kind = 'bug'
                        AND status NOT IN ('done', 'cancelled')) AS open_bugs
                 FROM tasks",
                [],
                |row| Ok(TaskCounts {
                    todo: row.get("todo")?, active: row.get("active")?,
                    blocked: row.get("blocked")?, done: row.get("done")?,
                    cancelled: row.get("cancelled")?, open_bugs: row.get("open_bugs")?,
                }),
            )?;
            Ok(ProjectOverview { project, focused_task, latest_task, counts })
        })
    }

    /// Follow explicit focus, or return bounded discovery context without
    /// selecting a task. Notes, evidence and spec files are never opened.
    pub fn resume(&self) -> Result<Resume> {
        self.resume_with_freshness(false)
    }

    pub fn resume_checked(&self) -> Result<Resume> {
        self.resume_with_freshness(true)
    }

    fn resume_with_freshness(&self, check_freshness: bool) -> Result<Resume> {
        self.read_project(|conn, project| {
            let task = match project.focused_task_id.as_deref() {
                None => None,
                Some(id) => {
                    let sql = format!("SELECT {CHECKPOINT_COLUMNS} FROM tasks WHERE id = ?1");
                    let mut statement = conn.prepare_cached(&sql)?;
                    let (mut task, scope, verified) = statement.query_row(params![id], |row| {
                        Ok((checkpoint_from_row(row)?,
                            row.get::<_, Option<String>>("source_scope")?,
                            row.get::<_, Option<String>>("verified_fingerprint")?))
                    })
                        .optional()?
                        .ok_or("focused task is missing; database state is inconsistent")?;
                    if matches!(task.status.as_str(), "done" | "cancelled") {
                        return Err("focus points to a closed task; explicit repair is required".into());
                    }
                    let current = if check_freshness && matches!(task.verification_status.as_str(), "passed" | "failed")
                        && scope.as_deref() == Some(crate::fingerprint::SCOPE) && verified.is_some()
                    {
                        self.capture_source().ok()
                    } else { None };
                    task.verification_freshness = crate::fingerprint::freshness(
                        &task.verification_status, scope.as_deref(), verified.as_deref(), current.as_ref(),
                    );
                    Some(task)
                }
            };
            let fallback = if task.is_none() {
                let columns = "id, substr(title, 1, 120) AS title, status, revision,
                    substr(summary, 1, 240) AS summary,
                    substr(next_action, 1, 200) AS next_action,
                    substr(blockers, 1, 200) AS blockers,
                    (length(title) > 120 OR length(summary) > 240
                     OR length(next_action) > 200 OR length(blockers) > 200) AS truncated";
                let sql = format!("SELECT {columns} FROM tasks
                    WHERE status NOT IN ('done', 'cancelled')
                    ORDER BY updated_at DESC, id DESC LIMIT 4");
                let mut candidates = conn.prepare_cached(&sql)?
                    .query_map([], resume_candidate_from_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let has_more = candidates.len() > 3;
                candidates.truncate(3);
                let sql = format!("SELECT {columns} FROM tasks
                    ORDER BY updated_at DESC, id DESC LIMIT 1");
                let latest = conn.query_row(&sql, [], resume_candidate_from_row).optional()?;
                let reason = if latest.is_none() { "empty_project" } else { "no_focus" };
                let latest_task = latest.filter(|latest|
                    !candidates.iter().any(|task| task.id == latest.id));
                Some(ResumeFallback { reason, candidates, has_more, latest_task })
            } else { None };
            Ok(Resume { project, task, fallback })
        })
    }

    /// Return None for an ID not found within the bound project.
    pub fn get_task(&self, id: &str) -> Result<Option<TaskDetails>> {
        self.get_task_with_freshness(id, false)
    }

    pub fn get_task_checked(&self, id: &str) -> Result<Option<TaskDetails>> {
        self.get_task_with_freshness(id, true)
    }

    fn get_task_with_freshness(&self, id: &str, check_freshness: bool) -> Result<Option<TaskDetails>> {
        validate_id(id)?;
        let mut details = self.read_project(|conn, _project| {
            let sql = format!(
                "SELECT {CHECKPOINT_COLUMNS}, notes, git_branch, git_head,
                 source_fingerprint, verification_notes,
                 verified_at, input_tokens, output_tokens, usage_status, models_used,
                 created_at, started_at, closed_at FROM tasks WHERE id = ?1"
            );
            let mut statement = conn.prepare_cached(&sql)?;
            Ok(statement.query_row(params![id], |row| {
                Ok(TaskDetails {
                    checkpoint: checkpoint_from_row(row)?,
                    notes: row.get("notes")?,
                    git_branch: row.get("git_branch")?,
                    git_head: row.get("git_head")?,
                    source_scope: row.get("source_scope")?,
                    source_fingerprint: row.get("source_fingerprint")?,
                    verification_notes: row.get("verification_notes")?,
                    verified_fingerprint: row.get("verified_fingerprint")?,
                    verified_at: row.get("verified_at")?,
                    input_tokens: row.get("input_tokens")?,
                    output_tokens: row.get("output_tokens")?,
                    usage_status: row.get("usage_status")?,
                    models_used_json: row.get("models_used")?,
                    created_at: row.get("created_at")?,
                    started_at: row.get("started_at")?,
                    closed_at: row.get("closed_at")?,
                })
            }).optional()?)
        })?;
        if let Some(task) = &mut details {
            let current = if check_freshness && matches!(task.checkpoint.verification_status.as_str(), "passed" | "failed")
                && task.source_scope.as_deref() == Some(crate::fingerprint::SCOPE)
                && task.verified_fingerprint.is_some()
            {
                self.capture_source().ok()
            } else { None };
            task.checkpoint.verification_freshness = crate::fingerprint::freshness(
                &task.checkpoint.verification_status, task.source_scope.as_deref(),
                task.verified_fingerprint.as_deref(), current.as_ref(),
            );
        }
        Ok(details)
    }

    /// A live page of 1..=50 rows. Concurrent updates may move rows across
    /// cursors: there is no snapshot spanning multiple page requests.
    pub fn list_tasks(
        &self,
        status: Option<&str>,
        limit: u32,
        cursor: Option<&TaskCursor>,
    ) -> Result<TaskPage> {
        self.list_tasks_filtered(status, None, limit, cursor)
    }

    /// Filter by kind as well as status without changing the basic list API.
    pub fn list_tasks_filtered(
        &self,
        status: Option<&str>,
        kind: Option<&str>,
        limit: u32,
        cursor: Option<&TaskCursor>,
    ) -> Result<TaskPage> {
        if !(1..=50).contains(&limit) {
            return Err("page limit must be between 1 and 50".into());
        }
        if let Some(status) = status {
            if !matches!(status, "todo" | "active" | "blocked" | "done" | "cancelled") {
                return Err("invalid task status".into());
            }
        }
        if let Some(cursor) = cursor {
            validate_id(&cursor.id)?;
            if cursor.status_filter.as_deref() != status {
                return Err("cursor status filter differs from this query".into());
            }
            if cursor.kind_filter.as_deref() != kind {
                return Err("cursor kind filter differs from this query".into());
            }
        }
        if let Some(kind) = kind {
            if !matches!(kind, "task" | "bug") {
                return Err("invalid task kind".into());
            }
        }

        self.read_project(|conn, project| {
            let mut predicates = Vec::new();
            let mut values = Vec::<Value>::new();
            if let Some(status) = status {
                predicates.push("status = ?");
                values.push(Value::Text(status.to_owned()));
            }
            if let Some(kind) = kind {
                predicates.push("kind = ?");
                values.push(Value::Text(kind.to_owned()));
            }
            if let Some(cursor) = cursor {
                if cursor.project_id != project.id {
                    return Err("cursor belongs to a different project".into());
                }
                predicates.push("(updated_at, id) < (?, ?)");
                values.push(Value::Integer(cursor.updated_at));
                values.push(Value::Text(cursor.id.clone()));
            }
            // One extra row tells us whether another page exists.
            values.push(Value::Integer(i64::from(limit) + 1));
            let filter = if predicates.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", predicates.join(" AND "))
            };
            // Only fixed SQL fragments are composed. User values are bound.
            // Filtered/unfiltered shapes match their respective schema indexes.
            let sql = format!(
                "SELECT id, title, kind, status, phase, revision, updated_at FROM tasks
                 {filter} ORDER BY updated_at DESC, id DESC LIMIT ?"
            );
            let mut statement = conn.prepare_cached(&sql)?;
            let mut items = statement.query_map(params_from_iter(values.iter()), |row| {
                Ok(TaskListItem {
                    id: row.get("id")?,
                    title: row.get("title")?,
                    kind: row.get("kind")?,
                    status: row.get("status")?,
                    phase: row.get("phase")?,
                    revision: row.get("revision")?,
                    updated_at: row.get("updated_at")?,
                })
            })?.collect::<rusqlite::Result<Vec<_>>>()?;
            let next_cursor = if items.len() > limit as usize {
                items.pop();
                items.last().map(|last| TaskCursor {
                    project_id: project.id,
                    status_filter: status.map(str::to_owned),
                    kind_filter: kind.map(str::to_owned),
                    updated_at: last.updated_at,
                    id: last.id.clone(),
                })
            } else {
                None
            };
            Ok(TaskPage { items, next_cursor })
        })
    }
}

fn checkpoint_from_row(row: &Row<'_>) -> rusqlite::Result<Checkpoint> {
    let verification_status: String = row.get("verification_status")?;
    let verification_freshness = match verification_status.as_str() {
        "not_run" | "not_required" => "not_applicable",
        _ => "unknown",
    };
    Ok(Checkpoint {
        id: row.get("id")?,
        title: row.get("title")?,
        kind: row.get("kind")?,
        spec_path: row.get("spec_path")?,
        status: row.get("status")?,
        phase: row.get("phase")?,
        summary: row.get("summary")?,
        next_action: row.get("next_action")?,
        blockers: row.get("blockers")?,
        verification_status,
        verification_freshness,
        revision: row.get("revision")?,
        updated_at: row.get("updated_at")?,
    })
}

fn validate_id(id: &str) -> Result<()> {
    if id.trim().is_empty() || id.chars().count() > 64 {
        return Err("task ID must contain 1..=64 characters".into());
    }
    Ok(())
}

fn serialize_models<S: serde::Serializer>(json: &str, serializer: S) -> std::result::Result<S::Ok, S::Error> {
    let models: Vec<String> = serde_json::from_str(json).map_err(serde::ser::Error::custom)?;
    serde::Serialize::serialize(&models, serializer)
}

fn list_item_from_row(row: &Row<'_>) -> rusqlite::Result<TaskListItem> {
    Ok(TaskListItem {
        id: row.get("id")?, title: row.get("title")?, kind: row.get("kind")?,
        status: row.get("status")?, phase: row.get("phase")?,
        revision: row.get("revision")?, updated_at: row.get("updated_at")?,
    })
}

fn overview_task_from_row(row: &Row<'_>) -> rusqlite::Result<OverviewTask> {
    Ok(OverviewTask {
        id: row.get("id")?, title: row.get("title")?, kind: row.get("kind")?,
        status: row.get("status")?, summary: row.get("summary")?,
        next_action: row.get("next_action")?, truncated: row.get("truncated")?,
        updated_at: row.get("updated_at")?,
    })
}

fn resume_candidate_from_row(row: &Row<'_>) -> rusqlite::Result<ResumeCandidate> {
    Ok(ResumeCandidate {
        id: row.get("id")?, title: row.get("title")?, status: row.get("status")?,
        revision: row.get("revision")?, summary: row.get("summary")?,
        next_action: row.get("next_action")?, blockers: row.get("blockers")?,
        truncated: row.get("truncated")?,
    })
}
