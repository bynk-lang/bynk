//! #3915: behavioural proof of the response-header methods: `withHeader` appends a header
//! line (repeats keep every value), `retryAfter` writes whole seconds, `at` writes `Location`,
//! `download` writes an RFC 6266 `Content-Disposition`, and a computed router-owned name
//! faults the request instead of reaching the wire. The fixture is compiled for Workers and driven through
//! the emitted `fetch` in-process under Node, like the other `http_*_behaviour` tests.
//! Like them it skips loudly without a TypeScript toolchain; `BYNK_REQUIRE_TSC=1`
//! turns the skip into a failure.

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

const env = {} as never;
const call = (path: string, method = "GET", body?: string) =>
  worker.fetch(
    new Request(`https://x.test${path}`, body === undefined ? { method } : { method, body: JSON.stringify(body), headers: { "content-type": "application/json" } }),
    env,
  );

async function main(): Promise<void> {
  let r = await call("/custom");
  assert(r.status === 200, `/custom is 200 (got ${r.status})`);
  assert(r.headers.get("x-request-id") === "abc", "withHeader sets X-Request-Id");
  assert(r.headers.get("x-trace") === "t1, t2", `repeated withHeader keeps both values (got ${r.headers.get("x-trace")})`);

  r = await call("/limited");
  assert(r.status === 429, `/limited is 429 (got ${r.status})`);
  assert(r.headers.get("retry-after") === "30", `Retry-After is 30 (got ${r.headers.get("retry-after")})`);

  r = await call("/busy");
  assert(r.status === 503, `/busy is 503 (got ${r.status})`);
  assert(r.headers.get("retry-after") === "90", `Retry-After is 90 (got ${r.headers.get("retry-after")})`);
  assert(r.headers.get("x-reason") === "deploy", "withHeader composes with retryAfter");

  r = await call("/things", "POST", "x");
  assert(r.status === 201, `/things is 201 (got ${r.status})`);
  assert(r.headers.get("location") === "/things/1", `Location is set (got ${r.headers.get("location")})`);

  r = await call("/export");
  assert(r.status === 200, `/export is 200 (got ${r.status})`);
  const cd = r.headers.get("content-disposition") ?? "";
  assert(cd.startsWith('attachment; filename="rep_rt 1.csv"'), `ASCII fallback (got ${cd})`);
  assert(cd.includes("filename*=UTF-8''rep%C3%B6rt%201.csv"), `RFC 8187 name (got ${cd})`);

  r = await call("/bad-name");
  assert(r.status === 500, `a computed router-owned name faults the request (got ${r.status})`);
  assert(r.headers.get("content-type") !== "text/evil", "the forbidden header never reaches the wire");

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
fn http_response_headers_run_on_workers_entry() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! HTTP RESPONSE-HEADERS BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!(
            "\n!!! HTTP RESPONSE-HEADERS BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n"
        );
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the response-headers fixture (workers) in-process.
    let fixture: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/3915_http_response_headers/src");
    let out = bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(fixture)
            .target(bynk_driver::project::BuildTarget::Workers),
    )
    .map_err(bynk_driver::project::ProjectFailure::flatten)
    .expect("the response-headers fixture must compile");

    let tmp =
        std::env::temp_dir().join(format!("bynk-http-response-headers-{}", std::process::id()));
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
        "the response-headers driver must type-check + compile:\n{log}"
    );
    let (ran, log) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ran && log.contains("ALL OK"),
        "the response-headers driver must run green:\n{log}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
