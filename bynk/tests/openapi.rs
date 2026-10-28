//! `bynk openapi` (#856): the OpenAPI 3.1 export for a project's `from http`
//! services.
//!
//! Three fixtures are pinned as goldens (`BYNK_BLESS=1 cargo test -p bynk`
//! re-blesses them): the rate-limiter example (a path param refined `NonEmpty`,
//! a `200` body and a `429` message body), `shapes` (a request body, a sum and
//! `Option`/`List` fields, every predicate kind, a bodyless `204`), and the
//! `from http` service of a multi-service project. Beside the goldens, a
//! structural check holds every generated document to the parts of the
//! OpenAPI 3.1 schema a consumer's tooling relies on — resolvable `$ref`s,
//! described responses, path templates and `in: path` parameters that agree.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn bynk() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bynk"))
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture(name: &str) -> PathBuf {
    manifest_dir().join("tests/fixtures/openapi").join(name)
}

fn rate_limiter() -> PathBuf {
    manifest_dir().join("../examples/rate-limiter")
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("openapi-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// `bynk openapi <args>` → (success, stdout, stderr).
fn openapi(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bynk())
        .arg("openapi")
        .args(args)
        .output()
        .expect("run bynk");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn doc(path: &Path, extra: &[&str]) -> Value {
    let mut args = vec![path.to_str().unwrap()];
    args.extend_from_slice(extra);
    let (ok, stdout, stderr) = openapi(&args);
    assert!(ok, "bynk openapi failed: {stderr}");
    serde_json::from_str(&stdout).expect("stdout is JSON")
}

fn golden(name: &str, actual: &str) {
    let path = manifest_dir().join("tests/golden").join(name);
    if std::env::var_os("BYNK_BLESS").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!("missing golden {name}; regenerate with BYNK_BLESS=1 cargo test -p bynk")
    });
    assert_eq!(
        expected, actual,
        "golden {name} drifted; re-bless with BYNK_BLESS=1 cargo test -p bynk"
    );
}

/// The parts of the OpenAPI 3.1 schema a consumer relies on.
fn assert_valid(doc: &Value) {
    assert_eq!(doc["openapi"], "3.1.0");
    assert!(doc["info"]["title"].is_string() && doc["info"]["version"].is_string());
    let paths = doc["paths"].as_object().expect("paths");
    assert!(!paths.is_empty());
    let mut ids = BTreeSet::new();
    for (path, item) in paths {
        assert!(path.starts_with('/'), "path {path}");
        assert!(!path.contains(':'), "`:seg` left in {path}");
        let templated: BTreeSet<&str> = path
            .split('/')
            .filter_map(|s| s.strip_prefix('{')?.strip_suffix('}'))
            .collect();
        for (method, op) in item.as_object().unwrap() {
            assert!(["get", "put", "post", "delete", "patch"].contains(&method.as_str()));
            assert!(ids.insert(op["operationId"].as_str().unwrap().to_string()));
            let declared: BTreeSet<&str> = op["parameters"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| p["in"] == "path")
                .map(|p| {
                    assert_eq!(p["required"], true, "a path parameter is required");
                    p["name"].as_str().unwrap()
                })
                .collect();
            assert_eq!(
                declared, templated,
                "{method} {path}: path params vs template"
            );
            let responses = op["responses"].as_object().expect("responses");
            assert!(!responses.is_empty());
            for (code, r) in responses {
                assert!(code.parse::<u16>().is_ok(), "status key {code}");
                assert!(r["description"].is_string(), "{code} needs a description");
            }
        }
    }
    check_refs(doc, doc);
}

fn check_refs(root: &Value, v: &Value) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == "$ref" {
                    let target = x.as_str().unwrap().strip_prefix('#').expect("local ref");
                    let found = root.pointer(target);
                    assert!(
                        found.is_some_and(|t| !t.is_null()),
                        "unresolved or empty $ref {x}"
                    );
                } else {
                    check_refs(root, x);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| check_refs(root, x)),
        _ => {}
    }
}

fn pretty(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap();
    s.push('\n');
    s
}

#[test]
fn rate_limiter_matches_the_issue_fixture() {
    let d = doc(&rate_limiter(), &[]);
    assert_valid(&d);
    let get = &d["paths"]["/check/{client}"]["get"];
    // The `client` path param is a `ClientId` (`String where NonEmpty`): the
    // parameter itself carries `minLength: 1`.
    let client = &get["parameters"][0];
    assert_eq!(
        (client["name"].as_str(), client["in"].as_str()),
        (Some("client"), Some("path"))
    );
    assert_eq!(client["schema"]["type"], "string");
    assert_eq!(client["schema"]["minLength"], 1);
    assert_eq!(client["schema"]["x-bynk-refinements"][0], "NonEmpty");
    // 200 → RateView, 429 → the message body, 400 → the boundary's rejection.
    let r = get["responses"].as_object().unwrap();
    assert_eq!(r.keys().collect::<Vec<_>>(), ["200", "400", "429"]);
    assert_eq!(
        r["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/RateView"
    );
    assert_eq!(
        r["429"]["content"]["application/json"]["schema"]["properties"]["error"]["type"],
        "string"
    );
    let rv = &d["components"]["schemas"]["RateView"];
    assert_eq!(rv["properties"]["remaining"]["type"], "integer");
    assert_eq!(rv["required"].as_array().unwrap().len(), 3);
    golden("openapi-rate-limiter.json", &pretty(&d));
}

#[test]
fn shapes_request_body_sum_and_every_predicate() {
    let d = doc(&fixture("shapes"), &[]);
    assert_valid(&d);
    assert_eq!(d["info"]["title"], "shapes");
    assert_eq!(d["info"]["version"], "1.2.3");

    // A request body: a record → object, its refined fields by `$ref`.
    let post = &d["paths"]["/orders"]["post"];
    assert_eq!(
        post["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/Order"
    );
    let s = &d["components"]["schemas"];
    assert_eq!(s["Order"]["type"], "object");
    assert_eq!(s["Order"]["properties"]["tags"]["type"], "array");
    // `Option[String]` is the `Some`/`None` pair the codec writes.
    assert_eq!(
        s["Order"]["properties"]["note"]["oneOf"][1]["properties"]["kind"]["const"],
        "None"
    );
    // The body-walked response set: Created + the constructed 422 + the boundary 400,
    // and no 200 — the handler never returns `Ok`.
    assert_eq!(
        post["responses"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["201", "400", "422"]
    );

    // A sum → `oneOf` keyed by `kind`.
    let shape = s["Shape"]["oneOf"].as_array().unwrap();
    assert_eq!(shape.len(), 2);
    assert_eq!(shape[0]["properties"]["kind"]["const"], "Circle");
    assert_eq!(
        shape[1]["required"],
        serde_json::json!(["kind", "width", "height"])
    );

    // Predicates → JSON Schema (Decision A).
    assert_eq!(s["Quantity"]["minimum"], 1);
    assert_eq!(s["Quantity"]["maximum"], 100);
    assert_eq!(s["Weight"]["minimum"], 0.5);
    assert_eq!(s["Weight"]["maximum"], 250);
    assert_eq!(s["Handle"]["minLength"], 2);
    assert_eq!(s["Handle"]["maxLength"], 32);
    // `Matches` is anchored the way the codec anchors it; `Length` is both bounds.
    assert_eq!(s["Sku"]["pattern"], "^(?:[A-Z]{3}[0-9]{2})$");
    assert_eq!(
        (s["Sku"]["minLength"].clone(), s["Sku"]["maxLength"].clone()),
        (5.into(), 5.into())
    );
    assert_eq!(
        s["Sku"]["x-bynk-refinements"],
        serde_json::json!(["Matches(\"[A-Z]{3}[0-9]{2}\")", "Length(5)"])
    );
    assert!(
        s["Sku"]["description"]
            .as_str()
            .unwrap()
            .contains("Re-validated at the boundary")
    );

    // A bodyless 204, and the multi-segment route's two path params.
    let del = &d["paths"]["/orders/{sku}"]["delete"]["responses"];
    assert_eq!(
        del.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["204", "400"]
    );
    assert!(del["204"].get("content").is_none());
    let line = &d["paths"]["/orders/{sku}/lines/{line}"]["get"]["parameters"];
    assert_eq!(line.as_array().unwrap().len(), 2);
    golden("openapi-shapes.json", &pretty(&d));
}

#[test]
fn yaml_carries_the_same_document() {
    let dir = scratch("yaml");
    let out = dir.join("api.yaml");
    let (ok, stdout, stderr) = openapi(&[
        fixture("shapes").to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "--out leaves stdout empty");
    // `.yaml` picks the YAML format.
    let yaml = std::fs::read_to_string(&out).unwrap();
    assert!(yaml.starts_with("components:\n"), "{yaml}");
    assert!(yaml.contains("openapi: \"3.1.0\"\n"));
    assert!(yaml.contains("  /orders/{sku}/lines/{line}:\n"));
    assert!(yaml.contains("pattern: \"^(?:[A-Z]{3}[0-9]{2})$\""));
    // And `--format` overrides the extension.
    let json_out = dir.join("api.yaml.json");
    let (ok, _, e) = openapi(&[
        fixture("shapes").to_str().unwrap(),
        "--out",
        json_out.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert!(ok, "{e}");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(json_out).unwrap()).unwrap();
    assert_valid(&v);
}

const TWO_SERVICES: &str = r#"context shop

type Name = String where NonEmpty

service admin from http {
  on GET("/admin/:name") (name: Name) -> Effect[HttpResult[String]] by v: Visitor {
    Ok("found")
  }
}

service public from http {
  on GET("/hello") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("hi")
  }
}
"#;

fn two_service_project(name: &str) -> PathBuf {
    let dir = scratch(name);
    write(
        &dir.join("bynk.toml"),
        "[project]\nname = \"shop\"\nversion = \"0.3.0\"\n",
    );
    write(&dir.join("src/shop.bynk"), TWO_SERVICES);
    dir
}

#[test]
fn service_flag_narrows_the_document() {
    let dir = two_service_project("two");
    let all = doc(&dir, &[]);
    assert_valid(&all);
    assert_eq!(all["paths"].as_object().unwrap().len(), 2);
    assert_eq!(all["tags"].as_array().unwrap().len(), 2);

    let only = doc(&dir, &["--service", "public"]);
    assert_valid(&only);
    assert_eq!(
        only["paths"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["/hello"]
    );
    // `unit.Name` is accepted too.
    let qualified = doc(&dir, &["--service", "shop.admin"]);
    assert!(qualified["paths"].get("/admin/{name}").is_some());
    golden("openapi-two-services.json", &pretty(&all));
}

#[test]
fn unknown_service_is_named_with_the_known_ones() {
    let dir = two_service_project("unknown");
    let (ok, _, stderr) = openapi(&[dir.to_str().unwrap(), "--service", "nope"]);
    assert!(!ok);
    assert!(stderr.contains("no `from http` service `nope`"), "{stderr}");
    assert!(stderr.contains("admin, public"), "{stderr}");
}

#[test]
fn a_project_with_no_http_service_says_so() {
    let dir = scratch("none");
    write(&dir.join("bynk.toml"), "[project]\nname = \"p\"\n");
    write(&dir.join("src/p.bynk"), "context p\n\ntype X = Int\n");
    let (ok, _, stderr) = openapi(&[dir.to_str().unwrap()]);
    assert!(!ok);
    assert!(stderr.contains("no `from http` service"), "{stderr}");
}

#[test]
fn a_project_with_errors_is_refused_with_them_listed() {
    let dir = scratch("broken");
    write(&dir.join("bynk.toml"), "[project]\nname = \"p\"\n");
    write(
        &dir.join("src/p.bynk"),
        "context p\n\nservice s from http {\n  on GET(\"/x\") () -> Effect[HttpResult[Int]] {\n    Ok(1)\n  }\n}\n",
    );
    let (ok, stdout, stderr) = openapi(&[dir.to_str().unwrap()]);
    assert!(!ok);
    assert!(stdout.is_empty(), "no partial document on failure");
    assert!(stderr.contains("fix them first"), "{stderr}");
    assert!(stderr.contains("src/p.bynk:4"), "{stderr}");
}

#[test]
fn no_manifest_is_a_clear_error() {
    let dir = scratch("nomanifest");
    let (ok, _, stderr) = openapi(&[dir.to_str().unwrap()]);
    assert!(!ok);
    assert!(stderr.contains("bynk.toml"), "{stderr}");
}
