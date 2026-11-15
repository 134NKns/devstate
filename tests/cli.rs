use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rusqlite::{Connection, params};
use serde_json::Value;
use uuid::Uuid;

struct Sandbox(PathBuf);

impl Sandbox {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("devstate-cli-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }

    fn init(&self) {
        success(&self.0, &["init", "--name", "CLI fixture"]);
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_devstate"))
        .current_dir(root).arg("--json").args(args).output().unwrap()
}

fn success(root: &Path, args: &[&str]) -> Value {
    let output = run(root, args);
    assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn help_and_argument_errors_do_not_initialize() {
    let sandbox = Sandbox::new();
    let help = run(&sandbox.0, &["--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("overview"));
    let invalid = run(&sandbox.0, &["tasks", "--limit", "0"]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());
    assert!(!sandbox.0.join(".devstate").exists());
}

#[test]
fn init_resume_and_runtime_errors_use_clean_json_channels() {
    let sandbox = Sandbox::new();
    sandbox.init();
    let resumed = success(&sandbox.0, &["resume"]);
    assert!(resumed["task"].is_null());
    assert_eq!(resumed["project"]["name"], "CLI fixture");
    let repeated = run(&sandbox.0, &["init", "--name", "replacement"]);
    assert_eq!(repeated.status.code(), Some(1));
    assert!(repeated.stdout.is_empty());
    assert!(serde_json::from_slice::<Value>(&repeated.stderr).unwrap()["error"].is_string());
    let missing = run(&sandbox.0, &["task", "missing"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(missing.stdout.is_empty());
    assert!(serde_json::from_slice::<Value>(&missing.stderr).unwrap()["error"].as_str().unwrap().contains("not found"));
    let pretty = run(&sandbox.0, &["resume", "--pretty"]);
    assert!(pretty.status.success());
    assert_eq!(serde_json::from_slice::<Value>(&pretty.stdout).unwrap(), resumed);
}

#[test]
fn task_output_includes_models_tokens_elapsed_and_pagination() {
    let sandbox = Sandbox::new();
    sandbox.init();
    let conn = Connection::open(sandbox.0.join(".devstate/state.db")).unwrap();
    for (id, kind) in [("a", "bug"), ("b", "task")] {
        conn.execute(
            "INSERT INTO tasks (id, title, kind, created_at, updated_at)
             VALUES (?1, ?1, ?2, 1000, 2000)", params![id, kind],
        ).unwrap();
    }
    conn.execute(
        "UPDATE tasks SET input_tokens = 100, output_tokens = 25, usage_status = 'complete',
         models_used = '[\"provider/model\"]', started_at = 1500,
         summary = ?1, notes = 'หมายเหตุ callback' WHERE id = 'a'",
        params!["ก".repeat(300)],
    ).unwrap();
    conn.execute("UPDATE project SET focused_task_id = 'a'", []).unwrap();
    let details = success(&sandbox.0, &["task", "a"]);
    assert_eq!(details["total_tokens"], 125);
    assert!(details["elapsed_ms"].as_i64().unwrap() > 0);
    assert_eq!(details["task"]["models_used"][0], "provider/model");
    assert!(details["task"].get("models_used_json").is_none());
    let resumed = success(&sandbox.0, &["resume"]);
    assert_eq!(resumed["truncated_fields"][0], "summary");
    assert_eq!(resumed["task"]["summary"].as_str().unwrap().chars().count(), 240);
    assert!(resumed["task"].get("notes").is_none());
    assert_eq!(success(&sandbox.0, &["tasks", "--kind", "bug"])["items"][0]["id"], "a");
    assert_eq!(success(&sandbox.0, &["search", "หมายเหตุ"])["items"][0]["id"], "a");
    assert_eq!(success(&sandbox.0, &["recent", "--limit", "1"])["items"][0]["id"], "b");
    let page = success(&sandbox.0, &["tasks", "--limit", "1"]);
    let cursor = page["next_cursor"].to_string();
    let next = success(&sandbox.0, &["tasks", "--limit", "1", "--cursor", &cursor]);
    assert_eq!(next["items"][0]["id"], "a");
    assert!(next["next_cursor"].is_null());
    assert!(!run(&sandbox.0, &["tasks", "--cursor", "invalid-json"]).status.success());
}

#[test]
fn explicit_registry_commands_work_without_local_focus() {
    let sandbox = Sandbox::new();
    sandbox.init();
    let registry_path = sandbox.0.join("registry.db");
    let registry = registry_path.to_str().unwrap();
    let missing = run(&sandbox.0, &["overview", "--registry", registry]);
    assert!(!missing.status.success());
    assert!(!registry_path.exists());
    assert!(!run(&sandbox.0, &["register"]).status.success());
    success(&sandbox.0, &["register", "--registry", registry]);
    let outside = Sandbox::new();
    let overview = success(&outside.0, &["overview", "--registry", registry]);
    assert_eq!(overview["entries"][0]["availability"], "available");
    assert_eq!(overview["entries"][0]["data"]["project"]["name"], "CLI fixture");
    assert!(!outside.0.join(".devstate").exists());
    let removed = success(&outside.0, &["unregister", sandbox.0.to_str().unwrap(), "--registry", registry]);
    assert_eq!(removed["removed"], true);
    assert!(sandbox.0.join(".devstate/state.db").exists());
}

#[test]
fn explicit_root_and_fingerprint_errors_do_not_select_other_projects() {
    let sandbox = Sandbox::new();
    sandbox.init();
    let outside = Sandbox::new();
    let result = success(&outside.0, &["--root", sandbox.0.to_str().unwrap(), "resume"]);
    assert_eq!(result["project"]["root_path"], sandbox.0.to_str().unwrap());
    assert!(!run(&outside.0, &["resume"]).status.success());
    let unsupported = run(&sandbox.0, &["fingerprint"]);
    assert_eq!(unsupported.status.code(), Some(1));
    assert!(unsupported.stdout.is_empty());
    let git_init = Command::new("git").current_dir(&sandbox.0).args(["init", "--quiet"]).output().unwrap();
    assert!(git_init.status.success());
    let snapshot = success(&sandbox.0, &["fingerprint"]);
    assert_eq!(snapshot["scope"], "git-source-v1");
    assert_eq!(snapshot["fingerprint"].as_str().unwrap().len(), 64);
}

fn human(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_devstate"))
        .current_dir(root).env("NO_COLOR", "1").args(args).output().unwrap()
}

#[test]
fn human_views_show_labels_thai_and_safe_terminal_text() {
    let sandbox = Sandbox::new();
    sandbox.init();
    let no_args = human(&sandbox.0, &[]);
    assert!(no_args.status.success());
    assert!(String::from_utf8_lossy(&no_args.stdout).contains("Pick up where you left off"));
    let empty = human(&sandbox.0, &["resume"]);
    assert!(String::from_utf8_lossy(&empty.stdout).contains("No focused task"));
    let conn = Connection::open(sandbox.0.join(".devstate/state.db")).unwrap();
    conn.execute(
        "INSERT INTO tasks (id, title, kind, summary, next_action, blockers, notes)
         VALUES ('thai-task', ?1, 'bug', 'เชื่อม API แล้ว', 'แก้ callback', 'รอข้อมูล', 'หมายเหตุ')",
        params!["งานภาษาไทย\u{1b}[31m\u{202e}"],
    ).unwrap();
    conn.execute("UPDATE project SET focused_task_id = 'thai-task'", []).unwrap();
    let resumed = human(&sandbox.0, &["resume"]);
    let text = String::from_utf8(resumed.stdout).unwrap();
    for label in ["devstate / Resume", "งานภาษาไทย", "Next action", "แก้ callback", "Blockers", "รอข้อมูล"] {
        assert!(text.contains(label), "missing {label}: {text}");
    }
    assert!(!text.contains('\u{1b}'));
    assert!(!text.contains('\u{202e}'));
    let task = human(&sandbox.0, &["task", "thai-task", "--section", "full"]);
    let text = String::from_utf8(task.stdout).unwrap();
    assert!(text.contains("unknown total"));
    assert!(text.contains("Elapsed"));
    assert!(text.contains("หมายเหตุ"));
    let list = human(&sandbox.0, &["tasks"]);
    let text = String::from_utf8(list.stdout).unwrap();
    assert!(text.contains("STATE        KIND   TASK"));
    assert!(text.contains("thai-task"));
    assert!(text.contains("งานภาษาไทย"));
    let error = human(&sandbox.0, &["task", "missing"]);
    assert_eq!(error.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&error.stderr).starts_with("devstate: "));
    assert!(error.stdout.is_empty());
}

#[test]
fn human_overview_includes_counts_paths_and_next_action() {
    let sandbox = Sandbox::new();
    sandbox.init();
    let conn = Connection::open(sandbox.0.join(".devstate/state.db")).unwrap();
    conn.execute(
        "INSERT INTO tasks (id, title, kind, next_action) VALUES ('a', 'แก้บัค', 'bug', 'ตรวจ callback')", [],
    ).unwrap();
    conn.execute("UPDATE project SET focused_task_id = 'a'", []).unwrap();
    let path = sandbox.0.join("registry.db");
    let registry = path.to_str().unwrap();
    success(&sandbox.0, &["register", "--registry", registry]);
    let result = human(&sandbox.0, &["overview", "--registry", registry]);
    assert!(result.status.success());
    let text = String::from_utf8(result.stdout).unwrap();
    for label in ["OPEN  BUGS", "CLI fixture", "แก้บัค", "ตรวจ callback", "1 project(s)"] {
        assert!(text.contains(label), "missing {label}: {text}");
    }
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn cli_task_lifecycle_persists_between_processes() {
    let sandbox = Sandbox::new();
    sandbox.init();
    let created = success(&sandbox.0, &["create", "--title", "Wire callback", "--kind", "bug"]);
    let id = created["id"].as_str().unwrap();
    assert_eq!(created["revision"], 1);
    let focused = success(&sandbox.0, &["focus", id, "--revision", "1"]);
    assert_eq!(focused["revision"], 2);
    let checkpoint = success(&sandbox.0, &["checkpoint", id, "--revision", "1", "--summary", "API connected", "--next", "Wire frontend"]);
    assert_eq!(checkpoint["revision"], 2);
    let resumed = success(&sandbox.0, &["resume"]);
    assert_eq!(resumed["task"]["id"], id);
    assert_eq!(resumed["task"]["next_action"], "Wire frontend");
    assert_eq!(resumed["project"]["revision"], 2);
    let stale = run(&sandbox.0, &["checkpoint", id, "--revision", "1", "--summary", "Stale", "--next", "Wrong"]);
    assert_eq!(stale.status.code(), Some(1));
    let closed = success(&sandbox.0, &["close", id, "--revision", "2", "--project-revision", "2",
        "--summary", "Documentation delivered", "--reason", "Documentation only; no runtime checks needed"]);
    assert_eq!(closed["project_revision"], 3);
    assert!(success(&sandbox.0, &["resume"])["task"].is_null());
    assert_eq!(success(&sandbox.0, &["task", id])["task"]["checkpoint"]["status"], "done");
}
