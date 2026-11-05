//! v0.141 (ADR 0164): behavioural proof that security response headers work when
//! a `from http` service is *run* — dispatched through the emitted Workers `fetch`
//! in-process (there is no separate bundle-mode router, ADR 0159 D9). Compiles the
//! `299_http_security` fixture, then a Node driver drives real `Request`s and
//! asserts the wire behaviour:
//!   - a service with no `security { }` block still stamps `X-Content-Type-Options:
//!     nosniff` (the safe default) and no HSTS;
//!   - `security { hsts: 180.days }` stamps `Strict-Transport-Security:
//!     max-age=15552000` alongside `nosniff`;
//!   - `security { nosniff: false }` opts out — no security headers at all;
//!   - the synthesised `405`/`OPTIONS` and the `HEAD` answer carry the policy too;
//!   - a boundary-rejection `400` (a refined path param refused before the handler)
//!     carries the addressed service's policy as well — the #659 (v0.188) fix, and
//!     the opt-out service's rejection opts out too (the policy is the service's own).
//!
//! Like the caching harness, this skips loudly when no TypeScript toolchain is
//! available; `BYNK_REQUIRE_TSC=1` turns the skip into a failure.

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
import worker from "./workers/secured/index.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

const env = {} as never;

function req(method: string, path: string, headers?: Record<string, string>): Request {
  return new Request(`https://x.test${path}`, { method, headers });
}

async function main(): Promise<void> {
  // --- No `security { }` block: the safe default still stamps `nosniff`, no HSTS. ---
  let r = await worker.fetch(req("GET", "/health"), env);
  assert(r.status === 200, "GET /health is 200");
  assert(r.headers.get("x-content-type-options") === "nosniff", "the default stamps nosniff");
  assert(r.headers.get("strict-transport-security") === null, "the default sends no HSTS");

  // --- `security { hsts: 180.days }`: nosniff + Strict-Transport-Security. ---
  r = await worker.fetch(req("GET", "/products/42"), env);
  assert(r.status === 200, "GET /products/:id is 200");
  assert(r.headers.get("x-content-type-options") === "nosniff", "hsts service still stamps nosniff");
  assert(
    r.headers.get("strict-transport-security") === "max-age=15552000",
    "180.days lowers to max-age=15552000",
  );

  // --- `security { nosniff: false }`: opts out of every security header. ---
  r = await worker.fetch(req("GET", "/admin/stats"), env);
  assert(r.status === 200, "GET /admin/stats is 200");
  assert(r.headers.get("x-content-type-options") === null, "nosniff:false opts out of nosniff");
  assert(r.headers.get("strict-transport-security") === null, "the opt-out service sends no HSTS");

  // --- The synthesised `405` for a live path carries the policy (defence in depth). ---
  r = await worker.fetch(req("POST", "/health"), env);
  assert(r.status === 405, "POST to a GET-only path is 405");
  assert(r.headers.get("allow") === "GET, HEAD, OPTIONS", "the 405 carries the Allow header");
  assert(r.headers.get("x-content-type-options") === "nosniff", "the 405 is security-stamped");

  // --- #659 (v0.188): a boundary-rejection `400` carries the service policy too.
  // `/store/:code` is `ShortCode = String where MinLength(3)`, so a 2-char segment
  // is refused before the handler with a `RefinementViolation` — the response class
  // that reflects attacker input and used to ship WITHOUT `nosniff`. ---
  r = await worker.fetch(req("GET", "/store/xy"), env);
  assert(r.status === 400, "a too-short path param is a 400");
  const rejBody = await r.text();
  assert(rejBody.includes("RefinementViolation"), "the 400 body is the RefinementViolation");
  assert(rejBody.includes("xy"), "the 400 reflects the offending input (the vector #659 is about)");
  assert(
    r.headers.get("x-content-type-options") === "nosniff",
    "the reflected-input 400 is now nosniff-stamped (the #659 fix)",
  );
  assert(
    r.headers.get("strict-transport-security") === "max-age=15552000",
    "the rejection carries the store service's HSTS, exactly as its 200 does",
  );

  // --- The policy is the ADDRESSED service's, not a blanket header: `admin` opts
  // out of nosniff, and its rejection opts out too. Proves the fix stamps the
  // per-service policy rather than hardcoding a header on every rejection. ---
  r = await worker.fetch(req("GET", "/admin/item/xy"), env);
  assert(r.status === 400, "the opt-out service still rejects a bad param with a 400");
  assert(
    r.headers.get("x-content-type-options") === null,
    "the opt-out service's rejection has no nosniff — the policy is the service's own",
  );

  // --- A bare `OPTIONS` (204) is stamped too. ---
  r = await worker.fetch(req("OPTIONS", "/products/42"), env);
  assert(r.status === 204, "a bare OPTIONS is 204");
  assert(
    r.headers.get("strict-transport-security") === "max-age=15552000",
    "the 204 carries the service's HSTS",
  );

  // --- The `HEAD` answer keeps the security headers (headResponse copies them). ---
  r = await worker.fetch(req("HEAD", "/products/42"), env);
  assert(r.status === 200, "HEAD /products/:id is 200");
  assert(r.headers.get("x-content-type-options") === "nosniff", "the HEAD answer keeps nosniff");
  assert(
    r.headers.get("strict-transport-security") === "max-age=15552000",
    "the HEAD answer keeps HSTS",
  );
  assert((await r.text()) === "", "the HEAD body is empty");

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
fn http_security_contract_runs_on_workers_entry() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! HTTP SECURITY BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!(
            "\n!!! HTTP SECURITY BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n"
        );
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the security fixture (workers) in-process.
    let fixture: PathBuf =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive/299_http_security/src");
    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_single(fixture).target(bynkc::BuildTarget::Workers),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .expect("the security fixture must compile");

    let tmp = std::env::temp_dir().join(format!(
        "bynk-http-security-behaviour-{}",
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
    assert!(ok, "the security driver must type-check + compile:\n{log}");
    let (ran, log) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ran && log.contains("ALL OK"),
        "the security driver must run green:\n{log}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
