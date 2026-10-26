//! v0.49: behavioral auth-bypass tests for the emitted Bearer verifier.
//!
//! The v0.47 `verifyBearerJwtHs256` (now a real file since v0.48) is the app's
//! authentication boundary. Its correctness was established by a one-time
//! `/security-review`; this is the *standing* regression guard — it imports the
//! emitted runtime and feeds it crafted JWTs, asserting the verdict for every
//! bypass class (a future refactor that reopens one fails here).
//!
//! The driver signs HS256 tokens with WebCrypto (`crypto.subtle`), so it needs
//! no `@types/node`. Like the other tsc-driven tests it skips loudly without a
//! toolchain; `BYNK_REQUIRE_TSC=1` turns the skip into a failure.

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

const DRIVER_TS: &str = r#"import { verifyBearerJwtHs256 } from "./__bynk_runtime/auth.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) throw new Error("FAIL: " + msg);
}

const SECRET = "correct-horse-battery-staple";

function b64url(s: string): string {
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
function bytesB64url(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

async function sign(
  payload: Record<string, unknown>,
  secret: string,
  header: Record<string, unknown> = { alg: "HS256", typ: "JWT" },
): Promise<string> {
  const h = b64url(JSON.stringify(header));
  const p = b64url(JSON.stringify(payload));
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey(
    "raw",
    enc.encode(secret) as BufferSource,
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const sig = await crypto.subtle.sign("HMAC", key, enc.encode(`${h}.${p}`) as BufferSource);
  return `${h}.${p}.${bytesB64url(new Uint8Array(sig))}`;
}

const now = Math.floor(Date.now() / 1000);

// --- accept path: a correctly signed, unexpired token mints the sub ---
let r = await verifyBearerJwtHs256(await sign({ sub: "user-1", exp: now + 3600 }, SECRET), SECRET);
assert(r.tag === "Ok", "valid token is accepted");
assert(r.tag === "Ok" && r.value.sub === "user-1", "valid token yields the sub claim");

// --- reject paths: every bypass class must fail closed (Err → 401) ---
async function rejects(token: string, secret: string, why: string): Promise<void> {
  const res = await verifyBearerJwtHs256(token, secret);
  assert(res.tag === "Err", why);
}

// tampered signature
const good = await sign({ sub: "u", exp: now + 3600 }, SECRET);
const parts = good.split(".");
const flippedSig = (parts[2][0] === "A" ? "B" : "A") + parts[2].slice(1);
await rejects(`${parts[0]}.${parts[1]}.${flippedSig}`, SECRET, "tampered signature rejected");

// wrong secret
await rejects(await sign({ sub: "u", exp: now + 3600 }, "other-secret"), SECRET, "wrong-secret token rejected");

// alg: none (unsigned)
const noneTok = `${b64url(JSON.stringify({ alg: "none", typ: "JWT" }))}.${b64url(JSON.stringify({ sub: "u", exp: now + 3600 }))}.`;
await rejects(noneTok, SECRET, "alg:none rejected");

// algorithm confusion (RS256 label, HMAC body)
await rejects(await sign({ sub: "u", exp: now + 3600 }, SECRET, { alg: "RS256", typ: "JWT" }), SECRET, "non-HS256 alg rejected");

// expired
await rejects(await sign({ sub: "u", exp: now - 10 }, SECRET), SECRET, "expired token rejected");

// not yet valid (nbf in the future)
await rejects(await sign({ sub: "u", exp: now + 3600, nbf: now + 1000 }, SECRET), SECRET, "nbf-future token rejected");

// malformed exp (string, not NumericDate)
await rejects(await sign({ sub: "u", exp: "later" }, SECRET), SECRET, "malformed exp rejected");

// missing exp — a token with no expiry must not verify (it would never age out)
await rejects(await sign({ sub: "u" }, SECRET), SECRET, "missing exp rejected");

// missing sub
await rejects(await sign({ exp: now + 3600 }, SECRET), SECRET, "missing sub rejected");

// empty sub
await rejects(await sign({ sub: "", exp: now + 3600 }, SECRET), SECRET, "empty sub rejected");

// malformed token shapes
await rejects("not.a.jwt.token", SECRET, "4-segment token rejected");
await rejects("garbage", SECRET, "non-jwt rejected");
await rejects("", SECRET, "empty token rejected");

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
  "include": ["*.ts"]
}
"#;

#[test]
fn bearer_verifier_rejects_every_bypass_class() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! BEARER AUTH VERIFICATION SKIPPED !!!\nneither `tsc` nor `npx` is on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! BEARER AUTH VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-bearer-auth-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    bynk_testkit::write_runtime_modules(&tmp).unwrap();
    fs::write(tmp.join("driver.ts"), DRIVER_TS).unwrap();
    fs::write(tmp.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
    fs::write(tmp.join("package.json"), "{ \"type\": \"module\" }").unwrap();

    let (program, prefix) = &runner;
    let (ok, out) = run(program, prefix, &["-p", "tsconfig.json"], &tmp);
    assert!(ok, "tsc failed on the bearer-auth driver:\n{out}");

    let (ok, out) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ok && out.contains("ALL OK"),
        "bearer-auth driver did not pass:\n{out}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
