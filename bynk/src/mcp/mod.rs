//! `bynk mcp` — a stdio Model Context Protocol server (#2466).
//!
//! The same read-only answers the VS Code language-model tools give (#2463), for
//! any agent that speaks MCP: diagnostics, a symbol's type and definition, a
//! unit's documentation, the project's architecture, a handler's wire contract,
//! a diagnostic's explanation, a project check, and a search of the Book. Each answers in compact
//! Markdown with `path:line:column` locations (1-based, relative to the
//! workspace), and every result is capped with a note on how to ask for less.
//!
//! **Where it lives (DECISION A).** In the driver, calling `bynk-ide` and the
//! project model directly rather than through `bynk-lsp`: an agent has no editor
//! to hold a language server open, and a process that analyses on demand needs
//! neither the LSP's transport nor its async runtime. The cost is that the
//! answers come from the files on disk, not from unsaved buffers.
//!
//! **Read-only (DECISION C).** No tool writes, runs the compiler's emitter, or
//! reaches the network, and a `file` argument cannot name anything outside the
//! workspace root (`Workspace::resolve_file`), so an agent steered
//! by text it read cannot point a tool at another project.
//!
//! **Transport (DECISION D).** Hand-written JSON-RPC 2.0 over newline-delimited
//! stdio, ~100 lines (`Server::handle`), with no MCP SDK: the driver is
//! synchronous and carries no async runtime, the protocol surface used here
//! (`initialize`, `ping`, `tools/list`, `tools/call`) is small and stable, and
//! an SDK would add a runtime and its dependency tree to every `bynk` install
//! for it. See the ADR in `design/pending/bynk-mcp.md`.
//!
//! Standard output carries protocol messages only; anything for a human goes to
//! standard error.

mod render;
mod tools;
mod workspace;

use std::io::{BufRead, Write};
use std::path::Path;
use std::process::ExitCode;

use serde_json::{Value, json};

use workspace::Workspace;

/// The protocol revisions this server speaks, newest first. `initialize` answers
/// with the client's if it is listed, else the newest.
const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// Run `bynk mcp` over the process's stdin and stdout, serving `path`.
pub fn run(path: &Path) -> ExitCode {
    let workspace = match Workspace::new(path) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("bynk mcp: {e}");
            return ExitCode::FAILURE;
        }
    };
    let server = Server { workspace };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = server.handle(&line)
            && (writeln!(stdout, "{reply}").is_err() || stdout.flush().is_err())
        {
            break;
        }
    }
    ExitCode::SUCCESS
}

/// The server's state: just the workspace it may read. Each call re-analyses
/// from disk, so an edit an agent just made is always seen.
struct Server {
    workspace: Workspace,
}

impl Server {
    /// One incoming line to its reply line, or `None` for a notification (which
    /// is never answered).
    fn handle(&self, line: &str) -> Option<String> {
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return Some(error(
                    Value::Null,
                    PARSE_ERROR,
                    &format!("parse error: {e}"),
                ));
            }
        };
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            // A response to a request we never sent, or not JSON-RPC at all.
            return msg
                .get("id")
                .map(|id| error(id.clone(), INVALID_REQUEST, "expected a request"));
        };
        // A request without an `id` is a notification: handled, never answered.
        let id = msg.get("id")?.clone();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        Some(match self.dispatch(method, &params) {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
            Err((code, message)) => error(id, code, &message),
        })
    }

    fn dispatch(&self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str);
                let version = asked
                    .and_then(|v| PROTOCOL_VERSIONS.iter().find(|p| **p == v))
                    .unwrap_or(&PROTOCOL_VERSIONS[0]);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "bynk", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": tools::INSTRUCTIONS,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools::definitions() })),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or((INVALID_PARAMS, "tools/call needs a tool `name`".to_string()))?;
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                if !args.is_object() {
                    return Err((INVALID_PARAMS, "`arguments` must be an object".to_string()));
                }
                // A tool's own failure is a result the agent can read and act
                // on (`isError`), not a protocol error.
                match tools::call(&self.workspace, name, &args) {
                    tools::Outcome::Unknown => {
                        Err((INVALID_PARAMS, format!("unknown tool `{name}`")))
                    }
                    tools::Outcome::Text(text) => Ok(tool_result(&text, false)),
                    tools::Outcome::Failed(text) => Ok(tool_result(&text, true)),
                }
            }
            other => Err((
                METHOD_NOT_FOUND,
                format!("method `{other}` is not supported"),
            )),
        }
    }
}

fn tool_result(text: &str, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn error(id: Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server {
            workspace: Workspace::new(&std::env::temp_dir()).unwrap(),
        }
    }

    fn reply(server: &Server, msg: Value) -> Value {
        serde_json::from_str(&server.handle(&msg.to_string()).expect("a reply")).unwrap()
    }

    #[test]
    fn initialise_echoes_a_supported_version_else_the_newest() {
        let s = server();
        let r = reply(
            &s,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}),
        );
        assert_eq!(r["result"]["protocolVersion"], "2024-11-05");
        let r = reply(
            &s,
            json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}),
        );
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
        assert_eq!(r["result"]["serverInfo"]["name"], "bynk");
    }

    #[test]
    fn a_notification_gets_no_reply() {
        let s = server();
        let line = json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string();
        assert!(s.handle(&line).is_none());
    }

    #[test]
    fn bad_input_is_a_protocol_error_with_the_request_id() {
        let s = server();
        let r = reply(&s, json!({"jsonrpc":"2.0","id":7,"method":"nope"}));
        assert_eq!(r["id"], 7);
        assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);
        let r: Value = serde_json::from_str(&s.handle("{not json").unwrap()).unwrap();
        assert_eq!(r["error"]["code"], PARSE_ERROR);
        assert_eq!(r["id"], Value::Null);
    }

    #[test]
    fn a_tool_failure_is_a_result_not_a_protocol_error() {
        let s = server();
        let r = reply(
            &s,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
                   "params":{"name":"bynk_describe_symbol","arguments":{}}}),
        );
        assert_eq!(r["result"]["isError"], true);
        let r = reply(
            &s,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"nope"}}),
        );
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn lists_every_tool_with_a_schema() {
        let s = server();
        let r = reply(&s, json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
        let tools = r["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 8);
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object", "{t}");
            assert_eq!(t["annotations"]["readOnlyHint"], true, "{t}");
        }
    }
}
