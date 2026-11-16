//! A conformance harness for a call or an event between two contexts (#3562,
//! G3-0, tests only).
//!
//! One project (a caller, a callee, a publisher and a subscriber) is built
//! once per *topology* and driven through the same scenarios under Node, with a
//! Service Binding stood in for by a function that calls the target Worker's
//! `fetch`. What a topology may change is how a call travels; what a caller, a
//! subscriber and a client can see must not change. The topologies today:
//!
//! - `separate`: one Worker per context, reached over a Service Binding;
//! - `grouped`: all four contexts in one `[deploy.workers.core]` Worker, where an
//!   intra-group edge is a Service Binding to the Worker itself.
//!
//! G3-2 makes a call between a group's members in-process. It adds a third
//! entry to [`TOPOLOGIES`] and these scenarios decide whether it conforms: the
//! same status, body and logs, the same refusals, the same fault mapping. The
//! `wire` record (which Service Binding each call went over) is the one thing a
//! topology is *expected* to change, so [`assert_topologies_agree`] leaves it out
//! and a topology that makes calls in-process states that it has no wire.
//!
//! Scenarios:
//!
//! - a call that succeeds;
//! - a callee that faults: the caller sees `CalleeFaulted` and the client a bare
//!   `500`, and nothing of the callee's own exception reaches either. The
//!   callee's *own* log does carry it, which is the control that makes the
//!   absence in the others mean something;
//! - a callee whose handler admits only a foreign caller (`by Legacy`), called by
//!   a context: refused with a bare `403`, which the caller reports as
//!   `Transport`, never as a fault of the callee, and again with nothing of the
//!   callee's internals. The refusal is asserted both through the caller and on
//!   the callee's own `/_bynk/call/` door;
//! - an event delivered to its subscriber, and one whose subscriber faults: the
//!   publisher's emit is unaffected, the fan-out logs only the status, and the
//!   subscriber's exception stays in the subscriber's log.
//!
//! Needs `tsc` and `node`; skips loudly without them, and `BYNK_REQUIRE_TSC=1`
//! (CI) turns the skip into a failure.

#[path = "../require/mod.rs"]
mod require;
#[path = "../tools/mod.rs"]
mod tools;

use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::OnceLock;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

/// The text of the callee's own exception (`Int` division by zero). It must be in
/// the callee's log and in no one else's.
const INTERNAL: &str = "division by zero";

/// A topology: its name, the manifest it builds with, and the entry module that
/// serves each context (a group serves all four from one).
struct Topology {
    name: &'static str,
    manifest: &'static str,
}

const TOPOLOGIES: &[Topology] = &[
    Topology {
        name: "separate",
        manifest: "[build]\ntarget = \"workers\"\n",
    },
    Topology {
        name: "grouped",
        manifest: "[build]\ntarget = \"workers\"\n\n[deploy.workers.core]\ncontexts = [\"app.a\", \"app.b\", \"app.pub\", \"app.sub\"]\n",
    },
];

const CALLER: &str = r#"context app.a

consumes app.b as B
consumes app.pub as Pub

service api from http {
  on GET("/ok/:n") (n: String) -> Effect[HttpResult[String]] by v: Visitor {
    match Int.parse(n) {
      Some(i) => {
        let r <- B.ok(i)
        Ok("\(r)")
      },
      None => BadRequest("not an int"),
    }
  }
  on GET("/boom/:n") (n: String) -> Effect[HttpResult[String]] by v: Visitor {
    match Int.parse(n) {
      Some(i) => {
        let r <- B.boom(i)
        Ok("\(r)")
      },
      None => BadRequest("not an int"),
    }
  }
  on GET("/legacy/:n") (n: String) -> Effect[HttpResult[String]] by v: Visitor {
    match Int.parse(n) {
      Some(i) => {
        let r <- B.only_legacy(i)
        Ok("\(r)")
      },
      None => BadRequest("not an int"),
    }
  }
  on GET("/place/:n") (n: String) -> Effect[HttpResult[String]] by v: Visitor {
    match Int.parse(n) {
      Some(i) => {
        let _ <- Pub.place(i)
        Ok("placed")
      },
      None => BadRequest("not an int"),
    }
  }
}
"#;

const CALLEE: &str = r#"context app.b

actor Legacy { auth = Foreign(secret = "LEGACY_CALL_KEY") }

service ok {
  on call(n: Int) -> Effect[Int] {
    Effect.pure(n + 1)
  }
}

service boom {
  on call(n: Int) -> Effect[Int] {
    Effect.pure(n / 0)
  }
}

service only_legacy {
  on call(n: Int) -> Effect[Int] by Legacy {
    Effect.pure(n)
  }
}
"#;

const PUBLISHER: &str = r#"context app.pub

exports transparent { Placed }
consumes bynk { Events }

event Placed = {
  n: Int,
}

service place {
  on call(n: Int) -> Effect[()] given Events {
    Events.emit[Placed](Placed { n: n })
  }
}
"#;

/// Faults when `n` is `0`, the subscriber's own exception.
const SUBSCRIBER: &str = r#"context app.sub

consumes app.pub

service OnPlaced from Events(Placed) {
  on event(e: Placed) -> Effect[()] {
    let r <- Effect.pure(100 / e.n)
    Effect.pure(())
  }
}
"#;

/// Runs every scenario against one topology (`argv[2]`: `grouped` or
/// `separate`) and prints the observations as one JSON line. A Service Binding is
/// a function over the target's `fetch`, and each call across one is recorded in
/// `wire`. A log line is attributed to the binding it was written under
/// (`client` when under none), and cut at its stack.
const DRIVER: &str = r#"import { AsyncLocalStorage } from "node:async_hooks";
import { signedHeaders } from "./out-js/__bynk_runtime/boundary.js";

const grouped = process.argv[2] === "grouped";
const KEY = "edge-key";
const CONTEXTS = ["app.a", "app.b", "app.pub", "app.sub"];
const upper = (c) => c.toUpperCase().replace(".", "__");
const binding = (c) => c.toUpperCase().replace(".", "_");

// Which binding a log line was written under follows the call it ran in, even
// when that work outlives the call that started it (a `waitUntil`).
const owner = new AsyncLocalStorage();
const logs = [];
const wire = [];
const show = (a) => typeof a === "string" ? a : a instanceof Error ? (a.stack ?? String(a)) : JSON.stringify(a);
console.error = (...args) => {
  // `raw` keeps any stack, for what must not leak; `text` is cut at it, which is
  // what topologies are compared on (their file names differ).
  const raw = args.map(show).join(" ");
  logs.push({ owner: owner.getStore() ?? "client", text: raw.split("\n    at ")[0], raw });
};
console.log = () => {};
const pending = [];
const ctx = { waitUntil(p) { pending.push(p); } };

const dir = (c) => c.replace(".", "-");
let Fanout;
const entries = {};
if (grouped) {
  const core = await import("./out-js/workers/core/index.js");
  for (const c of CONTEXTS) entries[c] = core.default;
  Fanout = core.__EventsFanout;
} else {
  for (const c of CONTEXTS) entries[c] = (await import(`./out-js/workers/${dir(c)}/index.js`)).default;
  Fanout = (await import("./out-js/workers/app-pub/index.js")).__EventsFanout;
}

// Every context holds the key for every edge, so a refusal is the callee's own.
const keys = {};
for (const c of CONTEXTS) {
  keys["BYNK_CALL_TO_" + upper(c)] = KEY;
  keys["BYNK_CALL_FROM_" + upper(c)] = KEY;
}
const envs = {};
const over = (name, target, run) => async (request) => {
  const record = { to: name, path: new URL(request.url).pathname };
  wire.push(record);
  const response = await owner.run(name, () => run(request));
  record.status = response.status;
  return response;
};
for (const c of CONTEXTS) {
  envs[c] = { ...keys };
  for (const other of CONTEXTS) {
    if (other === c) continue;
    envs[c][binding(other)] = { fetch: over(binding(other), other, (r) => entries[other].fetch(r, envs[other], ctx)) };
  }
}
if (grouped) {
  // One Worker, one `env`: a context's bindings are the Worker's.
  const shared = envs["app.a"];
  for (const c of CONTEXTS) {
    for (const other of CONTEXTS) shared[binding(other)] = shared[binding(other)] ?? { fetch: over(binding(other), other, (r) => entries[other].fetch(r, shared, ctx)) };
    envs[c] = shared;
  }
}
const fanout = new Fanout({}, envs["app.pub"]);
envs["app.pub"].___EVENTS_FANOUT = {
  idFromName: (n) => n,
  // A Durable Object stub takes a URL and an init, as the runtime calls it.
  get: () => ({ fetch: (url, init) => over("FANOUT", "app.pub", (r) => fanout.fetch(r))(new Request("http://fanout/_bynk/fanout", init)) }),
};
if (grouped) for (const c of CONTEXTS) envs[c].___EVENTS_FANOUT = envs["app.pub"].___EVENTS_FANOUT;

const results = [];
const scenario = async (label, run) => {
  logs.length = 0;
  wire.length = 0;
  const seen = await run();
  // Work a handler left to `waitUntil`, such as an event's fan-out, finishes
  // before the scenario is read.
  await Promise.allSettled(pending.splice(0));
  results.push({ label, ...seen, logs: logs.map((l) => ({ ...l })), wire: wire.map((w) => ({ ...w })) });
};
const viaClient = async (path) => {
  const r = await entries["app.a"].fetch(new Request("http://x" + path), envs["app.a"], ctx);
  return { status: r.status, body: await r.text(), headers: [...r.headers.keys()].sort() };
};

await scenario("call-ok", () => viaClient("/ok/1"));
await scenario("call-fault", () => viaClient("/boom/1"));
await scenario("call-refused", () => viaClient("/legacy/1"));
await scenario("door-refused", async () => {
  const path = "/_bynk/call/only_legacy";
  const body = JSON.stringify(1);
  const signed = await signedHeaders({ BYNK_CALL_TO_APP__B: KEY }, "app.a", "app.b", path, "", "1", body, "test");
  const r = await entries["app.b"].fetch(new Request("http://internal" + path, { method: "POST", headers: { "content-type": "application/json", ...signed }, body }), envs["app.b"], ctx);
  return { status: r.status, body: await r.text(), headers: [...r.headers.keys()].sort() };
});
await scenario("event-delivered", () => viaClient("/place/5"));
await scenario("event-faulted", () => viaClient("/place/0"));
console.info(JSON.stringify(results));
"#;

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn stage(topology: &Topology) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bynk-conformance-{}-{}",
        topology.name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src/app")).unwrap();
    fs::write(root.join("bynk.toml"), topology.manifest).unwrap();
    for (file, source) in [
        ("a", CALLER),
        ("b", CALLEE),
        ("pub", PUBLISHER),
        ("sub", SUBSCRIBER),
    ] {
        fs::write(root.join(format!("src/app/{file}.bynk")), source).unwrap();
    }
    root
}

/// Build the topology, compile it, and run [`DRIVER`]: one observation per
/// scenario. `None` when the toolchain is missing and the test skips.
fn observe(topology: &Topology) -> Option<(PathBuf, Vec<Value>)> {
    let Some(mut tsc) = tools::tsc_command() else {
        tools::skip(
            "CROSS-CONTEXT CONFORMANCE (tsc)",
            require::is_required(REQUIRE_ENV),
            REQUIRE_ENV,
            "no TypeScript toolchain (`tsc` or `npx`) is on PATH",
        );
        return None;
    };
    if !tools::tool_exists("node") {
        tools::skip(
            "CROSS-CONTEXT CONFORMANCE (node)",
            require::is_required(REQUIRE_ENV),
            REQUIRE_ENV,
            "`node` is not on PATH",
        );
        return None;
    }
    let root = stage(topology);
    let built = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(["build", "."])
        .current_dir(&root)
        .env("NO_COLOR", "1")
        .output()
        .expect("bynk runs");
    assert!(
        built.status.success(),
        "{}: bynk build failed:\n{}",
        topology.name,
        text(&built)
    );
    let emitted = tsc
        .arg("-p")
        .arg(root.join("out/tsconfig.json"))
        .current_dir(&root)
        .output()
        .expect("run tsc");
    assert!(
        emitted.status.success(),
        "{}: tsc failed:\n{}",
        topology.name,
        text(&emitted)
    );
    fs::write(root.join("out-js/package.json"), "{\"type\":\"module\"}").unwrap();
    fs::write(root.join("run.mjs"), DRIVER).unwrap();
    let ran = Command::new("node")
        .arg("run.mjs")
        .arg(topology.name)
        .current_dir(&root)
        .output()
        .expect("run node");
    assert!(
        ran.status.success(),
        "{}: node failed:\n{}",
        topology.name,
        text(&ran)
    );
    let line = String::from_utf8_lossy(&ran.stdout).trim().to_string();
    let results = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
    Some((root, results))
}

/// Every topology observed once, however many tests read it: a build and `tsc`
/// each. `None` when the toolchain is missing and the tests skip.
fn observed() -> Option<&'static [(&'static str, Vec<Value>)]> {
    static OBSERVED: OnceLock<Option<Vec<(&'static str, Vec<Value>)>>> = OnceLock::new();
    OBSERVED
        .get_or_init(|| {
            let mut all = Vec::new();
            for topology in TOPOLOGIES {
                let (root, results) = observe(topology)?;
                let _ = fs::remove_dir_all(&root);
                all.push((topology.name, results));
            }
            Some(all)
        })
        .as_deref()
}

fn scenario<'a>(results: &'a [Value], label: &str) -> &'a Value {
    results
        .iter()
        .find(|r| r["label"] == label)
        .unwrap_or_else(|| panic!("no scenario `{label}`"))
}

/// The log lines written under `owner` (`client` for the caller's own).
fn logs_of(result: &Value, owner: &str) -> Vec<String> {
    result["logs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["owner"] == owner)
        .map(|l| l["text"].as_str().unwrap().to_string())
        .collect()
}

fn wire_of(result: &Value) -> Vec<(String, String, Option<u64>)> {
    result["wire"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            (
                w["to"].as_str().unwrap().to_string(),
                w["path"].as_str().unwrap().to_string(),
                w["status"].as_u64(),
            )
        })
        .collect()
}

/// Everything a client, a caller or a subscriber's publisher can see of a
/// scenario, with each log line whole (its stack too), leaving only what the
/// callee wrote to its own log out.
fn exposed(result: &Value, callee_owners: &[&str]) -> String {
    let mut out = format!(
        "{} {} {}",
        result["status"], result["body"], result["headers"]
    );
    for line in result["logs"].as_array().unwrap() {
        let owner = line["owner"].as_str().unwrap();
        if !callee_owners.contains(&owner) {
            out.push_str(&format!("\n{owner}: {}", line["raw"].as_str().unwrap()));
        }
    }
    out
}

/// Whether `seen` carries a callee's own exception: its message, the runtime
/// helper that raised it, or a frame from one of its modules (`frames`, as a
/// path part). The caller's own frames are its own, so a stack is not a leak
/// by itself.
fn leaks(seen: &str, frames: &[&str]) -> bool {
    seen.contains(INTERNAL)
        || seen.contains("__bynkIntDiv")
        || frames.iter().any(|f| seen.contains(f))
}

/// What a call must do whichever way it travels.
fn assert_conforms(topology: &str, results: &[Value]) {
    let at = |label: &str| format!("{topology}/{label}");

    // A call that succeeds.
    let ok = scenario(results, "call-ok");
    assert_eq!(
        (ok["status"].as_u64(), ok["body"].as_str()),
        (Some(200), Some("\"2\"")),
        "{}: {ok}",
        at("call-ok")
    );
    assert!(
        logs_of(ok, "client").is_empty() && logs_of(ok, "APP_B").is_empty(),
        "{}: a call that succeeds logs nothing: {ok}",
        at("call-ok")
    );

    // A callee that faults: `CalleeFaulted` for the caller, a bare `500` for the
    // client, and the callee's own exception in the callee's log alone.
    let fault = scenario(results, "call-fault");
    assert_eq!(fault["status"], 500, "{}: {fault}", at("call-fault"));
    assert_eq!(fault["body"], "Internal Server Error", "{fault}");
    let caller = logs_of(fault, "client");
    assert!(
        caller
            .iter()
            .any(|l| l == "CalleeFaulted app.a -> app.b call boom"),
        "{}: the caller names the callee's fault: {caller:?}",
        at("call-fault")
    );
    assert!(
        caller
            .iter()
            .any(|l| l == "app.a GET /boom/:n faulted Error: BoundaryError: CalleeFaulted"),
        "{}: the caller's handler faults with `CalleeFaulted`, not `Transport`: {caller:?}",
        at("call-fault")
    );
    assert!(
        logs_of(fault, "APP_B")
            .iter()
            .any(|l| l.contains(INTERNAL) && l.contains("app.b call boom faulted")),
        "{}: control: the callee's own log carries its exception: {fault}",
        at("call-fault")
    );
    // Control: the same check on the callee's own log does find its exception.
    assert!(
        leaks(&exposed(fault, &[]), &["app-b"]),
        "{}: the leak check can fail",
        at("call-fault")
    );
    let seen = exposed(fault, &["APP_B"]);
    assert!(
        !leaks(&seen, &["app-b"]),
        "{}: the callee's exception detail reached the caller or the client:\n{seen}",
        at("call-fault")
    );

    // A callee that does not admit the caller: a bare `403`, reported as
    // `Transport` and not as a fault of the callee.
    let refused = scenario(results, "call-refused");
    assert_eq!(refused["status"], 500, "{}: {refused}", at("call-refused"));
    assert_eq!(refused["body"], "Internal Server Error", "{refused}");
    let caller = logs_of(refused, "client");
    assert!(
        caller
            .iter()
            .any(|l| l == "app.a GET /legacy/:n faulted Error: BoundaryError: Transport"),
        "{}: a refusal is `Transport`: {caller:?}",
        at("call-refused")
    );
    assert!(
        !caller.iter().any(|l| l.contains("CalleeFaulted")),
        "{}: a refusal is not the callee's fault: {caller:?}",
        at("call-refused")
    );
    let seen = exposed(refused, &["APP_B"]);
    assert!(
        !seen.contains("only_legacy") && !seen.contains("Legacy"),
        "{}: the refusal names what the callee admits:\n{seen}",
        at("call-refused")
    );

    // The same refusal on the callee's own door.
    let door = scenario(results, "door-refused");
    assert_eq!(door["status"], 403, "{}: {door}", at("door-refused"));
    let body = door["body"].as_str().unwrap();
    assert!(
        !body.contains("Legacy") && !body.contains("only_legacy") && !body.contains(INTERNAL),
        "{}: a bare refusal: {body:?}",
        at("door-refused")
    );

    // An event reaches its subscriber, and a subscriber that faults is the
    // fan-out's to report, not the publisher's.
    let delivered = scenario(results, "event-delivered");
    assert_eq!(
        delivered["status"],
        200,
        "{}: {delivered}",
        at("event-delivered")
    );
    assert!(
        logs_of(delivered, "client").is_empty()
            && logs_of(delivered, "APP_SUB").is_empty()
            && logs_of(delivered, "FANOUT").is_empty(),
        "{}: a delivered event logs nothing: {delivered}",
        at("event-delivered")
    );
    let faulted = scenario(results, "event-faulted");
    assert_eq!(
        faulted["status"],
        200,
        "{}: a faulting subscriber does not fail the emit: {faulted}",
        at("event-faulted")
    );
    assert!(
        logs_of(faulted, "FANOUT")
            .iter()
            .any(|l| l.contains("EventsFanout delivery failed") && l.contains("500")),
        "{}: the fan-out reports the failed delivery: {faulted}",
        at("event-faulted")
    );
    assert!(
        logs_of(faulted, "APP_SUB")
            .iter()
            .any(|l| l.contains(INTERNAL)),
        "{}: control: the subscriber's own log carries its exception: {faulted}",
        at("event-faulted")
    );
    assert!(
        leaks(&exposed(faulted, &[]), &["app-sub"]),
        "{}: the leak check can fail",
        at("event-faulted")
    );
    let seen = exposed(faulted, &["APP_SUB"]);
    assert!(
        !leaks(&seen, &["app-sub"]),
        "{}: the subscriber's exception reached the publisher or the client:\n{seen}",
        at("event-faulted")
    );
}

/// What a topology may change is the wire; the rest of every observation must be
/// the same in all of them.
fn assert_topologies_agree(all: &[(&str, Vec<Value>)]) {
    let (base_name, base) = &all[0];
    for (name, results) in &all[1..] {
        for result in base {
            let label = result["label"].as_str().unwrap();
            let other = scenario(results, label);
            let strip = |r: &Value| {
                let mut r = r.clone();
                r.as_object_mut().unwrap().remove("wire");
                for line in r["logs"].as_array_mut().unwrap() {
                    line.as_object_mut().unwrap().remove("raw");
                }
                r
            };
            assert_eq!(
                strip(result),
                strip(other),
                "`{label}` differs between `{base_name}` and `{name}`"
            );
        }
    }
}

#[test]
fn a_call_and_an_event_between_contexts_conform_in_every_topology() {
    let Some(all) = observed() else { return };
    for (name, results) in all {
        assert_conforms(name, results);
    }
    assert_topologies_agree(all);
}

/// Both topologies here go over a Service Binding: the call, the refusal, the
/// event delivery and the fan-out each appear on the wire, in this order. A
/// topology that makes a call in-process has no entry for it, and states so where
/// it is added.
#[test]
fn the_topologies_here_go_over_a_service_binding() {
    let Some(all) = observed() else { return };
    let wire = |entries: &[(&str, &str, u64)]| {
        entries
            .iter()
            .map(|(to, path, status)| (to.to_string(), path.to_string(), Some(*status)))
            .collect::<Vec<_>>()
    };
    for (name, results) in all {
        assert_eq!(
            wire_of(scenario(results, "call-ok")),
            wire(&[("APP_B", "/_bynk/call/ok", 200)]),
            "{name}"
        );
        assert_eq!(
            wire_of(scenario(results, "call-refused")),
            wire(&[("APP_B", "/_bynk/call/only_legacy", 403)]),
            "{name}"
        );
        assert_eq!(
            wire_of(scenario(results, "event-delivered")),
            wire(WIRE_EVENT),
            "{name}: the emit, the fan-out and the delivery"
        );
    }
}

const WIRE_EVENT: &[(&str, &str, u64)] = &[
    ("APP_PUB", "/_bynk/call/place", 200),
    ("FANOUT", "/_bynk/fanout", 204),
    ("APP_SUB", "/_bynk/event/OnPlaced", 204),
];
