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
use std::time::{Duration, Instant};

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
        ("shapes", shapes_project()),
        ("plain", plain_project()),
    ] {
        let dir = scratch(&format!("tsc-{name}"));
        let proj = dir.join("project");
        copy_project(&project, &proj);
        generate(&proj, &dir, &[]);
        tsc(&dir, &format!("the `{name}` client"));
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

    // Its own band, clear of the other smokes'.
    let pid = std::process::id();
    let port = 61_000 + 2 * (pid % 2_000) as u16;
    let log = dir.join("wrangler.log");
    let out_log = fs::File::create(&log).unwrap();
    let err_log = out_log.try_clone().unwrap();
    let spawned = wrangler::Wrangler::spawn(
        tools::base_command("npx")
            .args(["-y", wrangler::SPEC, "dev"])
            .args([
                "--port",
                &port.to_string(),
                "--inspector-port",
                &(port + 1).to_string(),
            ])
            .current_dir(dir.join("out/workers/rich"))
            .stdout(Stdio::from(out_log))
            .stderr(Stdio::from(err_log)),
        &dir.join("registry"),
    );
    let Ok(wrangler) = spawned else {
        skip_workerd("could not launch npx");
        return;
    };
    let mut wranglers = vec![wrangler];
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if get(port, "/search?q=up").is_ok_and(|r| r.contains("200 OK")) {
            break;
        }
        if Instant::now() > deadline {
            let log = fs::read_to_string(&log).unwrap_or_default();
            wrangler::stop_all(&mut wranglers);
            skip_workerd(&format!(
                "the Worker did not answer within the boot window (likely no network to provision {})\n{log}",
                wrangler::SPEC
            ));
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
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
