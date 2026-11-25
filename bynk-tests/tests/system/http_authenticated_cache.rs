//! #3922: behavioural proof that a route guarded by an actor that carries a credential
//! sends `Cache-Control: private` by default (on every response, a `404` and a `401`
//! included), that `@cache(noStore: true)` sends `no-store` with no `ETag`, and that a
//! `Visitor` route is unchanged. The fixture is compiled for Workers and driven through
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

const SECRET = "test-secret";
const env = { AUTH_JWT_SECRET: SECRET } as never;

function b64url(s: string): string {
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

async function token(): Promise<string> {
  const h = b64url(JSON.stringify({ alg: "HS256", typ: "JWT" }));
  const p = b64url(JSON.stringify({ sub: "user-1", exp: Math.floor(Date.now() / 1000) + 3600 }));
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey(
    "raw",
    enc.encode(SECRET) as BufferSource,
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const sig = new Uint8Array(await crypto.subtle.sign("HMAC", key, enc.encode(`${h}.${p}`) as BufferSource));
  let bin = "";
  for (const b of sig) bin += String.fromCharCode(b);
  return `${h}.${p}.${btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "")}`;
}

async function main(): Promise<void> {
  const auth = { authorization: `Bearer ${await token()}` };
  const get = (path: string, headers: Record<string, string> = auth) =>
    worker.fetch(new Request(`https://x.test${path}`, { headers }), env);

  // A credentialed route with no `@cache` is private, on a 200 and on a 404.
  let r = await get("/me");
  assert(r.status === 200, `GET /me is 200 (got ${r.status})`);
  assert(r.headers.get("cache-control") === "private", `/me is private (got ${r.headers.get("cache-control")})`);
  assert(r.headers.get("etag") !== null, "/me keeps its automatic ETag");
  r = await get("/missing");
  assert(r.status === 404, `GET /missing is 404 (got ${r.status})`);
  assert(r.headers.get("cache-control") === "private", `a 404 is private too (got ${r.headers.get("cache-control")})`);

  // So is a 401 from the actor's own check, and a write.
  r = await get("/me", {});
  assert(r.status === 401, `no token is 401 (got ${r.status})`);
  assert(r.headers.get("cache-control") === "private", `a 401 is private (got ${r.headers.get("cache-control")})`);
  r = await worker.fetch(
    new Request("https://x.test/things", {
      method: "POST",
      body: JSON.stringify("x"),
      headers: { ...auth, "content-type": "application/json" },
    }),
    env,
  );
  assert(r.status === 201, `POST /things is 201 (got ${r.status})`);
  assert(r.headers.get("cache-control") === "private", `a write is private (got ${r.headers.get("cache-control")})`);

  // An author's own `@cache` is kept.
  r = await get("/cached");
  assert(
    r.headers.get("cache-control") === "private, max-age=60",
    `@cache wins over the default (got ${r.headers.get("cache-control")})`,
  );

  // noStore sends no-store and no ETag, on a credentialed route and on a Visitor one.
  for (const [path, headers] of [["/tokens", auth], ["/open-tokens", {}]] as const) {
    r = await get(path, headers);
    assert(r.status === 200, `GET ${path} is 200 (got ${r.status})`);
    assert(r.headers.get("cache-control") === "no-store", `${path} is no-store (got ${r.headers.get("cache-control")})`);
    assert(r.headers.get("etag") === null, `${path} has no ETag`);
  }

  // A Visitor route is unchanged: no Cache-Control at all.
  r = await get("/hello", {});
  assert(r.status === 200, `GET /hello is 200 (got ${r.status})`);
  assert(r.headers.get("cache-control") === null, `a Visitor route sends none (got ${r.headers.get("cache-control")})`);

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
fn http_authenticated_cache_runs_on_workers_entry() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! HTTP AUTH-CACHE BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!(
            "\n!!! HTTP AUTH-CACHE BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n"
        );
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the auth-cache fixture (workers) in-process.
    let fixture: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/3922_http_authenticated_cache/src");
    let out = bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(fixture)
            .target(bynk_driver::project::BuildTarget::Workers),
    )
    .map_err(bynk_driver::project::ProjectFailure::flatten)
    .expect("the auth-cache fixture must compile");

    let tmp = std::env::temp_dir().join(format!("bynk-http-auth-cache-{}", std::process::id()));
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
        "the auth-cache driver must type-check + compile:\n{log}"
    );
    let (ran, log) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ran && log.contains("ALL OK"),
        "the auth-cache driver must run green:\n{log}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
