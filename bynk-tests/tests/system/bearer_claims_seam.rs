//! #3821: behavioural proof of a `Bearer` actor's `issuer` and `audience`. This compiles the
//! `3821_bearer_iss_aud` Workers fixture and drives the emitted Worker's `fetch` with real signed tokens
//! under Node, asserting the seam's verdict on each:
//!
//! * a token whose `iss` and `aud` match is accepted, and an `aud` array that contains the audience too;
//! * a wrong or missing `iss`, a wrong or missing `aud`, a non-string `aud` and an array that does not
//!   contain it are `401`;
//! * a token minted for one actor is `401` at a route of another actor on the same secret;
//! * an actor that declares no `issuer` does not check one, and a token's `iss` or `aud` that the actor
//!   does not name changes nothing;
//! * `trustFrom` replaces the declared values from the environment, and a malformed variable refuses
//!   every token.
//!
//! Like the other tsc-driven tests it skips loudly without a toolchain; `BYNK_REQUIRE_TSC=1` turns the
//! skip into a failure.

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

function b64url(s: string): string {
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

async function sign(payload: Record<string, unknown>, secret: string): Promise<string> {
  const h = b64url(JSON.stringify({ alg: "HS256", typ: "JWT" }));
  const p = b64url(JSON.stringify(payload));
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey(
    "raw",
    enc.encode(secret) as BufferSource,
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const sig = new Uint8Array(await crypto.subtle.sign("HMAC", key, enc.encode(`${h}.${p}`) as BufferSource));
  let bin = "";
  for (const b of sig) bin += String.fromCharCode(b);
  return `${h}.${p}.${btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "")}`;
}

const SECRET = "shared-secret";
const PARTNER_SECRET = "partner-secret";
const now = Math.floor(Date.now() / 1000);
const exp = now + 3600;

async function status(path: string, token: string | null, env: Record<string, unknown>): Promise<number> {
  const headers: Record<string, string> = {};
  if (token !== null) headers["authorization"] = `Bearer ${token}`;
  const res = await worker.fetch(new Request(`https://api.example${path}`, { headers }), env);
  return res.status;
}

const base: Record<string, unknown> = { AUTH_SECRET: SECRET, PARTNER_SECRET };
const web = (claims: Record<string, unknown>) => sign({ sub: "ada", exp, ...claims }, SECRET);

// Learner declares issuer and audience.
const ok = await web({ iss: "agilearn-web", aud: "agilearn-api" });
assert((await status("/me", ok, base)) === 200, "a matching token is accepted");
assert((await status("/me", await web({ iss: "agilearn-web", aud: ["x", "agilearn-api"] }), base)) === 200, "an aud array containing the audience is accepted");
for (const [claims, why] of [
  [{ iss: "somebody-else", aud: "agilearn-api" }, "a wrong iss"],
  [{ aud: "agilearn-api" }, "a missing iss"],
  [{ iss: 7, aud: "agilearn-api" }, "a non-string iss"],
  [{ iss: "agilearn-web", aud: "agilearn-internal" }, "a wrong aud"],
  [{ iss: "agilearn-web" }, "a missing aud"],
  [{ iss: "agilearn-web", aud: 7 }, "a non-string aud"],
  [{ iss: "agilearn-web", aud: ["x", "y"] }, "an aud array without the audience"],
  [{ iss: "agilearn-web", aud: ["agilearn-api", 3] }, "an aud array with a non-string member"],
  [{ iss: "agilearn-web", aud: [] }, "an empty aud array"],
] as [Record<string, unknown>, string][]) {
  assert((await status("/me", await web(claims), base)) === 401, `${why} is a 401`);
}
assert((await status("/me", null, base)) === 401, "no token is a 401");
assert((await status("/me", await sign({ sub: "ada", exp, iss: "agilearn-web", aud: "agilearn-api" }, "another-secret"), base)) === 401, "a foreign secret is a 401");

// Internal declares an audience only, on the same secret: each actor refuses the other's token.
const internal = await web({ iss: "agilearn-web", aud: "agilearn-internal" });
assert((await status("/internal", internal, base)) === 200, "the internal token passes the internal route");
assert((await status("/me", internal, base)) === 401, "the internal token is a 401 at the learner route");
assert((await status("/internal", ok, base)) === 401, "the learner token is a 401 at the internal route");
// No `issuer` declared: any `iss`, or none, passes where the `aud` matches.
assert((await status("/internal", await web({ aud: "agilearn-internal" }), base)) === 200, "an undeclared issuer is not checked");
assert((await status("/internal", await web({ iss: "anyone", aud: ["agilearn-internal", "more"] }), base)) === 200, "an iss the actor does not name changes nothing");
assert((await status("/internal", await web({ iss: "agilearn-web" }), base)) === 401, "a token with no aud is a 401 where an audience is declared");

// Partner: trustFrom. Unset leaves the declared values.
const partner = (claims: Record<string, unknown>) => sign({ sub: "ada", exp, ...claims }, PARTNER_SECRET);
const declared = await partner({ iss: "partner", aud: "partner-api" });
assert((await status("/partner", declared, base)) === 200, "trustFrom unset leaves the declared values");
// Set: the variable's JSON replaces them, so the declared token is refused and the staging one passes.
const staging = { ...base, PARTNER_TRUST: JSON.stringify({ issuer: "partner-stg", audience: "stg-api" }) };
assert((await status("/partner", declared, staging)) === 401, "trustFrom replaces the declared values");
assert((await status("/partner", await partner({ iss: "partner-stg", aud: "stg-api" }), staging)) === 200, "a token for the environment's values passes");
// An audience-only override keeps the declared issuer.
const audOnly = { ...base, PARTNER_TRUST: JSON.stringify({ audience: "stg-api" }) };
assert((await status("/partner", await partner({ iss: "partner", aud: "stg-api" }), audOnly)) === 200, "an override of the audience keeps the declared issuer");
// Set but unreadable refuses everything, fail closed, and never falls back to the declaration.
const log = console.error;
console.error = () => {};
for (const bad of ["not json", "[]", '{"issuer":3}']) {
  assert((await status("/partner", declared, { ...base, PARTNER_TRUST: bad })) === 401, `a malformed trustFrom (${bad}) refuses every token`);
}
console.error = log;

console.log("ALL OK");
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

/// Compile `fixture` (a Workers fixture) and run `driver` against its emitted Worker under Node.
fn drive(fixture_name: &str, driver: &str) {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!("\n!!! BEARER CLAIMS VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n");
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! BEARER CLAIMS VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the Bearer claims Workers fixture in-process.
    let fixture: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive")
        .join(fixture_name)
        .join("src");
    let out = bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(fixture)
            .target(bynk_check::project_model::BuildTarget::Workers),
    )
    .map_err(bynk_driver::project::ProjectFailure::flatten)
    .expect("the Bearer claims Workers fixture must compile");

    let tmp = std::env::temp_dir().join(format!(
        "bynk-bearer-claims-{fixture_name}-{}",
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
    fs::write(tmp.join("driver.ts"), driver).unwrap();
    fs::write(tmp.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
    fs::write(tmp.join("package.json"), "{ \"type\": \"module\" }").unwrap();

    // Type-check and compile to JS, then run the driver under Node.
    let (ok, log) = run(&runner.0, &runner.1, &["--project", "tsconfig.json"], &tmp);
    assert!(
        ok,
        "the Bearer claims driver must type-check + compile:\n{log}"
    );
    let (ran, log) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ran && log.contains("ALL OK"),
        "the Bearer claims driver must run green:\n{log}"
    );

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn a_bearer_actor_checks_the_iss_and_aud_it_declares() {
    drive("3821_bearer_iss_aud", DRIVER_TS);
}
