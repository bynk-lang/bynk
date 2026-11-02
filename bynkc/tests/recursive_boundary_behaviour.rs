//! #2704: behavioural proof that a recursive type is bounded on every path a
//! value crosses a boundary, not only the HTTP body #2656 proved
//! (`http_recursive_body_behaviour.rs`).
//!
//! Two fixtures are compiled for `workers`, then driven by Node with real
//! `Request`s to the emitted Workers `fetch`:
//!
//! - `2704_recursive_event_payload`: a `/_bynk/call/` argument nested 257 levels
//!   deep is a `400` `StructuralMismatch`, and one at the bound is answered. An
//!   event payload carrying a recursive field is decoded by the *subscriber's*
//!   `/_bynk/event/` route, which refuses one past the bound the same way and
//!   accepts one at it, and a payload far deeper than the host's stack allows
//!   is refused rather than faulting. The compile also asserts the subscribing
//!   context's listener is emitted, since the issue recorded that the fixture
//!   harness had not emitted it.
//! - `2704_recursive_cycle_across_files`: type cycles that cross two files of a
//!   commons and two files of a context, carried over a context boundary. The
//!   callee refuses a value past the bound at each, and so does a *caller*,
//!   which decodes the callee's reply with a codec of its own (`client.rs`
//!   emits it from the caller's view of the type table): a callee answering
//!   one level past the bound is refused by the caller, and one at the bound is
//!   accepted. A caller codec that dropped the `depth` argument would accept
//!   both.
//!
//! Like the tsc-verification stage, this skips loudly when no TypeScript
//! toolchain is available; `BYNK_REQUIRE_TSC=1` turns the skip into a failure.

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

/// The toolchain, or `None` after a loud skip.
fn toolchain(what: &str) -> Option<(String, Vec<String>)> {
    let Some(runner) = discover_tsc() else {
        eprintln!("\n!!! {what} SKIPPED !!!\nno tsc runner on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but no tsc runner was found");
        }
        return None;
    };
    if !tool_exists("node") {
        eprintln!("\n!!! {what} SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return None;
    }
    Some(runner)
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A fixture compiled for `workers` out of a scratch copy (an events project
/// writes `bynk.schema.lock` into its input tree).
struct Compiled {
    tmp: PathBuf,
    out: bynkc::ProjectOutput,
}

fn compile(fixture: &str, tag: &str) -> Compiled {
    let tmp = std::env::temp_dir().join(format!("bynk-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let src = tmp.join("src");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/positive")
            .join(fixture)
            .join("src"),
        &src,
    );
    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_single(src).target(bynkc::BuildTarget::Workers),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .unwrap_or_else(|e| {
        panic!(
            "{fixture} must compile:\n{}",
            bynkc::render_project_errors(&e)
        )
    });
    Compiled { tmp, out }
}

impl Compiled {
    fn doc(&self, suffix: &str) -> String {
        self.out
            .artefacts
            .docs
            .iter()
            .find(|(p, _)| p.to_string_lossy().ends_with(suffix))
            .unwrap_or_else(|| panic!("no emitted `{suffix}`"))
            .1
            .text()
    }

    /// The contract hash `workers/<context_dir>/index.ts` enforces for
    /// `service`, read out of the entry rather than the manifest: for a service
    /// naming a consumed context's type the manifest's `provides` hash differs
    /// from it (#2750).
    fn contract(&self, context_dir: &str, service: &str) -> String {
        let entry = self.doc(&format!("workers/{context_dir}/index.ts"));
        let marker = format!("{{ service: \"{service}\", expected: \"");
        let at = entry
            .find(&marker)
            .unwrap_or_else(|| panic!("{context_dir} enforces no contract for `{service}`"))
            + marker.len();
        entry[at..].split('"').next().unwrap().to_string()
    }

    /// Stage the emitted tree and `driver`, type-check it and run it.
    fn drive(&self, runner: &(String, Vec<String>), driver: &str, what: &str) {
        let run_dir = self.tmp.join("run");
        for (path, doc) in &self.out.artefacts.docs {
            let target = run_dir.join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(&target, doc.text()).unwrap();
        }
        bynk_testkit::write_runtime_modules(&run_dir).unwrap();
        fs::write(run_dir.join("driver.ts"), driver).unwrap();
        fs::write(run_dir.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
        fs::write(run_dir.join("package.json"), "{ \"type\": \"module\" }").unwrap();

        let (program, prefix) = runner;
        let (ok, log) = run(program, prefix, &["-p", "tsconfig.json"], &run_dir);
        assert!(ok, "{what}: tsc failed on the emitted tree:\n{log}");
        let (ran, log) = run("node", &[], &["js/driver.js"], &run_dir);
        assert!(
            ran && log.contains("ALL OK"),
            "{what}: the driver must run green:\n{log}"
        );
        let _ = fs::remove_dir_all(&self.tmp);
    }
}

fn bound() -> String {
    bynk_check::wire::MAX_RECURSIVE_NESTING.to_string()
}

const EVENT_DRIVER_TS: &str = r#"import provider from "./workers/rec-provider/index.js";
import listener from "./workers/rec-listener/index.js";
import { EDGE_TO_PREFIX, signedHeaders } from "./__bynk_runtime/boundary.js";

const BOUND = __BOUND__;
const TREES = "__TREES__";

function assert(cond: boolean, msg: string): void {
  if (!cond) throw new Error(`assertion failed: ${msg}`);
}

const KEY = "edge-key";
const providerEnv: any = { BYNK_CALL_FROM_DRIVER: KEY };
const listenerEnv: any = { BYNK_CALL_FROM_REC__PROVIDER: KEY };

// The JSON text of a `Tree` `levels` values deep, built without recursion: a
// left spine of `Node`s ending in a `Leaf`.
function tree(levels: number): string {
  let s = `{"kind":"Leaf"}`;
  for (let i = 1; i < levels; i++) s = `{"kind":"Node","left":${s},"value":1,"right":{"kind":"Leaf"}}`;
  return s;
}

function levelsOf(t: any): number {
  let n = 1;
  while (t.kind === "Node") {
    n++;
    t = t.left;
  }
  return n;
}

const DEEP_PATH = ".left".repeat(BOUND);

async function call(args: string): Promise<Response> {
  const signed = await signedHeaders({ [EDGE_TO_PREFIX + "REC__PROVIDER"]: KEY }, "driver", "rec.provider", "/_bynk/call/trees", TREES, "1", args, "test");
  return provider.fetch(
    new Request("http://internal/_bynk/call/trees", {
      method: "POST",
      headers: { "content-type": "application/json", "X-Bynk-Contract": TREES, ...signed },
      body: args,
    }),
    providerEnv,
  );
}

async function deliver(payload: string): Promise<Response> {
  const body = `{"payload":{"tree":${payload}},"envelope":{"eventId":"e1","publisherId":"rec.provider","emittedAt":1700000000000,"schemaVersion":1}}`;
  const signed = await signedHeaders({ [EDGE_TO_PREFIX + "REC__LISTENER"]: KEY }, "rec.provider", "rec.listener", "/_bynk/event/OnPlanted", "", "1", body, "test");
  return listener.fetch(
    new Request("http://internal/_bynk/event/OnPlanted", {
      method: "POST",
      headers: { "content-type": "application/json", ...signed },
      body,
    }),
    listenerEnv,
  );
}

async function expectDepthError(r: Response, path: string, what: string): Promise<void> {
  const text = await r.text();
  assert(r.status === 400, `${what} is 400, got ${r.status}: ${text.slice(0, 300)}`);
  const err = JSON.parse(text) as { kind?: string; path?: string; expected?: string };
  assert(err.kind === "StructuralMismatch", `${what} is a StructuralMismatch: ${text.slice(0, 300)}`);
  assert(err.path === path, `${what} is refused at the level that crosses the bound: ${text.slice(0, 300)}`);
  assert(err.expected === `a value nested at most ${BOUND} levels deep`, `${what} names the bound: ${text.slice(0, 300)}`);
}

async function main(): Promise<void> {
  // /_bynk/call/: the argument and the result.
  const ok = await call(tree(BOUND));
  assert(ok.status === 200, `a call argument at the bound is answered, got ${ok.status}`);
  assert(levelsOf(await ok.json()) === BOUND, `the callee returns all ${BOUND} levels`);
  await expectDepthError(await call(tree(BOUND + 1)), "$" + DEEP_PATH, "a call argument one past the bound");
  // Deep enough to overflow the decoder's stack before #2656.
  await expectDepthError(await call(tree(5000)), "$" + DEEP_PATH, "a call argument far past the bound");

  // /_bynk/event/: the subscriber's own decoder of the payload.
  const delivered = await deliver(tree(BOUND));
  assert(delivered.status === 204, `an event payload at the bound is delivered, got ${delivered.status}`);
  await expectDepthError(await deliver(tree(BOUND + 1)), "$.payload.tree" + DEEP_PATH, "an event payload one past the bound");
  await expectDepthError(await deliver(tree(5000)), "$.payload.tree" + DEEP_PATH, "an event payload far past the bound");

  console.log("ALL OK");
}

main().catch((e: unknown) => {
  console.error(e);
  throw e;
});
"#;

#[test]
fn call_arguments_and_event_payloads_nested_past_the_bound_are_a_400() {
    let Some(runner) = toolchain("RECURSIVE BOUNDARY (CALL, EVENT) VERIFICATION") else {
        return;
    };
    let c = compile("2704_recursive_event_payload", "recursive-event-payload");

    // The issue recorded that the fixture harness did not emit a subscribing
    // context's listener. It does, given a publisher that exports the event
    // (without `exports transparent { Planted }` the subscriber cannot name
    // the type, and the project does not compile); assert it so a change to
    // either path shows.
    let entry = c.doc("workers/rec-listener/index.ts");
    assert!(
        entry.contains("case \"OnPlanted\"") && entry.contains("handlers.__deserialise_Planted("),
        "the subscribing context must emit its listener and payload decoder:\n{entry}"
    );
    assert!(
        c.doc("workers/rec-listener/handlers.ts")
            .contains("__deserialise_Tree(obj[\"tree\"], `${path}.tree`)"),
        "the payload decoder must call the recursive Tree decoder"
    );

    let driver = EVENT_DRIVER_TS
        .replace("__BOUND__", &bound())
        .replace("__TREES__", &c.contract("rec-provider", "trees"));
    c.drive(&runner, &driver, "call and event payload");
}

const CYCLE_DRIVER_TS: &str = r#"import graph from "./workers/cyc-graph/index.js";
import client from "./workers/cyc-client/index.js";
import { EDGE_TO_PREFIX, signedHeaders } from "./__bynk_runtime/boundary.js";

const BOUND = __BOUND__;
const CONTRACTS: { [service: string]: string } = {
  dirs: "__DIRS__",
  entries: "__ENTRIES__",
  exprs: "__EXPRS__",
  relayDir: "__RELAY_DIR__",
  relayExpr: "__RELAY_EXPR__",
};

function assert(cond: boolean, msg: string): void {
  if (!cond) throw new Error(`assertion failed: ${msg}`);
}

const KEY = "edge-key";
const graphEnv: any = { BYNK_CALL_FROM_DRIVER: KEY, BYNK_CALL_FROM_CYC__CLIENT: KEY };
const clientEnv: any = {
  BYNK_CALL_FROM_DRIVER: KEY,
  BYNK_CALL_TO_CYC__GRAPH: KEY,
  CYC_GRAPH: { fetch: (req: Request) => graph.fetch(req, graphEnv) },
};

// A value `n` named-type levels deep, built without recursion. Levels alternate
// between the two types of a cycle, the first being `first`, and the value at
// level `i` is at nesting depth `i`.
//   Dir/Entry:  Dir = { name, entries: [Entry] },  Entry = File | Sub(dir)
//   Expr/Stmt:  Expr = Lit | Block(stmts: [Stmt]), Stmt = Skip | Eval(expr)
function dirChain(n: number, startsWithDir: boolean): string {
  const isDir = (i: number): boolean => (i % 2 === 0) === startsWithDir;
  let s = isDir(n - 1) ? `{"name":"d","entries":[]}` : `{"kind":"File","name":"f"}`;
  for (let i = n - 2; i >= 0; i--) s = isDir(i) ? `{"name":"d","entries":[${s}]}` : `{"kind":"Sub","dir":${s}}`;
  return s;
}

function exprChain(n: number): string {
  const isExpr = (i: number): boolean => i % 2 === 0;
  let s = isExpr(n - 1) ? `{"kind":"Lit","value":1}` : `{"kind":"Skip"}`;
  for (let i = n - 2; i >= 0; i--) s = isExpr(i) ? `{"kind":"Block","stmts":[${s}]}` : `{"kind":"Eval","expr":${s}}`;
  return s;
}

function dirLevels(v: any, startsWithDir: boolean): number {
  let n = 0;
  let isDir = startsWithDir;
  for (;;) {
    n++;
    if (isDir) {
      if (v.entries.length === 0) return n;
      v = v.entries[0];
    } else {
      if (v.kind === "File") return n;
      v = v.dir;
    }
    isDir = !isDir;
  }
}

function exprLevels(v: any): number {
  let n = 0;
  let isExpr = true;
  for (;;) {
    n++;
    if (isExpr) {
      if (v.kind === "Lit") return n;
      v = v.stmts[0];
    } else {
      if (v.kind === "Skip") return n;
      v = v.expr;
    }
    isExpr = !isExpr;
  }
}

async function post(worker: any, env: any, callee: string, envKey: string, service: string, body: string): Promise<Response> {
  const path = `/_bynk/call/${service}`;
  const signed = await signedHeaders({ [EDGE_TO_PREFIX + envKey]: KEY }, "driver", callee, path, CONTRACTS[service], "1", body, "test");
  return worker.fetch(
    new Request(`http://internal${path}`, {
      method: "POST",
      headers: { "content-type": "application/json", "X-Bynk-Contract": CONTRACTS[service], ...signed },
      body,
    }),
    env,
  );
}

const toGraph = (service: string, body: string) => post(graph, graphEnv, "cyc.graph", "CYC__GRAPH", service, body);
const toClient = (service: string, body: string) => post(client, clientEnv, "cyc.client", "CYC__CLIENT", service, body);

// Each repetition is two levels, so the paths below need an even bound.
assert(BOUND % 2 === 0, "the bound is even");
const DIR_PATH =".entries[0].dir".repeat(BOUND / 2);
const ENTRY_PATH = ".dir.entries[0]".repeat(BOUND / 2);
const EXPR_PATH = ".stmts[0].expr".repeat(BOUND / 2);

async function expectDepthError(r: Response, path: string, what: string): Promise<void> {
  const text = await r.text();
  assert(r.status === 400, `${what} is 400, got ${r.status}: ${text.slice(0, 300)}`);
  const err = JSON.parse(text) as { kind?: string; path?: string; expected?: string };
  assert(err.kind === "StructuralMismatch", `${what} is a StructuralMismatch: ${text.slice(0, 300)}`);
  assert(err.path === path, `${what}: expected the error at ${path}, got ${text.slice(0, 300)}`);
  assert(err.expected === `a value nested at most ${BOUND} levels deep`, `${what} names the bound: ${text.slice(0, 300)}`);
}

async function main(): Promise<void> {
  // The callee, directly. `Dir`/`Entry` cross two files of a context, `Expr`/
  // `Stmt` two files of a commons, and the commons cycle is read from a context.
  let r = await toGraph("dirs", dirChain(BOUND, true));
  assert(r.status === 200, `a Dir/Entry chain at the bound is answered, got ${r.status}`);
  assert(dirLevels(await r.json(), true) === BOUND, `all ${BOUND} levels of the Dir chain come back`);
  await expectDepthError(await toGraph("dirs", dirChain(BOUND + 1, true)), "$" + DIR_PATH, "a Dir chain one past the bound");

  r = await toGraph("entries", dirChain(BOUND, false));
  assert(r.status === 200, `an Entry/Dir chain at the bound is answered, got ${r.status}`);
  assert(dirLevels((await r.json()) as any, false) === BOUND, `all ${BOUND} levels of the Entry chain come back`);
  await expectDepthError(await toGraph("entries", dirChain(BOUND + 1, false)), "$" + ENTRY_PATH, "an Entry chain one past the bound");

  r = await toGraph("exprs", exprChain(BOUND));
  assert(r.status === 200, `an Expr/Stmt chain at the bound is answered, got ${r.status}`);
  assert(exprLevels(await r.json()) === BOUND, `all ${BOUND} levels of the Expr chain come back`);
  await expectDepthError(await toGraph("exprs", exprChain(BOUND + 1)), "$" + EXPR_PATH, "an Expr chain one past the bound");

  // The caller's own codecs, end to end: the caller decodes the argument,
  // encodes it for the callee, and decodes the callee's reply.
  r = await toClient("relayDir", dirChain(BOUND, true));
  assert(r.status === 200, `a Dir chain at the bound relays, got ${r.status}`);
  assert(dirLevels(await r.json(), true) === BOUND, `all ${BOUND} levels of the Dir chain survive the relay`);
  await expectDepthError(await toClient("relayDir", dirChain(BOUND + 1, true)), "$" + DIR_PATH, "a relayed Dir chain one past the bound");

  r = await toClient("relayExpr", exprChain(BOUND));
  assert(r.status === 200, `an Expr chain at the bound relays, got ${r.status}`);
  assert(exprLevels(await r.json()) === BOUND, `all ${BOUND} levels of the Expr chain survive the relay`);
  await expectDepthError(await toClient("relayExpr", exprChain(BOUND + 1)), "$" + EXPR_PATH, "a relayed Expr chain one past the bound");

  // The caller's decoder of the callee's *reply*. A callee Bynk compiled never
  // answers past the bound (its encoder throws), so the callee here is a stub
  // that answers whatever it is told to. The caller's reply decoder must
  // refuse one level past the bound and accept the bound itself: a decoder
  // that dropped `depth` between two of its own codecs would accept both.
  let reply = "";
  const stub = (req: Request): Promise<Response> =>
    Promise.resolve(new Response(reply, { status: 200, headers: { "content-type": "application/json" } }));
  const stubEnv: any = { ...clientEnv, CYC_GRAPH: { fetch: stub } };
  const viaStub = async (service: string, arg: string): Promise<Response> => {
    const body = arg;
    const signed = await signedHeaders({ [EDGE_TO_PREFIX + "CYC__CLIENT"]: KEY }, "driver", "cyc.client", `/_bynk/call/${service}`, CONTRACTS[service], "1", body, "test");
    return client.fetch(
      new Request(`http://internal/_bynk/call/${service}`, {
        method: "POST",
        headers: { "content-type": "application/json", "X-Bynk-Contract": CONTRACTS[service], ...signed },
        body,
      }),
      stubEnv,
    );
  };
  // The fault is logged by the entry's catch; capture it rather than let it
  // print, and require it to be a `StructuralMismatch` (the reply decoder's
  // refusal; the runtime logs the kind but not the message, which can carry
  // the value), so that a 500 from some other cause (a stub, contract or
  // header mistake: `ContractMismatch`, `Transport`) does not pass.
  const quiet = globalThis.console.error;
  let logged: string[] = [];
  globalThis.console.error = (...args: unknown[]) => {
    logged.push(args.map((a) => (a instanceof Error ? `${a.message} ${a.stack ?? ""} ${JSON.stringify(a)}` : typeof a === "string" ? a : JSON.stringify(a))).join(" "));
  };
  const refusedAsMismatch = (): boolean => logged.some((l) => l.includes("BoundaryError: StructuralMismatch"));
  try {
    reply = dirChain(BOUND, true);
    r = await viaStub("relayDir", dirChain(1, true));
    assert(r.status === 200, `a reply at the bound is accepted by the caller, got ${r.status}`);
    reply = dirChain(BOUND + 1, true);
    logged = [];
    r = await viaStub("relayDir", dirChain(1, true));
    assert(r.status === 500, `a Dir reply one past the bound is refused by the caller, got ${r.status}`);
    assert(refusedAsMismatch(), `the Dir refusal is a StructuralMismatch, logged: ${JSON.stringify(logged).slice(0, 400)}`);

    reply = exprChain(BOUND);
    r = await viaStub("relayExpr", exprChain(1));
    assert(r.status === 200, `an Expr reply at the bound is accepted by the caller, got ${r.status}`);
    reply = exprChain(BOUND + 1);
    logged = [];
    r = await viaStub("relayExpr", exprChain(1));
    assert(r.status === 500, `an Expr reply one past the bound is refused by the caller, got ${r.status}`);
    assert(refusedAsMismatch(), `the Expr refusal is a StructuralMismatch, logged: ${JSON.stringify(logged).slice(0, 400)}`);
  } finally {
    globalThis.console.error = quiet;
  }

  console.log("ALL OK");
}

main().catch((e: unknown) => {
  console.error(e);
  throw e;
});
"#;

#[test]
fn cycles_crossing_files_commons_and_contexts_are_bounded_at_every_site() {
    let Some(runner) = toolchain("RECURSIVE CYCLE (CROSS-FILE) VERIFICATION") else {
        return;
    };
    let c = compile("2704_recursive_cycle_across_files", "recursive-cycle-files");
    let driver = CYCLE_DRIVER_TS
        .replace("__BOUND__", &bound())
        .replace("__DIRS__", &c.contract("cyc-graph", "dirs"))
        .replace("__ENTRIES__", &c.contract("cyc-graph", "entries"))
        .replace("__EXPRS__", &c.contract("cyc-graph", "exprs"))
        .replace("__RELAY_DIR__", &c.contract("cyc-client", "relayDir"))
        .replace("__RELAY_EXPR__", &c.contract("cyc-client", "relayExpr"));
    c.drive(&runner, &driver, "cycles across files");
}
