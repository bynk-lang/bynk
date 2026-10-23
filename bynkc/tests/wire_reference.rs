//! #1969: the cross-context wire format, version 1, is a published contract
//! (`site/src/content/docs/book/reference/wire-format.md`). These guards keep
//! the page, the emitter and the runtime in agreement:
//!
//! - the committed page is exactly what `bynk_check::wire_format::reference`
//!   renders;
//! - the hashes the page shows for its example context are the constants the
//!   emitter stamps for it;
//! - the emitted `/_bynk/call/` dispatch makes the published checks in the
//!   published order;
//! - the runtime's `callService` sends the published method, path and headers;
//! - every example on the page, run through the codecs the compiler generates
//!   for the example context on Node, decodes and encodes as the page says, and
//!   the emitted dispatch answers each published failure with its published
//!   status and body.
//!
//! Regenerate the page after an intended change with:
//!     BYNK_BLESS=1 cargo test -p bynkc --test it wire_reference::
//! The Node guard skips loudly without `tsc` and `node`; `BYNK_REQUIRE_TSC=1`
//! turns the skip into a failure.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use bynk_check::wire_format::reference::{
    EXAMPLE_SOURCE, EXAMPLES, Outcome, example_contracts, render_reference,
};
use bynk_check::wire_format::{self as wf, CallCheck};
use bynkc::BuildTarget;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(rel)
}

#[test]
fn generated_wire_format_page_is_up_to_date() {
    let page = repo("site/src/content/docs/book/reference/wire-format.md");
    let rendered = common::to_site_page(&render_reference(), "reference");

    if std::env::var_os("BYNK_BLESS").is_some() {
        fs::write(&page, &rendered).unwrap();
        return;
    }

    let current = fs::read_to_string(&page).unwrap_or_default();
    assert_eq!(
        current, rendered,
        "site/src/content/docs/book/reference/wire-format.md is out of date with the wire format.\n\
         Regenerate with: BYNK_BLESS=1 cargo test -p bynkc --test it wire_reference::"
    );
}

/// Compile [`EXAMPLE_SOURCE`] to Workers and write the output under `dir`.
fn compile_example(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
    let proj = dir.join("proj/wire");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("sample.bynk"), EXAMPLE_SOURCE).unwrap();
    let out = match bynkc::compile_project(
        &bynk_testkit::compile_options_single(dir.join("proj")).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the wire-format example to Workers:\n{}",
            bynkc::render_project_errors(&failure.flatten())
        ),
    };
    let run_dir = dir.join("run");
    for (path, doc) in &out.artefacts.docs {
        let target = run_dir.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, doc.text()).unwrap();
    }
}

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("bynk-wire-ref-{name}-{}", std::process::id()))
}

/// The page's worked hashes are what the emitter stamps for the same source.
#[test]
fn the_pages_example_hashes_are_the_emitted_constants() {
    let tmp = scratch("hash");
    compile_example(&tmp);
    let index = fs::read_to_string(tmp.join("run/workers/wire-sample/index.ts")).unwrap();
    let contracts = example_contracts();
    assert_eq!(
        contracts.len(),
        4,
        "the example has four `on call` services"
    );
    for (name, form, hash) in &contracts {
        let needle =
            format!("kind: \"ContractMismatch\", service: \"{name}\", expected: \"{hash}\"");
        assert!(
            index.contains(&needle),
            "the emitter's hash for `{name}` is not the page's `{hash}` (normal form `{form}`)"
        );
    }
    let _ = fs::remove_dir_all(&tmp);
}

/// A string only the emitted code for `check` contains, so its position in
/// the dispatch is where the check is made.
fn marker(check: CallCheck) -> String {
    match check {
        CallCheck::Method => format!("if (method !== \"{}\")", wf::CALL_METHOD),
        // I2: one runtime call reads the body under the cap and verifies the
        // signature; its result is returned for either refusal.
        CallCheck::BodyTooLarge => "const __body = await __verifyCall(request, env, ".to_string(),
        CallCheck::Authentication => "if (typeof __body !== \"string\") return __body;".to_string(),
        CallCheck::WireVersion => format!("request.headers.get(\"{}\")", wf::WIRE_HEADER),
        CallCheck::UnknownService => "switch (servicePath)".to_string(),
        CallCheck::NotAdmitted => "\"Forbidden\", { status: 403 }".to_string(),
        CallCheck::Contract => format!("request.headers.get(\"{}\")", wf::CONTRACT_HEADER),
        CallCheck::MalformedJson => "globalThis.JSON.parse(__body)".to_string(),
        CallCheck::Arguments => "const __r_".to_string(),
    }
}

/// The emitted call dispatch makes the published checks in the published
/// order. Fixture 2013's callee admits foreign callers, so it makes all nine.
/// The two the runtime's `verifyCall` makes, the body cap and the signature,
/// are held to the page by `the_pages_examples_and_checks_hold_on_the_generated_code`.
#[test]
fn the_emitted_dispatch_makes_the_published_checks_in_order() {
    let golden = repo(
        "bynkc/tests/fixtures/positive/2013_foreign_callers/expected/workers/shop-orders/index.ts",
    );
    let ts = fs::read_to_string(&golden).unwrap();
    let start = ts
        .find(&format!("path.startsWith(\"{}\")", wf::CALL_PATH_PREFIX))
        .expect("the call dispatch");
    let ts = &ts[start..];
    let mut last = 0;
    for check in wf::CALL_CHECKS {
        let m = marker(check);
        let at = ts
            .find(&m)
            .unwrap_or_else(|| panic!("{check:?}: `{m}` is not in the call dispatch"));
        assert!(
            at > last || last == 0,
            "{check:?} (`{m}`) is made out of the published order"
        );
        last = at;
        if let Some(kind) = check.kind() {
            let body = format!("kind: \"{kind}\"");
            let status = format!("status: {}", check.status());
            let tail = &ts[at..];
            let kind_at = tail
                .find(&body)
                .unwrap_or_else(|| panic!("{check:?}: no `{body}`"));
            assert!(
                tail[kind_at..].find(&status).is_some_and(|s| s < 200),
                "{check:?} answers `{body}` with a status other than {}",
                check.status()
            );
        }
    }
}

/// The runtime's caller sends what the page publishes.
#[test]
fn the_runtime_caller_sends_the_published_request() {
    // The bundled runtime the emitter embeds, not the module it is bundled
    // from, so a stale bundle fails here too.
    let boundary = fs::read_to_string(repo("bynk-emit/src/emitter/runtime.ts")).unwrap();
    for needle in [
        format!(
            "export const WIRE_VERSION_HEADER = \"{}\";",
            wf::WIRE_HEADER
        ),
        format!("export const WIRE_VERSION = \"{}\";", wf::WIRE_VERSION),
        format!("export const CALLER_HEADER = \"{}\";", wf::CALLER_HEADER),
        format!(
            "export const TIMESTAMP_HEADER = \"{}\";",
            wf::TIMESTAMP_HEADER
        ),
        format!(
            "export const SIGNATURE_HEADER = \"{}\";",
            wf::SIGNATURE_HEADER
        ),
        format!(
            "export const SIGNATURE_WINDOW_SECONDS = {};",
            wf::SIGNATURE_WINDOW_SECONDS
        ),
        format!("export const MAX_BODY_BYTES = {};", wf::MAX_BODY_BYTES),
        format!("export const SIGNING_SCHEME = \"{}\";", wf::SIGNING_SCHEME),
        format!(
            "export const EDGE_FROM_PREFIX = \"{}\";",
            wf::EDGE_FROM_PREFIX
        ),
        format!("export const EDGE_TO_PREFIX = \"{}\";", wf::EDGE_TO_PREFIX),
        format!("`http://internal{}${{servicePath}}`", wf::CALL_PATH_PREFIX),
        format!("method: \"{}\",", wf::CALL_METHOD),
        format!("\"{}\": callerContext,", wf::CALLER_HEADER),
        format!("\"{}\": contractHash,", wf::CONTRACT_HEADER),
        "[WIRE_VERSION_HEADER]: WIRE_VERSION,".to_string(),
        "...signed,".to_string(),
    ] {
        assert!(
            boundary.contains(&needle),
            "bynk-emit/src/emitter/runtime.ts no longer contains `{needle}`"
        );
    }
    assert!(
        wf::ACCEPTED_WIRE_VERSIONS.contains(&wf::WIRE_VERSION),
        "a callee must accept the version its callers speak"
    );
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

fn base_command(program: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        Command::new(program)
    }
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
    cmd.args(prefix).args(args).current_dir(cwd);
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
  "include": ["**/*.ts"],
  "exclude": ["node_modules", "**/bynk-client.ts"]
}
"#;

/// A TypeScript string literal for `s`.
fn ts_str(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

/// The examples as a TS array the driver walks.
fn examples_ts() -> String {
    let mut out = String::from("[\n");
    for ex in EXAMPLES {
        let outcome = match &ex.outcome {
            Outcome::Accepted { encodes_as } => {
                format!("{{ accepted: {} }}", ts_str(encodes_as))
            }
            Outcome::Rejected { kind, path } => {
                format!("{{ kind: {}, path: {} }}", ts_str(kind), ts_str(path))
            }
        };
        out.push_str(&format!(
            "  {{ codec: {}, json: {}, outcome: {outcome} }},\n",
            ts_str(ex.codec),
            ts_str(ex.json)
        ));
    }
    out.push(']');
    out
}

const DRIVER_TS: &str = r#"import worker from "./workers/wire-sample/index.js";
import * as codecs from "./workers/wire-sample/handlers.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) throw new Error("FAIL: " + msg);
}

const EXAMPLES: any[] = __EXAMPLES__;
const c: any = codecs;
for (const ex of EXAMPLES) {
  const decode = c["__deserialise_" + ex.codec];
  const encode = c["__serialise_" + ex.codec];
  assert(typeof decode === "function", "no codec __deserialise_" + ex.codec);
  const r = decode(JSON.parse(ex.json));
  const what = ex.codec + " " + ex.json;
  if ("accepted" in ex.outcome) {
    assert(r.tag === "Ok", what + " → accepted, got " + JSON.stringify(r));
    const back = JSON.stringify(encode(r.value));
    assert(back === JSON.stringify(JSON.parse(ex.outcome.accepted)), what + " encodes back as " + back);
  } else {
    assert(r.tag === "Err", what + " → rejected, got " + JSON.stringify(r));
    assert(r.error.kind === ex.outcome.kind, what + " → " + ex.outcome.kind + ", got " + JSON.stringify(r.error));
    assert(r.error.path === ex.outcome.path, what + " → at " + ex.outcome.path + ", got " + JSON.stringify(r.error));
  }
}

// The dispatch, against the page's checks table. The callee holds the key
// for the edge from `shop.orders`; requests are signed here from the page's
// description of the signing input, not with the runtime's own signer.
const KEY = "edge-key";
const CALLER = "shop.orders";
const CALLEE = "wire.sample";
const LEGACY_KEY = "legacy-key";
const env: any = { __FROM_KEY__: KEY, LEGACY_CALL_KEY: LEGACY_KEY };
const QUOTE = "__QUOTE_HASH__";
const LINE = JSON.parse(__LINE__);

function hex(buf: ArrayBuffer): string {
  return Array.from(new Uint8Array(buf), (b) => b.toString(16).padStart(2, "0")).join("");
}

async function sign(key: string, fields: string[]): Promise<string> {
  const k = await crypto.subtle.importKey("raw", new TextEncoder().encode(key), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return hex(await crypto.subtle.sign("HMAC", k, new TextEncoder().encode(["__SCHEME__", ...fields].join("\n"))));
}

// A call signed over its final headers, then with `after` applied, so a test
// can break the signature without breaking anything else.
async function call(opts: { method?: string; headers?: Record<string, string>; body?: string | null; service?: string; key?: string; skew?: number; after?: Record<string, string> }): Promise<Request> {
  const method = opts.method ?? "POST";
  const path = "/_bynk/call/" + (opts.service ?? "quote");
  const body = method === "GET" || method === "HEAD" ? null : (opts.body ?? JSON.stringify({ line: LINE, size: 3 }));
  const headers: Record<string, string> = {
    "content-type": "application/json",
    "X-Bynk-Contract": QUOTE,
    "X-Bynk-Caller": CALLER,
    ...(opts.headers ?? {}),
  };
  const timestamp = String(Math.floor(Date.now() / 1000) + (opts.skew ?? 0));
  const digest = hex(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(body ?? "")));
  headers["X-Bynk-Timestamp"] = timestamp;
  headers["X-Bynk-Signature"] = await sign(opts.key ?? KEY, [
    headers["X-Bynk-Caller"] ?? "",
    CALLEE,
    method,
    path,
    headers["X-Bynk-Contract"] ?? "",
    headers["X-Bynk-Wire"] ?? "1",
    timestamp,
    digest,
  ]);
  Object.assign(headers, opts.after ?? {});
  return new Request("http://internal" + path, { method, headers, body });
}

async function expectStatus(req: Request | Promise<Request>, status: number, what: string): Promise<Response> {
  const res = await worker.fetch(await req, env);
  assert(res.status === status, what + " → " + status + ", got " + res.status);
  return res;
}

// Success, with and without the wire header.
let res = await expectStatus(call({}), 200, "a well-formed call");
let body: any = await res.json();
assert(body.kind === "Ok" && body.value.kind === "Dot", "the encoded return value, got " + JSON.stringify(body));
await expectStatus(call({ headers: { "X-Bynk-Wire": "1" } }), 200, "wire version 1");

// 1. Method.
res = await expectStatus(call({ method: "GET" }), 405, "a GET");
assert(res.headers.get("allow") === "POST", "405 names the allowed method");
await expectStatus(call({ method: "PUT" }), 405, "a PUT");

// 2. Body size, before authentication: an unsigned oversized body.
res = await expectStatus(new Request("http://internal/_bynk/call/nosuch", { method: "POST", body: "x".repeat(__MAX_BODY__ + 1) }), 413, "an oversized body");
assert((await res.text()) === "Payload Too Large", "413 is bare");

// 3. Authentication precedes everything after it.
for (const [what, req] of [
  ["the wrong key", call({ key: "nope", headers: { "X-Bynk-Wire": "9" }, service: "nosuch" })],
  ["no signature", call({ after: { "X-Bynk-Signature": "" }, service: "nosuch" })],
  ["a tampered contract", call({ after: { "X-Bynk-Contract": "0000000000000000" } })],
  ["a caller with no key here", call({ headers: { "X-Bynk-Caller": "shop.payment" } })],
  ["no caller", call({ after: { "X-Bynk-Caller": "" } })],
  ["a stale timestamp", call({ skew: -(__WINDOW__ + 5) })],
] as [string, Promise<Request>][]) {
  res = await expectStatus(req, 401, what);
  assert((await res.text()) === "Unauthorized", "401 is bare");
}

// 6. Admission (after the service lookup): a foreign caller only where its actor is admitted, and a
// context only where `Caller` is.
const LEGACY = "__LEGACY_HASH__";
const legacy = (opts: Parameters<typeof call>[0]) =>
  call({ service: "legacy", body: "7", ...opts, headers: { "X-Bynk-Contract": LEGACY, ...(opts.headers ?? {}) } });
res = await expectStatus(legacy({ headers: { "X-Bynk-Caller": "Legacy" }, key: LEGACY_KEY }), 200, "Legacy calling `by Legacy`");
body = await res.json();
assert(body.kind === "Ok" && body.value === 7, "the foreign call's result, got " + JSON.stringify(body));
res = await expectStatus(legacy({}), 403, "a context calling `by Legacy`");
assert((await res.text()) === "Forbidden", "403 is bare");
res = await expectStatus(call({ headers: { "X-Bynk-Caller": "Legacy" }, key: LEGACY_KEY }), 403, "Legacy calling the default `Caller`");
// Legacy's name under a context's key, or a context's name under Legacy's.
await expectStatus(legacy({ headers: { "X-Bynk-Caller": "Legacy" } }), 401, "Legacy signed with an edge key");
await expectStatus(legacy({ key: LEGACY_KEY }), 401, "a context signed with Legacy's secret");

// 4. Wire version.
res = await expectStatus(call({ headers: { "X-Bynk-Wire": "2" }, service: "nosuch" }), 400, "wire version 2");
body = await res.json();
assert(body.kind === "UnsupportedWireVersion" && JSON.stringify(body.accepted) === "[1]", "the accepted versions, got " + JSON.stringify(body));

// 5. Unknown service.
res = await expectStatus(call({ service: "nosuch" }), 404, "an unknown service");
assert((await res.text()) === "Not found", "404 body");

// 7. Contract.
res = await expectStatus(call({ headers: { "X-Bynk-Contract": "0000000000000000" }, body: "{" }), 409, "a skewed contract");
body = await res.json();
assert(body.kind === "ContractMismatch" && body.expected === QUOTE, "409 body, got " + JSON.stringify(body));

// 8. Malformed JSON.
for (const bad of ["{", "", "{\"line\": }"]) {
  res = await expectStatus(call({ body: bad }), 400, "malformed body " + JSON.stringify(bad));
  body = await res.json();
  assert(body.kind === "MalformedJson" && body.details === "Invalid request body", "MalformedJson body, got " + JSON.stringify(body));
}

// A handler that binds `by c: Caller` reads the proved caller.
const WHO = "__WHO_HASH__";
res = await expectStatus(call({ service: "who", headers: { "X-Bynk-Contract": WHO }, body: JSON.stringify({ n: 1, v: "x" }) }), 200, "with a caller");
body = await res.json();
assert(body.kind === "Ok" && body.value === CALLER, "the handler reads the caller, got " + JSON.stringify(body));

// 9. Arguments: an inline field refinement, through the dispatch's path.
res = await expectStatus(call({ body: JSON.stringify({ line: { ...LINE, qty: 0 }, size: 3 }) }), 400, "qty 0");
body = await res.json();
assert(body.kind === "RefinementViolation" && body.path === "$.line.qty", "the violation's path, got " + JSON.stringify(body));
res = await expectStatus(call({ body: JSON.stringify({ line: { ...LINE, wait: 0.5 }, size: 3 }) }), 400, "a fractional Duration");
body = await res.json();
assert(body.kind === "StructuralMismatch" && body.path === "$.line.wait", "the Duration's path, got " + JSON.stringify(body));

console.log("ALL OK");
"#;

/// The page's examples and checks, executed against the generated code.
#[test]
fn the_pages_examples_and_checks_hold_on_the_generated_code() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!("\n!!! WIRE-FORMAT REFERENCE VERIFICATION SKIPPED !!!\nno tsc runner.\n");
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! WIRE-FORMAT REFERENCE VERIFICATION SKIPPED !!!\n`node` not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    let tmp = scratch("run");
    compile_example(&tmp);
    let run_dir = tmp.join("run");
    let quote = example_contracts()
        .into_iter()
        .find(|(name, _, _)| name == "quote")
        .expect("the example's `quote` service")
        .2;
    let who = example_contracts()
        .into_iter()
        .find(|(name, _, _)| name == "who")
        .expect("the example's `who` service")
        .2;
    let legacy = example_contracts()
        .into_iter()
        .find(|(name, _, _)| name == "legacy")
        .expect("the example's `legacy` service")
        .2;
    let line = match &EXAMPLES[0].outcome {
        Outcome::Accepted { .. } => EXAMPLES[0].json,
        Outcome::Rejected { .. } => panic!("the first example is a whole, accepted `Line`"),
    };
    let driver = DRIVER_TS
        .replace("__EXAMPLES__", &examples_ts())
        .replace("__QUOTE_HASH__", &quote)
        .replace("__WHO_HASH__", &who)
        .replace("__LEGACY_HASH__", &legacy)
        .replace("__LINE__", &ts_str(line))
        .replace(
            "__FROM_KEY__",
            &wf::edge_key_from("shop.orders").expect("an edge name"),
        )
        .replace("__SCHEME__", wf::SIGNING_SCHEME)
        .replace("__MAX_BODY__", &wf::MAX_BODY_BYTES.to_string())
        .replace("__WINDOW__", &wf::SIGNATURE_WINDOW_SECONDS.to_string());
    fs::write(run_dir.join("driver.ts"), driver).unwrap();
    fs::write(run_dir.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
    fs::write(run_dir.join("package.json"), r#"{ "type": "module" }"#).unwrap();

    let (ok, out) = run(&runner.0, &runner.1, &["-p", "tsconfig.json"], &run_dir);
    assert!(ok, "tsc failed on the generated example:\n{out}");
    let (ok, out) = run("node", &[], &["js/driver.js"], &run_dir);
    assert!(
        ok && out.contains("ALL OK"),
        "the wire-format page disagrees with the generated code:\n{out}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
