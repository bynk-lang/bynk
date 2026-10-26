//! Behavioural test for `Matches` refinement anchoring.
//!
//! Snapshots prove the emitted shape; this proves the behaviour: a pattern
//! with top-level alternation is anchored as a whole. The historical form
//! `new RegExp("^" + "ab|cd" + "$")` parses as `(^ab)|(cd$)`, so `"abZZZ"`
//! and `"ZZZcd"` passed validation — an input-validation bypass on refined
//! HTTP params and request bodies. The fix wraps the pattern in a
//! non-capturing group: `^(?:ab|cd)$`. Compiles the alternation fixture in
//! single-file mode and drives `Code.of` with `tsc` + `node`.
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
import { Code } from "./demo.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

// Each alternation branch is admitted whole.
assert(Code.of("ab").tag === "Ok", "\"ab\" matches ab|cd");
assert(Code.of("cd").tag === "Ok", "\"cd\" matches ab|cd");

// The anchors bind around the whole alternation, not per-branch. With the
// historical `^ab|cd$` form these two passed validation.
assert(Code.of("abZZZ").tag === "Err", "\"abZZZ\" must not match ^(?:ab|cd)$");
assert(Code.of("ZZZcd").tag === "Err", "\"ZZZcd\" must not match ^(?:ab|cd)$");

// And plain non-matches are still rejected.
assert(Code.of("").tag === "Err", "empty string is rejected");
assert(Code.of("ef").tag === "Err", "\"ef\" is rejected");

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
fn regex_refinement_alternation_is_anchored() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! REGEX-REFINEMENT VERIFICATION SKIPPED !!!\nneither `tsc` nor `npx` is on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! REGEX-REFINEMENT VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/301_refined_matches_alternation/input.bynk");
    let source = fs::read_to_string(&fixture).unwrap();
    let ts = bynkc::compile(&source, "301_refined_matches_alternation/input.bynk")
        .expect("the alternation fixture must compile");

    let tmp = std::env::temp_dir().join(format!("bynk-regex-refinement-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    fs::write(tmp.join("demo.ts"), ts).unwrap();
    bynk_testkit::write_runtime_modules(&tmp).unwrap();
    fs::write(tmp.join("driver.ts"), DRIVER_TS).unwrap();
    fs::write(tmp.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
    fs::write(tmp.join("package.json"), "{ \"type\": \"module\" }").unwrap();

    let (program, prefix) = &runner;
    let (ok, out_text) = run(program, prefix, &["-p", "tsconfig.json"], &tmp);
    assert!(ok, "tsc failed on the regex-refinement driver:\n{out_text}");

    let (ok, out_text) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ok && out_text.contains("ALL OK"),
        "regex-refinement driver did not pass:\n{out_text}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
