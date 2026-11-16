//! #3632: `bynk/wireContract`'s per-response `body` and `bynk openapi` describe
//! the same fixture's responses the same way. A message is `{ error: String }`,
//! a redirect has no body and a `Location` header, raw responses are
//! author-owned bytes and a stream is `text/event-stream`.

use std::path::{Path, PathBuf};
use std::process::Command;

use bynk_ide::wire_contract::{
    ResponseBody, ResponseOrigin, WireContractModel, real_context_count, response_body,
    wire_contract_at,
};
use serde_json::Value;

const SRC: &str = r#"context bodies

type Item = { id: Int, name: String }

service api from http {
  on GET("/value/:id") (id: String) -> Effect[HttpResult[Item]] by Visitor {
    if id == "1" { Ok(Item { id: 1, name: "widget" }) } else { BadRequest("no") }
  }

  on GET("/moved") () -> Effect[HttpResult[Item]] by Visitor {
    SeeOther("/value/1")
  }

  on GET("/page") () -> Effect[HttpResult[()]] by Visitor {
    Raw(Bytes.fromUtf8("<p>hi</p>"), "text/html")
  }

  on GET("/events") () -> Effect[HttpResult[()]] by Visitor {
    Streaming(Stream.of(["a"]).take(1))
  }

  on GET("/gone") () -> Effect[HttpResult[()]] by Visitor {
    NoContent
  }
}
"#;

/// `(needle, OpenAPI path)` for each handler.
const HANDLERS: [(&str, &str); 5] = [
    ("GET(\"/value/:id\")", "/value/{id}"),
    ("GET(\"/moved\")", "/moved"),
    ("GET(\"/page\")", "/page"),
    ("GET(\"/events\")", "/events"),
    ("GET(\"/gone\")", "/gone"),
];

fn project() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("openapi-wire-drift");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("bynk.toml"),
        "[project]\nname = \"bodies\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("src/bodies.bynk"), SRC).unwrap();
    dir
}

fn openapi_doc(dir: &Path) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk"))
        .arg("openapi")
        .arg(dir)
        .output()
        .expect("run bynk");
    assert!(
        out.status.success(),
        "bynk openapi failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout is JSON")
}

fn models(dir: &Path) -> Vec<(&'static str, WireContractModel)> {
    let src = dir.join("src");
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(src.clone()));
    let diag = bynk_ide::diagnose_project(&src, &sources);
    let file = diag
        .files
        .iter()
        .find(|f| f.source_path.to_string_lossy().ends_with("bodies.bynk"))
        .expect("bodies.bynk analysed");
    let info = &diag.boundary_info["bodies"];
    let expr_types = diag
        .expr_types
        .get(&file.source_path)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    let count = real_context_count(&diag.boundary_info, &diag.unit_sources);
    HANDLERS
        .iter()
        .map(|(needle, path)| {
            let offset = file.text.find(needle).expect("handler in fixture");
            let model = wire_contract_at(
                "bodies",
                &file.text,
                offset,
                info,
                expr_types,
                &diag.ty_intern,
                count,
            )
            .expect("a wire contract at the handler");
            (*path, model)
        })
        .collect()
}

/// Every schema OpenAPI gives `media` under `response`, `oneOf` flattened.
fn schemas(response: &Value, media: &str) -> Vec<Value> {
    match &response["content"][media]["schema"] {
        Value::Null => Vec::new(),
        s => match s["oneOf"].as_array() {
            Some(alts) => alts.clone(),
            None => vec![s.clone()],
        },
    }
}

#[test]
fn every_response_body_agrees_with_openapi() {
    let dir = project();
    let doc = openapi_doc(&dir);
    let mut checked = Vec::new();
    for (path, model) in models(&dir) {
        let responses = &doc["paths"][path]["get"]["responses"];
        assert!(responses.is_object(), "no GET {path} in the document");
        for r in &model.responses {
            if r.origin == ResponseOrigin::DeclaredSuccess && !model.ok_reachable {
                continue; // openapi lists only what the handler can answer
            }
            let Some(body) = response_body(r, model.success.as_ref()) else {
                continue;
            };
            let resp = &responses[r.status.to_string()];
            assert!(resp.is_object(), "{path}: no {} in openapi", r.status);
            match &body {
                ResponseBody::Value(_) => {
                    assert!(
                        !schemas(resp, "application/json").is_empty(),
                        "{path}: a value body is JSON"
                    );
                }
                ResponseBody::Message => {
                    let error = |s: &Value| {
                        s["required"] == serde_json::json!(["error"])
                            && s["properties"]["error"]["type"] == "string"
                    };
                    assert!(
                        schemas(resp, "application/json").iter().any(error),
                        "{path}: a message is `{{ error: String }}`: {resp}"
                    );
                }
                ResponseBody::Redirect => {
                    assert!(resp["content"].is_null(), "{path}: a redirect has no body");
                    assert!(
                        resp["headers"][bynk_ide::wire_contract::REDIRECT_HEADER].is_object(),
                        "{path}: a redirect names its header"
                    );
                }
                ResponseBody::Raw => {
                    let raw = schemas(resp, "*/*");
                    assert!(
                        raw.iter().any(|s| s["format"] == "binary"),
                        "{path}: raw is author-owned bytes: {resp}"
                    );
                }
                ResponseBody::Stream => {
                    assert!(
                        !schemas(resp, "text/event-stream").is_empty(),
                        "{path}: a stream is text/event-stream"
                    );
                }
            }
            checked.push(std::mem::discriminant(&body));
        }
    }
    // Every kind was exercised, so a fixture drifting away from one fails here.
    checked.sort_by_key(|d| format!("{d:?}"));
    checked.dedup();
    assert_eq!(checked.len(), 5, "all five body kinds are covered");
}
