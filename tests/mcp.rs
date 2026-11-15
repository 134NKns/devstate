use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use uuid::Uuid;

struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("devstate-mcp-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}
fn initialized() -> Vec<Value> {
    vec![request(1, "initialize", json!({
        "protocolVersion":"2025-11-25", "capabilities":{},
        "clientInfo":{"name":"integration-test","version":"1"}
    })), json!({"jsonrpc":"2.0","method":"notifications/initialized"})]
}
fn call(id: i64, name: &str, args: Value) -> Value {
    request(id, "tools/call", json!({"name":name,"arguments":args}))
}
fn exchange(messages: Vec<Value>) -> Vec<Value> {
    exchange_bytes(messages.iter().map(|value| format!("{value}\n")).collect::<String>().as_bytes())
}
fn exchange_bytes(input: &[u8]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_devstate"))
        .arg("mcp").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty());
    String::from_utf8(output.stdout).unwrap().lines()
        .map(|line| serde_json::from_str(line).expect("stdout contains only JSON-RPC")).collect()
}

#[test]
fn lifecycle_discovery_and_notifications_are_protocol_only() {
    let mut messages = initialized();
    messages.push(request(2, "tools/list", json!({})));
    messages.push(json!({"jsonrpc":"2.0","method":"notifications/unknown"}));
    messages.push(request(3, "ping", json!({})));
    let replies = exchange(messages);
    assert_eq!(replies.len(), 3);
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-11-25");
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 14);
    let resume = tools.iter().find(|tool| tool["name"] == "devstate_resume").unwrap();
    assert_eq!(resume["inputSchema"]["required"][0], "root");
    assert_eq!(resume["annotations"]["readOnlyHint"], true);
    let init = tools.iter().find(|tool| tool["name"] == "devstate_init").unwrap();
    assert_eq!(init["annotations"]["readOnlyHint"], false);
    assert_eq!(replies[2]["id"], 3);
    assert_eq!(replies[2]["result"], json!({}));
}

#[test]
fn tool_calls_are_project_scoped_and_errors_do_not_kill_server() {
    let alpha = Sandbox::new();
    let beta = Sandbox::new();
    let mut messages = initialized();
    messages.extend([
        call(2, "devstate_init", json!({"root":alpha.0,"name":"alpha"})),
        call(3, "devstate_init", json!({"root":beta.0,"name":"beta"})),
        call(4, "devstate_resume", json!({"root":alpha.0})),
        call(5, "devstate_task", json!({"root":alpha.0,"id":"missing"})),
        call(6, "devstate_resume", json!({"root":beta.0})),
        call(7, "devstate_resume", json!({"root":"relative"})),
        call(8, "devstate_resume", json!({})),
        call(9, "devstate_tasks", json!({"root":alpha.0,"limit":51})),
        call(10, "devstate_resume", json!({"root":alpha.0,"unexpected":"value"})),
    ]);
    let replies = exchange(messages);
    assert_eq!(replies[3]["result"]["structuredContent"]["project"]["name"], "alpha");
    assert_eq!(replies[4]["result"]["isError"], true);
    assert_eq!(replies[5]["result"]["structuredContent"]["project"]["name"], "beta");
    for index in 6..=9 { assert_eq!(replies[index]["result"]["isError"], true); }
    let text = replies[3]["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(serde_json::from_str::<Value>(text).unwrap(), replies[3]["result"]["structuredContent"]);
}

#[test]
fn malformed_requests_and_initialization_order_are_rejected() {
    let mut messages = vec![request(0, "tools/list", json!({}))];
    messages.extend(initialized());
    messages.push(request(2, "unknown/method", json!({})));
    messages.push(call(3, "unknown_tool", json!({})));
    messages.push(request(4, "initialize", json!({})));
    let replies = exchange(messages);
    assert_eq!(replies[0]["error"]["code"], -32002);
    assert_eq!(replies[2]["error"]["code"], -32601);
    assert_eq!(replies[3]["error"]["code"], -32602);
    assert_eq!(replies[4]["error"]["code"], -32600);
    let replies = exchange_bytes(b"not-json\n[]\n{\"jsonrpc\":\"2.0\",\"id\":true,\"method\":\"ping\"}\n");
    assert_eq!(replies[0]["error"]["code"], -32700);
    assert_eq!(replies[1]["error"]["code"], -32600);
    assert_eq!(replies[2]["error"]["code"], -32600);
}

#[test]
fn explicit_registry_overview_is_read_only_and_opt_in() {
    let project = Sandbox::new();
    let registry_dir = Sandbox::new();
    let registry = registry_dir.0.join("registry.db");
    let mut messages = initialized();
    messages.extend([
        call(2, "devstate_overview", json!({"registry":registry})),
        call(3, "devstate_init", json!({"root":project.0,"name":"project"})),
        call(4, "devstate_register", json!({"root":project.0,"registry":registry})),
        call(5, "devstate_overview", json!({"registry":registry})),
        call(6, "devstate_unregister", json!({"registry":registry,"saved_root":project.0})),
        call(7, "devstate_overview", json!({"registry":registry})),
    ]);
    let replies = exchange(messages);
    assert_eq!(replies[1]["result"]["isError"], true);
    assert_eq!(replies[4]["result"]["structuredContent"]["entries"][0]["data"]["project"]["name"], "project");
    assert_eq!(replies[5]["result"]["structuredContent"]["removed"], true);
    assert_eq!(replies[6]["result"]["structuredContent"]["entries"], json!([]));
    assert!(project.0.join(".devstate/state.db").is_file());
}

#[test]
fn supported_versions_negotiate_and_unknown_versions_offer_latest() {
    for (offered, expected) in [("2024-11-05", "2024-11-05"), ("2099-01-01", "2025-11-25")] {
        let replies = exchange(vec![request(1, "initialize", json!({
            "protocolVersion":offered,"capabilities":{},"clientInfo":{"name":"test","version":"1"}
        }))]);
        assert_eq!(replies[0]["result"]["protocolVersion"], expected);
    }
}

#[test]
fn usage_guide_is_global_documentation_not_project_memory() {
    let mut messages = initialized();
    messages.push(call(2, "devstate_guide", json!({"query":"tokens"})));
    messages.push(call(3, "devstate_guide", json!({"query":"unknown-zz-883"})));
    messages.push(call(4, "devstate_guide", json!({"query":"start","limit":6})));
    let replies = exchange(messages);
    assert_eq!(replies[1]["result"]["structuredContent"]["topics"][0]["id"], "tokens");
    assert_eq!(replies[2]["result"]["structuredContent"]["topics"], json!([]));
    assert_eq!(replies[3]["result"]["isError"], true);
    assert!(replies[0]["result"]["instructions"].as_str().unwrap().contains("Reply to the user in their language"));
}

#[test]
fn mcp_writes_checkpoint_resume_and_close_with_revisions() {
    use devstate::mutations::NewTask;
    use devstate::store::Store;
    let sandbox = Sandbox::new();
    let mut store = Store::init(&sandbox.0, "mutations").unwrap();
    let task = store.create_task(NewTask { title: "Focused task".into(), kind: "task".into(), spec_path: None }).unwrap();
    let mut messages = initialized();
    messages.extend([
        call(2, "devstate_create_task", json!({"root":sandbox.0,"title":"Another task"})),
        call(3, "devstate_set_focus", json!({"root":sandbox.0,"id":task.id,"expected_project_revision":1})),
        call(4, "devstate_checkpoint", json!({"root":sandbox.0,"id":task.id,"expected_revision":1,
            "summary":"API connected","next_action":"Wire frontend"})),
        call(5, "devstate_resume", json!({"root":sandbox.0})),
        call(6, "devstate_checkpoint", json!({"root":sandbox.0,"id":task.id,"expected_revision":1,
            "summary":"Stale update","next_action":"Must not persist"})),
        call(7, "devstate_close_task", json!({"root":sandbox.0,"id":task.id,"expected_revision":2,
            "expected_project_revision":2,"summary":"Documentation delivered","status":"done",
            "not_required_reason":"Documentation only; no runtime check needed"})),
        call(8, "devstate_resume", json!({"root":sandbox.0})),
    ]);
    let replies = exchange(messages);
    assert_eq!(replies[1]["result"]["isError"], false);
    assert_eq!(replies[4]["result"]["structuredContent"]["task"]["next_action"], "Wire frontend");
    assert_eq!(replies[5]["result"]["isError"], true);
    assert_eq!(replies[6]["result"]["isError"], false);
    assert!(replies[7]["result"]["structuredContent"]["task"].is_null());
    let fallback = &replies[7]["result"]["structuredContent"]["fallback"];
    assert_eq!(fallback["reason"], "no_focus");
    assert_eq!(fallback["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(fallback["candidates"][0]["title"], "Another task");
    assert!(fallback["candidates"][0]["updated_at"].as_i64().unwrap() > 0);
    // Timestamp ties may make the open task latest; it is then deduplicated.
    if !fallback["latest_task"].is_null() {
        assert_eq!(fallback["latest_task"]["id"], task.id);
        assert_eq!(fallback["latest_task"]["summary"], "Documentation delivered");
        assert_eq!(fallback["latest_task"]["updated_at"].as_i64().unwrap(),
            store.get_task(&task.id).unwrap().unwrap().checkpoint.updated_at);
    }
    assert_eq!(fallback["has_more"], false);
    assert_eq!(store.get_task(&task.id).unwrap().unwrap().checkpoint.status, "done");
}
