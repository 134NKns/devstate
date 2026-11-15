use std::fs;
use std::path::{Path, PathBuf};

use devstate::registry::{OverviewEntry, Registry};
use devstate::store::Store;
use rusqlite::{Connection, params};
use uuid::Uuid;
use devstate::fingerprint::{capture, freshness, SCOPE};

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(root).args(args).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn fingerprint_detects_worktree_index_untracked_and_deletion() {
    let sandbox = Sandbox::new();
    let root = sandbox.project("source");
    git(&root, &["init", "--quiet"]);
    fs::write(root.join("code.rs"), "first").unwrap();
    git(&root, &["add", "code.rs"]);
    let baseline = capture(&root).unwrap();
    assert_eq!(baseline, capture(&root).unwrap());
    assert_eq!(baseline.fingerprint.len(), 64);

    fs::write(root.join("code.rs"), "other").unwrap();
    assert_ne!(baseline, capture(&root).unwrap());
    git(&root, &["add", "code.rs"]);
    fs::write(root.join("code.rs"), "first").unwrap();
    // Same working content, different staged content must differ too.
    assert_ne!(baseline, capture(&root).unwrap());
    git(&root, &["add", "code.rs"]);
    assert_eq!(baseline, capture(&root).unwrap());

    fs::write(root.join("untracked.rs"), "new").unwrap();
    assert_ne!(baseline, capture(&root).unwrap());
    fs::remove_file(root.join("untracked.rs")).unwrap();
    assert_eq!(baseline, capture(&root).unwrap());
    fs::remove_file(root.join("code.rs")).unwrap();
    assert_ne!(baseline, capture(&root).unwrap());
}

#[test]
fn fingerprint_excludes_state_outputs_secrets_and_ignored_files() {
    let sandbox = Sandbox::new();
    let root = sandbox.project("source");
    git(&root, &["init", "--quiet"]);
    fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
    fs::write(root.join("code.rs"), "source").unwrap();
    let baseline = capture(&root).unwrap();
    fs::write(root.join(".env.local"), "secret fixture").unwrap();
    fs::write(root.join("ignored.txt"), "ignored fixture").unwrap();
    fs::create_dir(root.join("node_modules")).unwrap();
    fs::write(root.join("node_modules/output"), "generated").unwrap();
    fs::write(root.join(".devstate/extra"), "state fixture").unwrap();
    assert_eq!(baseline, capture(&root).unwrap());
    fs::write(root.join(".gitignore"), "ignored.txt\nnew-rule\n").unwrap();
    assert_ne!(baseline, capture(&root).unwrap());
}

#[test]
fn freshness_in_queries_detects_edits_without_commits() {
    let sandbox = Sandbox::new();
    let root = sandbox.project("source");
    git(&root, &["init", "--quiet"]);
    fs::write(root.join("code.rs"), "first").unwrap();
    task(&root, "a", "Check code", "task");
    let store = Store::open(&root).unwrap();
    let baseline = store.capture_source().unwrap();
    let conn = db(&root);
    conn.execute(
        "UPDATE tasks SET source_scope = ?1, source_fingerprint = ?2,
         verified_fingerprint = ?2, verification_status = 'passed',
         verification_notes = 'Fixture evidence', verified_at = 1800",
        params![baseline.scope, baseline.fingerprint],
    ).unwrap();
    conn.execute("UPDATE project SET focused_task_id = 'a'", []).unwrap();
    assert_eq!(store.resume().unwrap().task.unwrap().verification_freshness, "unknown");
    assert_eq!(store.resume_checked().unwrap().task.unwrap().verification_freshness, "current");
    assert_eq!(store.get_task_checked("a").unwrap().unwrap().checkpoint.verification_freshness, "current");
    fs::write(root.join("code.rs"), "other").unwrap();
    assert_eq!(store.resume_checked().unwrap().task.unwrap().verification_freshness, "stale");
    assert_eq!(store.get_task_checked("a").unwrap().unwrap().checkpoint.verification_freshness, "stale");
    assert_eq!(freshness("passed", Some("unsupported"), Some(&baseline.fingerprint), Some(&baseline)), "unknown");
    assert_eq!(freshness("passed", Some(SCOPE), None, Some(&baseline)), "unknown");
    assert_eq!(freshness("passed", Some(SCOPE), Some(&baseline.fingerprint), None), "unknown");
    assert_eq!(freshness("not_run", None, None, None), "not_applicable");
}

#[test]
fn unsupported_sources_remain_unknown() {
    let sandbox = Sandbox::new();
    let root = sandbox.project("source");
    assert!(capture(&root).is_err());
    git(&root, &["init", "--quiet"]);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/outside/project", root.join("link")).unwrap();
        assert!(capture(&root).is_err());
    }
}

struct Sandbox(PathBuf);

impl Sandbox {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("devstate-test-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }

    fn project(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir(&path).unwrap();
        let store = Store::init(&path, name).unwrap();
        assert_eq!(store.project().unwrap().name, name);
        path
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// Fixtures write directly because task mutation APIs are not implemented yet.
fn db(root: &Path) -> Connection {
    Connection::open(root.join(".devstate/state.db")).unwrap()
}

fn task(root: &Path, id: &str, title: &str, kind: &str) {
    db(root).execute(
        "INSERT INTO tasks (id, title, kind, created_at, updated_at)
         VALUES (?1, ?2, ?3, 1000, 2000)",
        params![id, title, kind],
    ).unwrap();
}

#[test]
fn init_is_atomic_bound_and_does_not_overwrite() {
    let sandbox = Sandbox::new();
    let root = sandbox.project("alpha");
    let store = Store::open(&root).unwrap();
    let original = store.project().unwrap().id;
    assert!(store.resume().unwrap().task.is_none());
    assert!(Store::init(&root, "replacement").is_err());
    assert_eq!(Store::open(&root).unwrap().project().unwrap().id, original);

    // A validation failure must happen before creating a database.
    let invalid = sandbox.0.join("invalid");
    fs::create_dir(&invalid).unwrap();
    assert!(Store::init(&invalid, " ").is_err());
    assert!(!invalid.join(".devstate/state.db").exists());

    // Opening must not silently initialize a missing project database.
    assert!(Store::open(&invalid).is_err());
    assert!(!invalid.join(".devstate/state.db").exists());
}

#[test]
fn reads_reject_rebinding_and_unsupported_versions() {
    let sandbox = Sandbox::new();
    let root = sandbox.project("alpha");
    let store = Store::open(&root).unwrap();
    db(&root).execute("UPDATE project SET root_path = '/different/project'", []).unwrap();
    assert!(store.resume().is_err());
    assert!(Store::open(&root).is_err());
    db(&root).execute("UPDATE project SET root_path = ?1", params![root.to_str().unwrap()]).unwrap();
    db(&root).pragma_update(None, "user_version", 99).unwrap();
    assert!(Store::open(&root).is_err());
}

#[test]
fn resume_follows_focus_and_details_keep_usage_unknown() {
    let sandbox = Sandbox::new();
    let root = sandbox.project("alpha");
    task(&root, "a", "Focused bug", "bug");
    task(&root, "z", "Newer-looking task", "task");
    let conn = db(&root);
    conn.execute(
        "UPDATE tasks SET status = 'active', started_at = 1500,
         next_action = 'Fix callback', summary = 'API connected', notes = 'Detail only'
         WHERE id = 'a'", [],
    ).unwrap();
    conn.execute("UPDATE project SET focused_task_id = 'a'", []).unwrap();
    let store = Store::open(&root).unwrap();
    let focused = store.resume().unwrap().task.unwrap();
    assert_eq!(focused.id, "a");
    assert_eq!(focused.kind, "bug");
    assert_eq!(focused.summary, "API connected");
    let details = store.get_task("a").unwrap().unwrap();
    assert_eq!(details.notes, "Detail only");
    assert_eq!(details.input_tokens, None);
    assert_eq!(details.usage_status, "unknown");
    assert_eq!(details.elapsed_ms(2500), Some(1000));
    assert_eq!(details.elapsed_ms(1000), None);
    assert!(store.get_task("missing").unwrap().is_none());

    conn.execute("UPDATE project SET focused_task_id = NULL", []).unwrap();
    assert!(store.resume().unwrap().task.is_none());
}

#[test]
fn listing_handles_ties_filters_and_cursor_identity() {
    let sandbox = Sandbox::new();
    let root = sandbox.project("alpha");
    for (id, kind) in [("a", "bug"), ("b", "task"), ("c", "bug")] {
        task(&root, id, id, kind);
    }
    let store = Store::open(&root).unwrap();
    let first = store.list_tasks(None, 2, None).unwrap();
    assert_eq!(first.items.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(), ["c", "b"]);
    let cursor = first.next_cursor.unwrap();
    let second = store.list_tasks(None, 2, Some(&cursor)).unwrap();
    assert_eq!(second.items[0].id, "a");
    assert!(second.next_cursor.is_none());
    assert!(store.list_tasks(Some("todo"), 2, Some(&cursor)).is_err());
    assert!(store.list_tasks(None, 0, None).is_err());
    assert!(store.list_tasks(None, 51, None).is_err());
    assert!(store.list_tasks_filtered(None, Some("invalid"), 2, None).is_err());

    let bugs = store.list_tasks_filtered(None, Some("bug"), 1, None).unwrap();
    assert_eq!(bugs.items[0].id, "c");
    let bug_cursor = bugs.next_cursor.unwrap();
    assert!(store.list_tasks(None, 2, Some(&bug_cursor)).is_err());
    let next_bug = store.list_tasks_filtered(None, Some("bug"), 1, Some(&bug_cursor)).unwrap();
    assert_eq!(next_bug.items[0].id, "a");
    assert!(next_bug.next_cursor.is_none());

    let other = sandbox.project("beta");
    assert!(Store::open(other).unwrap().list_tasks(None, 2, Some(&cursor)).is_err());
    assert_eq!(store.recent_tasks(1).unwrap().items[0].id, "c");
}

#[test]
fn search_is_literal_including_thai_notes_and_bounded() {
    let sandbox = Sandbox::new();
    let root = sandbox.project("alpha");
    task(&root, "a", "100% literal", "task");
    task(&root, "b", "100 percent", "task");
    db(&root).execute("UPDATE tasks SET notes = 'หมายเหตุ ข้อผิดพลาด callback' WHERE id = 'b'", []).unwrap();
    let store = Store::open(root).unwrap();
    assert_eq!(store.search_tasks("%", 5).unwrap().items[0].id, "a");
    assert_eq!(store.search_tasks("ข้อผิด", 5).unwrap().items[0].id, "b");
    assert_eq!(store.search_tasks("LITERAL", 5).unwrap().items[0].id, "a");
    let bounded = store.search_tasks("100", 1).unwrap();
    assert_eq!(bounded.items.len(), 1);
    assert!(bounded.truncated);
    assert!(store.search_tasks("_", 5).unwrap().items.is_empty());
    assert!(store.search_tasks("  ", 5).is_err());
    assert!(store.search_tasks("100", 51).is_err());
}

#[test]
fn schema_rejects_invalid_usage_and_stale_completion() {
    let sandbox = Sandbox::new();
    let root = sandbox.project("alpha");
    task(&root, "a", "Task", "task");
    let conn = db(&root);
    assert!(conn.execute("UPDATE tasks SET input_tokens = -1", []).is_err());
    assert!(conn.execute("UPDATE tasks SET usage_status = 'complete'", []).is_err());
    assert!(conn.execute("UPDATE tasks SET models_used = 'invalid-json'", []).is_err());
    assert!(conn.execute("UPDATE tasks SET models_used = '{}'", []).is_err());
    conn.execute(
        "UPDATE tasks SET usage_status = 'complete', input_tokens = 100,
         output_tokens = 25, models_used = '[\"provider/model\"]'", [],
    ).unwrap();
    let old = "a".repeat(64);
    let current = "b".repeat(64);
    conn.execute(
        "UPDATE tasks SET source_scope = 'test-scope', source_fingerprint = ?1,
         verification_status = 'passed', verification_notes = 'Fixture test evidence',
         verified_at = 1800, verified_fingerprint = ?2, started_at = 1500",
        params![current, old],
    ).unwrap();
    assert!(conn.execute("UPDATE tasks SET status = 'done', closed_at = 2000", []).is_err());
    conn.execute("UPDATE tasks SET verified_fingerprint = source_fingerprint", []).unwrap();
    conn.execute("UPDATE tasks SET status = 'done', closed_at = 2000", []).unwrap();
    let details = Store::open(root).unwrap().get_task("a").unwrap().unwrap();
    assert_eq!(details.elapsed_ms(9999), Some(500));
    assert_eq!(details.input_tokens, Some(100));
    assert_eq!(details.checkpoint.verification_freshness, "unknown");
}

#[test]
fn registry_is_paged_isolated_and_tolerates_unavailable_projects() {
    let sandbox = Sandbox::new();
    let alpha = sandbox.project("alpha");
    let beta = sandbox.project("beta");
    task(&alpha, "a", "Alpha-only bug", "bug");
    task(&beta, "b", "Beta-only task", "task");
    let registry = Registry::open(sandbox.0.join("registry.db")).unwrap();
    registry.register(&alpha).unwrap();
    registry.register(&alpha).unwrap();
    registry.register(&beta).unwrap();
    let page = registry.overview(1, None).unwrap();
    match &page.entries[0] {
        OverviewEntry::Available(summary) => {
            assert_eq!(summary.project.name, "alpha");
            assert_eq!(summary.counts.open_bugs, 1);
            assert_eq!(summary.latest_task.as_ref().unwrap().id, "a");
            assert!(summary.focused_task.is_none());
        }
        other => panic!("unexpected entry: {other:?}"),
    }
    let second = registry.overview(1, page.next_cursor.as_deref()).unwrap();
    assert!(second.next_cursor.is_none());
    assert!(matches!(&second.entries[0], OverviewEntry::Available(summary) if summary.project.name == "beta"));
    assert_eq!(Store::open(&alpha).unwrap().get_task("b").unwrap().is_none(), true);

    // Simulate a moved project without deleting source files.
    fs::rename(&alpha, sandbox.0.join("alpha-moved")).unwrap();
    let page = registry.overview(10, None).unwrap();
    assert!(matches!(page.entries[0], OverviewEntry::Unavailable { .. }));
    assert!(matches!(page.entries[1], OverviewEntry::Available(_)));
    assert!(registry.unregister(alpha.to_str().unwrap()).unwrap());
    assert!(!registry.unregister(alpha.to_str().unwrap()).unwrap());
    assert!(registry.overview(0, None).is_err());
}

#[test]
fn registry_rejects_replaced_identity_and_unrelated_database() {
    let sandbox = Sandbox::new();
    let root = sandbox.project("alpha");
    let registry = Registry::open(sandbox.0.join("registry.db")).unwrap();
    registry.register(&root).unwrap();
    db(&root).execute("UPDATE project SET project_id = ?1", params![Uuid::new_v4().to_string()]).unwrap();
    assert!(registry.register(&root).is_err());
    assert!(matches!(registry.overview(10, None).unwrap().entries[0], OverviewEntry::Unavailable { .. }));

    let unrelated = sandbox.0.join("unrelated.db");
    Connection::open(&unrelated).unwrap().execute("CREATE TABLE keep_me (id INTEGER)", []).unwrap();
    assert!(Registry::open(&unrelated).is_err());
    assert_eq!(Connection::open(unrelated).unwrap().query_row(
        "SELECT count(*) FROM sqlite_schema WHERE name = 'keep_me'", [], |row| row.get::<_, i64>(0),
    ).unwrap(), 1);
}
