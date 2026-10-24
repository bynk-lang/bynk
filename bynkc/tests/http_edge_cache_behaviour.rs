//! Edge caching (first-party registry L7, #2414): behavioural proof that a
//! Worker whose context declares an `EdgeCache` instance answers as Decision C
//! says when *run*: dispatched through the emitted Workers `fetch` in-process.
//! Compiles the `2414_edge_cache` fixture, then a Node driver drives real
//! `Request`s with an `ExecutionContext` double and asserts:
//!   - a `@cache` route's `200` carries the route's `Cache-Control`, and its
//!     `304` and `HEAD` copy it;
//!   - a `404` from the same route, a `GET` route without `@cache`, a fault's
//!     `500`, and the router's own `404` and `405` all carry `no-store`;
//!   - another method's response is unchanged (no `Cache-Control`);
//!   - a purge reaches `ctx.cache.purge` with the path prefix, a refused one
//!     answers `false`, and with no `ctx` (an in-process caller) it is a
//!     logged no-op answering `true`.
//!
//! The non-opted-in Worker's output is unchanged, which the positive goldens
//! pin byte for byte. Like the tsc-verification stage, this skips loudly when
//! no TypeScript toolchain is available; `BYNK_REQUIRE_TSC=1` turns the skip
//! into a failure.

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
import worker from "./workers/registry-files/index.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

const env = {} as never;

function req(method: string, path: string, headers?: Record<string, string>): Request {
  return new Request(`https://x.test${path}`, { method, headers });
}

function makeCtx() {
  const purged: string[][] = [];
  const ctx = {
    succeeds: true,
    purged,
    waitUntil(_promise: Promise<unknown>): void {},
    cache: {
      async purge(options: { pathPrefixes: string[] }): Promise<{ success: boolean }> {
        purged.push([...options.pathPrefixes]);
        return { success: ctx.succeeds };
      },
    },
  };
  return ctx;
}

const IMMUTABLE = "public, max-age=31536000, immutable";

async function main(): Promise<void> {
  const ctx = makeCtx();

  // --- A @cache route's 200 keeps its Cache-Control; its 304 and HEAD copy it. ---
  let r = await worker.fetch(req("GET", "/dl/sha256/abc"), env, ctx);
  assert(r.status === 200, `the archive is a 200, got ${r.status}`);
  assert(r.headers.get("cache-control") === IMMUTABLE, "the 200 carries the route's Cache-Control");
  r = await worker.fetch(req("GET", "/dl/sha256/abc", { "if-none-match": '"sha256-abc"' }), env, ctx);
  assert(r.status === 304, `a matching tag is a 304, got ${r.status}`);
  assert(r.headers.get("cache-control") === IMMUTABLE, "the 304 copies the 200's Cache-Control");
  r = await worker.fetch(req("HEAD", "/dl/sha256/abc"), env, ctx);
  assert(r.headers.get("cache-control") === IMMUTABLE, "HEAD carries the GET's Cache-Control");

  // --- Everything else a GET or HEAD gets is no-store. ---
  r = await worker.fetch(req("GET", "/dl/sha256/missing"), env, ctx);
  assert(r.status === 404, `a missing archive is a 404, got ${r.status}`);
  assert(r.headers.get("cache-control") === "no-store", "the @cache route's 404 is no-store");
  r = await worker.fetch(req("HEAD", "/dl/sha256/missing"), env, ctx);
  assert(r.headers.get("cache-control") === "no-store", "the 404's HEAD is no-store");
  r = await worker.fetch(req("GET", "/status"), env, ctx);
  assert(r.status === 200, `status is a 200, got ${r.status}`);
  assert(r.headers.get("cache-control") === "no-store", "a GET without @cache is no-store");
  r = await worker.fetch(req("GET", "/nowhere"), env, ctx);
  assert(r.status === 404, `an unrouted path is a 404, got ${r.status}`);
  assert(r.headers.get("cache-control") === "no-store", "the router's 404 is no-store");
  const realError = console.error;
  console.error = () => {};
  try {
    r = await worker.fetch(req("GET", "/bad-tag"), env, ctx);
  } finally {
    console.error = realError;
  }
  assert(r.status === 500, `a fault is a 500, got ${r.status}`);
  assert(r.headers.get("cache-control") === "no-store", "a fault's 500 is no-store, even on a @cache route");
  r = await worker.fetch(req("GET", "/takedown/abc"), env, ctx);
  assert(r.status === 405, `a GET to a POST route is a 405, got ${r.status}`);
  assert(r.headers.get("cache-control") === "no-store", "the router's 405 is no-store");

  // --- Another method is unchanged; a takedown purges the archive's path. ---
  r = await worker.fetch(req("POST", "/takedown/abc"), env, ctx);
  assert(r.status === 200, `a takedown is a 200, got ${r.status}`);
  assert(r.headers.get("cache-control") === null, "a POST carries no Cache-Control");
  assert((await r.json()) === true, "an accepted purge answers true");
  assert(JSON.stringify(ctx.purged) === JSON.stringify([["/dl/sha256/abc"]]), `the purge names the path: ${JSON.stringify(ctx.purged)}`);

  // --- A refused purge answers false. ---
  ctx.succeeds = false;
  r = await worker.fetch(req("POST", "/takedown/def"), env, ctx);
  assert((await r.json()) === false, "a refused purge answers false");
  assert(ctx.purged.length === 2, "the refused purge was still asked for");

  // --- With no ctx there is no edge cache: a logged no-op answering true. ---
  const logged: unknown[][] = [];
  const realLog = console.log;
  console.log = (...args: unknown[]) => {
    logged.push(args);
  };
  try {
    r = await worker.fetch(req("POST", "/takedown/abc"), env);
  } finally {
    console.log = realLog;
  }
  assert((await r.json()) === true, "a purge with no ctx answers true");
  assert(
    logged.some((a) => String(a[0]) === "bynk: EdgeCache.purge is a no-op under bynk dev"),
    "a purge with no ctx says it did nothing",
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
fn edge_cache_contract_runs_on_workers_entry() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! EDGE CACHE BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! EDGE CACHE BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the fixture (workers) in-process, from its manifest, which
    // allows the experimental `EdgeCache`.
    let fixture: PathBuf =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive/2414_edge_cache");
    let paths = bynkc::try_read_project_paths(&fixture).expect("well-formed fixture manifest");
    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_split(fixture, paths).target(bynkc::BuildTarget::Workers),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .expect("the edge cache fixture must compile");

    let tmp = std::env::temp_dir().join(format!(
        "bynk-http-edge-cache-behaviour-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    for (path, doc) in &out.artefacts.docs {
        // Only the Worker tree and its shared modules: the suites' modules
        // are `bynk test`'s.
        if path.to_string_lossy() == "tsconfig.json" || path.starts_with("tests") {
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
        "the edge cache driver must type-check + compile:\n{log}"
    );
    let (ran, log) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ran && log.contains("ALL OK"),
        "the edge cache driver must run green:\n{log}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
