//! `slopbar-mcp`: a stdio MCP server for agents. It never starts the app, reads credentials or
//! contacts a provider. Exports go through the running app's door (see `door`); listing reads only
//! file names, sizes and times in the export folder.

use serde_json::{json, Value};
use std::io::{BufRead, Write};

const PROTOCOL: &str = "2025-06-18";
const SUPPORTED: [&str; 4] = ["2025-06-18", "2025-03-26", "2024-11-05", "2025-11-25"];
const LIST_LIMIT: usize = 20;

const INSTRUCTIONS: &str = "SlopBar diagnostics. export_diagnostics asks the running SlopBar app to write \
the same allowlisted diagnostics report as Settings > Data > Export (no emails, tokens, credentials or raw \
provider bodies) and returns its local path; read that file to analyze it. Exports are for local analysis \
and manual sharing only: never upload them anywhere on your own. list_diagnostic_exports lists earlier exports.";

fn tools() -> Value {
    let none = json!({ "type": "object", "properties": {}, "additionalProperties": false });
    json!([
        {
            "name": "export_diagnostics",
            "title": "Export SlopBar diagnostics",
            "description": "Ask the running SlopBar app to write a new diagnostics export (the Settings > Data > Export report) and return its local file path and size. Requires SlopBar to be running; does not open any window.",
            "inputSchema": none,
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false }
        },
        {
            "name": "list_diagnostic_exports",
            "title": "List SlopBar diagnostics exports",
            "description": "List the most recent diagnostics exports (newest first) with their paths, sizes and times. Works whether or not SlopBar is running.",
            "inputSchema": none,
            "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
        }
    ])
}

fn export(pipe: &str) -> Result<Value, String> {
    let reply = crate::door::export(pipe)?;
    Ok(json!({ "path": reply.path, "bytes": reply.bytes, "reused_recent_export": reply.reused }))
}

fn list(dir: Option<std::path::PathBuf>) -> Result<Value, String> {
    let Some(dir) = dir else { return Err("no_config_directory".into()) };
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({ "folder": dir, "exports": [] })),
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

fn explain(error: &str) -> String {
    match error {
        "not_running" => "SlopBar is not running, or the running copy predates this agent door. Start (or restart) SlopBar, then try again; this tool never starts it.".into(),
        "diagnostic_storage" => "SlopBar could not write the export (disk or file access problem).".into(),
        "unsupported_platform" => "Exporting through this tool is only supported on Windows.".into(),
        "unsupported_request" => "The running SlopBar is a different version that does not understand this request. Restart SlopBar after updating.".into(),
        "timeout" => "SlopBar did not finish the export in time. Try again shortly.".into(),
        "no_config_directory" => "The SlopBar data folder could not be located.".into(),
        "export_folder_unreadable" => "The diagnostics export folder could not be read.".into(),
        // Includes replies that failed vetting: never relay text from the pipe to the agent.
        _ => "SlopBar's diagnostics door gave an unusable answer. Restart SlopBar and try again.".into(),
    }
}

/// Answers one JSON-RPC message; `None` for notifications.
fn handle(message: &Value, pipe: &str, exports: &dyn Fn() -> Option<std::path::PathBuf>) -> Option<Value> {
    // Notifications, client responses (no method) and null ids get no reply.
    let id = message.get("id").filter(|id| id.is_string() || id.is_number()).cloned()?;
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        if message.get("result").is_some() || message.get("error").is_some() { return None; }
        return Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32600, "message": "Invalid Request" } }));
    };
    let result = match method {
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
            let outcome = match message.pointer("/params/name").and_then(Value::as_str) {
                Some("export_diagnostics") => export(pipe).map_err(|e| explain(&e)),
                Some("list_diagnostic_exports") => list(exports()).map_err(|e| explain(&e)),
                _ => return Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602, "message": "Unknown tool" } })),
            };
            Ok(match outcome {
                Ok(value) => json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default() }],
                    "structuredContent": value,
                    "isError": false,
                }),
                Err(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": true }),
            })
        }
        _ => Err(json!({ "code": -32601, "message": "Method not found" })),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    })
}

/// Serves MCP over stdin/stdout (newline-delimited JSON-RPC) until stdin closes.
pub fn serve_stdio() -> std::io::Result<()> {
    let pipe = crate::door::pipe_name();
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
                let replies: Vec<Value> = batch.iter().filter_map(|m| handle(m, &pipe, &crate::door::exports_dir)).collect();
                (!replies.is_empty()).then(|| Value::Array(replies))
            }
            Ok(message @ Value::Object(_)) => handle(&message, &pipe, &crate::door::exports_dir),
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

    const NOWHERE: &str = r"\\.\pipe\slopbar-door-test-nowhere";

    fn call(message: Value, dir: Option<std::path::PathBuf>) -> Option<Value> {
        handle(&message, NOWHERE, &move || dir.clone())
    }

    #[test]
    fn initializes_and_lists_only_the_two_tools() {
        let init = call(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26" } }), None).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "slopbar");
        let unknown = call(json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": { "protocolVersion": "1999-01-01" } }), None).unwrap();
        assert_eq!(unknown["result"]["protocolVersion"], PROTOCOL);
        assert!(call(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }), None).is_none());
        let tools = call(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }), None).unwrap();
        let names: Vec<_> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["export_diagnostics", "list_diagnostic_exports"]);
        let missing = call(json!({ "jsonrpc": "2.0", "id": 4, "method": "resources/list" }), None).unwrap();
        assert_eq!(missing["error"]["code"], -32601);
        let tool = call(json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": { "name": "switch_account" } }), None).unwrap();
        assert_eq!(tool["error"]["code"], -32602);
        assert!(call(json!({ "jsonrpc": "2.0", "id": 6, "result": {} }), None).is_none(), "client responses get no reply");
        assert!(call(json!({ "jsonrpc": "2.0", "id": null, "method": "ping" }), None).is_none(), "null ids get no reply");
        let no_method = call(json!({ "jsonrpc": "2.0", "id": 7 }), None).unwrap();
        assert_eq!(no_method["error"]["code"], -32600);
    }

    #[test]
    fn export_without_a_running_app_is_a_tool_error() {
        let reply = call(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "export_diagnostics", "arguments": {} } }), None).unwrap();
        assert_eq!(reply["result"]["isError"], true);
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        if cfg!(windows) { assert!(text.contains("not running"), "{text}"); }
    }

    #[test]
    fn lists_only_export_files_newest_first() {
        let dir = TempDir::new();
        std::fs::write(dir.0.join("SlopBar-Diagnostics-1.json"), b"{}").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.0.join("SlopBar-Diagnostics-2.json"), b"{\"a\":1}").unwrap();
        std::fs::write(dir.0.join("history.bin"), b"secret").unwrap();
        std::fs::create_dir(dir.0.join("SlopBar-Diagnostics-dir.json")).unwrap();
        let reply = call(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "list_diagnostic_exports" } }), Some(dir.0.clone())).unwrap();
        let listed = &reply["result"]["structuredContent"];
        assert_eq!(listed["total"], 2);
        let paths: Vec<_> = listed["exports"].as_array().unwrap().iter().map(|e| e["path"].as_str().unwrap().to_string()).collect();
        assert!(paths[0].ends_with("SlopBar-Diagnostics-2.json") && paths[1].ends_with("SlopBar-Diagnostics-1.json"), "{paths:?}");
        assert_eq!(listed["exports"][0]["bytes"], 7);
        let missing = call(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "list_diagnostic_exports" } }), Some(dir.0.join("absent"))).unwrap();
        assert_eq!(missing["result"]["structuredContent"]["exports"], json!([]));
    }
}
