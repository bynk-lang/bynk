//! Crate-hygiene B3 (#2999): a byte-for-byte snapshot of the entry-handler
//! signatures the playground's compile reports (#2695), taken *before* the
//! signatures move from `bynk-check` into the system model (H-1: output stays
//! byte-identical).
//!
//! One program declares every `EntryShape` variant as a parameter: each base
//! type, a refined and an opaque type, a record, a sum with and without
//! payloads, `List`/`Set`/`Option`/`Map`/`Result`, unit, a self-referential
//! record, a chain of records deeper than `MAX_DEPTH` and a generic record
//! instantiation. Services are reported in name order. An `http` service, a
//! `cron` service and an agent sit beside them and are left out.
//!
//! The expected text lives in `tests/snapshots/entry_signatures.txt`. Set
//! `BYNK_BLESS=1` to rewrite it after an *intended* change.

use std::path::Path;

use bynk_check::firstparty::Platform;

const SRC: &str = r#"context app.sig

type Slug = String where NonEmpty && MaxLength(20)
type Token = opaque String
type Point = { x: Int, y: Int }
type Shape = | Dot | Circle(r: Int) | Rect(w: Int, h: Int)
type Node = { label: String, next: Option[Node] }
type Box[T] = { value: T }
type L1 = { a: L2 }
type L2 = { b: L3 }
type L3 = { c: L4 }
type L4 = { d: L5 }
type L5 = { e: L6 }
type L6 = { f: L7 }
type L7 = { g: Int }

service zeta {
  on call() -> Effect[()] {
    ()
  }
}

service bases {
  on call(i: Int, s: String, b: Bool, f: Float, d: Duration, t: Instant, y: Bytes, m: Decimal) -> Effect[Int] {
    i
  }
}

service named {
  on call(tag: Slug, tok: Token, p: Point, sh: Shape, node: Node, deep: L1, boxed: Box[Int]) -> Effect[String] {
    "ok"
  }
}

service collections {
  on call(l: List[Point], st: Set[Int], o: Option[String], mp: Map[String, Int], r: Result[Int, String], u: ()) -> Effect[Result[(), String]] {
    Ok(())
  }
}

service web from http {
  on GET("/x") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("x")
  }
}

service ticker from cron {
  on schedule("*/5 * * * *") () -> Effect[Result[(), String]] {
    Ok(())
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

/// `serde_json::to_string_pretty`'s layout with object keys sorted, so the
/// snapshot doesn't depend on whether a build turns on
/// `serde_json/preserve_order`.
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
    let v: serde_json::Value =
        serde_json::from_str(&bynk_wasm::compile_to_json(SRC, Platform::Browser)).unwrap();
    assert_eq!(
        v["ok"], true,
        "the fixture must compile: {}",
        v["diagnostics"]
    );
    format!("{}\n", sorted_pretty(&v["entries"], 0))
}

/// The first line where `actual` and `expected` differ, with a little context,
/// so a drift is readable in a CI log without the whole snapshot.
fn first_difference(actual: &str, expected: &str) -> String {
    let (a, e): (Vec<&str>, Vec<&str>) = (actual.lines().collect(), expected.lines().collect());
    let at = (0..a.len().max(e.len()))
        .find(|&i| a.get(i) != e.get(i))
        .unwrap_or(0);
    let mut out = format!("first difference at line {}:\n", at + 1);
    for i in at.saturating_sub(3)..(at + 4) {
        let (x, y) = (a.get(i).copied(), e.get(i).copied());
        if x == y {
            if let Some(l) = x {
                out.push_str(&format!("    {l}\n"));
            }
        } else {
            if let Some(l) = y {
                out.push_str(&format!("  - {l}\n"));
            }
            if let Some(l) = x {
                out.push_str(&format!("  + {l}\n"));
            }
        }
    }
    out
}

#[test]
fn entry_signatures_are_pinned() {
    let actual = render();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/entry_signatures.txt");
    if std::env::var_os("BYNK_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e} (bless with BYNK_BLESS=1)", path.display()));
    assert!(
        actual == expected,
        "the entry signatures drifted from {} (H-1). Re-bless only for an intended change.\n{}",
        path.display(),
        first_difference(&actual, &expected)
    );
}
