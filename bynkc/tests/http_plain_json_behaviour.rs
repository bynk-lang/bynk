//! #2858: behavioural proof of the bytes a plain-JSON route writes. Compiles the
//! `2858_plain_json_options` fixture (workers), then a Node driver sends real
//! `Request`s to the emitted Workers `fetch` and asserts the raw response text:
//! `Some(v)` is `v` and `None` is `null` at every depth on a plain route, while
//! a tagged route, an unmarked service and a `@json(options: tagged)` override
//! write exactly the tagged form. (The lenient decoder accepts both forms, so
//! the system-tier suite in the fixture cannot tell them apart; this can.)
//!
//! Like the tsc-verification stage, this skips loudly when no TypeScript
//! toolchain is available; `BYNK_REQUIRE_TSC=1` turns the skip into a failure.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

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
    let finder = if cfg!(windows) { "where" } else { "which" };
    base_command(finder)
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
import worker from "./workers/api/index.js";

function assert(cond: boolean, msg: string, got?: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}${got === undefined ? "" : `: ${got}`}`);
  }
}

const env = {} as never;

async function body(path: string): Promise<string> {
  const r = await worker.fetch(new Request(`https://x.test${path}`), env);
  assert(r.status === 200, `${path} is 200, got ${r.status}`);
  return r.text();
}

async function main(): Promise<void> {
  const plain = await body("/plain");
  assert(
    plain ===
      '{"id":"a1","action":"opened","size":null,"tags":["x",null],' +
        '"scores":[["k",1],["z",null]],"detail":{"note":null,"count":2},' +
        '"event":{"kind":"Opened","actor":"bob"}}',
    "a plain route writes v / null at every depth",
    plain,
  );
  assert((await body("/top")) === "null", "a top-level None is null");
  assert(
    (await body("/tagged")) === '{"kind":"Some","value":"t"}',
    "@json(options: tagged) overrides the service's plain mode",
  );
  assert(
    (await body("/legacy")) === '{"kind":"Some","value":"t"}',
    "an unmarked service is byte-identical to today",
  );
  assert(
    (await body("/legacy/plain")) === '"p"',
    "@json(options: plain) opts one route in",
  );
  // The mode is restored after a plain response: a tagged route right after.
  assert(
    (await body("/legacy")) === '{"kind":"Some","value":"t"}',
    "the plain mode does not leak into the next response",
  );
  console.log("ALL OK");
}

main().catch((e: unknown) => {
  console.error(e);
  throw e;
});
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
  "include": ["**/*.ts"],
  "exclude": ["js"]
}
"#;

#[test]
fn plain_json_routes_write_v_and_null() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! HTTP PLAIN JSON BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!(
            "\n!!! HTTP PLAIN JSON BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n"
        );
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the plain-JSON fixture (workers) in-process.
    let fixture: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/2858_plain_json_options/src");
    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_single(fixture).target(bynkc::BuildTarget::Workers),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .expect("the plain-JSON fixture must compile");

    let tmp = std::env::temp_dir().join(format!(
        "bynk-http-plain-json-behaviour-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    for (path, doc) in &out.artefacts.docs {
        if path.to_string_lossy() == "tsconfig.json" {
            continue;
        }
        let target_path = tmp.join(path);
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&target_path, doc.text()).unwrap();
    }
    fs::write(tmp.join("driver.ts"), DRIVER_TS).unwrap();
    fs::write(tmp.join("tsconfig.json"), TSCONFIG_JSON).unwrap();

    // Type-check + compile the whole tree, then run the driver under Node.
    let (ok, log) = run(&runner.0, &runner.1, &["--project", "tsconfig.json"], &tmp);
    assert!(
        ok,
        "the plain-JSON driver must type-check + compile:\n{log}"
    );
    let (ran, log) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ran && log.contains("ALL OK"),
        "the plain-JSON driver must run green:\n{log}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
