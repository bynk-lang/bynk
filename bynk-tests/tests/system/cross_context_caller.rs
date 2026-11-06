//! v0.54: behavioral tests for the cross-context `CallerId` value (Q7).
//!
//! Workers (`cross_context_caller_reads_live_id_and_fails_closed`): drives the
//! callee worker's `/_bynk/call/` dispatch with requests signed per edge (I2,
//! #1971): a signed call → the `by c: Caller` handler reads the proved caller
//! name; an unsigned or mis-signed one, a caller with no key at the callee, or
//! no caller at all → a bare 401 before anything else, as a forged request from
//! outside the deployment gets.
//!
//! Bundle (`bundle_cross_context_caller_reads_the_consuming_context_name`,
//! v0.54 / #655): the same handler on the bundle target, where a cross-context
//! call is a direct `composeApp`-wired invocation. Composes the app and checks
//! each consumer reads its own name and the top-level entry self-attributes —
//! the runtime half the `396_…` fixture only tsc-checks.
//!
//! Both skip loudly without a toolchain; `BYNK_REQUIRE_TSC=1` turns the skip
//! into a failure.

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

const SOURCE_B: &str = r#"context app.b

service whoami {
  on call(ping: String) -> Effect[Result[String, String]] by c: Caller {
    Ok(c.identity)
  }
}
"#;

const SOURCE_A: &str = r#"context app.a

consumes app.b as B

service ask {
  on call(ping: String) -> Effect[Result[String, String]] {
    let r <- B.whoami(ping)
    r
  }
}
"#;

// A second consumer of the same provider, to prove each consumer's cross-context
// call reads *its own* qualified name — the per-consumer surface the compose root
// builds, not one shared instance carrying a single caller.
const SOURCE_D: &str = r#"context app.d

consumes app.b as B

service probe {
  on call(ping: String) -> Effect[Result[String, String]] {
    let r <- B.whoami(ping)
    r
  }
}
"#;

// v0.54 (#655): the bundle-mode analogue of the workers driver below. There is no
// `/_bynk/call/` door and no header — cross-context calls are direct invocations
// wired by `composeApp`, so the driver composes the app and reads the caller each
// path threads through `makeSurface`.
const BUNDLE_DRIVER_TS: &str = r#"import { composeApp } from "./compose.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) throw new Error("FAIL: " + msg);
}

const app: any = composeApp();

// A consumer's cross-context call reads the *consumer's* qualified name.
let r: any = await app.a.ask("hi");
assert(r.tag === "Ok" && r.value === "app.a", "app.a reads its own name, got " + JSON.stringify(r));

// A second consumer of the same provider reads *its* name — proving the surface
// is built per-consumer, not shared with a single baked-in caller.
r = await app.d.probe("hi");
assert(r.tag === "Ok" && r.value === "app.d", "app.d reads its own name, got " + JSON.stringify(r));

// The top-level entry addresses the provider directly: no calling context, so it
// self-attributes with the provider's own qualified name (ADR 0092 note — this is
// where bundle and workers diverge; workers' internal door fail-closes instead).
r = await app.b.whoami("hi");
assert(r.tag === "Ok" && r.value === "app.b", "top-level self-attributes, got " + JSON.stringify(r));

console.log("ALL OK");
"#;

const DRIVER_TS: &str = r#"import worker from "./workers/app-b/index.js";
import { EDGE_TO_PREFIX, signedHeaders } from "./__bynk_runtime/boundary.js";

// v0.177 (#643): the compiled contract hash for `whoami`, extracted from the
// emitted callee and injected here. A correct caller stamps it beside the
// caller identity.
const CONTRACT = "__CONTRACT_HASH__";

function assert(cond: boolean, msg: string): void {
  if (!cond) throw new Error("FAIL: " + msg);
}

// I2 (#1971): every Bynk call is signed under the key its caller shares with
// this callee; `app.b` holds the key for the edge from `app.a`.
const KEY = "edge-key";
const env: any = { BYNK_CALL_FROM_APP__A: KEY };

// A call as `app.a` would send it, signed over its final headers by the
// runtime's own signer; `after` is applied once signed, to break it.
async function call(headers: Record<string, string>, service = "whoami", after: Record<string, string> = {}): Promise<Request> {
  const all: Record<string, string> = {
    "content-type": "application/json",
    "X-Bynk-Contract": CONTRACT,
    "X-Bynk-Caller": "app.a",
    ...headers,
  };
  const caller = all["X-Bynk-Caller"];
  const body = JSON.stringify("hello");
  const signed = await signedHeaders(
    { [EDGE_TO_PREFIX + "APP__B"]: KEY },
    caller,
    "app.b",
    "/_bynk/call/" + service,
    all["X-Bynk-Contract"],
    "1",
    body,
    "test",
  );
  // `signedHeaders` stamps the caller it signed for; keep a test's override.
  return new Request("http://internal/_bynk/call/" + service, {
    method: "POST",
    headers: { ...all, ...signed, "X-Bynk-Caller": caller, ...after },
    body,
  });
}

// 1. A signed call → the body reads the proved caller name.
let res = await worker.fetch(await call({}), env);
assert(res.status === 200, "a signed call → 200, got " + res.status);
let body: any = await res.json();
assert(body.kind === "Ok" && body.value === "app.a", "body returns the caller id");

// v0.177 (#643): the deploy-skew seam.
//
// 4. A caller compiled against a *different* contract → fail closed with 409 and
//    a named ContractMismatch. This is the whole point: the pair disagree, so
//    the payload's interpretation is in doubt and the callee refuses rather than
//    misreading it.
res = await worker.fetch(await call({ "X-Bynk-Contract": "0000000000000000" }), env);
assert(res.status === 409, "skewed contract → 409, got " + res.status);
body = await res.json();
assert(body.kind === "ContractMismatch", "409 body names the fault");
assert(body.service === "whoami", "409 body names the service");
assert(body.expected === CONTRACT, "409 body reports the expected hash");
assert(body.actual === "0000000000000000", "409 body reports what arrived");

// 5. Absent contract header → fail closed too. A Bynk caller always stamps one,
//    so its absence is a non-Bynk or pre-upgrade caller — skewed by definition.
res = await worker.fetch(await call({ "X-Bynk-Contract": "" }), env);
assert(res.status === 409, "empty contract header → 409, got " + res.status);

// I2 (#1971): the signature. `/_bynk/call/` is reachable from the public
// internet, so a request that does not verify is refused first, with a bare
// 401 and nothing about the service, its contract or its caller.
async function refused(req: Request | Promise<Request>, e: any, what: string): Promise<void> {
  const r = await worker.fetch(await req, e);
  assert(r.status === 401, what + " → 401, got " + r.status);
  const text = await r.text();
  assert(text === "Unauthorized", what + " → no detail, got " + JSON.stringify(text));
}

// 6. A forged call: right caller, right contract, no signature.
await refused(call({}, "whoami", { "X-Bynk-Signature": "" }), env, "missing signature");

// 7. A caller claiming another context's name: `app.c` holds no key here.
await refused(call({ "X-Bynk-Caller": "app.c" }), env, "a caller with no edge");
await refused(call({}, "whoami", { "X-Bynk-Caller": "app.c" }), { ...env, BYNK_CALL_FROM_APP__C: KEY }, "a name the signature does not cover");

// 8. No caller at all.
await refused(call({}, "whoami", { "X-Bynk-Caller": "" }), env, "no caller");

// 9. Checked before the contract: a forger gets no 409 echoing the hash.
await refused(
  call({ "X-Bynk-Contract": "0000000000000000" }, "whoami", { "X-Bynk-Signature": "" }),
  env,
  "missing signature with a skewed contract",
);

// 10. And before the service lookup: no 404 to tell services apart.
await refused(call({}, "nosuch", { "X-Bynk-Signature": "" }), env, "missing signature, unknown service");

// 11. Fail closed: a Worker with no key for the edge refuses every call.
await refused(call({}), {}, "callee with no key");

console.log("ALL OK");
"#;

// #661: the caller-side-codec round-trip. `app.b` returns a **user record**
// across the boundary; `app.a` calls it and reads a field off the decoded
// value. Under `--target workers` `app.a` generates its *own*
// `deserialise_Result_Ticket_String` / `deserialise_Ticket` (it imports `app.b`
// for types only), so driving `app.a.forward` and asserting the field survived
// exercises exactly the codec this increment moves caller-side — which the
// existing driver above, hitting the *callee* directly, never runs.
const SOURCE_CODEC_B: &str = r#"context app.b

exports transparent { Ticket }

type Ticket = {
  seat:  Int,
  label: String,
}

service issue {
  on call(seat: Int) -> Effect[Result[Ticket, String]] {
    Ok(Ticket { seat: seat, label: "vip" })
  }
}
"#;

const SOURCE_CODEC_A: &str = r#"context app.a

consumes app.b as B

service forward {
  on call(seat: Int) -> Effect[Result[Int, String]] {
    let r <- B.issue(seat)
    match r {
      Ok(t)  => Ok(t.seat)
      Err(e) => Err(e)
    }
  }
}
"#;

// Drives `app.a.forward`, whose handler decodes `app.b`'s `Ticket` reply through
// `app.a`'s own generated codec, then returns the record's `seat` field. A
// caller-side codec that dropped or corrupted the payload would not return the
// seat that was sent.
const CODEC_DRIVER_TS: &str = r#"import workerA from "./workers/app-a/index.js";
import workerB from "./workers/app-b/index.js";
import { signedHeaders } from "./__bynk_runtime/boundary.js";

// `forward`'s own contract hash, read out of app-a's entry and injected here so
// the driver stamps the real constant rather than restating one.
const CONTRACT = "__CONTRACT_HASH__";

function assert(cond: boolean, msg: string): void {
  if (!cond) throw new Error("FAIL: " + msg);
}

// app.a's Worker reaches app.b through its APP_B service binding. Both hold
// the key for the edge between them (I2, #1971): app.a signs, app.b verifies.
// The driver calls app.a as `driver`, under that edge's key.
const KEY = "edge-key";
const envB: any = { BYNK_CALL_FROM_APP__A: KEY };
const envA: any = {
  BYNK_CALL_FROM_DRIVER: "driver-key",
  BYNK_CALL_TO_APP__B: KEY,
  APP_B: { fetch: (req: Request) => workerB.fetch(req, envB) },
};

async function forward(): Promise<Request> {
  const body = JSON.stringify(42);
  const signed = await signedHeaders({ BYNK_CALL_TO_APP__A: "driver-key" }, "driver", "app.a", "/_bynk/call/forward", CONTRACT, "1", body, "test");
  return new Request("http://internal/_bynk/call/forward", {
    method: "POST",
    headers: { "content-type": "application/json", "X-Bynk-Contract": CONTRACT, ...signed },
    body,
  });
}

const res = await workerA.fetch(await forward(), envA);
assert(res.status === 200, "forward → 200, got " + res.status);
const body: any = await res.json();
assert(
  body.kind === "Ok" && body.value === 42,
  "caller-side codec round-trips the record's seat field, got " + JSON.stringify(body),
);

// I2: a callee holding a different key for the edge refuses app.a's call,
// which app.a reports as a fault (500) rather than a decoded value.
const envBad: any = {
  ...envA,
  APP_B: { fetch: (req: Request) => workerB.fetch(req, { BYNK_CALL_FROM_APP__A: "other" } as any) },
};
const refused = await workerA.fetch(await forward(), envBad);
assert(refused.status === 500, "a callee on another key refuses the call, got " + refused.status);

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
fn cross_context_caller_reads_live_id_and_fails_closed() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!("\n!!! CROSS-CONTEXT CALLER VERIFICATION SKIPPED !!!\nno tsc runner.\n");
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! CROSS-CONTEXT CALLER VERIFICATION SKIPPED !!!\n`node` not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-xctx-caller-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj/app");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("a.bynk"), SOURCE_A).unwrap();
    fs::write(proj.join("b.bynk"), SOURCE_B).unwrap();

    let out = match bynkc::compile_project(
        &bynk_testkit::compile_options_single(tmp.join("proj")).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the cross-context project to Workers:\n{}",
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
    // v0.177 (#643): read the callee's compiled constant out of its emitted
    // entry and give it to the driver, so the guard drives the *real* hash
    // rather than restating one (which would only prove the test agrees with
    // itself).
    let callee_text = out
        .artefacts
        .docs
        .iter()
        .find(|(p, _)| p.ends_with("workers/app-b/index.ts"))
        .expect("app-b entry emitted")
        .1
        .text();
    let hash = regex::Regex::new(r#"expected: "([0-9a-f]{16})""#)
        .unwrap()
        .captures(&callee_text)
        .map(|c| c[1].to_string())
        .expect("app-b's entry stamps a contract hash");
    fs::write(
        run_dir.join("driver.ts"),
        DRIVER_TS.replace("__CONTRACT_HASH__", &hash),
    )
    .unwrap();
    fs::write(run_dir.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
    fs::write(run_dir.join("package.json"), "{ \"type\": \"module\" }").unwrap();

    let (program, prefix) = &runner;
    let (ok, msg) = run(program, prefix, &["-p", "tsconfig.json"], &run_dir);
    assert!(ok, "tsc failed on the cross-context-caller workers:\n{msg}");

    let (ok, msg) = run("node", &[], &["js/driver.js"], &run_dir);
    assert!(
        ok && msg.contains("ALL OK"),
        "cross-context-caller driver did not pass:\n{msg}"
    );
    let _ = fs::remove_dir_all(&tmp);
}

/// v0.54 (#655): the bundle-target twin of the workers test above. A bundle-mode
/// `by c: Caller` handler could not even compile before this fix (its
/// `makeSurface` fed `deps` without the `identity` field it typed, TS2345). This
/// drives the *runtime* value the fixture (`396_service_caller_binder_bundle_surface`)
/// only tsc-checks: each consumer reads its own qualified name, and the top-level
/// entry self-attributes — including the multiple-consumer path the compose root's
/// per-consumer surfaces exist to serve.
#[test]
fn bundle_cross_context_caller_reads_the_consuming_context_name() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! BUNDLE CROSS-CONTEXT CALLER VERIFICATION SKIPPED !!!\nno tsc runner.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!(
            "\n!!! BUNDLE CROSS-CONTEXT CALLER VERIFICATION SKIPPED !!!\n`node` not on PATH.\n"
        );
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-xctx-caller-bundle-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj/app");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("a.bynk"), SOURCE_A).unwrap();
    fs::write(proj.join("b.bynk"), SOURCE_B).unwrap();
    fs::write(proj.join("d.bynk"), SOURCE_D).unwrap();

    let out = match bynkc::compile_project(
        &bynk_testkit::compile_options_single(tmp.join("proj")).target(BuildTarget::Bundle),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the cross-context project to a bundle:\n{}",
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
    fs::write(run_dir.join("driver.ts"), BUNDLE_DRIVER_TS).unwrap();
    fs::write(run_dir.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
    fs::write(run_dir.join("package.json"), "{ \"type\": \"module\" }").unwrap();

    let (program, prefix) = &runner;
    let (ok, msg) = run(program, prefix, &["-p", "tsconfig.json"], &run_dir);
    assert!(ok, "tsc failed on the cross-context-caller bundle:\n{msg}");

    let (ok, msg) = run("node", &[], &["js/driver.js"], &run_dir);
    assert!(
        ok && msg.contains("ALL OK"),
        "bundle cross-context-caller driver did not pass:\n{msg}"
    );
    let _ = fs::remove_dir_all(&tmp);
}

/// #661 (ADR 0199 Decision G discharged): a **live** round-trip of a user-typed
/// payload through the *caller's own* boundary codec. Under `--target workers`
/// `app.a` no longer imports `app.b`'s module as a value — it generates its own
/// `deserialise_Result_Ticket_String` and imports `app.b` for types only — so a
/// codec that silently diverged from the callee's would break the wire. The e2e
/// goldens and `tsc --strict` prove the emitted shape; only this proves the
/// decoded *value*. Both sides type-check independently, so a payload assertion
/// is the guard a status code is not (the increment's stated top risk).
#[test]
fn cross_context_caller_side_codec_round_trips_a_user_payload() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!("\n!!! CALLER-SIDE CODEC VERIFICATION SKIPPED !!!\nno tsc runner.\n");
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! CALLER-SIDE CODEC VERIFICATION SKIPPED !!!\n`node` not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-xctx-codec-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj/app");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("a.bynk"), SOURCE_CODEC_A).unwrap();
    fs::write(proj.join("b.bynk"), SOURCE_CODEC_B).unwrap();

    let out = match bynkc::compile_project(
        &bynk_testkit::compile_options_single(tmp.join("proj")).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the caller-side-codec project to Workers:\n{}",
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
    // `forward`'s contract hash lives in app-a's own entry (the constant it
    // compares an incoming `X-Bynk-Contract` against). Extract it so the driver
    // stamps the real value.
    let caller_text = out
        .artefacts
        .docs
        .iter()
        .find(|(p, _)| p.ends_with("workers/app-a/index.ts"))
        .expect("app-a entry emitted")
        .1
        .text();
    let hash = regex::Regex::new(r#"expected: "([0-9a-f]{16})""#)
        .unwrap()
        .captures(&caller_text)
        .map(|c| c[1].to_string())
        .expect("app-a's entry stamps a contract hash");
    fs::write(
        run_dir.join("driver.ts"),
        CODEC_DRIVER_TS.replace("__CONTRACT_HASH__", &hash),
    )
    .unwrap();
    fs::write(run_dir.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
    fs::write(run_dir.join("package.json"), "{ \"type\": \"module\" }").unwrap();

    let (program, prefix) = &runner;
    let (ok, msg) = run(program, prefix, &["-p", "tsconfig.json"], &run_dir);
    assert!(ok, "tsc failed on the caller-side-codec workers:\n{msg}");

    let (ok, msg) = run("node", &[], &["js/driver.js"], &run_dir);
    assert!(
        ok && msg.contains("ALL OK"),
        "caller-side-codec driver did not pass:\n{msg}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
