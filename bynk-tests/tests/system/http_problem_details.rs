//! #3916: behavioural proof of `errors { format: problem }`: an error variant answers
//! `application/problem+json` with `type: about:blank`, a router rejection answers with a
//! `https://bynk-lang.org/problems/<kind>` type and its `kind` and `path`, `@errors(format: bynk)`
//! overrides the service, `RawError` and a body-less `404` are untouched, and a service with no
//! `errors { }` answers Bynk's own bodies. The fixture is compiled for Workers and driven through
//! the emitted `fetch` in-process under Node, like the other `http_*_behaviour` tests. Like them it
//! skips loudly without a TypeScript toolchain; `BYNK_REQUIRE_TSC=1` turns the skip into a failure.

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
const send = (path: string, method = "GET", body?: string) =>
  worker.fetch(
    new Request(`https://x.test${path}`, body === undefined ? { method } : { method, body, headers: { "content-type": "application/json" } }),
    env,
  );

async function main(): Promise<void> {
  // An error variant is a problem whose type is just its status.
  let r = await send("/conflict");
  assert(r.status === 409, `/conflict is 409 (got ${r.status})`);
  assert(r.headers.get("content-type") === "application/problem+json", `problem+json (got ${r.headers.get("content-type")})`);
  let b = (await r.json()) as Record<string, unknown>;
  assert(b.type === "about:blank", `type is about:blank (got ${b.type})`);
  assert(b.title === "Conflict", `title is Conflict (got ${b.title})`);
  assert(b.status === 409, "status is 409");
  assert(b.detail === "widget is out of stock", `detail is the message (got ${b.detail})`);
  assert(!("error" in b), "no Bynk `error` member");

  // A write route's error variant is a problem too, not only a read route's.
  r = await send("/reserve", "POST", JSON.stringify({ qty: 1 }));
  assert(r.status === 409, `POST /reserve is 409 (got ${r.status})`);
  assert(r.headers.get("content-type") === "application/problem+json", `a write's variant is problem+json (got ${r.headers.get("content-type")})`);
  b = (await r.json()) as Record<string, unknown>;
  assert(b.type === "about:blank" && b.detail === "widget is out of stock", "a write's variant carries its detail");

  // A router rejection carries a stable type URI, its kind and path.
  r = await send("/widgets", "POST", JSON.stringify({ qty: 0 }));
  assert(r.status === 400, `refinement violation is 400 (got ${r.status})`);
  assert(r.headers.get("content-type") === "application/problem+json", "rejection is problem+json");
  b = (await r.json()) as Record<string, unknown>;
  assert(b.type === "https://bynk-lang.org/problems/refinement-violation", `type URI (got ${b.type})`);
  assert(b.title === "Bad Request" && b.status === 400, "title and status");
  assert(b.kind === "RefinementViolation", `kind (got ${b.kind})`);
  assert(typeof b.path === "string" && b.path.includes("qty"), `path names the field (got ${b.path})`);
  assert(typeof b.detail === "string" && b.detail.length > 0, "detail is the violation message");
  const violation = b.violation as { field?: unknown; message?: unknown } | undefined;
  assert(violation !== undefined && typeof violation.field === "string" && typeof violation.message === "string", `the violation (field and message) is carried (got ${JSON.stringify(b.violation)})`);
  assert("rule" in (b.violation as object), `the violated rule is carried (got ${JSON.stringify(b.violation)})`);
  assert(!("value" in (b.violation as object)), "the rejected value is never echoed");

  // A structural mismatch carries what it expected and what it got.
  r = await send("/widgets", "POST", JSON.stringify({ qty: "many" }));
  assert(r.status === 400, `structural mismatch is 400 (got ${r.status})`);
  b = (await r.json()) as Record<string, unknown>;
  assert(b.kind === "StructuralMismatch", `kind (got ${b.kind})`);
  assert(typeof b.expected === "string" && typeof b.actual === "string", "expected and actual are carried");

  r = await send("/widgets", "POST", "{not json");
  assert(r.status === 400, `malformed JSON is 400 (got ${r.status})`);
  b = (await r.json()) as Record<string, unknown>;
  assert(b.type === "https://bynk-lang.org/problems/malformed-json", `malformed-json type (got ${b.type})`);

  // A route's `@errors(format: bynk)` overrides the service.
  r = await send("/legacy");
  assert(r.headers.get("content-type") === "application/json", `/legacy is Bynk JSON (got ${r.headers.get("content-type")})`);
  b = (await r.json()) as Record<string, unknown>;
  assert(b.error === "widget is out of stock" && !("type" in b), "/legacy keeps `{ error }`");

  // `RawError` is the author's body; a body-less status stays body-less.
  r = await send("/teapot");
  assert(r.status === 409, "/teapot is 409");
  b = (await r.json()) as Record<string, unknown>;
  assert(b.error === "mine", "RawError body is untouched");
  r = await send("/missing");
  assert(r.status === 404 && (await r.text()) === "", "a body-less 404 stays body-less");

  // A service with no `errors { }` is unchanged.
  r = await send("/plain/conflict");
  assert(r.headers.get("content-type") === "application/json", "plain service is Bynk JSON");
  b = (await r.json()) as Record<string, unknown>;
  assert(b.error === "widget is out of stock", "plain service keeps `{ error }`");
  r = await send("/plain/widgets", "POST", JSON.stringify({ qty: 0 }));
  b = (await r.json()) as Record<string, unknown>;
  assert(b.kind === "RefinementViolation" && !("type" in b), "plain rejection keeps `{ kind, … }`");

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
fn http_problem_details_run_on_workers_entry() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! HTTP PROBLEM-DETAILS BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!(
            "\n!!! HTTP PROBLEM-DETAILS BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n"
        );
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the problem-details fixture (workers) in-process.
    let fixture: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/3916_http_problem_details/src");
    let out = bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(fixture)
            .target(bynk_driver::project::BuildTarget::Workers),
    )
    .map_err(bynk_driver::project::ProjectFailure::flatten)
    .expect("the problem-details fixture must compile");

    let tmp =
        std::env::temp_dir().join(format!("bynk-http-problem-details-{}", std::process::id()));
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
        "the problem-details driver must type-check + compile:\n{log}"
    );
    let (ran, log) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ran && log.contains("ALL OK"),
        "the problem-details driver must run green:\n{log}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
