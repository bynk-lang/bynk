//! v0.53: behavioral 401/403/allow trichotomy test for a refinement actor
//! (`actor Admin = User where hasClaim("admin")`).
//!
//! The standing guard for the emitted authorisation seam (parallel to
//! `bearer_auth.rs`): a Node driver signs JWTs and drives the emitted compose
//! surface, asserting the trichotomy — no/invalid token → 401; a valid token
//! *without* the claim → 403; a valid token *with* the claim → the body runs
//! and mints the identity. The 401 and 403 channels stay distinct, and the
//! claim predicate cannot be bypassed. Skips loudly without a toolchain;
//! `BYNK_REQUIRE_TSC=1` turns the skip into a failure.

use bynkc::BuildTarget;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

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
    Command::new(finder)
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
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

const SOURCE: &str = r#"context api

type UserId = String where NonEmpty

actor User { auth = Bearer(secret = "AUTH_JWT_SECRET"), identity = UserId }
actor Admin = User where hasClaim("admin")

service api from http {
  on GET("/admin") () -> Effect[HttpResult[UserId]] by a: Admin {
    Ok(a.identity)
  }
}
"#;

const DRIVER_TS: &str = r#"import { compose } from "./workers/api/compose.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) throw new Error("FAIL: " + msg);
}

const AUTH = "auth-secret";
const env: any = { AUTH_JWT_SECRET: AUTH };
const enc = new TextEncoder();

function b64url(s: string): string {
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
function bytesB64url(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
async function signJwt(payload: Record<string, unknown>, secret: string): Promise<string> {
  const h = b64url(JSON.stringify({ alg: "HS256", typ: "JWT" }));
  const p = b64url(JSON.stringify(payload));
  const key = await crypto.subtle.importKey(
    "raw", enc.encode(secret) as BufferSource,
    { name: "HMAC", hash: "SHA-256" }, false, ["sign"],
  );
  const sig = await crypto.subtle.sign("HMAC", key, enc.encode(`${h}.${p}`) as BufferSource);
  return `${h}.${p}.${bytesB64url(new Uint8Array(sig))}`;
}

const surface = compose(env);
const now = Math.floor(Date.now() / 1000);
function get(headers: Record<string, string>): Request {
  return new Request("https://x/admin", { method: "GET", headers });
}

// 1. No token → 401 (authentication failure).
let r: any = await surface.http_GET_admin(get({}));
assert(r.tag === "Unauthorized", "no token → 401");

// 2. Invalid token → 401.
r = await surface.http_GET_admin(get({ Authorization: "Bearer not.a.jwt" }));
assert(r.tag === "Unauthorized", "invalid token → 401");

// 3. Valid token WITHOUT the admin claim → 403 (authorisation failure, distinct
//    from 401 — the scheme verified, the invariant did not).
const plain = await signJwt({ sub: "user-1", exp: now + 3600 }, AUTH);
r = await surface.http_GET_admin(get({ Authorization: "Bearer " + plain }));
assert(r.tag === "Forbidden", "valid token without claim → 403 (not 401)");

// 4. Valid token WITH a falsy admin claim → still 403.
const falsy = await signJwt({ sub: "user-1", admin: false, exp: now + 3600 }, AUTH);
r = await surface.http_GET_admin(get({ Authorization: "Bearer " + falsy }));
assert(r.tag === "Forbidden", "falsy admin claim → 403");

// 5. Valid token WITH the admin claim → body runs, identity minted from sub.
const admin = await signJwt({ sub: "user-1", admin: true, exp: now + 3600 }, AUTH);
r = await surface.http_GET_admin(get({ Authorization: "Bearer " + admin }));
assert(r.tag === "Ok" && r.value === "user-1", "valid admin token → body runs with identity");

console.log("ALL OK");
"#;

const TSCONFIG_JSON: &str = r#"{
  "compilerOptions": {
    "target": "ES2022",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "strict": true,
    "skipLibCheck": true,
    "outDir": "js",
    "rootDir": ".",
    "lib": ["ES2022", "DOM"]
  },
  "include": ["**/*.ts"]
}
"#;

#[test]
fn refinement_actor_enforces_401_403_allow_trichotomy() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!("\n!!! REFINEMENT AUTH VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n");
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! REFINEMENT AUTH VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-refinement-auth-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("api.bynk"), SOURCE).unwrap();

    let out = match bynkc::compile_project(
        &bynk_testkit::compile_options_single(&proj).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the refinement project to Workers:\n{}",
            bynkc::render_project_errors(&failure.flatten())
        ),
    };

    let run_dir = tmp.join("run");
    for (path, doc) in &out.artefacts.docs {
        let target = run_dir.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, doc.text()).unwrap();
    }
    bynk_testkit::write_runtime_modules(&run_dir).unwrap();
    fs::write(run_dir.join("driver.ts"), DRIVER_TS).unwrap();
    fs::write(run_dir.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
    fs::write(run_dir.join("package.json"), "{ \"type\": \"module\" }").unwrap();

    let (program, prefix) = &runner;
    let (ok, msg) = run(program, prefix, &["-p", "tsconfig.json"], &run_dir);
    assert!(ok, "tsc failed on the refinement-auth worker:\n{msg}");

    let (ok, msg) = run("node", &[], &["js/driver.js"], &run_dir);
    assert!(
        ok && msg.contains("ALL OK"),
        "refinement-auth driver did not pass:\n{msg}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
