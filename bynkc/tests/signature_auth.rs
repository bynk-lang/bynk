//! v0.51: behavioral auth-bypass tests for the emitted Signature verifier.
//!
//! The standing regression guard for `verifySignatureHmacSha256` (the webhook
//! HMAC seam), parallel to `bearer_auth.rs`: a Node driver signs bodies with
//! WebCrypto and asserts the verdict for every bypass class. Skips loudly
//! without a toolchain; `BYNK_REQUIRE_TSC=1` turns the skip into a failure.

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

const DRIVER_TS: &str = r#"import { verifySignatureHmacSha256 } from "./__bynk_runtime/auth.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) throw new Error("FAIL: " + msg);
}

const SECRET = "wh-secret";
const enc = new TextEncoder();

async function hmacHex(data: string, secret: string): Promise<string> {
  const key = await crypto.subtle.importKey(
    "raw",
    enc.encode(secret) as BufferSource,
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const sig = await crypto.subtle.sign("HMAC", key, enc.encode(data) as BufferSource);
  return [...new Uint8Array(sig)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

const body = JSON.stringify({ id: "evt_1" });

// --- no-timestamp shape (HMAC over the body) ---
const sig = await hmacHex(body, SECRET);
assert(await verifySignatureHmacSha256(body, SECRET, sig, null, null), "valid body signature accepted");
assert(await verifySignatureHmacSha256(body, SECRET, "sha256=" + sig, null, null), "sha256= prefix accepted");

async function rejects(
  b: string, secret: string, header: string | null, ts: string | null, tol: number | null, why: string,
): Promise<void> {
  assert(!(await verifySignatureHmacSha256(b, secret, header, ts, tol)), why);
}

await rejects(body + "x", SECRET, sig, null, null, "tampered body rejected");
await rejects(body, "other-secret", sig, null, null, "wrong secret rejected");
await rejects(body, SECRET, null, null, null, "absent signature header rejected");
await rejects(body, SECRET, "zzzz", null, null, "malformed hex rejected");
await rejects(body, SECRET, "", null, null, "empty signature rejected");

// --- timestamped shape (HMAC over `<ts>.<body>`, replay window) ---
const now = Math.floor(Date.now() / 1000);
const tsSig = await hmacHex(`${now}.${body}`, SECRET);
assert(await verifySignatureHmacSha256(body, SECRET, tsSig, String(now), 300), "valid timestamped signature accepted");

const stale = now - 1000;
const staleSig = await hmacHex(`${stale}.${body}`, SECRET);
await rejects(body, SECRET, staleSig, String(stale), 300, "stale timestamp rejected");
await rejects(body, SECRET, sig, String(now), 300, "body-only signature rejected when timestamp is bound");
await rejects(body, SECRET, tsSig, "not-a-number", 300, "non-numeric timestamp rejected");

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
fn signature_verifier_rejects_every_bypass_class() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! SIGNATURE AUTH VERIFICATION SKIPPED !!!\nneither `tsc` nor `npx` is on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! SIGNATURE AUTH VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-signature-auth-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    bynk_testkit::write_runtime_modules(&tmp).unwrap();
    fs::write(tmp.join("driver.ts"), DRIVER_TS).unwrap();
    fs::write(tmp.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
    fs::write(tmp.join("package.json"), "{ \"type\": \"module\" }").unwrap();

    let (program, prefix) = &runner;
    let (ok, out) = run(program, prefix, &["-p", "tsconfig.json"], &tmp);
    assert!(ok, "tsc failed on the signature-auth driver:\n{out}");

    let (ok, out) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ok && out.contains("ALL OK"),
        "signature-auth driver did not pass:\n{out}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
