//! Crate-hygiene track, slice B1 (#2939): byte-for-byte snapshots of every
//! editor output that renders a handler kind or a service protocol, taken
//! *before* `bynk-ide` moves onto the system model (H-1: LSP responses stay
//! byte-identical).
//!
//! One fixture declares all eight handler kinds (`call`, `GET`/`POST`,
//! `schedule`, `message`, `open`, `close`, `event`, `rejected`) and all six
//! service protocols. Per handler it pins:
//! - the `bynk/sequenceModel` wire JSON
//! - the `bynk/wireContract` wire JSON's `kind`, `service` and `handlerLine`
//!
//! Per service it pins the hover (`describe_symbol`), plus every
//! documentation-page entry's heading and Markdown.
//!
//! The expected text lives in `tests/snapshots/handler_kinds.txt`. Set
//! `BYNK_BLESS=1` to rewrite it after an *intended* change.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use bynk_ide::wire_contract::{real_context_count, wire_contract_at};

const KINDS_SRC: &str = r#"context kinds

consumes bynk { Events }

type Frame = { text: String }

event Pinged = { n: Int }

type UserId = opaque String

actor Caller { auth = Bearer(secret = "AUTH_SECRET"), identity = UserId }

service calc {
  on call(x: Int) -> Effect[Int] {
    x
  }
}

service web from http {
  on GET("/items/:id") (id: String) -> Effect[HttpResult[String]] by v: Visitor {
    Ok(id)
  }

  on POST("/items") (body: String) -> Effect[HttpResult[String]] by v: Visitor {
    Ok(body)
  }

  on rejected(why: BoundaryRejection) -> Effect[HttpResult[()]] {
    RawError(BadRequest, Bytes.fromUtf8("bad"), "text/plain")
  }
}

service ticker from cron {
  on schedule("*/5 * * * *") () -> Effect[Result[(), String]] {
    Ok(())
  }
}

service jobs from queue("jobs") {
  on message(job: String) -> Effect[QueueResult] {
    Ack
  }
}

service gate from websocket(in: Frame, out: Frame) {
  on open (room: String) -> Effect[()] by c: Caller {
    let _ <- Room(room).join(c.identity, connection)
    ()
  }

  on message (room: String, frame: Frame) -> Effect[()] by c: Caller {
    ()
  }

  on close (room: String) -> Effect[()] by c: Caller {
    ()
  }
}

service onPing from Events(Pinged) {
  on event(p: Pinged) -> Effect[()] {
    ()
  }
}

service ping {
  on call(n: Int) -> Effect[()] given Events {
    Events.emit[Pinged](Pinged { n: n })
  }
}

agent Room {
  key id: String

  store conns: Map[UserId, Connection[Frame]]

  on call join(u: UserId, conn: Connection[Frame]) -> Effect[()] {
    let _ <- conns.put(u, conn)
    ()
  }
}

agent Counter {
  key id: String

  store count: Cell[Int]

  on call bump() -> Effect[Int] {
    let _ <- count.update((c) => c + 1)
    count
  }
}
"#;

/// Each handler, located by a needle at its header. `(label, needle)`.
const HANDLERS: &[(&str, &str)] = &[
    ("calc call", "on call(x: Int)"),
    ("web GET", "on GET(\"/items/:id\")"),
    ("web POST", "on POST(\"/items\")"),
    ("web rejected", "on rejected("),
    ("ticker schedule", "on schedule("),
    ("jobs message", "on message(job"),
    ("gate open", "on open (room"),
    ("gate message", "on message (room"),
    ("gate close", "on close (room"),
    ("onPing event", "on event("),
    ("Counter bump", "on call bump()"),
];

const SERVICES: &[&str] = &["calc", "web", "ticker", "jobs", "gate", "onPing", "Counter"];

fn setup_project(files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-handler-kind-snapshots-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create test root");
    for (rel, contents) in files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).expect("create parent");
        std::fs::write(&p, contents).expect("write file");
    }
    root
}

fn find_offset(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not found in fixture"))
}

/// `serde_json::to_string_pretty`'s layout, with object keys sorted. A
/// `serde_json::Map`'s iteration order depends on whether some crate in the
/// build turns on `serde_json/preserve_order`, so the picked wire-contract
/// fields are printed in an order that doesn't.
fn sorted_pretty(v: &serde_json::Value, indent: usize) -> String {
    let pad = |n: usize| "  ".repeat(n);
    match v {
        serde_json::Value::Object(map) if !map.is_empty() => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let body: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        "{}{}: {}",
                        pad(indent + 1),
                        serde_json::to_string(k).unwrap(),
                        sorted_pretty(&map[k.as_str()], indent + 1)
                    )
                })
                .collect();
            format!("{{\n{}\n{}}}", body.join(",\n"), pad(indent))
        }
        serde_json::Value::Array(items) if !items.is_empty() => {
            let body: Vec<String> = items
                .iter()
                .map(|i| format!("{}{}", pad(indent + 1), sorted_pretty(i, indent + 1)))
                .collect();
            format!("[\n{}\n{}]", body.join(",\n"), pad(indent))
        }
        other => serde_json::to_string(other).unwrap(),
    }
}

fn render() -> String {
    let root = setup_project(&[("kinds.bynk", KINDS_SRC)]);
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let diag = bynk_ide::diagnose_project(&root, &sources);
    let errors: Vec<String> = diag
        .files
        .iter()
        .flat_map(|f| f.diagnostics.iter())
        .chain(diag.unattributed.iter())
        .filter(|d| d.severity == bynk_syntax::error::Severity::Error)
        .map(|d| format!("{:?}", d.error))
        .collect();
    assert!(
        errors.is_empty(),
        "the fixture must check cleanly: {errors:#?}"
    );

    let file = diag
        .files
        .iter()
        .find(|f| f.source_path.to_string_lossy().ends_with("kinds.bynk"))
        .expect("kinds.bynk analysed");
    let (rel, text) = (file.source_path.clone(), file.text.clone());
    let info = diag.boundary_info.get("kinds").expect("boundary info");
    let seq_info = diag.sequence_info.get("kinds");
    let expr_types = diag
        .expr_types
        .get(&rel)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    let context_count = real_context_count(&diag.boundary_info, &diag.unit_sources);
    let snapshots: HashMap<PathBuf, String> = diag
        .files
        .iter()
        .map(|f| (f.source_path.clone(), f.text.clone()))
        .collect();
    let search_order = diag.doc_scope.get("kinds").cloned().unwrap_or_default();
    let root_str = root.to_string_lossy().into_owned();

    let mut out = String::new();
    for (label, needle) in HANDLERS {
        let offset = find_offset(&text, needle);
        writeln!(out, "=== sequence: {label}").unwrap();
        let model = bynk_lsp::sequence_request::sequence_model_at(&text, offset, seq_info)
            .unwrap_or_else(|| panic!("a sequence model at {label}"));
        let wire = bynk_lsp::sequence_request::to_wire(&model, &text);
        writeln!(out, "{}", serde_json::to_string_pretty(&wire).unwrap()).unwrap();

        writeln!(out, "=== wire contract: {label}").unwrap();
        match wire_contract_at(
            "kinds",
            &text,
            offset,
            info,
            expr_types,
            &diag.ty_intern,
            context_count,
        ) {
            Some(model) => {
                let wire = bynk_lsp::wire_contract_request::to_wire(
                    &model,
                    &root,
                    &text,
                    &info.types,
                    &diag.index,
                    &snapshots,
                    &search_order,
                );
                let json = serde_json::to_value(&wire).unwrap();
                let pick = serde_json::json!({
                    "unit": json["unit"],
                    "service": json["service"],
                    "kind": json["kind"],
                    "handlerLine": json["handlerLine"],
                    "range": json["range"],
                });
                let s = sorted_pretty(&pick, 0);
                writeln!(out, "{}", s.replace(&root_str, "<root>")).unwrap();
            }
            None => writeln!(out, "(none)").unwrap(),
        }
    }
    for name in SERVICES {
        writeln!(out, "=== hover: {name}").unwrap();
        let hover = bynk_ide::symbols::describe_symbol(&text, name).unwrap_or_default();
        writeln!(out, "{hover}").unwrap();
    }
    writeln!(out, "=== documentation entries").unwrap();
    let doc = bynk_ide::documentation::documentation_model(&text).expect("a documentation model");
    for e in &doc.entries {
        writeln!(out, "--- [{}] {}", e.kind, e.name).unwrap();
        writeln!(out, "{}", e.markdown).unwrap();
    }
    let _ = std::fs::remove_dir_all(&root);
    out
}

#[test]
fn handler_kind_and_protocol_renderings_are_pinned() {
    let actual = render();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/handler_kinds.txt");
    if std::env::var_os("BYNK_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e} (bless with BYNK_BLESS=1)", path.display()));
    assert!(
        actual == expected,
        "handler-kind renderings drifted from {} (H-1). Re-bless only for an intended \
         change.\n--- actual ---\n{actual}",
        path.display()
    );
}
