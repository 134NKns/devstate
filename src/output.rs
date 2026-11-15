//! Human CLI views. Inherit terminal colors; status always has a text label.
//! Only headings use optional bold. Stored content cannot emit terminal escapes.

use std::fmt::Write;

use serde_json::Value;

use crate::Commands;

pub fn render(command: &Commands, value: &Value, styled: bool) -> String {
    let mut out = String::new();
    let title = match command {
        Commands::Mcp => "MCP",
        Commands::Guide { .. } => "Usage guide",
        Commands::Init { .. } => "Project initialized",
        Commands::Resume { .. } => "Resume",
        Commands::Create { .. } => "Task created",
        Commands::Focus { .. } => "Focus updated",
        Commands::Checkpoint { .. } => "Checkpoint saved",
        Commands::Close { .. } => "Task closed",
        Commands::Task { .. } => "Task",
        Commands::Tasks { .. } => "Tasks",
        Commands::Recent { .. } => "Recent work",
        Commands::Search { .. } => "Search results",
        Commands::Fingerprint => "Source fingerprint",
        Commands::Register => "Project registered",
        Commands::Unregister { .. } => "Project registry",
        Commands::Overview { .. } => "Projects",
    };
    if styled {
        writeln!(out, "\n  \x1b[1mdevstate / {title}\x1b[0m").unwrap();
    } else {
        writeln!(out, "\n  devstate / {title}").unwrap();
    }
    out.push_str("  ──────────────────────────────────────────────────\n");
    match command {
        Commands::Mcp => {},
        Commands::Guide { .. } => {
            if let Some(topics) = value["topics"].as_array() {
                for topic in topics {
                    field(&mut out, "Topic", text(topic, "id"));
                    section(&mut out, text(topic, "title"), text(topic, "content"));
                    if topic["truncated"] == true { out.push_str("  Topic shortened; request its ID alone.\n"); }
                }
                if topics.is_empty() {
                    out.push_str("  No matching usage topic. Try a topic ID:\n");
                    if let Some(ids) = value["available_topics"].as_array() {
                        writeln!(out, "    {}", ids.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap();
                    }
                }
            }
        }
        Commands::Init { .. } | Commands::Register => {
            let project = if matches!(command, Commands::Register) { &value["project"] } else { value };
            field(&mut out, "Project", text(project, "name"));
            field(&mut out, "Root", text(project, "root_path"));
            field(&mut out, "ID", text(project, "id"));
            out.push_str("\n  Next: devstate resume\n");
        }
        Commands::Create { .. } | Commands::Checkpoint { .. } | Commands::Close { .. } => {
            field(&mut out, "Task ID", text(value, "id"));
            field(&mut out, "Revision", &number(&value["revision"]));
            field(&mut out, "Project rev", &number(&value["project_revision"]));
        }
        Commands::Focus { .. } => {
            field(&mut out, "Project", text(value, "name"));
            field(&mut out, "Focus", value["focused_task_id"].as_str().unwrap_or("none"));
            field(&mut out, "Project rev", &number(&value["revision"]));
        }
        Commands::Resume { .. } => {
            field(&mut out, "Project", text(&value["project"], "name"));
            field(&mut out, "Root", text(&value["project"], "root_path"));
            field(&mut out, "Project rev", &number(&value["project"]["revision"]));
            if value["task"].is_null() {
                out.push_str("\n  No focused task.\n");
                let fallback = &value["fallback"];
                if fallback["reason"] == "empty_project" {
                    out.push_str("  No tasks yet. Create: devstate create --title 'Task title'\n");
                } else {
                    if let Some(candidates) = fallback["candidates"].as_array() {
                        for candidate in candidates {
                            out.push_str("\n  Open task\n");
                            resume_candidate(&mut out, candidate);
                        }
                    }
                    if !fallback["latest_task"].is_null() {
                        out.push_str("\n  Latest work\n");
                        resume_candidate(&mut out, &fallback["latest_task"]);
                    }
                    if fallback["has_more"] == true {
                        out.push_str("\n  More tasks: devstate tasks\n");
                    }
                    out.push_str("\n  Select: devstate focus ID --revision PROJECT_REV\n");
                }
            } else {
                checkpoint(&mut out, &value["task"]);
                if value["truncated_fields"].as_array().is_some_and(|fields| !fields.is_empty()) {
                    out.push_str("\n  Checkpoint shortened. Use devstate task ID for details.\n");
                }
            }
        }
        Commands::Task { .. } => {
            let task = &value["task"];
            checkpoint(&mut out, &task["checkpoint"]);
            if task.get("usage_status").is_some() {
            let usage = text(task, "usage_status");
            field(&mut out, "Tokens", &format!("{} total | {} input | {} output | {usage}",
                number(&value["total_tokens"]), number(&task["input_tokens"]), number(&task["output_tokens"])));
            field(&mut out, "Elapsed", &duration(&value["elapsed_ms"]));
            let models = task["models_used"].as_array().map(|models| {
                models.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
            }).unwrap_or_default();
            field(&mut out, "Models", if models.is_empty() { "unknown" } else { &models });
            }
            field(&mut out, "Scope", text(task, "source_scope"));
            section(&mut out, "Notes", text(task, "notes"));
            section(&mut out, "Verification evidence", text(task, "verification_notes"));
            if value["section"] == "brief" {
                out.push_str("\n  More: --section notes | evidence | usage | full\n");
            }
        }
        Commands::Tasks { .. } | Commands::Recent { .. } | Commands::Search { .. } => {
            let items = value["items"].as_array().expect("query always returns items");
            if items.is_empty() {
                out.push_str("\n  No tasks found.\n");
            } else {
                out.push_str("\n  STATE        KIND   TASK\n");
                for item in items {
                    writeln!(out, "  {:<12} {:<6} {}", safe(text(item, "status")),
                        safe(text(item, "kind")), safe(text(item, "title"))).unwrap();
                    writeln!(out, "                      {}", safe(text(item, "id"))).unwrap();
                }
                writeln!(out, "\n  {} task(s)", items.len()).unwrap();
            }
            if value["truncated"].as_bool() == Some(true) {
                out.push_str("  More matches omitted. Narrow the search or increase --limit.\n");
            }
            cursor(&mut out, &value["next_cursor"]);
        }
        Commands::Overview { .. } => {
            let entries = value["entries"].as_array().expect("overview always returns entries");
            if entries.is_empty() {
                out.push_str("\n  No registered projects.\n  Add one: devstate --root PATH --registry FILE register\n");
            } else {
                out.push_str("\n  STATE         OPEN  BUGS  PROJECT / FOCUS\n");
                for entry in entries {
                    let data = &entry["data"];
                    if entry["availability"] == "available" {
                        let focus = &data["focused_task"];
                        let state = if focus.is_null() { "no focus" } else { text(focus, "status") };
                        let counts = &data["counts"];
                        let open = ["todo", "active", "blocked"].iter()
                            .map(|key| counts[*key].as_i64().unwrap_or(0)).sum::<i64>();
                        writeln!(out, "  {state:<12} {open:>5} {:>5}  {}", number(&counts["open_bugs"]),
                            safe(text(&data["project"], "name"))).unwrap();
                        field(&mut out, "Root", text(&data["project"], "root_path"));
                        if !focus.is_null() {
                            field(&mut out, "Focus", text(focus, "title"));
                            field(&mut out, "Done so far", text(focus, "summary"));
                            field(&mut out, "Next", text(focus, "next_action"));
                            if focus["truncated"].as_bool() == Some(true) {
                                out.push_str("  Summary shortened; open the project task for details.\n");
                            }
                        } else if !data["latest_task"].is_null() {
                            field(&mut out, "Latest", &format!("{} [{}]", text(&data["latest_task"], "title"),
                                text(&data["latest_task"], "status")));
                        }
                    } else {
                        out.push_str("  unavailable\n");
                        field(&mut out, "Root", text(data, "root_path"));
                        field(&mut out, "Reason", text(data, "reason"));
                    }
                    out.push('\n');
                }
                writeln!(out, "  {} project(s)", entries.len()).unwrap();
            }
            cursor(&mut out, &value["next_cursor"]);
        }
        Commands::Fingerprint => {
            field(&mut out, "Scope", text(value, "scope"));
            field(&mut out, "SHA-256", text(value, "fingerprint"));
        }
        Commands::Unregister { .. } => {
            field(&mut out, "Root", text(value, "root_path"));
            field(&mut out, "Result", if value["removed"] == true { "Removed from registry" } else { "Not registered" });
        }
    }
    out.push('\n');
    out
}

fn resume_candidate(out: &mut String, task: &Value) {
    field(out, "Task", text(task, "title"));
    field(out, "Task ID", text(task, "id"));
    field(out, "Status", text(task, "status"));
    field(out, "Revision", &number(&task["revision"]));
    field(out, "Summary", text(task, "summary"));
    field(out, "Next action", text(task, "next_action"));
    field(out, "Blockers", text(task, "blockers"));
    if task["truncated"] == true {
        out.push_str("  Checkpoint shortened. Use devstate task ID for details.\n");
    }
}

fn checkpoint(out: &mut String, task: &Value) {
    out.push('\n');
    field(out, "Task", text(task, "title"));
    field(out, "ID", text(task, "id"));
    field(out, "State", &format!("{} | {} | {}", text(task, "status"), text(task, "kind"), text(task, "phase")));
    field(out, "Verification", &format!("{} | {}", text(task, "verification_status"), text(task, "verification_freshness")));
    field(out, "Spec", text(task, "spec_path"));
    section(out, "Done so far", text(task, "summary"));
    section(out, "Next action", text(task, "next_action"));
    section(out, "Blockers", text(task, "blockers"));
}

fn field(out: &mut String, label: &str, value: &str) {
    if !value.is_empty() {
        writeln!(out, "  {label:<12} {}", safe(value)).unwrap();
    }
}

fn section(out: &mut String, label: &str, value: &str) {
    if !value.is_empty() {
        writeln!(out, "\n  {label}").unwrap();
        for line in value.lines() { writeln!(out, "    {}", safe(line)).unwrap(); }
    }
}

fn cursor(out: &mut String, value: &Value) {
    if !value.is_null() {
        // Render JSON itself so raw paths/quotes are escaped, not shell code.
        writeln!(out, "  Next cursor (pass as one --cursor argument):\n    {}", safe(&value.to_string())).unwrap();
    }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}

fn number(value: &Value) -> String {
    value.as_i64().map(|value| value.to_string()).unwrap_or_else(|| "unknown".into())
}

fn duration(value: &Value) -> String {
    match value.as_i64() {
        Some(ms) => format!("{}h {:02}m {:02}s (includes pauses)", ms / 3_600_000, (ms / 60_000) % 60, (ms / 1000) % 60),
        None => "unknown".into(),
    }
}

pub fn safe(value: &str) -> String {
    value.chars().map(|ch| {
        if ch.is_control() || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') { ' ' } else { ch }
    }).collect()
}
