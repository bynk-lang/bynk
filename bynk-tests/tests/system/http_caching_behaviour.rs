//! v0.140 (ADR 0163): behavioural proof that conditional caching works when a
//! `from http` service is *run* — dispatched through the emitted Workers `fetch`
//! in-process (there is no separate bundle-mode router, ADR 0159 D9). Compiles
//! the `298_http_caching` fixture, then a Node driver drives real `Request`s and
//! asserts the wire behaviour:
//!   - an eligible `GET` (`Ok` body) carries a synthesised weak `ETag`;
//!   - a matching `If-None-Match` is answered `304` with an empty body and the
//!     same `ETag` + `Cache-Control`; a stale one gets the `200` + a new body;
//!   - `@cache` emits `Cache-Control` (`public`/`private` scopes); a `GET` without
//!     `@cache` carries an `ETag` but no `Cache-Control`;
//!   - a `Streaming` `GET` is excluded (no `ETag`, never `304`) yet still streams;
//!   - an unsafe method (`POST`) is unchanged (no `ETag`, no `Cache-Control`);
//!   - a cross-origin `304` still carries `Access-Control-Allow-Origin`.
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

function req(method: string, path: string, headers?: Record<string, string>): Request {
  return new Request(`https://x.test${path}`, { method, headers });
}

async function main(): Promise<void> {
  // --- An eligible GET (`Ok`) carries a weak ETag; `@cache` adds Cache-Control. ---
  let r = await worker.fetch(req("GET", "/config"), env);
  assert(r.status === 200, "GET /config is 200");
  const configEtag = r.headers.get("etag");
  assert(configEtag !== null && configEtag.startsWith('W/"'), "GET /config carries a weak ETag");
  assert(r.headers.get("cache-control") === "public, max-age=300", "GET /config @cache Cache-Control");
  assert((await r.json()) === "cfg", "GET /config carries the body");

  // The validator is stable — the same request yields the same ETag.
  r = await worker.fetch(req("GET", "/config"), env);
  assert(r.headers.get("etag") === configEtag, "the ETag is stable across identical requests");

  // --- A matching If-None-Match → 304, empty body, same ETag + Cache-Control. ---
  r = await worker.fetch(req("GET", "/config", { "if-none-match": configEtag! }), env);
  assert(r.status === 304, "matching If-None-Match yields 304");
  assert((await r.text()) === "", "the 304 has an empty body");
  assert(r.headers.get("etag") === configEtag, "the 304 echoes the ETag");
  assert(r.headers.get("cache-control") === "public, max-age=300", "the 304 keeps Cache-Control");

  // --- A stale If-None-Match → the full 200 + a fresh ETag. ---
  r = await worker.fetch(req("GET", "/config", { "if-none-match": 'W/"stale"' }), env);
  assert(r.status === 200, "a stale validator gets the 200");
  assert(r.headers.get("etag") === configEtag, "the 200 carries the current ETag");
  assert((await r.json()) === "cfg", "the 200 carries the body");

  // --- A GET without `@cache`: an ETag, but no Cache-Control. ---
  r = await worker.fetch(req("GET", "/plain"), env);
  assert(r.status === 200, "GET /plain is 200");
  const plainEtag = r.headers.get("etag");
  assert(plainEtag !== null && plainEtag.startsWith('W/"'), "GET /plain carries an ETag");
  assert(r.headers.get("cache-control") === null, "GET /plain emits no Cache-Control");
  // It still revalidates through its ETag alone.
  r = await worker.fetch(req("GET", "/plain", { "if-none-match": plainEtag! }), env);
  assert(r.status === 304, "GET /plain revalidates to 304 via its ETag");

  // --- The `private` scope. ---
  r = await worker.fetch(req("GET", "/private"), env);
  assert(r.headers.get("cache-control") === "private, max-age=30", "GET /private @cache scope");

  // --- A Streaming GET is excluded: no ETag, never 304, but still streams. ---
  r = await worker.fetch(req("GET", "/ticks"), env);
  assert(r.status === 200, "GET /ticks is 200");
  assert(r.headers.get("etag") === null, "a Streaming GET carries no ETag");
  assert((await r.text()) === "data: a\n\ndata: b\n\n", "GET /ticks streams both events");
  // Even with an If-None-Match, a no-ETag response is never revalidated.
  r = await worker.fetch(req("GET", "/ticks", { "if-none-match": "*" }), env);
  assert(r.status === 200, "a Streaming GET is never 304 even against `*`");

  // --- An unsafe method is byte-for-byte unchanged: no ETag, no Cache-Control. ---
  r = await worker.fetch(
    new Request("https://x.test/items", {
      method: "POST",
      body: JSON.stringify("thing"),
      headers: { "content-type": "application/json" },
    }),
    env,
  );
  assert(r.status === 201, "POST /items is 201");
  assert(r.headers.get("etag") === null, "POST carries no ETag");
  assert(r.headers.get("cache-control") === null, "POST carries no Cache-Control");

  // --- A HEAD revalidation is also 304 (empty body either way). ---
  r = await worker.fetch(req("HEAD", "/config", { "if-none-match": configEtag! }), env);
  assert(r.status === 304, "HEAD with a matching validator is 304");
  assert((await r.text()) === "", "the HEAD 304 has an empty body");

  // --- The cross-origin 304 still carries Access-Control-Allow-Origin. ---
  r = await worker.fetch(
    req("GET", "/config", { origin: ORIGIN, "if-none-match": configEtag! }),
    env,
  );
  assert(r.status === 304, "the cross-origin revalidation is 304");
  assert(
    r.headers.get("access-control-allow-origin") === ORIGIN,
    "the 304 is CORS-stamped so the browser can read it",
  );
  assert(r.headers.get("etag") === configEtag, "the cross-origin 304 keeps the ETag");

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
fn http_caching_contract_runs_on_workers_entry() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! HTTP CACHING BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!(
            "\n!!! HTTP CACHING BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n"
        );
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the caching fixture (workers) in-process.
    let fixture: PathBuf =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive/298_http_caching/src");
    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_single(fixture).target(bynkc::BuildTarget::Workers),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .expect("the caching fixture must compile");

    let tmp = std::env::temp_dir().join(format!(
        "bynk-http-caching-behaviour-{}",
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
    assert!(ok, "the caching driver must type-check + compile:\n{log}");
    let (ran, log) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ran && log.contains("ALL OK"),
        "the caching driver must run green:\n{log}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
