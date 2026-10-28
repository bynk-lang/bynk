//! #2554: a callee answers the older contracts its `bynk.contracts.lock`
//! records, and nothing else.
//!
//! Behavioural, like `cross_context_caller.rs`: the callee is compiled twice,
//! as it was and as it is, the second time with the lock; its Worker entry is
//! then driven with signed calls stamping each contract hash. The old caller's
//! hash is answered, the current one is, and a hash the lock does not record
//! (a contract the callee never agreed to keep answering, and a made-up one)
//! is still a `409 ContractMismatch`.
//!
//! Skips loudly without a toolchain; `BYNK_REQUIRE_TSC=1` turns the skip into
//! a failure.

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

/// The callee as the live callers know it.
const CALLEE_V1: &str = r#"context app.b

type Receipt = { id: String }

service issue {
  on call(n: Int) -> Effect[Receipt] {
    Effect.pure(Receipt { id: "r" })
  }
}
"#;

/// The callee as it is now: the receipt gains an optional field, which a
/// caller on V1 ignores. Compatible.
const CALLEE_V2: &str = r#"context app.b

type Receipt = { id: String, note: Option[String] }

service issue {
  on call(n: Int) -> Effect[Receipt] {
    Effect.pure(Receipt { id: "r", note: None })
  }
}
"#;

/// A contract the callee never agreed to keep answering: the receipt's `id`
/// is retyped, which breaks a caller on V1 or V2.
const CALLEE_OTHER: &str = r#"context app.b

type Receipt = { id: Int }

service issue {
  on call(n: Int) -> Effect[Receipt] {
    Effect.pure(Receipt { id: 1 })
  }
}
"#;

const DRIVER_TS: &str = r#"import worker from "./workers/app-b/index.js";
import { EDGE_TO_PREFIX, signedHeaders } from "./__bynk_runtime/boundary.js";

const CURRENT = "__CURRENT__";
const ACCEPTED_OLDER = "__V1__";
const NEVER_AGREED = "__OTHER__";

function assert(cond: boolean, msg: string): void {
  if (!cond) throw new Error("FAIL: " + msg);
}

const KEY = "edge-key";
const env: any = { BYNK_CALL_FROM_APP__A: KEY };

// A call as `app.a` would send it, stamping `contract` and signed over it.
async function call(contract: string): Promise<Request> {
  const body = JSON.stringify(7);
  const signed = await signedHeaders(
    { [EDGE_TO_PREFIX + "APP__B"]: KEY },
    "app.a",
    "app.b",
    "/_bynk/call/issue",
    contract,
    "1",
    body,
    "test",
  );
  return new Request("http://internal/_bynk/call/issue", {
    method: "POST",
    headers: { "content-type": "application/json", "X-Bynk-Contract": contract, "X-Bynk-Caller": "app.a", ...signed },
    body,
  });
}

// The contract the callee has now is answered.
let res = await worker.fetch(await call(CURRENT), env);
assert(res.status === 200, "the current contract → 200, got " + res.status);

// The older contract the lock records is answered too, with the current reply
// (a caller on it ignores the field it does not know).
res = await worker.fetch(await call(ACCEPTED_OLDER), env);
assert(res.status === 200, "the accepted older contract → 200, got " + res.status);
const reply: any = await res.json();
assert(reply.id === "r", "the older caller reads the receipt, got " + JSON.stringify(reply));

// A contract the lock does not record is refused, and the 409 still names the
// callee's own contract as the one expected.
res = await worker.fetch(await call(NEVER_AGREED), env);
assert(res.status === 409, "an unrecorded contract → 409, got " + res.status);
let body: any = await res.json();
assert(body.kind === "ContractMismatch" && body.expected === CURRENT, "409 names the current contract, got " + JSON.stringify(body));
assert(body.actual === NEVER_AGREED, "409 reports what arrived");

res = await worker.fetch(await call("0000000000000000"), env);
assert(res.status === 409, "a made-up contract → 409, got " + res.status);

// An absent header still fails closed.
res = await worker.fetch(await call(""), env);
assert(res.status === 409, "an empty contract header → 409, got " + res.status);

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

fn compile(
    tmp: &Path,
    name: &str,
    source: &str,
    lock: Option<String>,
) -> bynk_emit::project::ProjectOutput {
    let root = tmp.join(name);
    fs::create_dir_all(root.join("app")).unwrap();
    fs::write(root.join("app/b.bynk"), source).unwrap();
    let options = bynk_testkit::compile_options_single(&root)
        .target(BuildTarget::Workers)
        .contracts_lock(lock);
    match bynkc::compile_project(&options) {
        Ok(out) => out,
        Err(failure) => panic!(
            "compile {name}:\n{}",
            bynkc::render_project_errors(&failure.flatten())
        ),
    }
}

fn manifest(out: &bynk_emit::project::ProjectOutput) -> serde_json::Value {
    let doc = out
        .artefacts
        .docs
        .iter()
        .find(|(p, _)| p.ends_with("workers/app-b/bynk-contracts.json"))
        .expect("the callee records its contracts")
        .1
        .text();
    serde_json::from_str(&doc).unwrap()
}

fn provides(out: &bynk_emit::project::ProjectOutput) -> (String, String) {
    let m = manifest(out);
    (
        m["provides"]["issue"].as_str().unwrap().to_string(),
        m["provides_shapes"]["issue"].as_str().unwrap().to_string(),
    )
}

#[test]
fn a_callee_answers_the_older_contract_its_lock_records_and_no_other() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!("\n!!! ACCEPTED CONTRACTS VERIFICATION SKIPPED !!!\nno tsc runner.\n");
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! ACCEPTED CONTRACTS VERIFICATION SKIPPED !!!\n`node` not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-accepted-contracts-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);

    let v1 = compile(&tmp, "v1", CALLEE_V1, None);
    let (v1_hash, v1_shape) = provides(&v1);
    let other = compile(&tmp, "other", CALLEE_OTHER, None);
    let (other_hash, _) = provides(&other);

    let mut lock = bynk_project::contracts_lock::ContractsLock::new();
    lock.accept("app.b", "issue", &v1_shape);
    let v2 = compile(
        &tmp,
        "v2",
        CALLEE_V2,
        Some(bynk_project::contracts_lock::serialize(&lock)),
    );
    let (v2_hash, _) = provides(&v2);
    assert_ne!(v1_hash, v2_hash);
    assert_eq!(
        manifest(&v2)["accepts"]["issue"][0].as_str(),
        Some(v1_hash.as_str())
    );

    let run_dir = tmp.join("run");
    for (path, doc) in &v2.artefacts.docs {
        let target = run_dir.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, doc.text()).unwrap();
    }
    bynk_testkit::write_runtime_modules(&run_dir).unwrap();
    fs::write(
        run_dir.join("driver.ts"),
        DRIVER_TS
            .replace("__CURRENT__", &v2_hash)
            .replace("__V1__", &v1_hash)
            .replace("__OTHER__", &other_hash),
    )
    .unwrap();
    fs::write(run_dir.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
    fs::write(run_dir.join("package.json"), "{ \"type\": \"module\" }").unwrap();

    let (program, prefix) = &runner;
    let (ok, msg) = run(program, prefix, &["-p", "tsconfig.json"], &run_dir);
    assert!(ok, "tsc failed on the callee Worker:\n{msg}");
    let (ok, msg) = run("node", &[], &["js/driver.js"], &run_dir);
    assert!(
        ok && msg.contains("ALL OK"),
        "the accepted-contracts driver did not pass:\n{msg}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
