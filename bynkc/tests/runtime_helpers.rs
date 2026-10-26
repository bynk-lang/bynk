//! v0.9.2: behavioural unit tests for the emitted runtime's agent helpers,
//! separate from emission snapshots. A driver TypeScript program imports the
//! generated runtime and exercises `serialiseAgentKey`, `StateRegistry`,
//! `makeAgent` (bundle path), and `makeWorkersAgent` / `callDurableObjectMethod`
//! (workers path, against a fake Durable Object stub).
//!
//! Like the tsc-verification stage, this skips loudly when no TypeScript
//! toolchain is available; `BYNK_REQUIRE_TSC=1` turns the skip into a failure.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

/// Build a `Command` for `program`, routing through `cmd /C` on Windows so
/// npm's `.cmd` shims (`tsc.cmd`, `npx.cmd`) resolve — Rust's CreateProcess
/// deliberately refuses to run batch scripts directly (the BatBadBut
/// hardening), so a bare `Command::new("npx")` fails there.
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
    // `where` is the Windows counterpart of `which`.
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder)
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Returns a `tsc` invocation (program + prefix args) if one is available.
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

const DRIVER_TS: &str = r#"import {
  serialiseAgentKey,
  StateRegistry,
  makeAgent,
  makeWorkersAgent,
  callDurableObjectMethod,
  type DurableObjectNamespace,
  type DurableObjectStub,
} from "./__bynk_runtime/agent.js";
import type { DurableObjectState } from "./__bynk_runtime/storage.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) throw new Error("FAIL: " + msg);
}

// --- serialiseAgentKey ---
assert(serialiseAgentKey("abc") === "abc", "string key is itself");
assert(serialiseAgentKey(7) === "7", "number key is its JSON form");
assert(serialiseAgentKey(true) === "true", "bool key is its JSON form");
// Two semantically-equal records (different field order) serialise identically.
const k1 = serialiseAgentKey({ a: 1, b: "x" });
const k2 = serialiseAgentKey({ b: "x", a: 1 });
assert(k1 === k2, "equal records serialise identically regardless of order");

// --- StateRegistry ---
const reg = new StateRegistry<string>();
const s1 = reg.getOrCreate("k");
const s1again = reg.getOrCreate("k");
assert(s1 === s1again, "same key returns the same state");
const s2 = reg.getOrCreate("other");
assert(s1 !== s2, "different keys get different state");
await s1.storage.put("state", { count: 3 });
assert(((await reg.getOrCreate("k").storage.get<{ count: number }>("state"))!).count === 3, "writes persist per key");
reg.reset();
const s1fresh = reg.getOrCreate("k");
assert(s1fresh !== s1, "reset clears the registry");
assert((await s1fresh.storage.get("state")) === undefined, "fresh state after reset has no committed value");

// --- makeAgent bundle path ---
class Box {
  constructor(public state: DurableObjectState) {}
  async bump(deps: {}): Promise<number> {
    const cur = (await this.state.storage.get<{ n: number }>("state")) ?? { n: 0 };
    const next = cur.n + 1;
    await this.state.storage.put("state", { n: next });
    return next;
  }
}
const boxes = new StateRegistry<string>();
const make = (key: string) => makeAgent(boxes, undefined, key, (st) => new Box(st));
assert((await make("a").bump({})) === 1, "bundle agent first bump is 1");
assert((await make("a").bump({})) === 2, "bundle agent state persists across instantiations of the same key");
assert((await make("b").bump({})) === 1, "a different key starts fresh");

// --- makeWorkersAgent / callDurableObjectMethod against a fake DO stub ---
let lastUrl = "";
let lastBody: any = null;
const stub: DurableObjectStub = {
  async fetch(input: string, init?: any): Promise<Response> {
    lastUrl = input;
    lastBody = JSON.parse(init.body);
    // Echo back the method args + deps so we can assert routing.
    return new Response(JSON.stringify({ args: lastBody.args, deps: lastBody.deps }), {
      headers: { "content-type": "application/json" },
    });
  },
};
let idFromNameArg = "";
const ns: DurableObjectNamespace = {
  idFromName(name: string) {
    idFromNameArg = name;
    return name;
  },
  get(_id: unknown) {
    return stub;
  },
};

// #1678: the call returns raw JSON for the caller's wire codec to decode.
const direct = (await callDurableObjectMethod(
  stub,
  "increment",
  [1, 2],
  { token: "t" },
)) as { args: unknown[]; deps: unknown };
assert(lastUrl === "https://_bynk/_bynk/agent/increment", "callDurableObjectMethod posts to the agent wire path");
assert(JSON.stringify(direct.args) === "[1,2]", "args round-trip");
assert((direct.deps as any).token === "t", "deps round-trip");

interface Counter {
  increment(a: number, b: number, deps: unknown): Promise<{ args: unknown[]; deps: unknown }>;
}
const proxy = makeWorkersAgent<Counter>(ns, "key-1");
assert(idFromNameArg === "key-1", "workers agent derives the DO id from the serialised key");
const viaProxy = await proxy.increment(5, 6, { d: 1 });
assert(JSON.stringify(viaProxy.args) === "[5,6]", "proxy forwards all but the final argument as method args");
assert((viaProxy.deps as any).d === 1, "proxy treats the final argument as deps");

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

#[test]
fn runtime_agent_helpers_behaviour() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! RUNTIME-HELPER VERIFICATION SKIPPED !!!\nneither `tsc` nor `npx` is on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! RUNTIME-HELPER VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-runtime-helpers-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    bynk_testkit::write_runtime_modules(&tmp).unwrap();
    fs::write(tmp.join("driver.ts"), DRIVER_TS).unwrap();
    fs::write(tmp.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
    // ESM so the driver may use top-level `await`; node resolves the nearest
    // package.json, making the compiled `js/*.js` ES modules too.
    fs::write(tmp.join("package.json"), "{ \"type\": \"module\" }").unwrap();

    let (program, prefix) = &runner;
    let (ok, out) = run(program, prefix, &["-p", "tsconfig.json"], &tmp);
    assert!(ok, "tsc failed on the runtime-helper driver:\n{out}");

    let (ok, out) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ok && out.contains("ALL OK"),
        "runtime-helper driver did not pass:\n{out}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
