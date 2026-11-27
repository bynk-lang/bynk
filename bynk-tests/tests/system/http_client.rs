//! `bynk client` (#3508, BC1): the typed TypeScript client a front end imports.
//!
//! `bynk/tests/http_client.rs` pins the generated text. Two things only a real
//! toolchain shows:
//!
//! - **The module type-checks** under `tsc --strict` (with unused-name and exact
//!   optional-property checks on), for each project below.
//! - **It works against a real Worker.** The project is compiled for Workers, the
//!   generated client is given a `fetch` that hands each request to the Worker's
//!   own `fetch` handler (in process, the way `fetch_headers.rs` drives one), and
//!   a driver calls every route kind: JSON with an `Option` as `T | null` in both
//!   directions, path and query parameters (a default, a `List`, a `@name`), an
//!   opaque id, `Decimal` and `Instant`, a generic page, every `HttpResult` variant
//!   the routes answer, the boundary's `400`, a refinement refused before sending
//!   and a refinement the server refuses, a transport failure and a stale client's
//!   unexpected status. A second project answers in plain JSON.
//!
//! `the_client_round_trips_every_route_under_workerd` runs the same driver over the
//! network against the Worker under `wrangler dev` (`BYNK_REQUIRE_WORKERD=1` makes
//! its skip a failure).
//!
//! Skips loudly without `node` or `tsc`; `BYNK_REQUIRE_TSC=1` turns the skip into
//! a failure, as the other Node-driven suites do.

#[path = "../require/mod.rs"]
mod require;
#[path = "../tools/mod.rs"]
mod tools;
#[path = "../wrangler/mod.rs"]
mod wrangler;

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bynk/tests/fixtures/http_client")
        .join(name)
}

fn plain_project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive/2858_plain_json_options")
}

fn shapes_project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../bynk/tests/fixtures/openapi/shapes")
}

fn missing_tools() -> Option<String> {
    if !tools::tool_exists("node") {
        return Some("`node` is not on PATH".to_string());
    }
    if tools::tsc_command().is_none() {
        return Some("neither `tsc` nor `npx` is on PATH".to_string());
    }
    None
}

fn skip_without_tools() -> bool {
    match missing_tools() {
        Some(reason) => tools::skip(
            "HTTP CLIENT",
            require::is_required(REQUIRE_ENV),
            REQUIRE_ENV,
            &reason,
        ),
        None => false,
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bynk-http-client-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn bynk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
}

/// A copy of the project at `src`, so the run leaves nothing in the tree.
fn copy_project(src: &Path, into: &Path) {
    fn walk(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for e in fs::read_dir(from).unwrap().flatten() {
            let (f, t) = (e.path(), to.join(e.file_name()));
            if f.is_dir() {
                walk(&f, &t);
            } else if e.file_name() != "bynk.schema.lock" && e.file_name() != "expected" {
                fs::copy(&f, &t).unwrap();
            }
        }
    }
    walk(src, into);
    // A corpus fixture is a bare source tree; a project has a manifest.
    let manifest = into.join("bynk.toml");
    if !manifest.exists() {
        fs::write(
            manifest,
            "[project]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
    }
}

/// `bynk client` over `project`, to `api.ts` in `dir`; returns stderr.
fn generate(project: &Path, dir: &Path, extra: &[&str]) -> String {
    let out = bynk()
        .arg("client")
        .arg(project)
        .args(extra)
        .arg("--out")
        .arg(dir.join("api.ts"))
        .output()
        .expect("run bynk");
    assert!(
        out.status.success(),
        "bynk client failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

const TSCONFIG: &str = r#"{
  "compilerOptions": {
    "strict": true,
    "target": "ES2022",
    "module": "ES2022",
    "moduleResolution": "bundler",
    "lib": ["ES2022", "DOM"],
    "types": [],
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "exactOptionalPropertyTypes": true,
    "outDir": "dist",
    "skipLibCheck": false
  },
  "files": ["api.ts"]
}
"#;

/// `tsc -p` in `dir`; panics with the diagnostics when it fails.
fn tsc(dir: &Path, what: &str) {
    fs::write(dir.join("tsconfig.json"), TSCONFIG).unwrap();
    let out = tools::tsc_command()
        .expect("checked")
        .arg("-p")
        .arg(dir)
        .output()
        .expect("run tsc");
    assert!(
        out.status.success(),
        "{what} does not pass `tsc --strict`:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn every_generated_client_passes_tsc_strict() {
    if skip_without_tools() {
        return;
    }
    for (name, project) in [
        ("one", fixture("one")),
        ("rich", fixture("rich")),
        ("forms", fixture("forms")),
        ("raw", fixture("raw")),
        ("shapes", shapes_project()),
        ("plain", plain_project()),
        ("ws", fixture("ws")),
    ] {
        let dir = scratch(&format!("tsc-{name}"));
        let proj = dir.join("project");
        copy_project(&project, &proj);
        generate(&proj, &dir, &[]);
        tsc(&dir, &format!("the `{name}` client"));
        let _ = fs::remove_dir_all(&dir);
    }
}

/// The directory layout is `tsc --strict` clean too, under the strictest module
/// settings a front end may set (`verbatimModuleSyntax`, `isolatedModules`), so an
/// import of a type that is not marked as one, or an unused one, is caught here.
/// Its imports end in `.js`, which resolves to the `.ts` files under `bundler`
/// (Vite, SvelteKit, Next.js) and under `nodenext` (strict Node ESM) alike, so
/// both are checked.
#[test]
fn the_directory_layout_passes_tsc_strict_under_bundler_and_nodenext() {
    if skip_without_tools() {
        return;
    }
    for (name, project) in [
        ("rich", fixture("rich")),
        ("forms", fixture("forms")),
        ("raw", fixture("raw")),
        ("shapes", shapes_project()),
    ] {
        let dir = scratch(&format!("dir-{name}"));
        let proj = dir.join("project");
        copy_project(&project, &proj);
        let api = dir.join("api");
        let out = bynk()
            .args(["client", "--layout", "dir", "--out"])
            .arg(&api)
            .arg(&proj)
            .output()
            .expect("run bynk");
        assert!(
            out.status.success(),
            "bynk client --layout dir failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        // A `nodenext` build reads a `.ts` file as ESM only under a package of
        // `"type": "module"`.
        fs::write(api.join("package.json"), "{\"type\":\"module\"}\n").unwrap();
        for (module, resolution) in [("ES2022", "bundler"), ("NodeNext", "nodenext")] {
            fs::write(
                api.join("tsconfig.json"),
                format!(
                    r#"{{
  "compilerOptions": {{
    "strict": true,
    "target": "ES2022",
    "module": "{module}",
    "moduleResolution": "{resolution}",
    "lib": ["ES2022", "DOM"],
    "types": [],
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "exactOptionalPropertyTypes": true,
    "verbatimModuleSyntax": true,
    "isolatedModules": true,
    "noEmit": true,
    "skipLibCheck": false
  }},
  "files": ["index.ts"]
}}
"#
                ),
            )
            .unwrap();
            let tsc = tools::tsc_command()
                .expect("checked")
                .arg("-p")
                .arg(&api)
                .output()
                .expect("run tsc");
            assert!(
                tsc.status.success(),
                "the `{name}` directory client does not pass `tsc --strict` under `{resolution}`:\n{}{}",
                String::from_utf8_lossy(&tsc.stdout),
                String::from_utf8_lossy(&tsc.stderr)
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }
}

/// Compile `project` for Workers as JavaScript, next to the client; `worker` is the
/// directory the compiled Worker lands in.
fn compile_worker(project: &Path, dir: &Path) {
    let out = bynk()
        .args(["compile", "--target", "workers", "--emit", "js", "-o"])
        .arg(dir.join("out"))
        .arg(project)
        .output()
        .expect("run bynk");
    assert!(
        out.status.success(),
        "bynk compile failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Run `driver` (an ES module) in `dir` against the compiled Worker and the
/// client built by `tsc`; returns its stdout.
fn run_driver(dir: &Path, driver: &str, env: &[(&str, &str)]) -> String {
    fs::write(dir.join("dist/package.json"), "{\"type\":\"module\"}\n").unwrap();
    fs::write(dir.join("drive.mjs"), driver).unwrap();
    let run = Command::new("node")
        .arg("drive.mjs")
        .current_dir(dir)
        .envs(env.iter().copied())
        .output()
        .expect("run node");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    assert!(
        run.status.success(),
        "the driver failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    stdout
}

/// A line of the driver's output, by its label.
fn line<'a>(stdout: &'a str, label: &str) -> &'a str {
    let prefix = format!("{label} ");
    stdout
        .lines()
        .find_map(|l| l.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("no `{label}` line in:\n{stdout}"))
}

/// How a driver reaches the Worker: in process, or over the network to `wrangler dev`.
const IN_PROCESS: &str = r#"
import worker from "./out/workers/rich/index.js";
const BASE = "http://api.test";
const call = (url, init) => worker.fetch(new Request(url, init), {});
"#;

const NETWORK: &str = r#"
const BASE = process.env.BASE;
const call = (url, init) => fetch(url, init);
"#;

const RICH_DRIVER: &str = r#"
import { createClient } from "./dist/api.js";
const sent = [];
const viaWorker = async (url, init) => {
  sent.push(`${init.method} ${url}${init.body ? " " + init.body : ""}`);
  return call(url, init);
};
const show = (label, v) =>
  console.log(label, JSON.stringify(v, (_k, x) => (typeof x === "bigint" ? `${x}n` : x)));
const c = createClient({ baseUrl: BASE + "/", fetch: viaWorker });

show("create", await c.api.postOrders({ sku: "A1", qty: 2, note: null, tags: [] }));
show("create-note", await c.api.postOrders({ sku: "A1", qty: 2, note: "hi", tags: ["a"] }));
show("conflict", await c.api.postOrders({ sku: "A1", qty: 60, note: null, tags: [] }));
show("invalid-qty", await c.api.postOrders({ sku: "A1", qty: 100, note: null, tags: [] }));
show("invalid-sku", await c.api.postOrders({ sku: "", qty: 1, note: null, tags: [] }));
show("invalid-note", await c.api.postOrders({ sku: "A", qty: 1, note: "this is too long", tags: [] }));
show("list", await c.api.getOrders({ tags: ["x", "y"], since: 5 }));
show("list-null", await c.api.getOrders({ tags: ["q"], sku: null, limit: 3 }));
show("get", await c.api.getOrderById("abc"));
show("search", await c.api.getSearch({ q: "a b&c", page: 2 }));
show("delete", await c.api.deleteOrderById("x/y"));
show("dots", [await c.api.getOrderById(".."), await c.api.getOrderById(".")]);

// What the server refuses and the client did not: a field the client does not know.
const lax = createClient({
  baseUrl: BASE,
  fetch: async (url, init) => {
    const body = JSON.parse(init.body);
    body.qty = 0;
    return call(url, { ...init, body: JSON.stringify(body) });
  },
});
show("server-400", await lax.api.postOrders({ sku: "A1", qty: 1, note: null, tags: [] }));

const down = createClient({ fetch: async () => { throw new Error("down"); } });
const failed = await down.api.getSearch({ q: "x" });
show("transport", { kind: failed.kind, cause: String(failed.cause) });
const stale = createClient({ fetch: async () => new Response("nope", { status: 418 }) });
show("unexpected", await stale.api.getSearch({ q: "x" }));
const drifted = createClient({ fetch: async () => Response.json({ id: 1 }, { status: 201 }) });
const bad = await drifted.api.postOrders({ sku: "A1", qty: 1, note: null, tags: [] });
show("drift", { kind: bad.kind, status: bad.status, path: bad.error?.path });

const auth = [];
const h = createClient({
  fetch: async (_u, init) => { auth.push([...new Headers(init.headers)].sort().join(";")); return Response.json("x"); },
  headers: async () => ({ authorization: "Bearer t" }),
  origin: "https://app.test",
});
await h.api.getSearch({ q: "x" }, { headers: { "x-extra": "1" } });
console.log("headers", auth[0]);
console.log("requests", JSON.stringify(sent));
"#;

/// What the rich driver must have printed, against a Worker answering at `base`.
fn check_rich(out: &str, base: &str) {
    // `Option` is `T | null` in both directions, and an opaque id, an `Instant`
    // and a `Decimal` come back decoded.
    assert_eq!(
        line(out, "create"),
        r#"{"status":201,"kind":"Created","body":{"id":"o-1","sku":"A1","qty":2,"note":null,"placedAt":1700000000000,"price":{"coefficient":"1250n","scale":2}}}"#
    );
    assert!(line(out, "create-note").contains(r#""note":"hi""#));
    // A status the handler answers by name is a typed member.
    assert_eq!(
        line(out, "conflict"),
        r#"{"status":409,"kind":"Conflict","error":"too many"}"#
    );
    // A refinement is refused before sending, with the server's own message.
    for (label, path, field, message) in [
        ("invalid-qty", "$.qty", "Qty", "must be in range [1, 99]"),
        ("invalid-sku", "$.sku", "Sku", "must be non-empty"),
        (
            "invalid-note",
            "$.note.value",
            "Note",
            "length must be at most 10",
        ),
    ] {
        let l = line(out, label);
        assert!(
            l.starts_with(r#"{"kind":"Invalid","error":{"kind":"RefinementViolation""#)
                && l.contains(&format!(r#""path":"{path}""#))
                && l.contains(&format!(r#""field":"{field}","message":"{message}""#)),
            "{label}: {l}"
        );
    }
    // Typed query: a `List` repeats its key, an `Option` is its value or absent,
    // a default is left to the server, a `@name` is the wire key.
    assert_eq!(
        line(out, "list"),
        r#"{"status":200,"kind":"Ok","body":{"items":["x","y"],"next":5}}"#
    );
    assert_eq!(
        line(out, "list-null"),
        r#"{"status":200,"kind":"Ok","body":{"items":["q"],"next":null}}"#
    );
    // An `Option` of a sum, decoded to the value.
    assert_eq!(
        line(out, "get"),
        r#"{"status":200,"kind":"Ok","body":{"tag":"Opened","by":"ann"}}"#
    );
    assert_eq!(
        line(out, "search"),
        r#"{"status":200,"kind":"Ok","body":"a b&c"}"#
    );
    assert_eq!(line(out, "delete"), r#"{"status":204,"kind":"NoContent"}"#);
    // A `.` or `..` path parameter would be collapsed by the URL parser, so it is
    // refused as a value, and nothing is sent.
    assert_eq!(
        line(out, "dots"),
        r#"[{"kind":"Invalid","error":{"kind":"StructuralMismatch","path":"path.id","expected":"a path segment","actual":".."}},{"kind":"Invalid","error":{"kind":"StructuralMismatch","path":"path.id","expected":"a path segment","actual":"."}}]"#
    );
    // The boundary's own `400`, which the client did not foresee, is typed.
    let server_400 = line(out, "server-400");
    assert!(
        server_400.starts_with(
            r#"{"status":400,"kind":"StructuralMismatch","error":{"kind":"RefinementViolation""#
        ) && server_400.contains(r#""path":"$.qty""#),
        "{server_400}"
    );
    // Failures are returned, never thrown.
    assert_eq!(
        line(out, "transport"),
        r#"{"kind":"Transport","cause":"Error: down"}"#
    );
    assert_eq!(
        line(out, "unexpected"),
        r#"{"kind":"Unexpected","status":418,"text":"nope"}"#
    );
    // A body that no longer decodes is a stale client's `Unexpected`, with why.
    assert_eq!(
        line(out, "drift"),
        r#"{"kind":"Unexpected","status":201,"path":"$.id"}"#
    );
    // `headers`, a per-call header and `origin` reach the wire.
    assert_eq!(
        line(out, "headers"),
        "authorization,Bearer t;origin,https://app.test;x-extra,1"
    );
    let requests = line(out, "requests");
    for expected in [
        format!(
            r#"POST {base}/orders {{\"sku\":\"A1\",\"qty\":2,\"note\":{{\"kind\":\"None\"}},\"tags\":[]}}"#
        ),
        format!("GET {base}/orders?tags=x&tags=y&since=5"),
        format!("GET {base}/orders?limit=3&tags=q"),
        format!("GET {base}/orders/abc"),
        format!("GET {base}/search?q=a+b%26c&x-page=2"),
        format!("DELETE {base}/orders/x%2Fy"),
    ] {
        assert!(requests.contains(&expected), "{expected} not in {requests}");
    }
}

#[test]
fn the_client_round_trips_every_route_against_a_real_worker() {
    if skip_without_tools() {
        return;
    }
    let dir = scratch("rich");
    let proj = dir.join("project");
    copy_project(&fixture("rich"), &proj);
    generate(&proj, &dir, &[]);
    compile_worker(&proj, &dir);
    tsc(&dir, "the `rich` client");
    let out = run_driver(&dir, &format!("{IN_PROCESS}{RICH_DRIVER}"), &[]);
    check_rich(&out, "http://api.test");
    let _ = fs::remove_dir_all(&dir);
}

const PLAIN_DRIVER: &str = r#"
import worker from "./out/workers/api/index.js";
import { createClient } from "./dist/api.js";
const viaWorker = (url, init) => worker.fetch(new Request(url, init), {});
const c = createClient({ baseUrl: "http://api.test", fetch: viaWorker });
const show = (label, v) => console.log(label, JSON.stringify(v));
show("top", await c.plain.getTop());
show("tagged", await c.plain.getTagged());
show("filtered", await c.plain.getFiltered({ limit: 4 }));
show("filtered-none", await c.plain.getFiltered({}));
show("legacy", await c.legacy.getLegacy());
show("legacy-plain", await c.legacy.getLegacyPlain());
console.log("keys", Object.keys(c.plain).sort().join(","));
"#;

#[test]
fn a_plain_json_route_and_a_tagged_one_decode_to_the_same_null() {
    if skip_without_tools() {
        return;
    }
    let dir = scratch("plain");
    let proj = dir.join("project");
    copy_project(&plain_project(), &proj);
    let stderr = generate(&proj, &dir, &[]);
    // `/plain` holds an `Option` inside a `Map`, which has no `T | null` form.
    assert!(
        stderr.contains("skipped `GET /plain` of `plain`") && stderr.contains("Map"),
        "{stderr}"
    );
    compile_worker(&proj, &dir);
    // The Worker's directory is named for the context.
    let worker = fs::read_dir(dir.join("out/workers"))
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .file_name();
    let driver = PLAIN_DRIVER.replace(
        "workers/api/",
        &format!("workers/{}/", worker.to_string_lossy()),
    );
    tsc(&dir, "the `plain` client");
    let out = run_driver(&dir, &driver, &[]);
    assert_eq!(
        line(&out, "top"),
        r#"{"status":200,"kind":"Ok","body":null}"#
    );
    assert_eq!(
        line(&out, "tagged"),
        r#"{"status":200,"kind":"Ok","body":"t"}"#
    );
    assert_eq!(
        line(&out, "filtered"),
        r#"{"status":200,"kind":"Ok","body":4}"#
    );
    assert_eq!(
        line(&out, "filtered-none"),
        r#"{"status":200,"kind":"Ok","body":null}"#
    );
    assert_eq!(
        line(&out, "legacy"),
        r#"{"status":200,"kind":"Ok","body":"t"}"#
    );
    assert_eq!(
        line(&out, "legacy-plain"),
        r#"{"status":200,"kind":"Ok","body":"p"}"#
    );
    assert_eq!(line(&out, "keys"), "getFiltered,getTagged,getTop");
    let _ = fs::remove_dir_all(&dir);
}

/// #3916: on a route that answers its errors as problem details, a handler's `BadRequest` and the
/// router's `400` share a status and a `Problem` shape, and the client tells them apart: the variant's
/// problem has no `kind` and the rejection's has one.
const PROBLEM_DRIVER: &str = r#"
import worker from "./out/workers/api/index.js";
import { createClient } from "./dist/api.js";
// A call whose body the client would refuse to send is sent as written here, so the router rejects it.
const send = (body) => (url, init) => worker.fetch(new Request(url, body === undefined ? init : { ...init, body }), {});
const show = (label, v) => console.log(label, JSON.stringify(v));
const ok = createClient({ baseUrl: "http://api.test", fetch: send(undefined) });
const bad = createClient({ baseUrl: "http://api.test", fetch: send('{"qty":"many"}') });
show("variant", await ok.api.postWidgets({ qty: 1 }));
show("rejection", await bad.api.postWidgets({ qty: 2 }));
"#;

#[test]
fn a_problem_variant_and_a_problem_rejection_at_one_status_are_told_apart() {
    if skip_without_tools() {
        return;
    }
    let dir = scratch("problem");
    let proj = dir.join("project");
    fs::create_dir_all(proj.join("src")).unwrap();
    fs::write(
        proj.join("bynk.toml"),
        "[project]\nname = \"problems\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        proj.join("src/api.bynk"),
        "context api\n\ntype Widget = { qty: Int }\n\n\
         service api from http {\n  errors { format: problem }\n\n  \
           on POST(\"/widgets\") (body: Widget) -> Effect[HttpResult[String]] by v: Visitor {\n    \
             if body.qty == 1 { BadRequest(\"no such widget\") } else { Created(\"ok\") }\n  }\n}\n",
    )
    .unwrap();
    generate(&proj, &dir, &[]);
    compile_worker(&proj, &dir);
    tsc(&dir, "the problem-details client");
    let out = run_driver(&dir, PROBLEM_DRIVER, &[]);
    let variant: serde_json::Value = serde_json::from_str(line(&out, "variant")).unwrap();
    assert_eq!(variant["kind"], "BadRequest", "{out}");
    assert_eq!(variant["problem"]["type"], "about:blank", "{out}");
    assert_eq!(variant["problem"]["detail"], "no such widget", "{out}");
    let rejection: serde_json::Value = serde_json::from_str(line(&out, "rejection")).unwrap();
    assert_eq!(rejection["kind"], "StructuralMismatch", "{out}");
    assert_eq!(
        rejection["problem"]["type"], "https://bynk-lang.org/problems/structural-mismatch",
        "{out}"
    );
    assert_eq!(rejection["problem"]["kind"], "StructuralMismatch", "{out}");
    let _ = fs::remove_dir_all(&dir);
}

/// #4434: a `@rateLimit` route's router `429` (`RateLimited`) and a handler's own `TooManyRequests`
/// share a status, and the client tells them apart in both modes: by the `error` a handler's variant
/// carries and the `kind` the router's refusal does, and in problem mode by the same `kind`.
const RATE_LIMIT_DRIVER: &str = r#"
import worker from "./out/workers/api/index.js";
import { createClient } from "./dist/api.js";
// A limiter that answers the same to every key: `allow` is whether it lets the request through.
const limiter = (allow) => ({ limit: async () => ({ success: allow }) });
const via = (allow) => (url, init) => worker.fetch(new Request(url, init), { RATELIMIT_SIGNUP_LIMIT: limiter(allow) });
const show = (label, v) => console.log(label, JSON.stringify(v));
const refused = createClient({ baseUrl: "http://api.test", fetch: via(false) });
const allowed = createClient({ baseUrl: "http://api.test", fetch: via(true) });
show("router", await refused.api.postMixed("slow"));
show("handler", await allowed.api.postMixed("slow"));
show("fine", await allowed.api.postMixed("fine"));
"#;

fn rate_limit_project(dir: &Path, problem: bool) -> PathBuf {
    let proj = dir.join("project");
    fs::create_dir_all(proj.join("src")).unwrap();
    fs::write(
        proj.join("bynk.toml"),
        "[project]\nname = \"limits\"\nversion = \"0.1.0\"\n\n[unstable]\nallow = [\"bynk.cloudflare.RateLimit\"]\n",
    )
    .unwrap();
    let errors = if problem {
        "  errors { format: problem }\n\n"
    } else {
        ""
    };
    fs::write(
        proj.join("src/api.bynk"),
        format!(
            "context api\n\nconsumes bynk.cloudflare {{ RateLimit }}\n\n\
             capability SignupLimit = RateLimit @limit(1) @period(60.seconds)\n\n\
             service api from http {{\n{errors}  \
               @rateLimit(SignupLimit, key: clientIp)\n  \
               on POST(\"/mixed\") (body: String) -> Effect[HttpResult[String]] by v: Visitor {{\n    \
                 if body == \"slow\" {{ TooManyRequests(\"slow down\") }} else {{ Created(\"ok\") }}\n  }}\n}}\n"
        ),
    )
    .unwrap();
    proj
}

#[test]
fn a_router_429_and_a_handler_429_are_told_apart() {
    if skip_without_tools() {
        return;
    }
    for problem in [false, true] {
        let dir = scratch(if problem {
            "rate-limit-problem"
        } else {
            "rate-limit"
        });
        let proj = rate_limit_project(&dir, problem);
        generate(&proj, &dir, &[]);
        compile_worker(&proj, &dir);
        tsc(&dir, "the rate-limited client");
        let out = run_driver(&dir, RATE_LIMIT_DRIVER, &[]);
        let router: serde_json::Value = serde_json::from_str(line(&out, "router")).unwrap();
        let handler: serde_json::Value = serde_json::from_str(line(&out, "handler")).unwrap();
        let fine: serde_json::Value = serde_json::from_str(line(&out, "fine")).unwrap();
        assert_eq!(router["status"], 429, "{out}");
        assert_eq!(router["kind"], "RateLimited", "problem={problem}: {out}");
        assert_eq!(handler["status"], 429, "{out}");
        assert_eq!(
            handler["kind"], "TooManyRequests",
            "problem={problem}: {out}"
        );
        assert_eq!(fine["kind"], "Created", "{out}");
        if problem {
            assert_eq!(router["problem"]["kind"], "RateLimited", "{out}");
            assert_eq!(
                router["problem"]["type"], "https://bynk-lang.org/problems/rate-limited",
                "{out}"
            );
            assert_eq!(handler["problem"]["type"], "about:blank", "{out}");
            assert_eq!(handler["problem"]["detail"], "slow down", "{out}");
        } else {
            assert_eq!(router["error"]["kind"], "RateLimited", "{out}");
            assert!(router["error"]["details"].is_string(), "{out}");
            assert_eq!(handler["error"], "slow down", "{out}");
        }
        let _ = fs::remove_dir_all(&dir);
    }
}

/// #3910: a typed path parameter is written as its text: an `Int` its digits, an enum its
/// variant's name, and the router decodes it back.
const PATH_DRIVER: &str = r#"
import worker from "./out/workers/shop-api/index.js";
import { createClient } from "./dist/api.js";
const viaWorker = (url, init) => worker.fetch(new Request(url, init), {});
const c = createClient({ baseUrl: "http://api.test", fetch: viaWorker });
console.log("keys", Object.keys(c.api).sort().join(","));
const show = (label, v) => console.log(label, JSON.stringify(v));
show("order", await c.api.getOrderById(42));
show("status", await c.api.getOrdersStatuByS({ tag: "Closed" }));
show("price", await c.api.getPriceByP({ coefficient: 1250n, scale: 2 }));
"#;

#[test]
fn a_typed_path_parameter_round_trips_through_the_client() {
    if skip_without_tools() {
        return;
    }
    let dir = scratch("typed-path");
    let proj = dir.join("project");
    copy_project(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/positive/3910_http_path_param_types"),
        &proj,
    );
    generate(&proj, &dir, &[]);
    compile_worker(&proj, &dir);
    tsc(&dir, "the typed path-parameter client");
    let out = run_driver(&dir, PATH_DRIVER, &[]);
    assert_eq!(
        line(&out, "order"),
        r#"{"status":200,"kind":"Ok","body":"order 42"}"#
    );
    assert_eq!(
        line(&out, "status"),
        r#"{"status":200,"kind":"Ok","body":"closed"}"#
    );
    assert_eq!(
        line(&out, "price"),
        r#"{"status":200,"kind":"Ok","body":"price 12.50"}"#
    );
    let _ = fs::remove_dir_all(&dir);
}

/// #3911: a typed `headers` record is sent as request headers, under each field's `@name`.
const HEADERS_DRIVER: &str = r#"
import worker from "./out/workers/shop-api/index.js";
import { createClient } from "./dist/api.js";
const viaWorker = (url, init) => worker.fetch(new Request(url, init), {});
const c = createClient({ baseUrl: "http://api.test", fetch: viaWorker });
console.log("keys", Object.keys(c.api).sort().join(","));
const show = (label, v) => console.log(label, JSON.stringify(v));
show("probe", await c.api.getProbe({ length: 7, key: "abcdefgh", tags: ["a", "b"], mode: { tag: "Slow" } }));
show("short", await c.api.getProbe({ length: 7, key: "x", tags: [] }));
"#;

#[test]
fn a_typed_headers_record_is_sent_as_headers_by_the_client() {
    if skip_without_tools() {
        return;
    }
    let dir = scratch("typed-headers");
    let proj = dir.join("project");
    copy_project(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/positive/3911_http_typed_headers"),
        &proj,
    );
    generate(&proj, &dir, &[]);
    compile_worker(&proj, &dir);
    tsc(&dir, "the typed headers client");
    let out = run_driver(&dir, HEADERS_DRIVER, &[]);
    assert_eq!(
        line(&out, "probe"),
        r#"{"status":200,"kind":"Ok","body":"7 abcdefgh 2 slow 5"}"#
    );
    // The client checks the refinement before sending, as the server would.
    assert!(line(&out, "short").contains("Invalid"), "{out}");
    let _ = fs::remove_dir_all(&dir);
}

/// `GET path` on `port`: the response, or why there is none yet.
fn get(port: u16, path: &str) -> Result<String, String> {
    let addr = format!("127.0.0.1:{port}");
    let mut stream = std::net::TcpStream::connect_timeout(
        &addr.parse().map_err(|e| format!("{e}"))?,
        Duration::from_secs(2),
    )
    .map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .map_err(|e| e.to_string())?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| e.to_string())?;
    Ok(response)
}

fn skip_workerd(reason: &str) -> bool {
    tools::skip(
        "HTTP CLIENT (WORKERD)",
        require::is_required("BYNK_REQUIRE_WORKERD"),
        "BYNK_REQUIRE_WORKERD",
        reason,
    )
}

/// Boot `wrangler dev` over the compiled Worker `worker` of `dir` and wait until
/// `probe` answers `200`: the process and its port, or `None` (and a skip) where
/// workerd cannot be had.
fn boot_workerd(dir: &Path, worker: &str, probe: &str) -> Option<(Vec<wrangler::Wrangler>, u16)> {
    let [port, inspector] = wrangler::ports::free_ports();
    let log = dir.join("wrangler.log");
    let out_log = fs::File::create(&log).unwrap();
    let err_log = out_log.try_clone().unwrap();
    let spawned = wrangler::Wrangler::spawn(
        wrangler::npx()
            .args(["dev"])
            .args([
                "--port",
                &port.to_string(),
                "--inspector-port",
                &inspector.to_string(),
            ])
            .current_dir(dir.join("out/workers").join(worker))
            .stdout(Stdio::from(out_log))
            .stderr(Stdio::from(err_log)),
        &dir.join("registry"),
    );
    let Ok(wrangler) = spawned else {
        skip_workerd("could not launch npx");
        return None;
    };
    let mut wranglers = vec![wrangler];
    if let Err(why) = wrangler::wait_until_ready(&log, port, &|| {
        get(port, probe).is_ok_and(|r| r.contains("200 OK"))
    }) {
        let log = fs::read_to_string(&log).unwrap_or_default();
        wrangler::stop_all(&mut wranglers);
        skip_workerd(&format!(
            "{why} (likely no network to provision {})\n{log}",
            wrangler::SPEC
        ));
        return None;
    }
    Some((wranglers, port))
}

#[test]
fn the_client_round_trips_every_route_under_workerd() {
    if skip_without_tools() {
        return;
    }
    if !tools::tool_exists("npx") && skip_workerd("`npx` is not on PATH") {
        return;
    }
    let dir = scratch("workerd");
    let proj = dir.join("project");
    copy_project(&fixture("rich"), &proj);
    generate(&proj, &dir, &[]);
    compile_worker(&proj, &dir);
    tsc(&dir, "the `rich` client");

    let Some((mut wranglers, port)) = boot_workerd(&dir, "rich", "/search?q=up") else {
        return;
    };
    let base = format!("http://127.0.0.1:{port}");
    let out = run_driver(
        &dir,
        &format!("{NETWORK}{RICH_DRIVER}"),
        &[("BASE", base.as_str())],
    );
    wrangler::stop_all(&mut wranglers);
    check_rich(&out, &base);
    let _ = fs::remove_dir_all(&dir);
}

const FORMS_DRIVER: &str = r#"
import { createClient } from "./dist/api.js";
const sent = [];
const viaWorker = async (url, init) => {
  const body = init.body;
  sent.push(`${init.method} ${url} ${new Headers(init.headers).get("content-type") ?? "-"} ${typeof body === "string" ? body : body === undefined ? "-" : body.constructor.name} cookie=${new Headers(init.headers).get("cookie") ?? "-"}`);
  return call(url, init);
};
const show = (label, v) => console.log(label, JSON.stringify(v));
const c = createClient({ baseUrl: "http://localhost", fetch: viaWorker });

show("note", await c.api.postNotes({ title: "Hi there", body: null, tags: ["a", "b"], pinned: true, rank: 3 }));
show("note-some", await c.api.postNotes({ title: "T", body: "x y&z", tags: [], pinned: false, rank: null }));
show("note-invalid", await c.api.postNotes({ title: "", body: null, tags: [], pinned: false, rank: null }));

const png = new File([new Uint8Array([1, 2, 3])], "p.png", { type: "image/png" });
show("upload", await c.api.postUpload({ title: "T", caption: null, photo: png, cover: null, extras: [new Blob(["x"]), new Blob(["yy"])] }));
show("upload-cover", await c.api.postUpload({ title: "T", caption: "cap", photo: png, cover: new Blob(["abcd"]), extras: [] }));
show("upload-invalid", await c.api.postUpload({ title: "", caption: null, photo: png, cover: null, extras: [] }));

show("prefs", await c.api.getPrefs({ theme: "dark", lang: "fr" }));
show("prefs-default", await c.api.getPrefs({ theme: "light" }));
// A value that is not cookie text would add a cookie the caller never declared.
show("prefs-inject", await c.api.getPrefs({ theme: "dark; __Host-sid=attacker" }));
// A `Cookie` the caller configured is joined, not replaced; a JSON `content-type`
// configured for the whole client does not spoil a form or a multipart body.
const configured = createClient({
  baseUrl: "http://localhost",
  headers: { cookie: "sid=abc", "content-type": "application/json" },
  fetch: async (url, init) => {
    sent.push(`CONFIGURED ${new Headers(init.headers).get("cookie") ?? "-"} ${new Headers(init.headers).get("content-type") ?? "-"}`);
    return call(url, init);
  },
});
show("prefs-joined", await configured.api.getPrefs({ theme: "dark" }));
show("note-configured", await configured.api.postNotes({ title: "J", body: null, tags: [], pinned: false, rank: null }));
show("upload-configured", await configured.api.postUpload({ title: "U", caption: null, photo: png, cover: null, extras: [] }));
// Where `document` exists a script cannot set `Cookie`: refused, not silently lost.
globalThis.document = {};
show("prefs-browser", await c.api.getPrefs({ theme: "dark" }));
show("origin-in-browser", await c.api.getOrigin());
delete globalThis.document;
show("origin-none", await c.api.getOrigin());
const withOrigin = createClient({ baseUrl: "http://localhost", fetch: viaWorker, origin: "https://app.test" });
show("origin", await withOrigin.api.getOrigin());
console.log("requests", JSON.stringify(sent));
"#;

#[test]
fn forms_multipart_and_cookies_round_trip_against_a_real_worker() {
    if skip_without_tools() {
        return;
    }
    let dir = scratch("forms");
    let proj = dir.join("project");
    copy_project(&fixture("forms"), &proj);
    generate(&proj, &dir, &[]);
    compile_worker(&proj, &dir);
    tsc(&dir, "the `forms` client");
    let driver = format!("{IN_PROCESS}{FORMS_DRIVER}").replace("workers/rich/", "workers/forms/");
    let out = run_driver(&dir, &driver, &[]);

    // A `@form` body: a `List` repeats its key, an `Option` is its value or absent.
    assert_eq!(
        line(&out, "note"),
        r#"{"status":200,"kind":"Ok","body":"Hi there|-|2|true|3"}"#
    );
    assert_eq!(
        line(&out, "note-some"),
        r#"{"status":200,"kind":"Ok","body":"T|x y&z|0|false|-"}"#
    );
    // A refinement is refused before sending, rooted at `body` as the server's is.
    let l = line(&out, "note-invalid");
    assert!(
        l.starts_with(
            r#"{"kind":"Invalid","error":{"kind":"RefinementViolation","path":"body.title""#
        ),
        "{l}"
    );
    // `@multipart`: text fields and file parts reach the handler as they were given.
    assert_eq!(
        line(&out, "upload"),
        r#"{"status":200,"kind":"Ok","body":"T|-|p.png|image/png|3|-|2"}"#
    );
    assert_eq!(
        line(&out, "upload-cover"),
        r#"{"status":200,"kind":"Ok","body":"T|cap|p.png|image/png|3|4|0"}"#
    );
    let l = line(&out, "upload-invalid");
    assert!(
        l.starts_with(
            r#"{"kind":"Invalid","error":{"kind":"RefinementViolation","path":"body.title""#
        ),
        "{l}"
    );
    // Typed `cookies`: the `Cookie` header, a `@name` as its name, a default left to the server.
    assert_eq!(
        line(&out, "prefs"),
        r#"{"status":200,"kind":"Ok","body":"dark|fr|1"}"#
    );
    assert_eq!(
        line(&out, "prefs-default"),
        r#"{"status":200,"kind":"Ok","body":"light|-|1"}"#
    );
    let l = line(&out, "prefs-inject");
    assert!(
        l.starts_with(
            r#"{"kind":"Invalid","error":{"kind":"StructuralMismatch","path":"cookies.theme""#
        ),
        "{l}"
    );
    assert_eq!(
        line(&out, "prefs-joined"),
        r#"{"status":200,"kind":"Ok","body":"dark|-|1"}"#
    );
    assert_eq!(
        line(&out, "note-configured"),
        r#"{"status":200,"kind":"Ok","body":"J|-|0|false|-"}"#
    );
    assert_eq!(
        line(&out, "upload-configured"),
        r#"{"status":200,"kind":"Ok","body":"U|-|p.png|image/png|3|-|0"}"#
    );
    let requests = line(&out, "requests");
    for expected in [
        "CONFIGURED sid=abc; theme=dark application/json",
        "CONFIGURED sid=abc application/x-www-form-urlencoded",
        "CONFIGURED sid=abc -",
    ] {
        assert!(requests.contains(expected), "{expected} not in {requests}");
    }
    // A browser drops a `Cookie` header, so typed cookies there are refused up front.
    let l = line(&out, "prefs-browser");
    assert!(
        l.starts_with(r#"{"kind":"Invalid","error":{"kind":"StructuralMismatch","path":"cookies""#)
            && l.contains("credentials"),
        "{l}"
    );
    assert_eq!(
        line(&out, "origin-in-browser"),
        r#"{"status":200,"kind":"Ok","body":"none"}"#,
        "a call with no typed cookies is untouched in a browser"
    );
    // `origin` reaches the wire as `Origin`; without it the request carries none.
    assert_eq!(
        line(&out, "origin-none"),
        r#"{"status":200,"kind":"Ok","body":"none"}"#
    );
    assert_eq!(
        line(&out, "origin"),
        r#"{"status":200,"kind":"Ok","body":"https://app.test"}"#
    );
    let requests = line(&out, "requests");
    for expected in [
        r#"POST http://localhost/notes application/x-www-form-urlencoded title=Hi+there&tags=a&tags=b&pinned=true&rank=3 cookie=-"#,
        "POST http://localhost/upload - FormData",
        "GET http://localhost/prefs - - cookie=theme=dark; lang-pref=fr",
    ] {
        assert!(requests.contains(expected), "{expected} not in {requests}");
    }
    let _ = fs::remove_dir_all(&dir);
}

/// A server's `fetch`, in a Worker, calling a Bynk route with `origin` set.
const ORIGIN_WORKER: &str = r#"import { createClient } from "./api";

export default {
  async fetch(request: Request): Promise<Response> {
    const base = new URL(request.url).searchParams.get("base") ?? "";
    const client = createClient({ baseUrl: base, origin: "https://app.example" });
    return new Response(JSON.stringify(await client.api.getOrigin()));
  },
};
"#;

/// #3508's open fact: whether a framework's server, hosted in a Worker, may set `Origin`
/// on an outgoing `fetch`. A browser forbids it; a server runtime generally does not.
/// The generated client runs inside a real `workerd` here, calls a Bynk Worker with
/// `origin` set, and the route answers with the `Origin` header it received.
#[test]
fn a_worker_hosted_server_can_set_origin_through_the_client() {
    if skip_without_tools() {
        return;
    }
    if !tools::tool_exists("npx") && skip_workerd("`npx` is not on PATH") {
        return;
    }
    let dir = scratch("origin");
    let proj = dir.join("project");
    copy_project(&fixture("forms"), &proj);
    compile_worker(&proj, &dir);
    // The server Worker's repository: the client as a front end commits it, and a
    // handler that calls it.
    let web = dir.join("web");
    fs::create_dir_all(&web).unwrap();
    generate(&proj, &web, &[]);
    fs::write(web.join("worker.ts"), ORIGIN_WORKER).unwrap();
    fs::write(
        web.join("wrangler.toml"),
        "name = \"origin-probe\"\nmain = \"worker.ts\"\ncompatibility_date = \"2026-08-11\"\n",
    )
    .unwrap();

    let [bynk_port, web_port, bynk_inspector, web_inspector] = wrangler::ports::free_ports();
    let spawn = |cwd: &Path, port: u16, inspector: u16, log: &str| {
        let out_log = fs::File::create(dir.join(log)).unwrap();
        let err_log = out_log.try_clone().unwrap();
        wrangler::Wrangler::spawn(
            wrangler::npx()
                .args(["dev"])
                .args([
                    "--port",
                    &port.to_string(),
                    "--inspector-port",
                    &inspector.to_string(),
                ])
                .current_dir(cwd)
                .stdout(Stdio::from(out_log))
                .stderr(Stdio::from(err_log)),
            &dir.join("registry"),
        )
    };
    let (a, b) = (
        spawn(
            &dir.join("out/workers/forms"),
            bynk_port,
            bynk_inspector,
            "bynk.log",
        ),
        spawn(&web, web_port, web_inspector, "web.log"),
    );
    let (Ok(a), Ok(b)) = (a, b) else {
        skip_workerd("could not launch npx");
        return;
    };
    let mut wranglers = vec![a, b];
    let path = format!("/?base=http://127.0.0.1:{bynk_port}");
    let answered = std::cell::RefCell::new(None);
    if let Err(why) = wrangler::wait_until_all_ready(
        &[(&dir.join("bynk.log"), bynk_port)],
        (&dir.join("web.log"), web_port),
        &|| match get(web_port, &path) {
            Ok(r) if r.contains("\"status\":200") => {
                *answered.borrow_mut() = Some(r);
                true
            }
            _ => false,
        },
    ) {
        let logs = format!(
            "{}\n{}",
            fs::read_to_string(dir.join("bynk.log")).unwrap_or_default(),
            fs::read_to_string(dir.join("web.log")).unwrap_or_default()
        );
        wrangler::stop_all(&mut wranglers);
        skip_workerd(&format!(
            "{why} (likely no network to provision {})\n{logs}",
            wrangler::SPEC
        ));
        return;
    }
    let answer = answered.into_inner().expect("the probe answered");
    wrangler::stop_all(&mut wranglers);
    assert!(
        answer.contains(r#""body":"https://app.example""#),
        "a Worker's `fetch` set `Origin` through the client, and the route saw it:\n{answer}"
    );
    let _ = fs::remove_dir_all(&dir);
}

const RAW_DRIVER: &str = r#"
import { createClient, StreamFailure, API_CONTRACTS } from "./dist/api.js";
const sent = [];
const viaWorker = async (url, init) => {
  sent.push(`${init.method} ${url.replace(BASE, "")} ${new Headers(init.headers).get("content-type") ?? "-"} duplex=${init.duplex ?? "-"}`);
  return call(url, init);
};
const show = (label, v) => console.log(label, JSON.stringify(v));
const c = createClient({ baseUrl: BASE, fetch: viaWorker });
const text = new TextDecoder();

// A raw upload goes as the caller's bytes under the declared type, whatever the client was configured with.
const octets = new Uint8Array([0, 255, 7, 128]);
show("put", await c.api.putBlobByName("a b", octets));
show("put-buffer", await c.api.putBlobByName("x", octets.buffer));
show("put-blob", await c.api.putBlobByName("y", new Blob([octets])));
show("put-stream", await c.api.putBlobByName("z", new ReadableStream({ start(k) { k.enqueue(octets); k.close(); } })));
const json = createClient({ baseUrl: BASE, headers: { "content-type": "application/json" }, fetch: viaWorker });
show("put-configured", await json.api.putBlobByName("w", octets));
// `*/*` names no type, so the caller's own is sent.
show("any", await createClient({ baseUrl: BASE, headers: { "content-type": "image/png" }, fetch: viaWorker }).api.postAny(octets));

const logo = await c.api.getLogo();
show("logo", { status: logo.status, kind: logo.kind, ct: logo.contentType, weak: /^W\//.test(logo.etag ?? ""), text: await logo.text() });
const doc = await c.api.getDocByName("a");
show("doc", { status: doc.status, kind: doc.kind, ct: doc.contentType, etag: doc.etag, text: await doc.text() });
show("doc-gone", await c.api.getDocByName("gone"));
const chunks = await c.api.getChunks();
const read = [];
for await (const part of chunks.body) read.push(text.decode(part));
// Where the network cuts the body is its own business: the text is what is read.
show("chunks", { status: chunks.status, kind: chunks.kind, ct: chunks.contentType, read: read.join("") });
const made = await c.api.postMadeByName("n", "hi");
show("made", { status: made.status, kind: made.kind, text: await made.text() });
const taken = await c.api.postMadeByName("taken", "hi");
show("taken", { status: taken.status, kind: taken.kind, ct: taken.contentType, text: await taken.text() });
const bytes = await (await c.api.getLogo()).bytes();
show("bytes", Array.from(bytes));

// Server-sent events, as the server frames them.
const ticks = await c.api.getTicks();
const events = [];
for await (const e of ticks.events) events.push(e);
show("ticks", { status: ticks.status, kind: ticks.kind, events });
const live = await c.api.getFeedByMode("live");
const seen = [];
for await (const e of live.events) { seen.push(e); break; }
show("feed-break", seen);
show("feed-404", await c.api.getFeedByMode("off"));

// #3918: the same routes through the browser's own `EventSource`.
const opened = [];
globalThis.EventSource = class { constructor(url, init) { opened.push([url, init.withCredentials]); } };
const viaEs = createClient({ baseUrl: "http://x", credentials: "include" }).api;
viaEs.getTicksEventSource();
viaEs.getFeedByModeEventSource("a b");
c.api.getTicksEventSource();
show("es-opened", opened);
show("es-dot", viaEs.getFeedByModeEventSource(".."));
// #3918: a header or a cookie cannot reach the wire through an `EventSource`, so one that is set is
// refused, not dropped; an absent optional header is fine.
opened.length = 0;
show("es-header-absent", [viaEs.getResumeEventSource({}) instanceof globalThis.EventSource, opened.length]);
show("es-header-set", viaEs.getResumeEventSource({ lastId: "x" }));
show("es-cookie-set", viaEs.getWatchEventSource({ watcher: "w" }));
delete globalThis.EventSource;
show("es-none", Object.keys(viaEs.getTicksEventSource()));

// The parser, over frames cut at awkward places and written by other servers.
const framed = (parts, type = "text/event-stream") => async () =>
  new Response(new ReadableStream({ start(k) { for (const p of parts) k.enqueue(typeof p === "string" ? new TextEncoder().encode(p) : p); k.close(); } }), { status: 200, headers: { "content-type": type } });
const parse = async (parts) => {
  const r = await createClient({ baseUrl: "http://x", fetch: framed(parts) }).api.getTicks();
  const out = [];
  for await (const e of r.events) out.push(e);
  return out;
};
show("p-crlf", await parse(["data: a\r", "\ndata: b\r\n\r\ndata: c\r\r"]));
show("p-nospace", await parse(["data:x\n\ndata:  y\n\n"]));
show("p-fields", await parse([": hi\nevent: t\nid: 4\nretry: 1\ndata: d\n\n"]));
show("p-open", await parse(["data: kept\n\ndata: dropped"]));
show("p-empty", await parse(["data\n\ndata:\n\n"]));
const euro = new TextEncoder().encode("data: €\n\n");
show("p-split", await parse([euro.slice(0, 7), euro.slice(7)]));
// An element with a bare `\r`, which the server does not split on, is two lines to a reader.
const cr = await c.api.getCr();
const crEvents = [];
for await (const e of cr.events) crEvents.push(e);
show("cr", crEvents);
// A body that fails rejects the iteration.
const broken = createClient({ baseUrl: "http://x", fetch: async () => new Response(new ReadableStream({ start(k) { k.enqueue(new TextEncoder().encode("data: one\n\n")); }, pull() { throw new Error("cut"); } }), { status: 200, headers: { "content-type": "text/event-stream" } }) });
const cut = [];
try { for await (const e of (await broken.api.getTicks()).events) cut.push(e); } catch (e) { cut.push((e instanceof StreamFailure ? "StreamFailure " : "other ") + e.reason.message); }
show("p-cut", cut);
// A body that is not what the route answers is `Unexpected`, with the text.
show("html", await createClient({ baseUrl: "http://x", fetch: async () => new Response("<p>", { status: 200, headers: { "content-type": "text/html" } }) }).api.getTicks());
// A status that answers in JSON and raw: the content type says which. A missing one is raw.
const member = async (r) => r.status === 200 && r.kind === "Raw" ? { kind: r.kind, ct: r.contentType, text: await r.text() } : r;
show("either-json", await c.api.getEitherByMode("json"));
show("either-raw", await member(await c.api.getEitherByMode("raw")));
const stub = (make) => createClient({ baseUrl: "http://x", fetch: async () => make() }).api;
show("either-stub-json", await stub(() => new Response('"hello"', { status: 200, headers: { "content-type": "application/json; charset=utf-8" } })).getEitherByMode("x"));
show("either-stub-none", await member(await stub(() => new Response(new Uint8Array([104, 105]), { status: 200 })).getEitherByMode("x")));
// Two raw variants at a status are one member, and its `kind` is the first of them.
const two = await c.api.getTwoByMode("tagged");
show("two-tagged", { status: two.status, kind: two.kind, etag: two.etag, text: await two.text() });
const one = await c.api.getTwoByMode("raw");
show("two-raw", { status: one.status, kind: one.kind, etag: /^W\//.test(one.etag ?? "") ? "weak" : one.etag, text: await one.text() });
// A raw body beside a bodiless answer at its status that arrives as JSON is the bodiless one; as anything else, the raw one.
show("gone-bodiless", await c.api.getGoneByMode("raw"));
show("gone-stub", (await stub(() => new Response("x", { status: 404, headers: { "content-type": "text/plain" } })).getGoneByMode("raw")).kind);
// A `RawTagged` the server did not give an `ETag` is `Unexpected`, with the text.
show("tagged-no-etag", await stub(() => new Response("body", { status: 200, headers: { "content-type": "text/plain" } })).getDocByName("a"));
// The contract hash is sent only when asked for, and the server ignores it on every route.
show("contract-off", await c.api.getContract());
const sending = createClient({ baseUrl: BASE, fetch: viaWorker, sendContract: true });
const sentHash = await sending.api.getContract();
show("contract-on", { same: sentHash.body === API_CONTRACTS.api.routes.getContract, len: sentHash.body.length });
show("contract-other-routes", [(await sending.api.getOrigin()).status, (await sending.api.putBlobByName("a", new Uint8Array([1]))).status, (await (await sending.api.getTicks()).events[Symbol.asyncIterator]().next()).value]);
console.log("requests", JSON.stringify(sent));
"#;

/// What the raw driver prints, checked: the same under a Worker called in process and
/// under workerd, where a response is a real HTTP one.
fn check_raw(out: &str) {
    let v = |label: &str| line(out, label);
    // The octets `0 255 7 128`, whichever way the caller held them.
    for (label, name) in [
        ("put", "a b"),
        ("put-buffer", "x"),
        ("put-blob", "y"),
        ("put-stream", "z"),
        ("put-configured", "w"),
    ] {
        assert_eq!(
            v(label),
            format!(r#"{{"status":200,"kind":"Ok","body":"{name} AP8HgA=="}}"#)
        );
    }
    assert_eq!(v("any"), r#"{"status":200,"kind":"Ok","body":"AP8HgA=="}"#);
    assert_eq!(
        v("logo"),
        r#"{"status":200,"kind":"Raw","ct":"image/svg+xml","weak":true,"text":"<svg/>"}"#
    );
    assert_eq!(
        v("doc"),
        r#"{"status":200,"kind":"RawTagged","ct":"text/plain","etag":"\"v1\"","text":"doc a"}"#
    );
    assert_eq!(v("doc-gone"), r#"{"status":404,"kind":"NotFound"}"#);
    // The empty chunk the server skips is not on the wire.
    assert_eq!(
        v("chunks"),
        r#"{"status":200,"kind":"RawStreamed","ct":"text/csv","read":"one,two,three"}"#
    );
    assert_eq!(
        v("made"),
        r#"{"status":201,"kind":"RawCreated","text":"made n: hi"}"#
    );
    assert_eq!(
        v("taken"),
        r#"{"status":409,"kind":"RawError","ct":"text/plain","text":"taken"}"#
    );
    assert_eq!(v("bytes"), "[60,115,118,103,47,62]");
    // An event is an element, a multi-line one keeping its lines, an empty one kept.
    assert_eq!(
        v("ticks"),
        r#"{"status":200,"kind":"Streaming","events":["tick-1","two\nlines","","tick-4"]}"#
    );
    assert_eq!(v("feed-break"), r#"["a"]"#);
    assert_eq!(v("feed-404"), r#"{"status":404,"kind":"NotFound"}"#);
    // An `EventSource` opens the URL a call would fetch, with credentials only when asked.
    let opened = v("es-opened");
    assert!(
        opened.contains(r#"["http://x/ticks",true]"#)
            && opened.contains(r#"["http://x/feed/a%20b",true]"#),
        "{opened}"
    );
    assert!(opened.ends_with(r#",false]]"#), "{opened}");
    assert!(
        v("es-dot").contains(r#""kind":"Invalid""#),
        "{}",
        v("es-dot")
    );
    assert_eq!(v("es-header-absent"), "[true,1]");
    let header = v("es-header-set");
    assert!(
        header.contains(r#""kind":"Invalid""#) && header.contains("headers.last-event-id"),
        "{header}"
    );
    let cookie = v("es-cookie-set");
    assert!(
        cookie.contains(r#""kind":"Invalid""#) && cookie.contains(r#""path":"cookies""#),
        "{cookie}"
    );
    assert_eq!(v("es-none"), r#"["kind","cause"]"#);
    // The server splits an element on `\n` only, so a `\r` in one ends a line for a reader.
    assert_eq!(v("cr"), r#"["a"]"#);
    // The parser follows the specification.
    assert_eq!(v("p-crlf"), r#"["a\nb","c"]"#);
    assert_eq!(v("p-nospace"), r#"["x"," y"]"#);
    assert_eq!(v("p-fields"), r#"["d"]"#);
    assert_eq!(v("p-open"), r#"["kept"]"#);
    assert_eq!(v("p-empty"), r#"["",""]"#);
    assert_eq!(v("p-split"), r#"["\u20ac"]"#.replace("\\u20ac", "€"));
    assert_eq!(v("p-cut"), r#"["one","StreamFailure cut"]"#);
    assert_eq!(
        v("either-json"),
        r#"{"status":200,"kind":"Ok","body":"json"}"#
    );
    assert_eq!(
        v("either-raw"),
        r#"{"kind":"Raw","ct":"text/plain","text":"plain"}"#
    );
    assert_eq!(
        v("either-stub-json"),
        r#"{"status":200,"kind":"Ok","body":"hello"}"#
    );
    assert_eq!(
        v("either-stub-none"),
        r#"{"kind":"Raw","ct":"","text":"hi"}"#
    );
    assert_eq!(
        v("two-tagged"),
        r#"{"status":200,"kind":"Raw","etag":"\"t1\"","text":"t"}"#
    );
    assert_eq!(
        v("two-raw"),
        r#"{"status":200,"kind":"Raw","etag":"weak","text":"r"}"#
    );
    assert_eq!(v("gone-bodiless"), r#"{"status":404,"kind":"NotFound"}"#);
    assert_eq!(v("gone-stub"), r#""RawError""#);
    assert_eq!(
        v("tagged-no-etag"),
        r#"{"kind":"Unexpected","status":200,"text":"body"}"#
    );
    assert_eq!(
        v("contract-off"),
        r#"{"status":200,"kind":"Ok","body":"none"}"#
    );
    assert_eq!(v("contract-on"), r#"{"same":true,"len":16}"#);
    assert_eq!(v("contract-other-routes"), r#"[200,200,"tick-1"]"#);
    assert_eq!(
        v("html"),
        r#"{"kind":"Unexpected","status":200,"text":"<p>"}"#
    );
}

#[test]
fn raw_bodies_and_streams_round_trip_against_a_real_worker() {
    if skip_without_tools() {
        return;
    }
    let dir = scratch("raw");
    let proj = dir.join("project");
    copy_project(&fixture("raw"), &proj);
    generate(&proj, &dir, &[]);
    compile_worker(&proj, &dir);
    tsc(&dir, "the `raw` client");
    let driver = format!("{IN_PROCESS}{RAW_DRIVER}").replace("workers/rich/", "workers/raw/");
    let out = run_driver(&dir, &driver, &[]);
    check_raw(&out);
    // A raw upload names its type, and only a stream asks for `duplex`.
    let requests = line(&out, "requests");
    assert!(
        requests.contains("PUT /blob/a%20b application/vnd.bynk.blob duplex=-"),
        "{requests}"
    );
    assert!(
        requests.contains("PUT /blob/z application/vnd.bynk.blob duplex=half"),
        "{requests}"
    );
    assert!(
        requests.contains("PUT /blob/w application/vnd.bynk.blob duplex=-"),
        "{requests}"
    );
    assert!(
        requests.contains("POST /any image/png duplex=-"),
        "{requests}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The same driver over real HTTP, where a streamed body is chunked by the network and
/// a stream upload is a real one.
#[test]
fn raw_bodies_and_streams_round_trip_under_workerd() {
    if skip_without_tools() {
        return;
    }
    if !tools::tool_exists("npx") && skip_workerd("`npx` is not on PATH") {
        return;
    }
    let dir = scratch("raw-workerd");
    let proj = dir.join("project");
    copy_project(&fixture("raw"), &proj);
    generate(&proj, &dir, &[]);
    compile_worker(&proj, &dir);
    tsc(&dir, "the `raw` client");
    let Some((mut wranglers, port)) = boot_workerd(&dir, "raw", "/logo") else {
        return;
    };
    let base = format!("http://127.0.0.1:{port}");
    let out = run_driver(
        &dir,
        &format!("{NETWORK}{RAW_DRIVER}"),
        &[("BASE", base.as_str())],
    );
    wrangler::stop_all(&mut wranglers);
    check_raw(&out);
    let _ = fs::remove_dir_all(&dir);
}

/// The front end's Worker: the generated client over a Service Binding to the Bynk
/// Worker. It reports what each call saw, as JSON.
const BINDING_WORKER: &str = r#"import { createClient, viaBinding, StreamFailure, API_CONTRACTS } from "./api";

interface Env { readonly API: { fetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> } }

export default {
  async fetch(_request: Request, env: Env): Promise<Response> {
    const out: Record<string, unknown> = {};
    const api = createClient({ ...viaBinding(env.API) }).api;
    out.put = await api.putBlobByName("a", new Uint8Array([0, 255, 7, 128]));
    const logo = await api.getLogo();
    out.logo = logo.status === 200 ? { kind: logo.kind, text: await logo.text() } : logo;
    const ticks = await api.getTicks();
    const events: string[] = [];
    if (ticks.status === 200) for await (const e of ticks.events) events.push(e);
    out.events = events;
    out.originNone = await api.getOrigin();
    out.origin = await createClient({ ...viaBinding(env.API), origin: "https://app.example" }).api.getOrigin();
    out.cookie = await createClient({ ...viaBinding(env.API), headers: { cookie: "sid=1" } }).api.getCookie();
    // The method detached from its binding is the trap `viaBinding` avoids.
    const detached = await createClient({ baseUrl: "https://bynk.invalid", fetch: env.API.fetch as typeof fetch }).api.getOrigin();
    out.detached = detached.kind;
    // A binding call has no preflight, so `viaBinding` sends the contract by itself; a caller can turn it off.
    const sent = await api.getContract();
    out.contractSent = sent.status === 200 && sent.body === API_CONTRACTS.api.routes.getContract;
    const off = await createClient({ ...viaBinding(env.API), sendContract: false }).api.getContract();
    out.contractOff = off.status === 200 ? off.body : off.status;
    out.streamFailureExported = typeof StreamFailure;
    return new Response(JSON.stringify(out), { headers: { "content-type": "application/json" } });
  },
};
"#;

/// #3508 slice BC5: the client over a Service Binding, as a framework server on
/// Cloudflare calls a Bynk Worker. Two `wrangler dev` processes under real `workerd`,
/// the front end bound to the API by `[[services]]`.
#[test]
fn the_client_calls_a_bynk_worker_through_a_service_binding() {
    if skip_without_tools() {
        return;
    }
    if !tools::tool_exists("npx") && skip_workerd("`npx` is not on PATH") {
        return;
    }
    let dir = scratch("binding");
    let proj = dir.join("project");
    copy_project(&fixture("raw"), &proj);
    compile_worker(&proj, &dir);
    let api_dir = dir.join("out/workers/raw");
    let api_toml = fs::read_to_string(api_dir.join("wrangler.toml")).unwrap();
    let api_name = api_toml
        .lines()
        .find_map(|l| l.strip_prefix("name = "))
        .expect("the Worker has a name")
        .trim()
        .trim_matches('"')
        .to_string();
    let web = dir.join("web");
    fs::create_dir_all(&web).unwrap();
    generate(&proj, &web, &[]);
    fs::write(web.join("worker.ts"), BINDING_WORKER).unwrap();
    fs::write(
        web.join("wrangler.toml"),
        format!(
            "name = \"binding-probe\"\nmain = \"worker.ts\"\ncompatibility_date = \"2026-08-11\"\n\n[[services]]\nbinding = \"API\"\nservice = \"{api_name}\"\n"
        ),
    )
    .unwrap();

    let [api_port, web_port, api_inspector, web_inspector] = wrangler::ports::free_ports();
    let spawn = |cwd: &Path, port: u16, inspector: u16, log: &str| {
        let out_log = fs::File::create(dir.join(log)).unwrap();
        let err_log = out_log.try_clone().unwrap();
        wrangler::Wrangler::spawn(
            wrangler::npx()
                .args(["dev"])
                .args([
                    "--port",
                    &port.to_string(),
                    "--inspector-port",
                    &inspector.to_string(),
                ])
                .current_dir(cwd)
                .stdout(Stdio::from(out_log))
                .stderr(Stdio::from(err_log)),
            &dir.join("registry"),
        )
    };
    // The API first, so the binding finds it in the dev registry.
    let Ok(a) = spawn(&api_dir, api_port, api_inspector, "api.log") else {
        skip_workerd("could not launch npx");
        return;
    };
    let mut wranglers = vec![a];
    if let Err(why) = wrangler::wait_until_ready(&dir.join("api.log"), api_port, &|| {
        get(api_port, "/logo").is_ok_and(|r| r.contains("200 OK"))
    }) {
        let log = fs::read_to_string(dir.join("api.log")).unwrap_or_default();
        wrangler::stop_all(&mut wranglers);
        skip_workerd(&format!("the API Worker: {why}\n{log}"));
        return;
    }
    let Ok(b) = spawn(&web, web_port, web_inspector, "web.log") else {
        wrangler::stop_all(&mut wranglers);
        skip_workerd("could not launch npx");
        return;
    };
    wranglers.push(b);
    let answered = std::cell::RefCell::new(None);
    if let Err(why) = wrangler::wait_until_ready(&dir.join("web.log"), web_port, &|| match get(
        web_port, "/",
    ) {
        Ok(r) if r.contains("\"put\"") => {
            *answered.borrow_mut() = Some(r);
            true
        }
        _ => false,
    }) {
        let logs = format!(
            "{}\n{}",
            fs::read_to_string(dir.join("api.log")).unwrap_or_default(),
            fs::read_to_string(dir.join("web.log")).unwrap_or_default()
        );
        wrangler::stop_all(&mut wranglers);
        skip_workerd(&format!("the front end: {why}\n{logs}"));
        return;
    }
    let answer = answered.into_inner().expect("the probe answered");
    wrangler::stop_all(&mut wranglers);
    let json = answer.split("\r\n\r\n").nth(1).unwrap_or(&answer);
    let v: serde_json::Value =
        serde_json::from_str(json.trim()).unwrap_or_else(|e| panic!("{e}: {answer}"));
    assert_eq!(v["put"]["body"], "a AP8HgA==", "{answer}");
    assert_eq!(v["logo"]["text"], "<svg/>", "{answer}");
    assert_eq!(
        v["events"],
        serde_json::json!(["tick-1", "two\nlines", "", "tick-4"]),
        "{answer}"
    );
    // A binding call carries no `Origin` of its own; the option sets one.
    assert_eq!(v["originNone"]["body"], "none", "{answer}");
    assert_eq!(v["origin"]["body"], "https://app.example", "{answer}");
    assert_eq!(v["cookie"]["body"], "sid=1", "{answer}");
    assert_eq!(
        v["detached"], "Transport",
        "a detached `fetch` fails as a Transport failure: {answer}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// #3906: the connection of a `from websocket` service, against the real Worker under `wrangler dev`.
/// Node 22's own `WebSocket` is the platform one the client defaults to.
const WS_DRIVER: &str = r#"
import { createHmac } from "node:crypto";
import { createClient } from "./dist/api.js";

const BASE = process.env.BASE;
const SECRET = "ws-test-secret";
const assert = (cond, msg) => { if (!cond) throw new Error(`assertion failed: ${msg}`); };
const b64 = (o) => Buffer.from(JSON.stringify(o)).toString("base64url");
const token = (sub, expIn = 3600, secret = SECRET) => {
  const head = b64({ alg: "HS256", typ: "JWT" });
  const body = b64({ sub, exp: Math.floor(Date.now() / 1000) + expIn });
  return `${head}.${body}.${createHmac("sha256", secret).update(`${head}.${body}`).digest("base64url")}`;
};
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const next = (conn) => conn.frames[Symbol.asyncIterator]().next();

const api = createClient({ baseUrl: BASE });

// The HTTP route of the same client still works beside the connection.
const health = await api.health.getHealth();
assert(health.kind === "Ok" && health.body === "up", "the HTTP route answers");

// A route parameter the server would refuse is refused before any connection is made.
const badRoom = api.gateway.connect("Not A Room", { bearer: token("ada") });
assert(badRoom.kind === "Invalid", "an invalid route parameter is a CallFailure");

// A good connection: the welcome frame, typed frames both ways, an Option as `T | null`.
const conn = api.gateway.connect("lobby", { bearer: token("ada") });
assert(conn.state !== undefined, "connect returns a connection");
const frames = conn.frames[Symbol.asyncIterator]();
await conn.opened;
const welcome = await frames.next();
assert(welcome.value.text === "welcome" && welcome.value.note === null, "the welcome frame is decoded, None as null");
assert(conn.send({ text: "hi", reply: "r" }) === null, "a valid frame is sent");
const echo = await frames.next();
assert(echo.value.text === "hi" && echo.value.note === "r", "the echo is decoded, Some as a value");
assert(conn.send({ text: "again", reply: null }) === null, "a null Option is sent");
const echo2 = await frames.next();
assert(echo2.value.note === null, "None comes back as null");

// A frame the server would refuse is refused locally, and never reaches it.
assert(conn.send({ text: "", reply: null }).kind === "Invalid", "an empty text is refused locally");
assert(conn.send({ text: "x".repeat(21), reply: null }).kind === "Invalid", "an over-long text is refused locally");
assert(conn.send({ text: "after", reply: null }) === null, "the connection is still good");
const after = await frames.next();
assert(after.value.text === "after", "only the valid frames reached the server");
conn.close(1000, "bye");
const closed = await new Promise((resolve) => conn.onClose(resolve));
assert(closed.code === 1000 && closed.willReconnect === false, "a close the client made does not reconnect");
assert(conn.state === "closed", "the connection is closed");

// A token that is a function is read at connect; a forged one is refused at the edge.
const viaFunction = api.gateway.connect("lobby", { bearer: async () => token("grace") });
await viaFunction.opened;
viaFunction.close();
const forged = api.gateway.connect("lobby", { bearer: token("ada", 3600, "another-secret") });
let refused = false;
await forged.opened.catch(() => { refused = true; });
assert(refused && forged.state === "closed", "a forged token never opens");

// An expired token: the server closes with 1008 at the next frame, and the client reconnects with a fresh one.
let issued = 0;
const short = api.gateway.connect("lobby", { bearer: token("ada", 2) }, {
  reconnect: { credentials: async () => ({ bearer: token("ada", 3600) }), backoff: () => 50 },
});
const shortFrames = short.frames[Symbol.asyncIterator]();
await short.opened;
await shortFrames.next();
const reconnected = new Promise((resolve) => short.onReconnect(resolve));
const closes = [];
short.onClose((c) => closes.push(c));
await wait(3000);
short.send({ text: "late", reply: null });
assert((await reconnected) === 1, "the client reconnected once");
assert(closes.length === 1 && closes[0].code === 1008 && closes[0].reason === "authentication expired" && closes[0].willReconnect === true, "it was the expiry close");
assert(short.state === "open", "and is open again");
const welcome2 = await shortFrames.next();
assert(welcome2.value.text === "welcome", "the new connection's welcome frame reaches the same iterator");
short.send({ text: "fresh", reply: null });
assert((await shortFrames.next()).value.text === "fresh", "frames flow after the reconnect");
short.close();

// A cookie service offers no credential of its own: the upgrade carries no subprotocol, and with no session
// the Worker refuses it.
const lounge = api.lounge.connect("main");
let noSession = false;
await lounge.opened.catch(() => { noSession = true; });
assert(noSession, "a cookie upgrade with no session or origin is refused");

console.log("ALL OK");
"#;

#[test]
fn a_websocket_connection_is_typed_both_ways_against_a_real_worker() {
    if skip_without_tools() {
        return;
    }
    if !tools::tool_exists("npx") && skip_workerd("`npx` is not on PATH") {
        return;
    }
    let dir = scratch("ws");
    let proj = dir.join("project");
    copy_project(&fixture("ws"), &proj);
    generate(&proj, &dir, &[]);
    compile_worker(&proj, &dir);
    tsc(&dir, "the `ws` client");
    fs::write(
        dir.join("out/workers/chat/.dev.vars"),
        "CHAT_SECRET=ws-test-secret\n",
    )
    .unwrap();

    let Some((mut wranglers, port)) = boot_workerd(&dir, "chat", "/health") else {
        return;
    };
    let base = format!("http://127.0.0.1:{port}");
    let out = run_driver(&dir, WS_DRIVER, &[("BASE", base.as_str())]);
    wrangler::stop_all(&mut wranglers);
    assert!(out.contains("ALL OK"), "{out}");
    let _ = fs::remove_dir_all(&dir);
}

/// #3906: a real browser connects to a `SignedCookie` service. Only Chromium can show that the browser
/// itself sends the session cookie and the `Origin` on the upgrade, which the injected constructor of a Node
/// test cannot prove: a page that signed in over HTTP opens the socket with no credential, and a page at an
/// origin the service does not list is refused.
const BROWSER_DRIVER: &str = r#"
import http from "node:http";
import fs from "node:fs";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const [pagePort, base] = [process.env.PAGE_PORT, process.env.BASE];
const WAIT_MS = 60000;

const server = http.createServer((req, res) => {
  if (req.url === "/api.js") {
    res.writeHead(200, { "content-type": "text/javascript" }).end(fs.readFileSync("dist/api.js"));
  } else {
    res.writeHead(200, { "content-type": "text/html" }).end("<!doctype html><meta charset=utf-8><title>ws</title>");
  }
});
await new Promise((resolve) => server.listen(Number(pagePort), "127.0.0.1", resolve));

const browser = await chromium.launch({ timeout: WAIT_MS });
try {
  const context = await browser.newContext();
  const run = (origin, signIn) => async () => {
    const tab = await context.newPage();
    await tab.goto(`${origin}/`, { timeout: WAIT_MS });
    const seen = await tab.evaluate(async ({ base, signIn, waitMs }) => {
      const within = (what, p) => Promise.race([p, new Promise((_, no) => setTimeout(() => no(new Error(`timed out waiting for ${what}`)), waitMs))]);
      const { createClient } = await import("/api.js");
      const api = createClient({ baseUrl: base, credentials: "include" });
      const out = {};
      if (signIn) out.login = (await within("the sign-in", api.sign.postLogin({ name: "ada" }))).kind;
      const conn = api.lounge.connect("main");
      if ("kind" in conn) return { ...out, failure: conn.kind };
      let opened = true;
      await within("the upgrade", conn.opened.catch(() => { opened = false; }));
      out.opened = opened;
      if (opened) {
        const first = await within("the welcome frame", conn.frames[Symbol.asyncIterator]().next());
        out.welcome = first.value.text;
        conn.close();
      }
      return out;
    }, { base, signIn, waitMs: WAIT_MS });
    await tab.close();
    return seen;
  };
  const listed = await run(`http://127.0.0.1:${pagePort}`, true)();
  // The same browser, the session it now holds, from a page at an origin the service does not list.
  const unlisted = await run(`http://localhost:${pagePort}`, false)();
  console.log("RESULT " + JSON.stringify({ listed, unlisted }));
} finally {
  await browser.close();
  server.close();
}
process.exit(0);
"#;

#[test]
fn a_browser_connects_a_cookie_service_with_the_session_it_signed_in_with() {
    if skip_without_tools() {
        return;
    }
    let has_playwright = Command::new("node")
        .args(["-e", "require('playwright').chromium"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !has_playwright {
        eprintln!("\n!!! WS BROWSER VERIFICATION SKIPPED !!!\nPlaywright is not installed\n");
        if std::env::var("BYNK_REQUIRE_BROWSER").is_ok() {
            panic!("BYNK_REQUIRE_BROWSER is set but Playwright is not installed");
        }
        return;
    }
    if !tools::tool_exists("npx") && skip_workerd("`npx` is not on PATH") {
        return;
    }
    // The page's origin, which the service lists exactly: a port chosen now.
    let page_port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let dir = scratch("ws-browser");
    let proj = dir.join("project");
    copy_project(&fixture("ws"), &proj);
    let lobby = proj.join("src/lobby.bynk");
    let text = fs::read_to_string(&lobby).unwrap().replace(
        "https://app.example.com",
        &format!("http://127.0.0.1:{page_port}"),
    );
    fs::write(&lobby, text).unwrap();
    generate(&proj, &dir, &[]);
    compile_worker(&proj, &dir);
    tsc(&dir, "the `ws` client");
    // 32 bytes, base64url: the key the session cookie is signed with.
    fs::write(
        dir.join("out/workers/lobby/.dev.vars"),
        "SESSION_KEY=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n",
    )
    .unwrap();
    let Some((mut wranglers, port)) = boot_workerd(&dir, "lobby", "/health") else {
        return;
    };
    fs::write(dir.join("dist/package.json"), "{\"type\":\"module\"}\n").unwrap();
    fs::write(dir.join("browser.mjs"), BROWSER_DRIVER).unwrap();
    let run = Command::new("node")
        .arg("browser.mjs")
        .current_dir(&dir)
        .env("PAGE_PORT", page_port.to_string())
        .env("BASE", format!("http://127.0.0.1:{port}"))
        .output()
        .expect("run node");
    wrangler::stop_all(&mut wranglers);
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(run.status.success(), "the browser driver failed:\n{log}");
    let line = log
        .lines()
        .find_map(|l| l.strip_prefix("RESULT "))
        .unwrap_or_else(|| panic!("the driver printed no result:\n{log}"));
    let seen: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(
        seen["listed"],
        serde_json::json!({ "login": "Created", "opened": true, "welcome": "welcome" }),
        "a page at the listed origin that signed in opens the socket with no credential of its own\n{log}"
    );
    assert_eq!(
        seen["unlisted"],
        serde_json::json!({ "opened": false }),
        "a page at an origin the service does not list is refused, session or not\n{log}"
    );
    let _ = fs::remove_dir_all(&dir);
}
