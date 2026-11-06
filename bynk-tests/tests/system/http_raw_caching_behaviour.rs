//! Caching raw responses: behavioural proof that a `from http` service's raw
//! `GET` responses are revalidatable when *run* — dispatched through the emitted
//! Workers `fetch` in-process. Compiles the `2335_http_raw_caching` fixture,
//! then a Node driver drives real `Request`s and asserts the wire behaviour:
//!   - a `Raw` `GET` of at most 1 MiB carries a weak `ETag`, and a matching
//!     `If-None-Match` gets `304` carrying the `ETag` and `Cache-Control`;
//!   - a `Raw` body over 1 MiB carries no `ETag` and is never `304`;
//!   - a `RawTagged` `GET` sends the handler's tag and answers `304` to it;
//!   - `@cache(immutable: true)` appends `immutable` to `Cache-Control`;
//!   - a `HEAD` carries the same `ETag` as its `GET`;
//!   - a malformed handler-supplied tag is a `500`, logged with its route.
//!
//! The exact 1 MiB boundary is pinned in the runtime's own unit tests
//! (`bynk-emit/runtime/test/http.test.ts`). Like the tsc-verification stage, this
//! skips loudly when no TypeScript toolchain is available; `BYNK_REQUIRE_TSC=1`
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
import worker from "./workers/files/index.js";

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
  // --- A small Raw GET: the automatic weak ETag + @cache's Cache-Control. ---
  let r = await worker.fetch(req("GET", "/index/money"), env);
  assert(r.status === 200, "GET /index/money is 200");
  const indexEtag = r.headers.get("etag");
  assert(indexEtag !== null && indexEtag.startsWith('W/"'), "a small Raw GET carries a weak ETag");
  assert(r.headers.get("cache-control") === "public, max-age=60", "the Raw GET carries @cache's Cache-Control");
  assert(r.headers.get("content-type") === "application/x-ndjson", "the declared content-type");
  assert((await r.text()) === '{"name":"money"}\n', "the raw body");

  // The validator is over the bytes: a different body gets a different tag.
  r = await worker.fetch(req("GET", "/index/other"), env);
  assert(r.headers.get("etag") !== indexEtag, "a different body gets a different ETag");

  // --- A matching If-None-Match → 304 with the ETag and Cache-Control. ---
  r = await worker.fetch(req("GET", "/index/money", { "if-none-match": indexEtag! }), env);
  assert(r.status === 304, "a matching If-None-Match on a Raw GET is 304");
  assert((await r.text()) === "", "the 304 has an empty body");
  assert(r.headers.get("etag") === indexEtag, "the 304 echoes the ETag");
  assert(r.headers.get("cache-control") === "public, max-age=60", "the 304 keeps Cache-Control");

  // --- HEAD carries the same ETag as its GET. ---
  r = await worker.fetch(req("HEAD", "/index/money"), env);
  assert(r.status === 200, "HEAD /index/money is 200");
  assert(r.headers.get("etag") === indexEtag, "HEAD carries the GET's ETag");

  // --- A body over 1 MiB: no automatic ETag, never 304. ---
  r = await worker.fetch(req("GET", "/big"), env);
  assert(r.status === 200, "GET /big is 200");
  assert(r.headers.get("etag") === null, "a Raw body over 1 MiB carries no ETag");
  assert((await r.arrayBuffer()).byteLength === 2 * 1024 * 1024, "the 2 MiB body arrives whole");
  r = await worker.fetch(req("GET", "/big", { "if-none-match": "*" }), env);
  assert(r.status === 200, "an untagged large body is never 304");

  // --- RawTagged: the handler's strong tag, immutable, and its 304. ---
  r = await worker.fetch(req("GET", "/dl/9f00"), env);
  assert(r.status === 200, "GET /dl/9f00 is 200");
  assert(r.headers.get("etag") === '"sha256-9f00"', "RawTagged sends the supplied ETag");
  assert(
    r.headers.get("cache-control") === "public, max-age=31536000, immutable",
    "@cache(immutable: true) appends immutable",
  );
  assert((await r.text()) === "archive 9f00", "the RawTagged body");
  r = await worker.fetch(req("GET", "/dl/9f00", { "if-none-match": '"sha256-9f00"' }), env);
  assert(r.status === 304, "a matching If-None-Match on RawTagged is 304");
  assert(r.headers.get("etag") === '"sha256-9f00"', "the RawTagged 304 echoes the ETag");
  assert(
    r.headers.get("cache-control") === "public, max-age=31536000, immutable",
    "the RawTagged 304 keeps the immutable Cache-Control",
  );
  r = await worker.fetch(req("HEAD", "/dl/9f00"), env);
  assert(r.headers.get("etag") === '"sha256-9f00"', "HEAD carries the RawTagged ETag");

  // --- A malformed handler-supplied tag is a 500, logged with its route. ---
  const logged: unknown[][] = [];
  const realError = console.error;
  console.error = (...args: unknown[]) => {
    logged.push(args);
  };
  try {
    r = await worker.fetch(req("GET", "/bad-tag"), env);
  } finally {
    console.error = realError;
  }
  assert(r.status === 500, "a malformed RawTagged tag is a 500");
  assert(r.headers.get("etag") === null, "the 500 carries no ETag");
  const line = logged.map((a) => a.map(String).join(" ")).join("\n");
  assert(line.includes("files GET /bad-tag faulted"), `the fault names the route: ${line}`);
  assert(line.includes("is not an entity tag"), `the fault says why: ${line}`);

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
fn http_raw_caching_contract_runs_on_workers_entry() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! HTTP RAW CACHING BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!(
            "\n!!! HTTP RAW CACHING BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n"
        );
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the caching fixture (workers) in-process.
    let fixture: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/2335_http_raw_caching/src");
    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_single(fixture).target(bynkc::BuildTarget::Workers),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .expect("the raw caching fixture must compile");

    let tmp = std::env::temp_dir().join(format!(
        "bynk-http-raw-caching-behaviour-{}",
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
