//! #4099: the JSON Schema `Ai.generateValue[T]` gives a model and the one `bynk openapi` publishes
//! for a plain-JSON route (#3974) are two derivations of the same wire shape, so they must agree.
//! A table of types (records, enums, sums, lists, `Option`, maps, `Instant`, refined bases) is
//! compiled once, rendered both ways, and compared after the differences that are deliberate are
//! removed (see [`normalise`]). Any other difference fails.
//!
//! Follow-up: one derivation, shared by both.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use bynk_syntax::ast::{CommonsItem, Ident, SourceUnit, TypeDecl, TypeRef};
use serde_json::Value;

const TYPES: &str = r#"
type Severity = enum { Low, Medium, High }

type Summary = String where MaxLength(200)

type Qty = Int where InRange(1, 100)

type Ratio = Float where InRange(0.0, 1.0)

type Shape =
  | Circle(radius: Ratio)
  | Rect(width: Qty, height: Qty)

type Triage = {
  severity: Severity,
  summary: Summary,
  tags: List[String],
  owner: Option[String],
  score: Int where InRange(0, 10),
}

type Counts = { byTag: Map[String, Int], at: Instant }

type Drawing = { shapes: List[Shape], title: Option[Summary] }

type Pos = Int where Positive

type Count = Int where NonNegative

type Wait = Duration where InRange(1.second, 1.day)

type Mixed =
  | Circle(radius: Ratio)
  | Dot

type Misc = {
  data: Bytes,
  price: Decimal,
  wait: Duration,
  flags: Set[String],
  outcome: Result[Int, String],
  shape: Mixed,
}
"#;

/// The types the table puts through both derivations.
const NAMES: &[&str] = &[
    "Severity", "Summary", "Qty", "Ratio", "Shape", "Triage", "Counts", "Drawing", "Pos", "Count",
    "Wait", "Mixed", "Misc",
];

fn scratch() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ai-schema-drift");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    dir
}

/// The project: the table, and one plain-JSON route per type that answers with what it was sent, so
/// each is published (a plain route's request body is rendered tagged, so the plain copy is
/// `<Type>.plain`).
fn project() -> PathBuf {
    let dir = scratch();
    std::fs::write(
        dir.join("bynk.toml"),
        "[project]\nname = \"drift\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let mut routes = String::new();
    for (i, name) in NAMES.iter().enumerate() {
        routes.push_str(&format!(
            "  @limit(maxBody: 4096)\n  on POST(\"/t{i}\") (body: {name}) -> Effect[HttpResult[{name}]] by Visitor {{ Ok(body) }}\n"
        ));
    }
    let src = format!(
        "context drift\n{TYPES}\nservice api from http {{\n  json {{ options: plain }}\n{routes}}}\n"
    );
    std::fs::write(dir.join("src/drift.bynk"), src).unwrap();
    dir
}

fn published(dir: &PathBuf) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk"))
        .arg("openapi")
        .arg(dir)
        .output()
        .expect("run bynk openapi");
    assert!(
        out.status.success(),
        "bynk openapi failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("JSON")
}

/// `bynk-check`'s own schema for each type of the table.
fn ai_schemas() -> BTreeMap<String, Value> {
    let source = format!("commons drift\n{TYPES}\n");
    let tokens = bynk_syntax::lexer::tokenize(&source).expect("lexes");
    let (unit, errors) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, &source);
    assert!(errors.is_empty(), "{errors:?}");
    let Some(SourceUnit::Commons(c)) = unit else {
        panic!("a commons")
    };
    let types: std::collections::HashMap<String, Arc<TypeDecl>> = c
        .items
        .iter()
        .filter_map(|i| match i {
            CommonsItem::Type(t) => Some((t.name.name.clone(), Arc::new(t.clone()))),
            _ => None,
        })
        .collect();
    NAMES
        .iter()
        .map(|name| {
            let t = TypeRef::Named(Ident {
                name: (*name).to_string(),
                span: bynk_syntax::span::Span::default(),
            });
            let text = bynk_check::ai_schema::schema_of(&t, name, &types)
                .unwrap_or_else(|e| panic!("{name} has no schema: {e:?}"));
            ((*name).to_string(), serde_json::from_str(&text).unwrap())
        })
        .collect()
}

/// The published schema with its `$ref`s resolved in place.
fn inline(v: &Value, components: &Value) -> Value {
    match v {
        Value::Object(m) => {
            if let Some(Value::String(r)) = m.get("$ref") {
                let name = r.rsplit('/').next().unwrap();
                return inline(&components[name], components);
            }
            Value::Object(
                m.iter()
                    .map(|(k, v)| (k.clone(), inline(v, components)))
                    .collect(),
            )
        }
        Value::Array(a) => Value::Array(a.iter().map(|v| inline(v, components)).collect()),
        other => other.clone(),
    }
}

/// What differs on purpose, and is removed from both sides before they are compared:
///
/// - the publisher's prose and extensions (`description`, `title`, `x-bynk-*`), which a model does
///   not need and a published document does;
/// - `additionalProperties: false`, which the model's schema states to keep the reply to the
///   declared fields (the decoder ignores unknown keys, so it is not a wire rule);
/// - a `Duration`'s `type`: the publisher says `number`, and the model's schema says `integer`,
///   which is what the decoder reads (whole milliseconds, a safe integer), so a model is not
///   invited to answer `1.5`; the publisher's `x-bynk-base` marks it, and it is read as `integer`;
/// - `required`'s order, which is the declaration order on one side and the publisher's on the
///   other.
fn normalise(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut out = serde_json::Map::new();
            let duration = m.get("x-bynk-base").and_then(Value::as_str) == Some("Duration");
            for (k, v) in m {
                if duration && k == "type" {
                    out.insert(k.clone(), Value::String("integer".to_string()));
                    continue;
                }
                if k == "description"
                    || k == "title"
                    || k.starts_with("x-bynk")
                    || (k == "additionalProperties" && v == &Value::Bool(false))
                {
                    continue;
                }
                let v = if k == "required" {
                    let mut names: Vec<Value> = v.as_array().cloned().unwrap_or_default();
                    names.sort_by_key(|n| n.as_str().unwrap_or_default().to_string());
                    Value::Array(names)
                } else {
                    normalise(v)
                };
                out.insert(k.clone(), v);
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(normalise).collect()),
        other => other.clone(),
    }
}

#[test]
fn the_models_schema_and_the_published_one_agree() {
    let dir = project();
    let doc = published(&dir);
    let components = &doc["components"]["schemas"];
    let mine = ai_schemas();
    let mut differences = Vec::new();
    for name in NAMES {
        let plain = format!("{name}.plain");
        let schema = if components.get(&plain).is_some() {
            &components[plain.as_str()]
        } else {
            &components[*name]
        };
        let published = normalise(&inline(schema, components));
        let model = normalise(&mine[*name]);
        if published != model {
            differences.push(format!(
                "{name}:\n  published: {published}\n  model:     {model}"
            ));
        }
    }
    assert!(
        differences.is_empty(),
        "the two derivations of the wire shape disagree:\n{}",
        differences.join("\n")
    );
}
