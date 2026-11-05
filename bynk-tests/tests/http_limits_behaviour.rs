//! v0.142 (ADR 0165): behavioural proof that request body-size limits work when a
//! `from http` service is *run* — dispatched through the emitted Workers `fetch`
//! in-process (there is no separate bundle-mode router, ADR 0159 D9). Compiles
//! the `300_http_limits` fixture, then a Node driver drives real `Request`s and
//! asserts the wire behaviour:
//!   - a `POST` whose `Content-Length` exceeds the effective cap is rejected with
//!     a `413` `{ kind: "PayloadTooLarge" }` and the body is *not* processed (an
//!     oversized malformed body still yields `413`, never `400`);
//!   - a `POST` within the cap succeeds normally;
//!   - the `@limit` route uses its own (larger) cap, not the service default — a
//!     body the service default would reject is accepted there;
//!   - a cross-origin oversized `POST`'s `413` still carries
//!     `Access-Control-Allow-Origin` (CORS-stamped so the browser can read it);
//!   - the capless `GET` is unaffected.
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
const ORIGIN = "https://app.example.com";

// The service default cap is 1 MiB (1_048_576); the `@limit` route on `/bulk`
// overrides it to 25 MiB (26_214_400). A declared Content-Length of 2 MiB sits
// between the two: the service default rejects it, the override accepts it. The
// guard reads the *declared* `Content-Length` (exactly what a client sends), so
// the test sets that header explicitly rather than allocating a real 2 MiB body.
const OVER_DEFAULT_CL = "2000000"; // 2_000_000: > 1 MiB, < 25 MiB.
const WITHIN = "hello";

// A POST with an explicit `content-length` header and a JSON string body. The
// declared length drives the ceiling guard; the body drives the handler.
function post(
  path: string,
  bodyStr: string,
  contentLength: string,
  headers?: Record<string, string>,
): Request {
  return new Request(`https://x.test${path}`, {
    method: "POST",
    body: JSON.stringify(bodyStr),
    headers: {
      "content-type": "application/json",
      "content-length": contentLength,
      ...(headers ?? {}),
    },
  });
}

async function main(): Promise<void> {
  // --- A POST within the service default cap succeeds normally. ---
  let r = await worker.fetch(post("/upload", WITHIN, "100"), env);
  assert(r.status === 201, `POST /upload within the cap is 201 (got ${r.status})`);
  assert((await r.json()) === WITHIN, "POST /upload echoes the accepted body");

  // --- A POST whose Content-Length exceeds the service default cap is 413. ---
  const big = await worker.fetch(post("/upload", WITHIN, OVER_DEFAULT_CL), env);
  assert(big.status === 413, `an oversized POST /upload is 413 (got ${big.status})`);
  const payload = (await big.json()) as { kind?: string };
  assert(payload.kind === "PayloadTooLarge", "the 413 body is `{ kind: PayloadTooLarge }`");

  // --- The body is NOT processed: an oversized *malformed* body still 413s.
  //     Were the guard not ahead of the read, request.json() would 400. ---
  const malformed = new Request("https://x.test/upload", {
    method: "POST",
    body: "not json", // would 400 if the handler ever read it.
    headers: { "content-type": "application/json", "content-length": OVER_DEFAULT_CL },
  });
  r = await worker.fetch(malformed, env);
  assert(
    r.status === 413,
    `an oversized malformed body is 413, not 400 — the body is never read (got ${r.status})`,
  );

  // --- The @limit route uses its own (larger) cap, not the service default:
  //     the same declared length the default rejects is accepted on /bulk. ---
  r = await worker.fetch(post("/bulk", WITHIN, OVER_DEFAULT_CL), env);
  assert(
    r.status === 201,
    `POST /bulk accepts a length over the service default via its @limit override (got ${r.status})`,
  );

  // --- A cross-origin oversized 413 still carries Access-Control-Allow-Origin. ---
  r = await worker.fetch(post("/upload", WITHIN, OVER_DEFAULT_CL, { origin: ORIGIN }), env);
  assert(r.status === 413, "the cross-origin oversized POST is 413");
  assert(
    r.headers.get("access-control-allow-origin") === ORIGIN,
    "the 413 is CORS-stamped so the browser can read it",
  );

  // --- The capless GET is unaffected (no Content-Length guard at all). ---
  r = await worker.fetch(new Request("https://x.test/status", { method: "GET" }), env);
  assert(r.status === 200, `GET /status is 200 (got ${r.status})`);
  assert((await r.json()) === "ok", "GET /status carries its body");

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
fn http_limits_contract_runs_on_workers_entry() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! HTTP LIMITS BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! HTTP LIMITS BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the limits fixture (workers) in-process.
    let fixture: PathBuf =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive/300_http_limits/src");
    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_single(fixture).target(bynkc::BuildTarget::Workers),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .expect("the limits fixture must compile");

    let tmp =
        std::env::temp_dir().join(format!("bynk-http-limits-behaviour-{}", std::process::id()));
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
    assert!(ok, "the limits driver must type-check + compile:\n{log}");
    let (ran, log) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ran && log.contains("ALL OK"),
        "the limits driver must run green:\n{log}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
