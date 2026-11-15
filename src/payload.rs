//! Context projections shared by CLI and MCP. Full records remain retrievable.

use serde_json::{Value, json};

pub fn task(mut value: Value, section: &str) -> Value {
    let task = value["task"].as_object_mut().expect("task payload");
    let has_notes = task["notes"].as_str().is_some_and(|text| !text.is_empty());
    let has_evidence = task["verification_notes"].as_str().is_some_and(|text| !text.is_empty());
    if section != "full" {
        task.retain(|key, _| {
            key == "checkpoint" || match section {
                "brief" | "usage" => matches!(key.as_str(), "input_tokens" | "output_tokens" | "usage_status" | "models_used"),
                "notes" => key == "notes",
                "evidence" => matches!(key.as_str(), "verification_notes" | "verified_at" | "source_scope" | "source_fingerprint" | "verified_fingerprint"),
                _ => false,
            }
        });
        if section != "brief" {
            task["checkpoint"].as_object_mut().expect("checkpoint").retain(|key, _|
                matches!(key.as_str(), "id" | "title" | "revision" | "verification_status" | "verification_freshness"));
        } else {
            task.insert("has_notes".into(), json!(has_notes));
            task.insert("has_evidence".into(), json!(has_evidence));
        }
    }
    if matches!(section, "notes" | "evidence") {
        value.as_object_mut().expect("task response").retain(|key, _| key == "task");
    }
    value["section"] = json!(section);
    if section == "brief" {
        value["available_sections"] = json!(["notes", "evidence", "usage", "full"]);
    }
    value
}

pub fn overview(mut value: Value) -> Value {
    if let Some(entries) = value["entries"].as_array_mut() {
        for entry in entries {
            if entry["availability"] != "available" { continue; }
            let data = &mut entry["data"];
            if !data["focused_task"].is_null() && data["focused_task"]["id"] == data["latest_task"]["id"] {
                data["latest_task"] = Value::Null;
                data["latest_task_same_as_focus"] = json!(true);
            }
            data["project"].as_object_mut().expect("project").retain(|key, _|
                matches!(key.as_str(), "id" | "name" | "root_path"));
        }
    }
    value
}
