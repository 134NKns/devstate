use std::fs;
use std::path::PathBuf;
use std::process::Command;

use devstate::guide::retrieve;
use devstate::registry::Registry;
use devstate::store::Store;
use rusqlite::{Connection, params};
use serde_json::Value;
use uuid::Uuid;

struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("devstate-context-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}
fn cli(sandbox: &Sandbox, args: &[&str]) -> Value {
    let result = Command::new(env!("CARGO_BIN_EXE_devstate"))
        .current_dir(&sandbox.0).arg("--json").args(args).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    serde_json::from_slice(&result.stdout).unwrap()
}

#[test]
fn brief_context_is_smaller_and_detail_is_opt_in() {
    let sandbox = Sandbox::new();
    Store::init(&sandbox.0, "context").unwrap();
    let conn = Connection::open(sandbox.0.join(".devstate/state.db")).unwrap();
    conn.execute(
        "INSERT INTO tasks (id, title, notes, verification_status, verification_notes)
         VALUES ('a', 'Fix callback', ?1, 'not_required', ?2)",
        params!["notes-marker ".repeat(400), "evidence-marker ".repeat(150)],
    ).unwrap();
    let brief = cli(&sandbox, &["task", "a"]);
    let full = cli(&sandbox, &["task", "a", "--section", "full"]);
    assert_eq!(brief["section"], "brief");
    assert!(brief["task"].get("notes").is_none());
    assert!(brief["task"].get("verification_notes").is_none());
    assert!(brief["task"].get("source_fingerprint").is_none());
    assert_eq!(brief["task"]["has_notes"], true);
    assert!(full["task"]["notes"].as_str().unwrap().contains("notes-marker"));
    let notes = cli(&sandbox, &["task", "a", "--section", "notes"]);
    assert!(notes["task"]["notes"].is_string());
    assert!(notes["task"].get("verification_notes").is_none());
    assert!(notes.get("total_tokens").is_none());
    let evidence = cli(&sandbox, &["task", "a", "--section", "evidence"]);
    assert!(evidence["task"]["verification_notes"].is_string());
    assert!(evidence["task"].get("notes").is_none());
    let brief_bytes = brief.to_string().len();
    let full_bytes = full.to_string().len();
    println!("fixture payload bytes: brief={brief_bytes}, full={full_bytes}");
    assert!(brief_bytes < full_bytes / 2);
}

#[test]
fn lists_default_to_five_and_overview_deduplicates_focus() {
    let sandbox = Sandbox::new();
    Store::init(&sandbox.0, "context").unwrap();
    let conn = Connection::open(sandbox.0.join(".devstate/state.db")).unwrap();
    for id in ["a", "b", "c", "d", "e", "f"] {
        conn.execute("INSERT INTO tasks (id, title, created_at, updated_at) VALUES (?1, ?1, 1000, 2000)", params![id]).unwrap();
    }
    conn.execute("UPDATE project SET focused_task_id = 'f'", []).unwrap();
    let tasks = cli(&sandbox, &["tasks"]);
    assert_eq!(tasks["items"].as_array().unwrap().len(), 5);
    assert!(tasks["next_cursor"].is_object());
    let registry = sandbox.0.join("registry.db");
    Registry::open(&registry).unwrap().register(&sandbox.0).unwrap();
    let overview = cli(&sandbox, &["overview", "--registry", registry.to_str().unwrap()]);
    let data = &overview["entries"][0]["data"];
    assert_eq!(data["focused_task"]["id"], "f");
    assert!(data["latest_task"].is_null());
    assert_eq!(data["latest_task_same_as_focus"], true);
    assert!(data["project"].get("focused_task_id").is_none());
    assert!(data["project"].get("revision").is_none());
}

#[test]
fn usage_retrieval_is_bounded_and_has_explicit_unknowns() {
    let result = retrieve("token context resume init registry fingerprint limits", 5).unwrap();
    assert!(result.topics.len() <= 5);
    assert!(result.topics.iter().map(|topic| topic.content.chars().count()).sum::<usize>() <= 1800);
    assert!(result.more_matches);
    let result = retrieve("resume", 1).unwrap();
    assert_eq!(result.topics[0].id, "resume");
    assert!(result.topics[0].content.contains("devstate_resume"));
    let result = retrieve("zzz-nonexistent-483", 2).unwrap();
    assert!(result.topics.is_empty());
    assert!(result.available_topics.contains(&"start"));
    assert!(retrieve("", 2).is_err());
    assert!(retrieve("start", 6).is_err());
}

#[test]
fn guide_does_not_require_or_read_project_state() {
    let sandbox = Sandbox::new();
    let result = cli(&sandbox, &["guide", "tokens"]);
    assert_eq!(result["topics"][0]["id"], "tokens");
    let content = result["topics"][0]["content"].as_str().unwrap();
    assert!(content.contains("same language as the user's message"));
    assert!(content.contains("structured fields/enums"));
    assert!(!sandbox.0.join(".devstate").exists());
    let result = cli(&sandbox, &["guide", "บันทึก"]);
    assert_eq!(result["topics"][0]["id"], "limits");
    assert!(result["topics"][0]["content"].as_str().unwrap().contains("NOT implemented"));
}
