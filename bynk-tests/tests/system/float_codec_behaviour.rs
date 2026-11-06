//! v0.21 behavioural test for the `Float` boundary codec (ADR 0040).
//!
//! Snapshots prove the emitted shape; this proves the behaviour: a decimal
//! round-trips, a non-finite number from the wire is rejected on decode
//! (`JSON.parse("1e999")` yields `Infinity`), and serialising a non-finite
//! `Float` throws instead of letting `JSON.stringify` silently produce
//! `null`. Compiles the workers Float-boundary fixture in-process, then
//! drives its `serialise_Quote`/`deserialise_Quote` with `tsc` + `node`.
//!
//! Like the tsc-verification stage, this skips loudly when no TypeScript
//! toolchain is available; `BYNK_REQUIRE_TSC=1` turns the skip into a
//! failure (CI).

use std::fs;
use std::path::{Path, PathBuf};
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
import { __serialise_Quote, __deserialise_Quote, type Quote } from "./workers/quote/handlers.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

// 1) A decimal round-trips through the codec exactly.
const q: Quote = { sku: "widget", price: 19.99, qty: 2 };
const wire = JSON.stringify(__serialise_Quote(q));
const back = __deserialise_Quote(JSON.parse(wire));
assert(back.tag === "Ok", "decimal round-trip deserialises");
if (back.tag === "Ok") {
  assert(back.value.price === 19.99, "price survives the round-trip exactly");
}

// 2) A non-finite number from the wire is rejected on decode.
//    JSON.parse admits Infinity via an overflowing literal.
const overflowed = JSON.parse('{"sku":"widget","price":1e999,"qty":2}');
assert(overflowed.price === Infinity, "JSON.parse yields Infinity for 1e999");
const rejected = __deserialise_Quote(overflowed);
assert(rejected.tag === "Err", "non-finite Float from the wire is rejected");

// 3) Serialising a non-finite Float is a contract violation (throws).
let threw = false;
try {
  __serialise_Quote({ sku: "widget", price: Number.NaN, qty: 2 } as Quote);
} catch {
  threw = true;
}
assert(threw, "serialising a NaN Float throws");

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
  "include": ["*.ts", "workers/**/*.ts"]
}
"#;

#[test]
fn float_boundary_codec_behaviour() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! FLOAT-CODEC VERIFICATION SKIPPED !!!\nneither `tsc` nor `npx` is on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! FLOAT-CODEC VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the workers Float-boundary fixture in-process.
    let fixture: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/207_workers_float_boundary/src");
    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_single(fixture)
            .target(bynkc::BuildTarget::Workers)
            .platform(bynkc::Platform::Cloudflare),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .expect("the Float-boundary fixture must compile");

    let tmp = std::env::temp_dir().join(format!("bynk-float-codec-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    // Only the handlers module (which holds the codec) and the runtime it
    // imports are needed; the workers entrypoint pulls in platform types.
    for (path, doc) in &out.artefacts.docs {
        let p = path.to_string_lossy();
        if p.starts_with("__bynk_runtime/") || p.ends_with("handlers.ts") {
            let target_path = tmp.join(path);
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&target_path, doc.text()).unwrap();
        }
    }
    // handlers.ts imports `../../__bynk_runtime/*.js`, which the layout above
    // (the runtime directory at the root, handlers under workers/quote/) satisfies.
    assert!(
        tmp.join("workers/quote/handlers.ts").exists(),
        "fixture layout changed — update this test's import path"
    );

    fs::write(tmp.join("driver.ts"), DRIVER_TS).unwrap();
    fs::write(tmp.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
    fs::write(tmp.join("package.json"), "{ \"type\": \"module\" }").unwrap();

    let (program, prefix) = &runner;
    let (ok, out_text) = run(program, prefix, &["-p", "tsconfig.json"], &tmp);
    assert!(ok, "tsc failed on the float-codec driver:\n{out_text}");

    let (ok, out_text) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ok && out_text.contains("ALL OK"),
        "float-codec driver did not pass:\n{out_text}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
