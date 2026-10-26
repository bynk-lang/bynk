//! v0.22b behavioural test for the typed JSON codec (ADRs 0045/0047/0049).
//!
//! Snapshots prove the emitted shape; this proves the behaviour: a record
//! with a `Float` field round-trips exactly (the v0.21 dependency), a
//! malformed document decodes to a `Malformed` JsonError, a shape mismatch
//! carries the tracked field path, and — the 0049 tightening — a fractional
//! number into a bare-`Int` field is rejected. Compiles the json-codec
//! fixture in single-file mode and drives `save`/`load` with `tsc` + `node`.
//!
//! Like the tsc-verification stage, this skips loudly when no TypeScript
//! toolchain is available; `BYNK_REQUIRE_TSC=1` turns the skip into a
//! failure (CI).

use std::fs;
use std::path::Path;
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

/// Build a `Command` for `program`, routing through `cmd /C` on Windows so
/// npm's `.cmd` shims (`tsc.cmd`, `npx.cmd`) resolve — Rust's CreateProcess
/// deliberately refuses to run batch scripts directly (the BatBadBut
/// hardening), so a bare `Command::new("npx")` fails there.
fn base_command(program: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        Command::new(program)
    }
}

fn tool_exists(name: &str) -> bool {
    // `where` is the Windows counterpart of `which`.
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder)
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn discover_tsc() -> Option<(String, Vec<String>)> {
    if tool_exists("tsc") {
        return Some(("tsc".to_string(), vec![]));
    }
    if tool_exists("npx") {
        return Some((
            "npx".to_string(),
            vec![
                "--yes".to_string(),
                "-p".to_string(),
                format!("typescript@{}", bynk_emit::TYPESCRIPT_MAJOR_TESTED),
                "tsc".to_string(),
            ],
        ));
    }
    None
}

fn run(program: &str, prefix: &[String], args: &[&str], cwd: &Path) -> (bool, String) {
    let mut cmd = base_command(program);
    for p in prefix {
        cmd.arg(p);
    }
    for a in args {
        cmd.arg(a);
    }
    cmd.current_dir(cwd);
    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => return (false, format!("could not launch {program}: {e}")),
    };
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), combined)
}

const DRIVER_TS: &str = r#"
import { save, load, loadMany, describe } from "./orders.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

// 1) A record with a Float field round-trips exactly (the v0.21 dependency).
const order = {
  id: "o-1",
  price: 19.99,
  qty: 2,
  status: { tag: "Shipped", tracking: "t-9" },
  tags: ["a", "b"],
} as Parameters<typeof save>[0];
const wire = save(order);
const back = load(wire);
assert(back.tag === "Ok", "round-trip decodes");
if (back.tag === "Ok") {
  assert(back.value.price === 19.99, "price survives exactly");
  assert(back.value.qty === 2, "qty survives");
  assert(back.value.tags.length === 2, "tags survive");
}

// 2) Malformed input -> a Malformed JsonError.
const malformed = load("{");
assert(malformed.tag === "Err", "malformed input errs");
if (malformed.tag === "Err") {
  assert(malformed.error.kind === "Malformed", "kind is Malformed");
  assert(malformed.error.path === "$", "path is the root");
}

// 3) A shape mismatch carries the tracked field path.
const missing = load('{"id":"o-1","price":1.5,"qty":2,"status":{"kind":"Pending"},"tags":[]}');
assert(missing.tag === "Ok", "well-shaped decodes");
const wrong = load('{"id":42,"price":1.5,"qty":2,"status":{"kind":"Pending"},"tags":[]}');
assert(wrong.tag === "Err", "wrong field type errs");
if (wrong.tag === "Err") {
  assert(wrong.error.kind === "StructuralMismatch", "kind is StructuralMismatch");
  assert(wrong.error.path === "$.id", `path tracks the field, got ${wrong.error.path}`);
}

// 4) ADR 0049: a fractional number into a bare-Int field is rejected.
const fractional = load('{"id":"o-1","price":1.5,"qty":2.5,"status":{"kind":"Pending"},"tags":[]}');
assert(fractional.tag === "Err", "fractional Int errs");
if (fractional.tag === "Err") {
  assert(fractional.error.path === "$.qty", `path is $.qty, got ${fractional.error.path}`);
  assert(fractional.error.message.includes("integer"), "message names the integer expectation");
}

// 5) decode[List[Order]] — a generic-instantiation target.
const many = loadMany('[{"id":"a","price":1.0,"qty":1,"status":{"kind":"Pending"},"tags":[]}]');
assert(many.tag === "Ok" && many.value.length === 1, "List[Order] decodes");

// 6) The Bynk-side JsonError fields compose (describe() formats kind/path/message).
const description = describe("{");
assert(description.startsWith("Malformed at $"), `describe formats the error, got: ${description}`);

console.log("ALL OK");
"#;

const TSCONFIG_JSON: &str = r#"{
  "compilerOptions": {
    "module": "Node16",
    "moduleResolution": "node16",
    "target": "ES2022",
    "strict": true,
    "skipLibCheck": true,
    "outDir": "js",
    "rootDir": ".",
    "lib": ["ES2022", "DOM"]
  },
  "include": ["*.ts"]
}
"#;

#[test]
fn json_codec_behaviour() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! JSON-CODEC VERIFICATION SKIPPED !!!\nneither `tsc` nor `npx` is on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! JSON-CODEC VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/212_json_codec/input.bynk");
    let source = fs::read_to_string(&fixture).unwrap();
    let ts = bynkc::compile(&source, "212_json_codec/input.bynk")
        .expect("the json-codec fixture must compile");

    let tmp = std::env::temp_dir().join(format!("bynk-json-codec-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    fs::write(tmp.join("orders.ts"), ts).unwrap();
    bynk_testkit::write_runtime_modules(&tmp).unwrap();
    fs::write(tmp.join("driver.ts"), DRIVER_TS).unwrap();
    fs::write(tmp.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
    fs::write(tmp.join("package.json"), "{ \"type\": \"module\" }").unwrap();

    let (program, prefix) = &runner;
    let (ok, out_text) = run(program, prefix, &["-p", "tsconfig.json"], &tmp);
    assert!(ok, "tsc failed on the json-codec driver:\n{out_text}");

    let (ok, out_text) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ok && out_text.contains("ALL OK"),
        "json-codec driver did not pass:\n{out_text}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
