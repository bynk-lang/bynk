//! #1886: `Fetch.send` carries extra request headers and returns the
//! response's.
//!
//! The goldens pin the bindings' text; this pins what they do, against a stub
//! `globalThis.fetch` that records each call's headers and answers with an
//! `ETag`. For both the `cloudflare` and the `node` binding:
//! - a `headers` entry reaches the wire, its name lowercased;
//! - `headers` may supply Content-Type / Authorization while the typed slot is
//!   `None`, but naming one (in any case) while the slot is `Some` is
//!   `Err(InvalidHeader)` and nothing is sent;
//! - each of the nine platform-owned names (sent upper-cased), a duplicate
//!   name differing only in case, an illegal header name or value, and an
//!   illegal value in either typed slot are refused the same way;
//! - `Response.headers` carries the response's headers, keys lowercased, with
//!   a repeated header joined by ", " — `set-cookie` included, whether the
//!   runtime has `Headers.getSetCookie()` or not (the `legacySetCookie` case
//!   hides it, standing in for a runtime without it).
//!
//! The Workers pass also drives a compiled route end to end, so a Bynk `match`
//! on `Err(InvalidHeader)` and a `r.headers.get("etag")` read are exercised
//! through the real binding, not only through direct `FetchProvider` calls.
//!
//! Skips loudly without `node`; `BYNK_REQUIRE_TSC=1` turns the skip into a
//! failure, as the other Node-driven suites do.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

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

const CONTEXT: &str = "context net.probe

consumes bynk { Fetch }

service api from http {
\ton GET(\"/conflict\") () -> Effect[HttpResult[String]] by Visitor given Fetch {
\t\tlet res <- Fetch.send(Request {
\t\t\tmethod: Get,
\t\t\turl: \"https://upstream.test/conflict\",
\t\t\tcontentType: Some(\"text/plain\"),
\t\t\tauthorization: None,
\t\t\theaders: Map.empty().insert(\"Content-Type\", \"application/json\"),
\t\t\tbody: None,
\t\t})
\t\tmatch res {
\t\t\tOk(r) => Ok(\"ok \\(r.status)\")
\t\t\tErr(InvalidHeader) => Ok(\"invalid\")
\t\t\tErr(_) => Ok(\"other\")
\t\t}
\t}

\ton GET(\"/etag\") () -> Effect[HttpResult[String]] by Visitor given Fetch {
\t\tlet res <- Fetch.send(Request {
\t\t\tmethod: Get,
\t\t\turl: \"https://upstream.test/etag\",
\t\t\tcontentType: None,
\t\t\tauthorization: None,
\t\t\theaders: Map.empty().insert(\"User-Agent\", \"bynk-test/1\"),
\t\t\tbody: None,
\t\t})
\t\tmatch res {
\t\t\tOk(r) => match r.headers.get(\"etag\") {
\t\t\t\tSome(tag) => Ok(\"etag \\(tag)\")
\t\t\t\tNone => Ok(\"no etag\")
\t\t\t}
\t\t\tErr(_) => Ok(\"other\")
\t\t}
\t}
}
";

/// Replaces `globalThis.fetch` with a recorder, then sends each case through
/// the binding's `FetchProvider` and prints one `CASE` line per case: the
/// result tag, the headers the stub saw (or `unsent`), and any response
/// headers. `ROUTES` (Workers pass only) drives the compiled routes.
fn driver(binding: &str, runtime: &str, bynk: &str, worker: Option<&str>) -> String {
    let forbidden = format!("{FORBIDDEN:?}");
    let worker_import = worker
        .map(|w| format!("import worker from \"{w}\";\n"))
        .unwrap_or_default();
    let routes = if worker.is_some() {
        "for (const p of [\"/conflict\", \"/etag\"]) {
  const before = calls.length;
  const res = await worker.fetch(new Request(\"http://probe.example\" + p), {});
  console.log(`ROUTE ${p} ${res.status} ${await res.text()} sent=${calls.length - before}`);
}
"
    } else {
        ""
    };
    format!(
        "import {{ FetchProvider }} from \"{binding}\";
import {{ Some, None }} from \"{runtime}\";
import {{ Method }} from \"{bynk}\";
{worker_import}
const calls = [];
let legacy = false;
globalThis.fetch = async (_url, init) => {{
  calls.push(Object.fromEntries(new Headers(init.headers)));
  const res = new Response(\"hi\", {{
    status: 200,
    headers: new Headers([
      [\"ETag\", '\"v1\"'],
      [\"X-Rate-Limit\", \"9\"],
      [\"X-Multi\", \"1\"],
      [\"X-Multi\", \"2\"],
      [\"Set-Cookie\", \"a=1; Expires=Wed, 09 Jun 2027 10:18:14 GMT\"],
      [\"Set-Cookie\", \"b=2\"],
    ]),
  }});
  if (legacy) {{
    Object.defineProperty(res.headers, \"getSetCookie\", {{ value: undefined }});
  }}
  return res;
}};
const req = (contentType, authorization, entries) => ({{
  method: Method.Get,
  url: \"https://upstream.test/\",
  contentType,
  authorization,
  headers: new Map(entries),
  body: None,
}});
const FORBIDDEN = {forbidden};
const cases = {{
  extra: req(None, None, [[\"User-Agent\", \"bynk-test/1\"], [\"Accept\", \"application/json\"]]),
  legacySetCookie: req(None, None, []),
  slotFreeAuth: req(None, None, [[\"Authorization\", \"Bearer t\"]]),
  conflictType: req(Some(\"text/plain\"), None, [[\"CONTENT-TYPE\", \"application/json\"]]),
  conflictAuth: req(None, Some(\"Bearer a\"), [[\"authorization\", \"Bearer b\"]]),
  duplicate: req(None, None, [[\"X-A\", \"1\"], [\"x-a\", \"2\"]]),
  illegalValue: req(None, None, [[\"X-B\", \"a\\nb\"]]),
  illegalName: req(None, None, [[\"X B\", \"1\"]]),
  illegalTypeSlot: req(Some(\"text/plain\\nX-Evil: 1\"), None, []),
  illegalAuthSlot: req(None, Some(\"Bearer a\\nb\"), []),
}};
for (const n of FORBIDDEN) {{
  cases[`forbidden:${{n}}`] = req(None, None, [[n.toUpperCase(), \"x\"]]);
}}
const provider = new FetchProvider();
for (const [name, r] of Object.entries(cases)) {{
  const before = calls.length;
  legacy = name === \"legacySetCookie\";
  const res = await provider.send(r);
  const sent = calls.length > before ? JSON.stringify(calls[calls.length - 1]) : \"unsent\";
  const detail = res.tag === \"Ok\"
    ? `${{res.value.status}} ${{JSON.stringify(Object.fromEntries(res.value.headers))}}`
    : res.error.tag;
  console.log(`CASE ${{name}} ${{res.tag}} ${{detail}} ${{sent}}`);
}}
{routes}"
    )
}

fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(root).ok()? {
        let path = entry.ok()?.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|n| n == name) {
            return Some(path);
        }
    }
    None
}

/// `./`-relative import specifier for `path`, from `tmp` (where the driver lives).
fn spec(tmp: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(tmp).expect("under the project");
    format!("./{}", rel.to_string_lossy().replace('\\', "/"))
}

fn compile(tmp: &Path, out: &str, args: &[&str]) {
    let compiled = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .arg("compile")
        .args(args)
        .args(["--emit", "js", "-o"])
        .arg(tmp.join(out))
        .arg(tmp)
        .output()
        .expect("run bynkc");
    assert!(
        compiled.status.success(),
        "bynkc compile {args:?} failed:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
}

fn run_driver(tmp: &Path, file: &str, source: &str) -> String {
    fs::write(tmp.join(file), source).unwrap();
    let run = Command::new("node")
        .arg(file)
        .current_dir(tmp)
        .output()
        .expect("run node");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{file} failed:\n{stdout}\n{stderr}");
    stdout
}

/// The nine names a `Request.headers` entry may not set — both bindings' copies
/// of `FORBIDDEN_REQUEST_HEADERS`, pinned by behaviour.
const FORBIDDEN: [&str; 9] = [
    "host",
    "content-length",
    "connection",
    "keep-alive",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "expect",
];

/// The comma-joined response headers every `Ok` case must carry.
const JOINED: [&str; 2] = [
    r#""x-multi":"1, 2""#,
    r#""set-cookie":"a=1; Expires=Wed, 09 Jun 2027 10:18:14 GMT, b=2""#,
];

/// The `CASE <name> …` line, or a panic naming the missing case.
fn case<'a>(stdout: &'a str, name: &str) -> &'a str {
    let prefix = format!("CASE {name} ");
    stdout
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no `{name}` case in:\n{stdout}"))
}

fn assert_cases(platform: &str, stdout: &str) {
    let extra = case(stdout, "extra");
    assert!(
        extra.contains(r#""user-agent":"bynk-test/1""#)
            && extra.contains(r#""accept":"application/json""#),
        "[{platform}] extra headers reach the wire:\n{stdout}"
    );
    assert!(
        extra.contains(r#"Ok 200 {"#)
            && extra.contains(r#""etag":"\"v1\"""#)
            && extra.contains(r#""x-rate-limit":"9""#),
        "[{platform}] the response's headers come back, keys lowercased:\n{stdout}"
    );
    for name in ["extra", "legacySetCookie"] {
        for joined in JOINED {
            assert!(
                case(stdout, name).contains(joined),
                "[{platform}] `{name}`: a repeated response header arrives joined ({joined}):\n{stdout}"
            );
        }
    }
    assert!(
        case(stdout, "slotFreeAuth").contains(r#""authorization":"Bearer t""#),
        "[{platform}] headers may supply a header whose typed slot is None:\n{stdout}"
    );
    for (name, why) in [
        (
            "conflictType",
            "Content-Type in headers while contentType is Some",
        ),
        (
            "conflictAuth",
            "Authorization in headers while authorization is Some",
        ),
        ("duplicate", "a name repeated in a different case"),
        ("illegalValue", "a value that is not a legal header value"),
        ("illegalName", "a name that is not a legal header name"),
        (
            "illegalTypeSlot",
            "an illegal value in the contentType slot",
        ),
        (
            "illegalAuthSlot",
            "an illegal value in the authorization slot",
        ),
    ] {
        assert!(
            case(stdout, name).ends_with("Err InvalidHeader unsent"),
            "[{platform}] {why} is Err(InvalidHeader), and nothing is sent:\n{stdout}"
        );
    }
    for name in FORBIDDEN {
        assert!(
            case(stdout, &format!("forbidden:{name}")).ends_with("Err InvalidHeader unsent"),
            "[{platform}] the platform-owned `{name}` is Err(InvalidHeader), and nothing is sent:\n{stdout}"
        );
    }
}

#[test]
fn fetch_send_carries_request_headers_and_returns_response_headers() {
    if !tool_exists("node") {
        eprintln!("\n!!! FETCH HEADERS SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok_and(|v| !v.is_empty()) {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }
    let tmp = std::env::temp_dir().join(format!("bynk-fetch-headers-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("src/net")).unwrap();
    fs::write(
        tmp.join("bynk.toml"),
        "[project]\nname = \"fetch-headers\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(tmp.join("src/net/probe.bynk"), CONTEXT).unwrap();

    // Cloudflare: the Workers build, driven both directly and by route.
    compile(&tmp, "out-workers", &["--target", "workers"]);
    let worker_root = tmp.join("out-workers/workers/net-probe");
    let binding =
        find_file(&tmp.join("out-workers"), "bynk-cloudflare.js").expect("cloudflare binding");
    let dir = binding.parent().unwrap();
    let stdout = run_driver(
        &tmp,
        "drive-workers.mjs",
        &driver(
            &spec(&tmp, &binding),
            &spec(&tmp, &dir.join("__bynk_runtime/result.js")),
            &spec(&tmp, &dir.join("bynk.js")),
            Some(&spec(&tmp, &worker_root.join("index.js"))),
        ),
    );
    assert_cases("cloudflare", &stdout);
    assert!(
        stdout.contains(r#"ROUTE /conflict 200 "invalid" sent=0"#),
        "a Bynk match sees Err(InvalidHeader), and nothing is sent:\n{stdout}"
    );
    assert!(
        stdout.contains(r#"ROUTE /etag 200 "etag \"v1\"" sent=1"#),
        "a Bynk handler reads a response header by its lowercased name:\n{stdout}"
    );

    // Node: the bundle build's binding, driven directly.
    compile(&tmp, "out-node", &["--platform", "node"]);
    let binding = find_file(&tmp.join("out-node"), "bynk-node.js").expect("node binding");
    let dir = binding.parent().unwrap();
    let stdout = run_driver(
        &tmp,
        "drive-node.mjs",
        &driver(
            &spec(&tmp, &binding),
            &spec(&tmp, &dir.join("__bynk_runtime/result.js")),
            &spec(&tmp, &dir.join("bynk.js")),
            None,
        ),
    );
    assert_cases("node", &stdout);

    let _ = fs::remove_dir_all(&tmp);
}
