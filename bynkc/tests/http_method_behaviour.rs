//! v0.139 (ADR 0162): behavioural proof that a `from http` service answers the
//! synthesised method contract when *run* — dispatched through the emitted
//! Workers `fetch` in-process (there is no separate bundle-mode router, ADR 0159
//! D9). Compiles the `297_http_method_semantics` fixture, then a Node driver
//! drives real `Request`s and asserts:
//!   - a wrong method to a live path is `405` + the derived `Allow`;
//!   - a bare `OPTIONS` is `204` + `Allow`;
//!   - a `HEAD` returns the `GET` status and headers with an empty body, across
//!     the `Ok`, `Streaming` and `RawStreamed` variant families (a
//!     `RawStreamed` `GET` also writes each chunk, with no `ETag`);
//!   - an unknown path is still `404`.
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

function req(method: string, path: string, init?: RequestInit): Request {
  return new Request(`https://x.test${path}`, { method, ...init });
}

async function main(): Promise<void> {
  // --- GET: the ordinary success path is untouched. ---
  let r = await worker.fetch(req("GET", "/notes"), env);
  assert(r.status === 200, "GET /notes is 200");
  assert((await r.json()) === "all notes", "GET /notes carries the body");

  // --- HEAD from GET (Ok family): same status + headers, empty body. ---
  r = await worker.fetch(req("HEAD", "/notes"), env);
  assert(r.status === 200, "HEAD /notes mirrors the GET status");
  assert(r.headers.get("content-type") === "application/json", "HEAD keeps the GET content-type");
  assert((await r.text()) === "", "HEAD /notes has an empty body");

  // --- HEAD from GET on a param route. ---
  r = await worker.fetch(req("HEAD", "/notes/abc"), env);
  assert(r.status === 200, "HEAD /notes/:id is 200");
  assert((await r.text()) === "", "HEAD /notes/:id has an empty body");

  // --- HEAD from a Streaming GET: stream headers, empty body, not drained. ---
  r = await worker.fetch(req("HEAD", "/ticks"), env);
  assert(r.status === 200, "HEAD /ticks is 200");
  assert(r.headers.get("content-type") === "text/event-stream", "HEAD /ticks keeps the SSE content-type");
  assert((await r.text()) === "", "HEAD /ticks has an empty body");
  // The real GET still streams every event (the lazy body is consumed here).
  r = await worker.fetch(req("GET", "/ticks"), env);
  assert((await r.text()) === "data: tick-1\n\ndata: tick-2\n\n", "GET /ticks streams both events");

  // --- HEAD from a Raw GET: the author content-type, empty body. ---
  r = await worker.fetch(req("HEAD", "/logo"), env);
  assert(r.status === 200, "HEAD /logo is 200");
  assert(r.headers.get("content-type") === "image/svg+xml", "HEAD /logo keeps the Raw content-type");
  assert((await r.text()) === "", "HEAD /logo has an empty body");

  // --- K-streams (#2623): a RawStreamed GET writes every chunk, with no ETag
  // and no content-length; its HEAD keeps the content-type and an empty body. ---
  r = await worker.fetch(req("GET", "/chunks"), env);
  assert(r.status === 200, "GET /chunks is 200");
  assert(r.headers.get("content-type") === "text/csv", "GET /chunks keeps the author content-type");
  assert(r.headers.get("etag") === null, "a streamed body carries no ETag");
  assert(r.headers.get("content-length") === null, "a streamed body has no content-length");
  const reader = r.body!.getReader();
  const seen: string[] = [];
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    seen.push(new TextDecoder().decode(value));
  }
  assert(seen.join("") === "one,two,three", "GET /chunks streams every chunk in order");
  assert(seen.length === 3, `the empty chunk is skipped, the rest arrive apart (${seen.length})`);
  r = await worker.fetch(req("HEAD", "/chunks"), env);
  assert(r.status === 200, "HEAD /chunks is 200");
  assert(r.headers.get("content-type") === "text/csv", "HEAD /chunks keeps the content-type");
  assert((await r.text()) === "", "HEAD /chunks has an empty body");

  // --- 405 + Allow: a wrong method to a live path, Allow = the derived union. ---
  r = await worker.fetch(req("DELETE", "/notes"), env);
  assert(r.status === 405, "DELETE /notes is 405");
  assert(r.headers.get("allow") === "GET, HEAD, OPTIONS, POST", "405 Allow is the GET+POST union");
  assert((await r.text()) === "", "the synthesised 405 is bodyless");

  r = await worker.fetch(req("PUT", "/notes/abc"), env);
  assert(r.status === 405, "PUT /notes/:id is 405");
  assert(r.headers.get("allow") === "GET, HEAD, OPTIONS", "405 Allow for the GET-only path");

  // --- Plain OPTIONS: 204 + Allow (no CORS on this service). ---
  r = await worker.fetch(req("OPTIONS", "/notes"), env);
  assert(r.status === 204, "OPTIONS /notes is 204");
  assert(r.headers.get("allow") === "GET, HEAD, OPTIONS, POST", "OPTIONS Allow is the union");

  // --- POST still works (the fall-through never shadows a real dispatch). ---
  r = await worker.fetch(
    req("POST", "/notes", { body: JSON.stringify("hi"), headers: { "content-type": "application/json" } }),
    env,
  );
  assert(r.status === 201, "POST /notes is 201");
  assert((await r.json()) === "hi", "POST /notes echoes the body");

  // --- Unknown path is still 404 (not 405). ---
  r = await worker.fetch(req("GET", "/nope"), env);
  assert(r.status === 404, "an unknown path is still 404");

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
fn http_method_contract_runs_on_workers_entry() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! HTTP METHOD BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! HTTP METHOD BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the method-semantics fixture (workers) in-process.
    let fixture: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/297_http_method_semantics/src");
    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_single(fixture).target(bynkc::BuildTarget::Workers),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .expect("the method-semantics fixture must compile");

    let tmp =
        std::env::temp_dir().join(format!("bynk-http-method-behaviour-{}", std::process::id()));
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
        "the method-semantics driver must type-check + compile:\n{log}"
    );
    let (ran, log) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ran && log.contains("ALL OK"),
        "the method-semantics driver must run green:\n{log}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
