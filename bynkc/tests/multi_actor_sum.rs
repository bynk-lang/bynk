//! v0.52: behavioral first-wins / fail-closed tests for a multi-actor sum
//! handler (`by who: User | Hook`).
//!
//! The standing guard for the emitted resolution wrapper (parallel to
//! `bearer_auth.rs` / `signature_auth.rs`): a Node driver compiles a mixed
//! Bearer-or-Signature route to a real Worker, then drives its compose surface
//! with crafted requests and asserts the resolution — a valid Bearer token
//! resolves the `User` arm; a valid body signature (no token) resolves the
//! `Hook` arm; an invalid token + valid signature falls through to `Hook`
//! (first-wins); neither credential, or a tampered body, fails closed (401).
//! Skips loudly without a toolchain; `BYNK_REQUIRE_TSC=1` turns the skip into a
//! failure.

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

type Event = {
  id: String,
}

actor User { auth = Bearer(secret = "AUTH_JWT_SECRET"), identity = UserId }
actor Hook { auth = Signature(secret = "WH_SECRET", header = "X-Signature") }

service api from http {
  on POST("/ingest") (body: Event) -> Effect[HttpResult[String]] by who: User | Hook {
    match who {
      User(u) => Ok(u)
      Hook => Ok(body.id)
    }
  }
}
"#;

const DRIVER_TS: &str = r#"import { compose } from "./workers/api/compose.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) throw new Error("FAIL: " + msg);
}

const AUTH = "auth-secret";
const WH = "wh-secret";
const env: any = { AUTH_JWT_SECRET: AUTH, WH_SECRET: WH };
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
async function hmacHex(data: string, secret: string): Promise<string> {
  const key = await crypto.subtle.importKey(
    "raw", enc.encode(secret) as BufferSource,
    { name: "HMAC", hash: "SHA-256" }, false, ["sign"],
  );
  const sig = await crypto.subtle.sign("HMAC", key, enc.encode(data) as BufferSource);
  return [...new Uint8Array(sig)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

const surface = compose(env);
const now = Math.floor(Date.now() / 1000);
const body = JSON.stringify({ id: "evt-1" });
function req(headers: Record<string, string>, b: string = body): Request {
  return new Request("https://x/ingest", { method: "POST", headers, body: b });
}

// 1. A valid Bearer token resolves the `User` arm (the body returns the sub).
const jwt = await signJwt({ sub: "user-1", exp: now + 3600 }, AUTH);
let r: any = await surface.http_POST_ingest(req({ Authorization: "Bearer " + jwt }));
assert(r.tag === "Ok" && r.value === "user-1", "valid bearer resolves the User arm");

// 2. A valid body signature (no token) resolves the `Hook` arm.
const sig = await hmacHex(body, WH);
r = await surface.http_POST_ingest(req({ "X-Signature": sig }));
assert(r.tag === "Ok" && r.value === "evt-1", "valid signature resolves the Hook arm");

// 3. First-wins: an invalid token falls through to the signature member.
r = await surface.http_POST_ingest(req({ Authorization: "Bearer not.a.jwt", "X-Signature": sig }));
assert(r.tag === "Ok" && r.value === "evt-1", "invalid bearer falls through to Hook (first-wins)");

// 4. No credential at all fails closed → 401.
r = await surface.http_POST_ingest(req({}));
assert(r.tag === "Unauthorized", "no credential fails closed (401)");

// 5. A tampered body (signature no longer matches the bytes) fails closed.
r = await surface.http_POST_ingest(req({ "X-Signature": sig }, JSON.stringify({ id: "tampered" })));
assert(r.tag === "Unauthorized", "tampered body fails closed (401)");

// 6. A wrong-secret signature fails closed.
const badSig = await hmacHex(body, "not-the-secret");
r = await surface.http_POST_ingest(req({ "X-Signature": badSig }));
assert(r.tag === "Unauthorized", "wrong-secret signature fails closed (401)");

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
fn multi_actor_sum_resolves_first_wins_and_fails_closed() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!("\n!!! MULTI-ACTOR SUM VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n");
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! MULTI-ACTOR SUM VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-multi-actor-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("api.bynk"), SOURCE).unwrap();

    let out = match bynkc::compile_project(
        &bynk_testkit::compile_options_single(&proj).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the multi-actor sum project to Workers:\n{}",
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
    assert!(ok, "tsc failed on the multi-actor-sum worker:\n{msg}");

    let (ok, msg) = run("node", &[], &["js/driver.js"], &run_dir);
    assert!(
        ok && msg.contains("ALL OK"),
        "multi-actor-sum driver did not pass:\n{msg}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
