//! #2466: `bynk mcp` over its real transport — spawn the binary, speak JSON-RPC
//! on stdio, initialise, list the tools, and call each one against a small
//! project on disk. The protocol loop and the renderers have unit tests beside
//! them; this proves the wiring: the subcommand, the workspace boundary, and
//! the answers an agent would actually read.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const MANIFEST: &str =
    "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\"]\n";

const APP: &str = r#"context shop

---
An item for sale; the quantity is positive at the boundary.
---
type Item = {
  id:  String,
  qty: Int where Positive,
}

service api from http {
  on POST("/items") (body: Item) -> Effect[HttpResult[Item]] by Visitor {
    Created(body)
  }
}
"#;

/// A second unit with a deliberate error, so diagnostics have something to say.
const BROKEN: &str = "commons broken\n\ntype Bad = {\n  x: Missing,\n}\n";

/// A throwaway project, removed on drop.
struct Project(PathBuf);

impl Project {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("bynk-mcp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("bynk.toml"), MANIFEST).unwrap();
        std::fs::write(root.join("src/shop.bynk"), APP).unwrap();
        std::fs::write(root.join("src/broken.bynk"), BROKEN).unwrap();
        Self(root.canonicalize().unwrap())
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `bynk mcp <dir>` with every request on stdin, then close it (which ends
/// the server) and return the replies in order.
fn session(dir: &Path, requests: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bynk"))
        .arg("mcp")
        .arg(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn bynk mcp");
    let mut stdin = child.stdin.take().unwrap();
    for r in requests {
        writeln!(stdin, "{r}").unwrap();
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {l}"))
        })
        .collect()
}

fn call(id: u64, tool: &str, args: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":tool,"arguments":args}})
}

/// A tool reply's text and whether it is flagged an error.
fn text(reply: &Value) -> (&str, bool) {
    let r = &reply["result"];
    (
        r["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("no text in {reply}")),
        r["isError"].as_bool().unwrap(),
    )
}

#[test]
fn initialises_lists_and_calls_every_tool() {
    let p = Project::new("tools");
    let replies = session(
        &p.0,
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-06-18","capabilities":{},
                "clientInfo":{"name":"test","version":"0"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            call(3, "bynk_get_diagnostics", json!({})),
            call(4, "bynk_get_diagnostics", json!({"file":"src/shop.bynk"})),
            call(5, "bynk_check", json!({})),
            call(
                6,
                "bynk_explain_diagnostic",
                json!({"code":"bynk.resolve.unknown_type"}),
            ),
            call(7, "bynk_describe_symbol", json!({"name":"Item"})),
            call(
                8,
                "bynk_describe_symbol",
                json!({"file":"src/shop.bynk","line":6,"name":"Item"}),
            ),
            call(9, "bynk_get_documentation", json!({"file":"src/shop.bynk"})),
            call(10, "bynk_get_architecture", json!({})),
            call(
                11,
                "bynk_get_wire_contract",
                json!({"file":"src/shop.bynk","line":12}),
            ),
        ],
    );
    // The notification gets no reply: one per request.
    assert_eq!(replies.len(), 11, "{replies:#?}");

    assert_eq!(replies[0]["id"], 1);
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "bynk");
    assert!(replies[0]["result"]["capabilities"]["tools"].is_object());

    let names: Vec<&str> = replies[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "bynk_get_diagnostics",
            "bynk_check",
            "bynk_explain_diagnostic",
            "bynk_describe_symbol",
            "bynk_get_documentation",
            "bynk_get_architecture",
            "bynk_get_wire_contract",
        ]
    );

    let (diags, err) = text(&replies[2]);
    assert!(!err);
    assert!(
        diags.contains("src/broken.bynk:4:6 error[bynk.resolve.unknown_type]"),
        "{diags}"
    );
    assert!(
        diags.starts_with("1 error(s), 0 warning(s) in the workspace."),
        "{diags}"
    );

    // Narrowed to a clean file.
    assert_eq!(
        text(&replies[3]).0,
        "No Bynk diagnostics in src/shop.bynk.\n"
    );

    let (check, _) = text(&replies[4]);
    assert!(check.contains("src/broken.bynk:4:6"), "{check}");
    assert!(check.contains("from `bynk check`"), "{check}");

    let (explained, _) = text(&replies[5]);
    assert!(
        explained.starts_with("# `bynk.resolve.unknown_type`"),
        "{explained}"
    );
    assert!(explained.contains("Book: https://"), "{explained}");

    for (i, label) in [(6, "by name"), (7, "at a position")] {
        let (sym, err) = text(&replies[i]);
        assert!(!err, "{label}: {sym}");
        assert!(sym.contains("type Item"), "{label}: {sym}");
        assert!(sym.contains("An item for sale"), "{label}: {sym}");
        assert!(
            sym.contains("Defined at: src/shop.bynk:6:6"),
            "{label}: {sym}"
        );
    }

    let (docs, _) = text(&replies[8]);
    assert!(docs.starts_with("# context `shop`"), "{docs}");
    assert!(docs.contains("type `Item` (src/shop.bynk:6:6)"), "{docs}");

    let (arch, _) = text(&replies[9]);
    assert!(arch.contains("## context `shop`"), "{arch}");
    assert!(arch.contains("- service `api`: 1 handler(s)"), "{arch}");

    let (wire, err) = text(&replies[10]);
    assert!(!err, "{wire}");
    assert!(
        wire.contains("service `api` · POST /items (src/shop.bynk:12:3)"),
        "{wire}"
    );
    assert!(wire.contains("Request: `body: Item`."), "{wire}");
    assert!(wire.contains("- 201 `Created`"), "{wire}");
    assert!(wire.contains("`qty`: Int"), "{wire}");
}

#[test]
fn tool_failures_are_results_and_the_workspace_is_a_boundary() {
    let p = Project::new("errors");
    let outside =
        std::env::temp_dir().join(format!("bynk-mcp-outside-{}.bynk", std::process::id()));
    std::fs::write(&outside, "commons outside\n").unwrap();
    let replies = session(
        &p.0,
        &[
            call(
                1,
                "bynk_get_documentation",
                json!({"file": outside.to_str().unwrap()}),
            ),
            call(
                2,
                "bynk_get_documentation",
                json!({"file": "../../etc/passwd"}),
            ),
            call(
                3,
                "bynk_get_documentation",
                json!({"file": "src/nope.bynk"}),
            ),
            call(
                4,
                "bynk_get_wire_contract",
                json!({"file":"src/shop.bynk","line":3}),
            ),
            call(5, "bynk_describe_symbol", json!({})),
            call(
                6,
                "bynk_explain_diagnostic",
                json!({"code":"bynk.not.a.code"}),
            ),
            call(7, "no_such_tool", json!({})),
            json!({"jsonrpc":"2.0","id":8,"method":"resources/list"}),
        ],
    );
    let _ = std::fs::remove_file(&outside);
    for reply in &replies[..3] {
        let (msg, err) = text(reply);
        assert!(err, "{msg}");
    }
    assert!(text(&replies[0]).0.contains("outside this workspace"));
    assert!(text(&replies[2]).0.contains("No file `src/nope.bynk`"));
    assert!(
        text(&replies[3])
            .0
            .contains("No service handler at src/shop.bynk:3")
    );
    assert!(text(&replies[4]).1);
    assert!(text(&replies[5]).0.contains("No extended explanation"));
    assert_eq!(replies[6]["error"]["code"], -32602);
    assert_eq!(replies[7]["error"]["code"], -32601);
}

#[test]
fn nothing_the_tools_do_writes_to_the_project() {
    let p = Project::new("readonly");
    let before = snapshot(&p.0);
    session(
        &p.0,
        &[
            call(1, "bynk_check", json!({})),
            call(2, "bynk_get_documentation", json!({"file":"src/shop.bynk"})),
            call(3, "bynk_get_architecture", json!({})),
        ],
    );
    assert_eq!(before, snapshot(&p.0));
}

/// Every file under `dir` with its contents.
fn snapshot(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push((
                    path.clone(),
                    std::fs::read_to_string(&path).unwrap_or_default(),
                ));
            }
        }
    }
    out.sort();
    out
}

#[cfg(unix)]
#[test]
fn analysis_does_not_follow_the_manifest_or_a_symlink_out_of_the_workspace() {
    let p = Project::new("escape");
    let outside = std::env::temp_dir().join(format!("bynk-mcp-elsewhere-{}", std::process::id()));
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(
        outside.join("secret.bynk"),
        "commons secret\n\ntype Hidden = Int\n",
    )
    .unwrap();
    // A symlinked source directory inside the project, and an include that climbs out.
    std::os::unix::fs::symlink(&outside, p.0.join("src/linked")).unwrap();
    std::fs::write(
        p.0.join("bynk.toml"),
        MANIFEST.replace(
            "[\"src\"]",
            &format!("[\"src\", \"{}\"]", outside.display()),
        ),
    )
    .unwrap();
    let replies = session(
        &p.0,
        &[
            call(1, "bynk_describe_symbol", json!({"name": "Hidden"})),
            call(2, "bynk_get_architecture", json!({})),
        ],
    );
    let _ = std::fs::remove_dir_all(&outside);
    let (sym, _) = text(&replies[0]);
    assert!(sym.starts_with("No Bynk symbol named `Hidden`"), "{sym}");
    assert!(!sym.contains("elsewhere"), "{sym}");
}
