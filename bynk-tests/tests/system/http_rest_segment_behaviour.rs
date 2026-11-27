//! #3752 (pass-through HTTP, slice 1): the `*name` rest segment, run through the
//! emitted Workers router in-process. Compiles `3752_rest_segments` to
//! JavaScript with the CLI, then a Node driver sends real `Request`s and checks
//! every row of the track's §5.2 (what a rest binds) and §5.4 (precedence), the
//! method semantics of §5.5 (`HEAD`, `405` with `Allow`, `OPTIONS`, CORS) on a
//! rest route, and that a root catch-all never answers under `/_bynk/`.
//!
//! The fixture's own suites cover the unit and system tiers (`behaviour_fixtures.rs`),
//! and `binding_workerd.rs` the R2 keys on workerd. This skips loudly when `node` is
//! missing; `BYNK_REQUIRE_TSC=1` turns the skip into a failure.

#[path = "../require/mod.rs"]
mod require;
#[path = "../tools/mod.rs"]
mod tools;

use std::fs;
use std::path::Path;
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

const DRIVER: &str = r#"import worker from "./out/workers/web/index.js";
const env = {};
const failures = [];
function check(cond, msg) { if (!cond) failures.push(msg); }
async function call(method, path, headers = {}) {
  const r = await worker.fetch(new Request(`https://x.test${path}`, { method, headers }), env);
  return { status: r.status, headers: r.headers, text: await r.text() };
}
async function body(path) {
  const r = await call("GET", path);
  return r.status === 200 ? JSON.parse(r.text) : `${r.status}`;
}
// §5.2, every row, for a `/raw/*path` route.
for (const [path, want] of [
  ["/raw", "[]"], ["/raw/", "[/]"], ["/raw/posts/money/", "[/posts/money/]"], ["/raw/posts/money", "[/posts/money]"],
  ["/raw//x", "[//x]"], ["/raw/caf%C3%A9", "[/caf%C3%A9]"], ["/raw/a%2Fb", "[/a%2Fb]"],
]) check((await body(path)) === want, `§5.2 ${path}: ${await body(path)} != ${want}`);
// §5.4, every line.
check((await body("/files/index.json")) === "index", "literal beats rest");
check((await body("/a/b/c")) === "y [/c]", "`/a/b/*y` beats `/a/*x` for /a/b/c");
check((await body("/a/1")) === "id 1", "`/a/:id` beats `/a/*x` for /a/1");
check((await body("/a/1/2")) === "x [/1/2]", "/a/1/2 goes to /a/*x");
check((await body("/a")) === "x []", "/a/*x answers /a");
check((await body("/blog")) === "blog literal", "/blog beats /blog/*path for /blog");
check((await body("/blog/")) === "blog literal", "and for /blog/");
check((await body("/blog/x")) === "rest [/x]", "/blog/x is the rest's");
// D4: a dot segment the host didn't normalise is a 400; a malformed escape inside the rest isn't.
check((await body("/raw/%E0%A4%A")) === "[/%E0%A4%A]", "a malformed escape in the rest reaches the handler");
check((await call("GET", "/t/%E0%A4%A/x")).status === 400, "a malformed escape before the rest is 400");
// §5.5: HEAD, 405 + Allow, OPTIONS on a rest route.
let r = await call("HEAD", "/raw/a/b");
check(r.status === 200 && r.text === "", "HEAD from GET on a rest route");
r = await call("POST", "/raw/a/b");
check(r.status === 405 && r.headers.get("allow") === "GET, HEAD, OPTIONS", `405 + Allow on a rest route: ${r.status} ${r.headers.get("allow")}`);
r = await call("OPTIONS", "/raw/a/b");
check(r.status === 204 && r.headers.get("allow") === "GET, HEAD, OPTIONS", "OPTIONS on a rest route");
r = await call("PUT", "/files/x/y");
check(r.status !== 405, "PUT /files/*key is routed");
// CORS on a rest route: the preflight and the stamp.
r = await call("OPTIONS", "/api/docs/a/b", { origin: "https://app.example.com", "access-control-request-method": "GET" });
check(r.status === 204 && r.headers.get("access-control-allow-origin") === "https://app.example.com", `CORS preflight on a rest route: ${r.status}`);
r = await call("GET", "/api/docs/a/b", { origin: "https://app.example.com" });
check(r.headers.get("access-control-allow-origin") === "https://app.example.com" && JSON.parse(r.text) === "doc [/a/b]", "CORS stamp on a rest route");
// The root catch-all answers what nothing else matches, never `/_bynk/`.
check((await body("/elsewhere/x")) === "spa [/elsewhere/x]", "root catch-all");
check((await body("/")) === "spa [/]", "root catch-all at /");
r = await call("GET", "/_bynk/inspect");
check(!r.text.includes("spa ["), `/_bynk/ never reaches the catch-all: ${r.status} ${r.text.slice(0, 80)}`);
r = await call("POST", "/_bynk/call/files");
check(!r.text.includes("spa ["), "/_bynk/call never reaches the catch-all");
if (failures.length) { console.error(failures.join("\n")); process.exit(1); }
console.log("ALL OK");
"#;

#[test]
fn rest_segments_route_on_the_workers_entry() {
    if !tools::tool_exists("node") {
        if require::is_required(REQUIRE_ENV) {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        eprintln!("\n!!! REST SEGMENT BEHAVIOUR SKIPPED !!!\n`node` is not on PATH.\n");
        return;
    }
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive/3752_rest_segments");
    let tmp = std::env::temp_dir().join(format!("bynk-rest-segments-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let compiled = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(["compile", "--target", "workers", "--emit", "js", "-o"])
        .arg(tmp.join("out"))
        .arg(&fixture)
        .output()
        .expect("run bynk");
    assert!(
        compiled.status.success(),
        "the rest-segment fixture must compile:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    fs::write(tmp.join("package.json"), r#"{"type":"module"}"#).unwrap();
    fs::write(tmp.join("driver.mjs"), DRIVER).unwrap();
    let ran = Command::new("node")
        .arg("driver.mjs")
        .current_dir(&tmp)
        .output()
        .expect("run node");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(
        ran.status.success() && log.contains("ALL OK"),
        "the rest-segment driver must run green:\n{log}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
