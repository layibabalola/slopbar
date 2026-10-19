//! `slopbar-mcp`: a stdio MCP server that gives agents what the Settings window offers. It never
//! starts the app, reads credentials or contacts a provider itself: every operation goes through
//! the running app's door (see `door`), and listing exports reads only file names, sizes and times.

use serde_json::{json, Value};
use std::io::{BufRead, Write};

use crate::door::Request;

const PROTOCOL: &str = "2025-06-18";
const SUPPORTED: [&str; 4] = ["2025-06-18", "2025-03-26", "2024-11-05", "2025-11-25"];
const LIST_LIMIT: usize = 20;

const INSTRUCTIONS: &str = "SlopBar shows Claude and Codex account usage and switches accounts. These tools \
act on the running SlopBar app exactly like its Settings window. get_usage returns each account's id, email, \
plan and usage windows; use the provider and id from it for account tools. add_account and connect_monitoring \
open the official CLI sign-in for the user to complete: never sign in, type credentials or approve prompts \
yourself. switch_account changes which account the CLI uses, which affects running agent sessions. \
remove_account and disconnect_monitoring revoke saved sign-ins. Diagnostics exports exclude emails, tokens and \
raw provider bodies; they are for local analysis and manual sharing only, never upload them on your own.";

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

fn account_schema() -> Value {
    schema(json!({
        "provider": { "type": "string", "enum": ["claude", "codex"] },
        "id": { "type": "string", "description": "The account id from get_usage." },
    }), &["provider", "id"])
}

/// (name, title, description, input schema, read only, destructive, idempotent)
fn catalog() -> Vec<(&'static str, &'static str, &'static str, Value, bool, bool, bool)> {
    let none = schema(json!({}), &[]);
    vec![
        ("get_usage", "Get account usage", "Every Claude and Codex account SlopBar knows: id, email, plan, which one is in use, usage windows (percent used, reset time), banked resets, monitoring state and errors, as last refreshed.", none.clone(), true, false, true),
        ("refresh_usage", "Refresh usage", "Poll every account's usage now (as the tray's Refresh does) and return the new snapshot. May switch accounts if automatic switching is on and a threshold is reached.", none.clone(), false, false, true),
        ("get_settings", "Get settings", "SlopBar's settings: refresh interval, providers, language, launch at login, updates, account aliases, automatic switching rules and notifications.", none.clone(), true, false, true),
        ("update_settings", "Update settings", "Change settings with a JSON merge patch (RFC 7386) over get_settings' shape, e.g. {\"refresh_minutes\": 15, \"balancer\": {\"claude\": {\"auto\": true, \"threshold\": 85}}, \"aliases\": {\"claude:<id>\": \"Work\"}}. null removes an alias. Only values the Settings window offers are accepted. Returns the saved settings.",
            schema(json!({ "patch": { "type": "object", "description": "Merge patch over the current settings." } }), &["patch"]), false, false, true),
        ("switch_account", "Switch account", "Make this saved account the one the provider's CLI uses (as Use in Settings). Running agent sessions on that CLI are affected. Returns the new usage snapshot.", account_schema(), false, false, true),
        ("add_account", "Add account", "Open the provider CLI's own sign-in in a terminal for the user to complete; the account appears in get_usage once they finish. Returns at once.",
            schema(json!({ "provider": { "type": "string", "enum": ["claude", "codex"] } }), &["provider"]), false, false, false),
        ("connect_monitoring", "Connect monitoring", "Start a separate monitoring sign-in for this account (the user completes it) so its usage stays visible while it is not in use. Returns at once; check get_usage afterwards.", account_schema(), false, false, false),
        ("disconnect_monitoring", "Disconnect monitoring", "Stop independent monitoring for this account and revoke its monitoring sign-in. Returns the new snapshot and whether the provider confirmed the revocation.", account_schema(), false, true, true),
        ("remove_account", "Remove account", "Remove a saved account that is not in use, revoking its monitoring sign-in. Returns the new usage snapshot.", account_schema(), false, true, true),
        ("check_for_updates", "Check for updates", "Check for a newer SlopBar release (only builds with automatic updates).", none.clone(), true, false, true),
        ("install_update", "Install update", "Download and install the available SlopBar update; SlopBar restarts.", none.clone(), false, false, false),
        ("get_app_info", "Get app info", "SlopBar's version, build (commit, dirty flag) and whether automatic updates are available.", none.clone(), true, false, true),
        ("open_settings", "Open Settings window", "Show SlopBar's Settings window on the user's screen.", none.clone(), false, false, true),
        ("export_diagnostics", "Export diagnostics", "Write a diagnostics export (as Settings > Data > Export, without opening a folder) and return its local path and size; a repeat within 30 seconds returns the same export. Read the file to analyze it.", none.clone(), false, false, false),
        ("list_diagnostic_exports", "List diagnostics exports", "The most recent diagnostics exports, newest first, with paths, sizes and times. Works while SlopBar is closed.", none, true, false, true),
    ]
}

fn tools() -> Value {
    Value::Array(catalog().into_iter().map(|(name, title, description, input, read_only, destructive, idempotent)| json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": input,
        "annotations": { "readOnlyHint": read_only, "destructiveHint": destructive, "idempotentHint": idempotent, "openWorldHint": false },
    })).collect())
}

/// The door request for a tool call, or a message for the agent when the arguments are wrong.
fn request(name: &str, args: &Value) -> Option<Result<Request, String>> {
    let simple = |op: &str| serde_json::from_value::<Request>(json!({ "op": op }));
    let with = |op: &str| {
        let mut args = args.as_object().cloned().unwrap_or_default();
        args.insert("op".into(), op.into());
        serde_json::from_value::<Request>(Value::Object(args))
    };
    let parsed = match name {
        "get_usage" => simple("get_usage"),
        "refresh_usage" => simple("refresh"),
        "get_settings" => simple("get_settings"),
        "update_settings" => with("update_settings"),
        "switch_account" | "add_account" | "connect_monitoring" | "disconnect_monitoring" | "remove_account" => with(name),
        "check_for_updates" => simple("check_for_updates"),
        "install_update" => simple("install_update"),
        "get_app_info" => simple("get_app_info"),
        "open_settings" => simple("open_settings"),
        "export_diagnostics" => simple("export_diagnostics"),
        _ => return None,
    };
    Some(parsed.map_err(|e| format!("Invalid arguments: {e}")))
}

fn list(dir: Option<std::path::PathBuf>) -> Result<Value, String> {
    let Some(dir) = dir else { return Err("no_config_directory".into()) };
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({ "folder": dir, "total": 0, "exports": [] })),
        Err(_) => return Err("export_folder_unreadable".into()),
    };
    let mut found: Vec<(std::time::SystemTime, Value)> = entries
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("SlopBar-Diagnostics-") && name.ends_with(".json")
        })
        .filter_map(|e| {
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            let modified = meta.modified().ok()?;
            let at = chrono::DateTime::<chrono::Utc>::from(modified);
            Some((modified, json!({ "path": e.path(), "bytes": meta.len(), "modified": at })))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    let total = found.len();
    Ok(json!({ "folder": dir, "total": total, "exports": found.into_iter().take(LIST_LIMIT).map(|(_, v)| v).collect::<Vec<_>>() }))
}

/// Plain-language text for door and app error codes; anything else is passed on as is (the
/// door has already checked that the app answering runs as this user).
fn explain(error: &str) -> String {
    match error {
        "not_running" => "SlopBar is not running, or the running copy predates this agent door. Start (or restart) SlopBar, then try again; these tools never start it.".into(),
        "door_untrusted" => "The process answering on SlopBar's door is not running as this Windows user, so it was not trusted. Restart SlopBar.".into(),
        "door_unavailable" => "SlopBar's door could not be reached. Try again shortly.".into(),
        "unsupported_request" => "The running SlopBar is a different version that does not understand this request. Restart SlopBar after updating.".into(),
        "bad_door_reply" => "SlopBar gave an unusable answer. Restart SlopBar and try again.".into(),
        "timeout" => "SlopBar did not answer in time. Try again shortly.".into(),
        "diagnostic_storage" => "SlopBar could not write the export (disk or file access problem).".into(),
        "unsupported_platform" => "These tools need SlopBar on Windows.".into(),
        "no_config_directory" => "The SlopBar data folder could not be located.".into(),
        "export_folder_unreadable" => "The diagnostics export folder could not be read.".into(),
        "active" => "That account is in use; switch to another account before removing it.".into(),
        "monitor_busy" => "Another operation on that account is in progress. Try again when it finishes.".into(),
        "monitor_wrong_account" => "No saved account matches that provider and id. Use the ids from get_usage.".into(),
        "monitor_login_failed" => "The monitoring sign-in could not be started.".into(),
        "CLI not found" => "That provider's CLI is not installed or not found on PATH.".into(),
        other => other.into(),
    }
}

fn result(outcome: Result<Value, String>) -> Value {
    match outcome {
        Ok(value) => {
            let structured = if value.is_object() { value.clone() } else { json!({ "result": value }) };
            json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default() }],
                "structuredContent": structured,
                "isError": false,
            })
        }
        Err(error) => json!({ "content": [{ "type": "text", "text": explain(&error) }], "isError": true }),
    }
}

/// Answers one JSON-RPC message; `None` for notifications and anything that must not be answered.
fn handle(message: &Value, call: &dyn Fn(&Request) -> Result<Value, String>, exports: &dyn Fn() -> Option<std::path::PathBuf>) -> Option<Value> {
    // Notifications, client responses (no method) and null ids get no reply.
    let id = message.get("id").filter(|id| id.is_string() || id.is_number()).cloned()?;
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        if message.get("result").is_some() || message.get("error").is_some() { return None; }
        return Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32600, "message": "Invalid Request" } }));
    };
    let reply = match method {
        "initialize" => {
            let asked = message.pointer("/params/protocolVersion").and_then(Value::as_str);
            let version = asked.filter(|v| SUPPORTED.contains(v)).unwrap_or(PROTOCOL);
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "slopbar", "title": "SlopBar", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let name = message.pointer("/params/name").and_then(Value::as_str).unwrap_or("");
            let args = message.pointer("/params/arguments").cloned().unwrap_or(json!({}));
            if name == "list_diagnostic_exports" {
                Ok(result(list(exports())))
            } else {
                match request(name, &args) {
                    None => Err(json!({ "code": -32602, "message": format!("Unknown tool: {name}") })),
                    Some(Err(text)) => Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": true })),
                    Some(Ok(request)) => Ok(result(call(&request))),
                }
            }
        }
        _ => Err(json!({ "code": -32601, "message": "Method not found" })),
    };
    Some(match reply {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    })
}

/// Serves MCP over stdin/stdout (newline-delimited JSON-RPC) until stdin closes.
pub fn serve_stdio() -> std::io::Result<()> {
    let pipe = crate::door::pipe_name();
    let call = |request: &Request| crate::door::call(&pipe, request);
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    let mut line = Vec::new();
    loop {
        line.clear();
        if stdin.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let reply = match serde_json::from_slice::<Value>(&line) {
            Ok(Value::Array(batch)) if !batch.is_empty() => {
                let replies: Vec<Value> = batch.iter().filter_map(|m| handle(m, &call, &crate::door::exports_dir)).collect();
                (!replies.is_empty()).then(|| Value::Array(replies))
            }
            Ok(message @ Value::Object(_)) => handle(&message, &call, &crate::door::exports_dir),
            Ok(_) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32600, "message": "Invalid Request" } })),
            Err(_) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "Parse error" } })),
        };
        if let Some(reply) = reply {
            serde_json::to_writer(&mut stdout, &reply)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// Calls a tool against a fake app that echoes each request (or fails with `error`).
    fn tool(name: &str, args: Value, error: Option<&str>) -> Value {
        let message = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } });
        let call = |request: &Request| match error {
            Some(e) => Err(e.to_string()),
            None => Ok(serde_json::to_value(request).unwrap()),
        };
        handle(&message, &call, &|| None).unwrap()
    }

    fn rpc(message: Value) -> Option<Value> {
        handle(&message, &|_| Err("not_running".into()), &|| None)
    }

    #[test]
    fn initializes_and_answers_only_what_it_should() {
        let init = rpc(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26" } })).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "slopbar");
        let unknown = rpc(json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": { "protocolVersion": "1999-01-01" } })).unwrap();
        assert_eq!(unknown["result"]["protocolVersion"], PROTOCOL);
        assert!(rpc(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).is_none());
        assert!(rpc(json!({ "jsonrpc": "2.0", "id": 6, "result": {} })).is_none(), "client responses get no reply");
        assert!(rpc(json!({ "jsonrpc": "2.0", "id": null, "method": "ping" })).is_none(), "null ids get no reply");
        assert_eq!(rpc(json!({ "jsonrpc": "2.0", "id": 7 })).unwrap()["error"]["code"], -32600);
        assert_eq!(rpc(json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/list" })).unwrap()["error"]["code"], -32601);
        assert_eq!(tool("read_vault", json!({}), None)["error"]["code"], -32602);
    }

    #[test]
    fn every_listed_tool_maps_to_a_door_operation() {
        let listed = rpc(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).unwrap();
        let names: Vec<String> = listed["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names.len(), 15);
        let account = json!({ "provider": "claude", "id": "a1" });
        for name in names.iter().filter(|n| *n != "list_diagnostic_exports") {
            let args = match name.as_str() {
                "update_settings" => json!({ "patch": { "refresh_minutes": 15 } }),
                "add_account" => json!({ "provider": "codex" }),
                "switch_account" | "connect_monitoring" | "disconnect_monitoring" | "remove_account" => account.clone(),
                _ => json!({}),
            };
            let reply = tool(name, args, None);
            assert_eq!(reply["result"]["isError"], false, "{name}: {reply}");
        }
        let switched = tool("switch_account", account, None);
        assert_eq!(switched["result"]["structuredContent"], json!({ "op": "switch_account", "provider": "claude", "id": "a1" }));
        let destructive: Vec<_> = listed["result"]["tools"].as_array().unwrap().iter()
            .filter(|t| t["annotations"]["destructiveHint"] == true).map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(destructive, ["disconnect_monitoring", "remove_account"]);
    }

    #[test]
    fn bad_arguments_and_app_errors_are_tool_errors() {
        let missing = tool("switch_account", json!({ "provider": "claude" }), None);
        assert_eq!(missing["result"]["isError"], true);
        let extra = tool("remove_account", json!({ "provider": "claude", "id": "a", "force": true }), None);
        assert_eq!(extra["result"]["isError"], true, "unknown arguments are refused");
        let provider = tool("add_account", json!({ "provider": "gemini" }), None);
        assert_eq!(provider["result"]["isError"], true);
        let active = tool("remove_account", json!({ "provider": "claude", "id": "a" }), Some("active"));
        assert!(active["result"]["content"][0]["text"].as_str().unwrap().contains("in use"));
        let down = tool("get_usage", json!({}), Some("not_running"));
        assert!(down["result"]["content"][0]["text"].as_str().unwrap().contains("not running"));
    }

    #[test]
    fn lists_only_export_files_newest_first() {
        let dir = TempDir::new();
        std::fs::write(dir.0.join("SlopBar-Diagnostics-1.json"), b"{}").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.0.join("SlopBar-Diagnostics-2.json"), b"{\"a\":1}").unwrap();
        std::fs::write(dir.0.join("history.bin"), b"secret").unwrap();
        std::fs::create_dir(dir.0.join("SlopBar-Diagnostics-dir.json")).unwrap();
        let listed = list(Some(dir.0.clone())).unwrap();
        assert_eq!(listed["total"], 2);
        let paths: Vec<_> = listed["exports"].as_array().unwrap().iter().map(|e| e["path"].as_str().unwrap().to_string()).collect();
        assert!(paths[0].ends_with("SlopBar-Diagnostics-2.json") && paths[1].ends_with("SlopBar-Diagnostics-1.json"), "{paths:?}");
        assert_eq!(listed["exports"][0]["bytes"], 7);
        assert_eq!(list(Some(dir.0.join("absent"))).unwrap()["exports"], json!([]));
    }
}
