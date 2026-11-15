use std::fs;
use std::path::PathBuf;

use devstate::mutations::{Checkpoint, CloseTask, NewTask, Verification};
use devstate::store::Store;
use uuid::Uuid;

struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("devstate-write-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        Store::init(&root, "writes").unwrap();
        Self(root)
    }
    fn store(&self) -> Store { Store::open(&self.0).unwrap() }
}
impl Drop for Sandbox {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}
fn new_task(title: &str) -> NewTask {
    NewTask { title: title.into(), kind: "task".into(), spec_path: None }
}
fn checkpoint(id: &str, revision: i64) -> Checkpoint {
    Checkpoint { id: id.into(), expected_revision: revision, summary: "API connected".into(),
        next_action: "Wire frontend".into(), status: "active".into(), phase: "implement".into(),
        blockers: String::new(), notes: Some("Keep existing contract".into()), verification: None }
}
fn close(id: &str, revision: i64, project_revision: i64) -> CloseTask {
    CloseTask { id: id.into(), expected_revision: revision, expected_project_revision: project_revision,
        summary: "Delivered documented change".into(), status: "done".into(),
        not_required_reason: Some("Documentation-only change; no runtime check required".into()) }
}

#[test]
fn persisted_task_cycle_survives_reopening_and_clears_focus() {
    let sandbox = Sandbox::new();
    let created = sandbox.store().create_task(new_task("Finish callback")).unwrap();
    assert_eq!(created.revision, 1);
    let focused = sandbox.store().set_focus(Some(&created.id), 1).unwrap();
    assert_eq!(focused.revision, 2);
    let saved = sandbox.store().checkpoint(checkpoint(&created.id, 1)).unwrap();
    assert_eq!(saved.revision, 2);
    let resumed = sandbox.store().resume().unwrap().task.unwrap();
    assert_eq!(resumed.next_action, "Wire frontend");
    let started = sandbox.store().get_task(&created.id).unwrap().unwrap().started_at;
    let mut blocked = checkpoint(&created.id, 2);
    blocked.status = "blocked".into();
    blocked.blockers = "Waiting on callback contract".into();
    blocked.notes = None;
    sandbox.store().checkpoint(blocked).unwrap();
    let task = sandbox.store().get_task(&created.id).unwrap().unwrap();
    assert_eq!(task.started_at, started);
    assert_eq!(task.notes, "Keep existing contract");
    let closed = sandbox.store().close_task(close(&created.id, 3, 2)).unwrap();
    assert_eq!(closed.revision, 4);
    assert_eq!(closed.project_revision, 3);
    assert!(sandbox.store().resume().unwrap().task.is_none());
    let task = sandbox.store().get_task(&created.id).unwrap().unwrap();
    assert_eq!(task.checkpoint.status, "done");
    assert!(task.closed_at.is_some());
    assert!(sandbox.store().set_focus(Some(&created.id), 3).is_err());
}

#[test]
fn stale_writes_and_invalid_evidence_roll_back() {
    let sandbox = Sandbox::new();
    let mut first = sandbox.store();
    let mut second = sandbox.store();
    let task = first.create_task(new_task("Concurrent work")).unwrap();
    first.checkpoint(checkpoint(&task.id, 1)).unwrap();
    assert!(second.checkpoint(checkpoint(&task.id, 1)).is_err());
    let mut invalid = checkpoint(&task.id, 2);
    invalid.summary = "Must not persist".into();
    invalid.verification = Some(Verification { status: "not_required".into(), notes: "x".repeat(4001), before: None });
    assert!(first.checkpoint(invalid).is_err());
    let stored = second.get_task(&task.id).unwrap().unwrap();
    assert_eq!(stored.checkpoint.revision, 2);
    assert_eq!(stored.checkpoint.summary, "API connected");
    first.set_focus(Some(&task.id), 1).unwrap();
    assert!(second.set_focus(None, 1).is_err());
    assert_eq!(second.project().unwrap().focused_task_id.as_deref(), Some(task.id.as_str()));
}

#[test]
fn close_conflicts_do_not_close_task_or_replace_another_focus() {
    let sandbox = Sandbox::new();
    let mut store = sandbox.store();
    let a = store.create_task(new_task("A")).unwrap();
    let b = store.create_task(new_task("B")).unwrap();
    store.set_focus(Some(&a.id), 1).unwrap();
    assert!(store.close_task(close(&a.id, 1, 1)).is_err());
    assert_eq!(store.get_task(&a.id).unwrap().unwrap().checkpoint.status, "todo");
    store.set_focus(Some(&b.id), 2).unwrap();
    store.close_task(close(&a.id, 1, 3)).unwrap();
    assert_eq!(store.project().unwrap().focused_task_id.as_deref(), Some(b.id.as_str()));
    assert_eq!(store.project().unwrap().revision, 3);
    assert!(store.close_task(close(&a.id, 2, 3)).is_err());
}

#[test]
fn completion_rechecks_source_and_never_runs_checks_itself() {
    let sandbox = Sandbox::new();
    assert!(std::process::Command::new("git").current_dir(&sandbox.0).args(["init", "--quiet"]).status().unwrap().success());
    fs::write(sandbox.0.join("code.rs"), "original").unwrap();
    let mut store = sandbox.store();
    let task = store.create_task(new_task("Verified work")).unwrap();
    let before = store.capture_source().unwrap();
    let mut data = checkpoint(&task.id, 1);
    data.verification = Some(Verification { status: "passed".into(), notes: "Fixture evidence, not an executed command".into(), before: Some(before.fingerprint) });
    store.checkpoint(data).unwrap();
    assert_eq!(store.get_task(&task.id).unwrap().unwrap().checkpoint.verification_freshness, "unknown");
    assert_eq!(store.get_task_checked(&task.id).unwrap().unwrap().checkpoint.verification_freshness, "current");
    fs::write(sandbox.0.join("code.rs"), "edited").unwrap();
    let mut completion = close(&task.id, 2, 1);
    completion.not_required_reason = None;
    assert!(store.close_task(completion).is_err());
    assert_eq!(store.get_task(&task.id).unwrap().unwrap().checkpoint.status, "active");
    fs::write(sandbox.0.join("code.rs"), "original").unwrap();
    let mut completion = close(&task.id, 2, 1);
    completion.not_required_reason = None;
    store.close_task(completion).unwrap();
    assert_eq!(store.get_task(&task.id).unwrap().unwrap().checkpoint.status, "done");
}

#[test]
fn changed_before_fingerprint_and_path_escape_are_rejected() {
    let sandbox = Sandbox::new();
    let mut store = sandbox.store();
    let mut invalid = new_task("Outside spec");
    invalid.spec_path = Some("../outside.md".into());
    assert!(store.create_task(invalid).is_err());
    assert!(store.list_tasks(None, 5, None).unwrap().items.is_empty());
    assert!(std::process::Command::new("git").current_dir(&sandbox.0).args(["init", "--quiet"]).status().unwrap().success());
    fs::write(sandbox.0.join("code.rs"), "before").unwrap();
    let task = store.create_task(new_task("Changed source")).unwrap();
    let before = store.capture_source().unwrap();
    fs::write(sandbox.0.join("code.rs"), "after").unwrap();
    let mut data = checkpoint(&task.id, 1);
    data.verification = Some(Verification { status: "passed".into(), notes: "Fixture result".into(), before: Some(before.fingerprint) });
    assert!(store.checkpoint(data).is_err());
    assert_eq!(store.get_task(&task.id).unwrap().unwrap().checkpoint.revision, 1);
}

#[test]
fn read_only_store_cannot_mutate() {
    let sandbox = Sandbox::new();
    let mut read_only = Store::open_read_only(&sandbox.0).unwrap();
    assert!(read_only.create_task(new_task("Must not persist")).is_err());
    assert!(sandbox.store().list_tasks(None, 5, None).unwrap().items.is_empty());
}
