//! #1887: behavioural proof that an HTTP request body decodes an `Option`
//! field leniently. Compiles the `1887_lenient_option_http_body` fixture
//! (workers), then a Node driver posts real `Request`s to the emitted Workers
//! `fetch` and asserts that the field may be absent, `null`, a bare value or
//! Bynk's tagged form, and that a bare value of the wrong type is still a
//! `400` naming every form the decoder accepts.
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

async function post(body: string): Promise<Response> {
  const req = new Request("https://x.test/repos", {
    method: "POST",
    body,
    headers: { "content-type": "application/json" },
  });
  return worker.fetch(req, env);
}

async function expectCreated(body: string, want: string): Promise<void> {
  const r = await post(body);
  const got = await r.text();
  assert(r.status === 201, `${body} is 201, got ${r.status}: ${got}`);
  assert(JSON.parse(got) === want, `${body} decodes to ${want}, got ${got}`);
}

async function main(): Promise<void> {
  await expectCreated(`{"repo": "a"}`, "<none>");
  await expectCreated(`{"repo": "a", "description": null}`, "<none>");
  await expectCreated(`{"repo": "a", "description": "d"}`, "d");
  await expectCreated(`{"repo": "a", "description": {"kind": "Some", "value": "d"}}`, "d");
  await expectCreated(`{"repo": "a", "description": {"kind": "None"}}`, "<none>");

  const r = await post(`{"repo": "a", "description": 7}`);
  const text = await r.text();
  assert(r.status === 400, `a bare number is 400, got ${r.status}: ${text}`);
  const err = JSON.parse(text) as { path?: string; expected?: string };
  assert(err.path === "$.description", `the error path is $.description: ${text}`);
  assert(
    err.expected === 'string | null | {"kind": "None"} | {"kind": "Some", "value": ...}',
    `the error names every Option form: ${text}`,
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
fn http_body_option_field_decodes_leniently() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! HTTP OPTION BODY BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!(
            "\n!!! HTTP OPTION BODY BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n"
        );
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the lenient-Option body fixture (workers) in-process.
    let fixture: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/1887_lenient_option_http_body/src");
    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_single(fixture).target(bynkc::BuildTarget::Workers),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .expect("the lenient-Option body fixture must compile");

    let tmp = std::env::temp_dir().join(format!(
        "bynk-http-option-body-behaviour-{}",
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
        "the lenient-Option body driver must type-check + compile:\n{log}"
    );
    let (ran, log) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ran && log.contains("ALL OK"),
        "the lenient-Option body driver must run green:\n{log}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
