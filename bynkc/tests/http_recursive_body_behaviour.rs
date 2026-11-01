//! #2656: behavioural proof that an HTTP request body of a recursive type is
//! decoded with the nesting bound. Compiles the `2656_recursive_http_body`
//! fixture (workers), then a Node driver posts real `Request`s to the emitted
//! Workers `fetch` and asserts that a body at the bound is accepted, that one
//! level deeper is a `400` `StructuralMismatch` at the path where the bound is
//! crossed, and that a body far deeper than the host's stack allows (which
//! overflowed the decoder and was answered as a fault before #2656) is a `400`
//! too, not the fault response.
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

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

// A dependency-free `from http` service composes against an empty Env.
const env = {} as never;

const BOUND = __BOUND__;

// The JSON text of a chain `n` levels deep, built without recursion.
function chain(n: number): string {
  let s = `{"value": 1, "next": null}`;
  for (let i = 1; i < n; i++) s = `{"value": 1, "next": {"kind": "Some", "value": ${s}}}`;
  return s;
}

async function post(body: string): Promise<Response> {
  const req = new Request("https://x.test/chains", {
    method: "POST",
    body,
    headers: { "content-type": "application/json" },
  });
  return worker.fetch(req, env);
}

async function expectDepthError(levels: number): Promise<void> {
  const r = await post(chain(levels));
  const text = await r.text();
  assert(r.status === 400, `${levels} levels is 400, got ${r.status}: ${text.slice(0, 300)}`);
  const err = JSON.parse(text) as { kind?: string; path?: string; expected?: string };
  assert(err.kind === "StructuralMismatch", `${levels} levels is a StructuralMismatch: ${text.slice(0, 300)}`);
  assert(
    err.path === "$" + ".next.value".repeat(BOUND),
    `the error is at the level that crosses the bound: ${text.slice(0, 300)}`,
  );
  assert(
    err.expected === `a value nested at most ${BOUND} levels deep`,
    `the error names the bound: ${text.slice(0, 300)}`,
  );
}

async function main(): Promise<void> {
  const ok = await post(chain(BOUND));
  const got = await ok.text();
  assert(ok.status === 201, `a chain at the bound is 201, got ${ok.status}: ${got.slice(0, 300)}`);
  assert(JSON.parse(got) === BOUND, `the handler sees all ${BOUND} levels, got ${got}`);

  await expectDepthError(BOUND + 1);
  // Deep enough to overflow the decoder's stack before #2656.
  await expectDepthError(20000);

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
fn http_body_nested_past_the_bound_is_a_400() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! HTTP RECURSIVE BODY BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!(
            "\n!!! HTTP RECURSIVE BODY BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n"
        );
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the recursive-body fixture (workers) in-process.
    let fixture: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/2656_recursive_http_body/src");
    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_single(fixture).target(bynkc::BuildTarget::Workers),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .expect("the recursive-body fixture must compile");

    let tmp = std::env::temp_dir().join(format!(
        "bynk-http-recursive-body-behaviour-{}",
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
    let driver = DRIVER_TS.replace(
        "__BOUND__",
        &bynk_check::wire::MAX_RECURSIVE_NESTING.to_string(),
    );
    fs::write(tmp.join("driver.ts"), driver).unwrap();
    fs::write(tmp.join("tsconfig.json"), TSCONFIG_JSON).unwrap();

    // Type-check + compile the whole tree, then run the driver under Node.
    let (ok, log) = run(&runner.0, &runner.1, &["--project", "tsconfig.json"], &tmp);
    assert!(
        ok,
        "the recursive-body driver must type-check + compile:\n{log}"
    );
    let (ran, log) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ran && log.contains("ALL OK"),
        "the recursive-body driver must run green:\n{log}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
