//! Run one example on a *real* Workers runtime (#518).
//!
//! The 21 behavioural suites execute compiled output against in-memory
//! fakes; nothing ever ran the emitted Worker on the runtime it targets, so
//! fake-vs-real drift was invisible. This smoke test compiles
//! `examples/hello-world` for the Workers target, strips it to the JS
//! artefact, boots it under `wrangler dev` (embedded workerd — the actual
//! Workers runtime), and asserts an end-to-end HTTP round-trip.
//!
//! Like the tsc-verification stage, this skips loudly when the toolchain is
//! unavailable; a *non-empty* `BYNK_REQUIRE_WORKERD` (CI sets `1`) turns the
//! skip into a failure. Empty counts as unset — see `required`.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

mod require;
mod wrangler;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";

/// Pinned provisioning, per the repo's npx convention — the spec itself lives in
/// [`wrangler`], shared with the other smokes and with CI's pre-warm step.
const WRANGLER: &str = wrangler::SPEC;

fn tool_exists(name: &str) -> bool {
    which::which(name).is_ok()
}

/// Route through `cmd /C` on Windows so npm's `npx.cmd` shim resolves
/// (Rust's CreateProcess refuses batch scripts directly — BatBadBut).
fn base_command(program: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        Command::new(program)
    }
}

/// Is this run required to actually reach workerd?
///
/// Presence alone is not enough — empty counts as unset. The contract, and the
/// history that made it necessary, live in [`require`].
fn required() -> bool {
    require::is_required(REQUIRE_ENV)
}

fn skip(reason: &str) -> bool {
    eprintln!("\n!!! WORKERS-RUNTIME SMOKE SKIPPED !!!\n{reason}\n");
    if required() {
        panic!("{REQUIRE_ENV} is set but {reason}");
    }
    true
}

#[test]
fn hello_world_serves_on_workerd() {
    if !tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    if !tool_exists("node") && skip("`node` is not on PATH") {
        return;
    }

    // Compile the example for Workers and strip to the JS artefact — the
    // form `wrangler dev` runs directly, no tsc in the loop (ADR 0137).
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/hello-world");
    let paths = bynkc::try_read_project_paths(&root).expect("well-formed fixture manifest");
    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_split(root, paths).target(bynkc::BuildTarget::Workers),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .expect("hello-world compiles for Workers");
    let out = bynkc::strip_project_to_js(out).expect("hello-world strips to JS");

    let tmp = std::env::temp_dir().join(format!("bynk-workerd-smoke-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    bynkc::write_output(&out, &tmp).unwrap();

    let worker_dir = tmp.join("workers/hello-web");
    assert!(
        worker_dir.join("index.js").is_file() && worker_dir.join("wrangler.toml").is_file(),
        "example layout changed — update this test's worker path"
    );

    // A pid-derived port keeps parallel test binaries off each other.
    let port = 20000 + (std::process::id() % 10000) as u16;
    let child = wrangler::Wrangler::spawn(
        base_command("npx")
            .args(["-y", WRANGLER, "dev", "--port", &port.to_string()])
            .current_dir(&worker_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    );
    let child = match child {
        Ok(c) => c,
        Err(e) => {
            if skip(&format!("could not launch npx: {e}")) {
                return;
            }
            unreachable!()
        }
    };

    // First run provisions wrangler + workerd via npx; allow a generous
    // boot window, then a strict assertion.
    let deadline = Instant::now() + Duration::from_secs(180);
    let url = format!("http://127.0.0.1:{port}/");
    let mut last_err = String::new();
    loop {
        if Instant::now() > deadline {
            let mut child = child;
            // Stop first: the pipe reaches EOF only once the whole group is gone.
            child.stop();
            let mut logs = String::new();
            if let Some(mut e) = child.child().stderr.take() {
                let _ = e.read_to_string(&mut logs);
            }
            if skip(&format!(
                "wrangler dev did not serve within the boot window (likely no \
                 network to provision {WRANGLER}); last error: {last_err}\n{logs}"
            )) {
                return;
            }
            unreachable!()
        }
        std::thread::sleep(Duration::from_millis(500));
        match fetch(&url, "/") {
            Ok(body) => {
                assert!(
                    body.contains("Hello, World!"),
                    "unexpected body from workerd: {body}"
                );
                // Drop stops wrangler and its workerds before cleanup.
                drop(child);
                // #1686: nothing wrangler started may outlive it. Checked
                // before the removal, which would hide a survivor's cwd.
                #[cfg(target_os = "linux")]
                {
                    let left = wrangler::processes_under(&tmp);
                    assert!(left.is_empty(), "wrangler left processes running: {left:?}");
                }
                let _ = fs::remove_dir_all(&tmp);
                return;
            }
            Err(e) => last_err = e,
        }
    }
}

/// #1649 (runtime-semantics track §3.2): agent state round-trips through real
/// Durable Object storage. The behavioural fixtures run the bundle target, whose
/// storage is an in-memory stand-in. Only workerd's own storage (V8
/// serialisation, a fresh copy on every `get`) proves the wire-shape encode and
/// decode end to end. One request writes an enum and an `Option`; a later
/// request, which reloads state from storage, reads both back. Before #1649 that
/// read faulted with `RehydrationViolation`. #1685: the same requests write a
/// store `Map` entry under the key `__proto__`, which must come back as an own
/// entry through workerd's V8 serialisation.
const AGENT_STATE_SOURCE: &str = r#"context smoke

type Light = enum { Red, Green }

agent Lamp {
  key id: String
  store light: Cell[Light] = Red
  store reading: Cell[Option[Int]]
  store tags: Map[String, Int]

  on call turnGreen() -> Effect[()] { light := Green }
  on call isGreen() -> Effect[Bool] { light == Green }
  on call note(n: Int) -> Effect[()] { reading := Some(n) }
  on call tag(k: String) -> Effect[()] {
    let _ <- tags.put(k, 1)
    ()
  }
  on call tagCount() -> Effect[Int] {
    let n <- tags.size()
    n
  }
  on call hasTag(k: String) -> Effect[Bool] {
    let r <- tags.contains(k)
    r
  }
  on call lastReading() -> Effect[Int] {
    match reading {
      Some(n) => n
      None => 0
    }
  }
}

service api from http {
  on GET("/") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }

  on GET("/set") () -> Effect[HttpResult[String]] by v: Visitor {
    do Lamp("k").turnGreen()
    do Lamp("k").note(7)
    do Lamp("k").tag("__proto__")
    Ok("set")
  }

  on GET("/get") () -> Effect[HttpResult[String]] by v: Visitor {
    let green <- Lamp("k").isGreen()
    let n <- Lamp("k").lastReading()
    let t <- Lamp("k").tagCount()
    let p <- Lamp("k").hasTag("__proto__")
    let c <- Lamp("k").hasTag("constructor")
    Ok("green=\(green) reading=\(n) tags=\(t) proto=\(p) ctor=\(c)")
  }
}
"#;

#[test]
fn agent_state_round_trips_on_workerd() {
    let Some(served) = serve_smoke(AGENT_STATE_SOURCE, "state", 30000) else {
        return;
    };
    let url = &served.url;
    let before = fetch(url, "/get").expect("GET /get before any write");
    assert!(
        before.contains("green=false reading=0 tags=0 proto=false ctor=false"),
        "a fresh key reads its zero values: {before}"
    );
    let set = fetch(url, "/set").expect("GET /set writes the agent's state");
    assert!(set.contains("set"), "unexpected /set body: {set}");
    let after = fetch(url, "/get").expect("GET /get reloads the written state");
    assert!(
        after.contains("green=true reading=7 tags=1 proto=true ctor=false"),
        "the enum, Option and `__proto__` map entry written by /set must read back \
         after a reload: {after}"
    );
}

/// #1678 (runtime-semantics track S11): on the workers target an agent call
/// crosses a Durable Object `fetch`, so its arguments and result are on a wire.
/// Each handler here echoes a value whose in-memory shape is not JSON-faithful,
/// or is only by accident. The route compares what comes back with what went
/// out. Before #1678 the call used raw `JSON.stringify` both ways, so `Bytes`
/// arrived as `{"0":104,"1":105}` and a value `Map` as `{}`. `touch` returns
/// `()`, whose codec must put JSON `null` on the wire (`JSON.stringify` of
/// `undefined` is not JSON, #527).
const AGENT_CODEC_SOURCE: &str = r#"context smoke

type Light = enum { Red, Green }

type Blob = { label: String, data: Bytes }

type Box[T] = { item: T }

agent Echo {
  key id: String
  store calls: Cell[Int] = 0

  on call echoBytes(b: Bytes) -> Effect[Bytes] { b }
  on call echoLight(l: Light) -> Effect[Light] { l }
  on call echoOpt(o: Option[Int]) -> Effect[Option[Int]] { o }
  on call echoMap(m: Map[String, Int]) -> Effect[Map[String, Int]] { m }
  on call echoBlob(b: Blob) -> Effect[Blob] { b }
  on call echoList(xs: List[Bytes]) -> Effect[List[Bytes]] { xs }
  on call echoBox(b: Box[Bytes]) -> Effect[Box[Bytes]] { b }
  on call twice(n: Int, b: Bytes) -> Effect[Int] { n + b.length() }
  on call touch() -> Effect[()] { calls.update((n) => n + 1) }
  on call touches() -> Effect[Int] { calls }
}

service api from http {
  on GET("/") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }

  on GET("/rt") () -> Effect[HttpResult[String]] by v: Visitor {
    let hi = Bytes.fromUtf8("hi")
    let m0: Map[String, Int] = Map.empty()
    let m1 = m0.insert("a", 1)
    let b <- Echo("k").echoBytes(hi)
    let l <- Echo("k").echoLight(Green)
    let o <- Echo("k").echoOpt(Some(3))
    let m <- Echo("k").echoMap(m1)
    let bl <- Echo("k").echoBlob(Blob { label: "x", data: hi })
    let xs <- Echo("k").echoList([hi, hi])
    let bx <- Echo("k").echoBox(Box { item: hi })
    let t <- Echo("k").twice(1, hi)
    do Echo("k").touch()
    let c <- Echo("k").touches()
    Ok("bytes=\(b == hi) light=\(l == Green) opt=\(o == Some(3)) map=\(m == m1) blob=\(bl.data == hi) list=\(xs == [hi, hi]) box=\(bx.item == hi) twice=\(t) touched=\(c)")
  }
}
"#;

#[test]
fn agent_calls_use_the_boundary_codec_on_workerd() {
    let Some(served) = serve_smoke(AGENT_CODEC_SOURCE, "codec", 40000) else {
        return;
    };
    let body = fetch(&served.url, "/rt").expect("GET /rt round-trips values through an agent");
    assert!(
        body.contains(
            "bytes=true light=true opt=true map=true blob=true list=true box=true twice=3 touched=1"
        ),
        "every value must come back from the agent equal to what was sent: {body}"
    );
}

/// #2016 (standard-library K1): every `Bytes` operation runs on workerd, whose
/// `btoa`/`atob` and typed arrays the helpers rely on.
const BYTES_OPS_SOURCE: &str = r#"context smoke

service api from http {
  on GET("/") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }

  on GET("/bytes") () -> Effect[HttpResult[String]] by v: Visitor {
    let b = Bytes.fromUtf8("v1:").concat(Bytes.fromUtf8("hi"))
    // A negative start clamps to 0 and an end past the length to the length.
    let hex = b.slice(-2, 99).toHex()
    let url = Bytes.fromUtf8("fo").toBase64Url()
    let back = match Bytes.fromBase64Url(url) {
      Some(x) => x == Bytes.fromUtf8("fo")
      None => false
    }
    let parsed = match Bytes.fromHex("807f") {
      Some(x) => x.slice(0, 1).compare(x.slice(1, 2))
      None => 0
    }
    Ok("hex=\(hex) url=\(url) back=\(back) order=\(parsed)")
  }
}
"#;

#[test]
fn bytes_operations_run_on_workerd() {
    let Some(served) = serve_smoke(BYTES_OPS_SOURCE, "bytes", 10000) else {
        return;
    };
    let body = fetch(&served.url, "/bytes").expect("GET /bytes runs the Bytes operations");
    assert!(
        body.contains("hex=76313a6869 url=Zm8 back=true order=1"),
        "each operation gives the same answer on workerd as under Node: {body}"
    );
}

/// #1820: a context split across files. On Workers each file used to be
/// written to `handlers.ts` in turn, each overwriting the last, so the Worker
/// kept only one file's declarations. Each file is now its own module, which
/// `handlers.ts` re-exports. The route calls an agent declared in another
/// file, with a third file's type, and the agent is `given` a provider
/// declared in a fourth: its Durable Object rebuilds the provider through
/// `handlers.ts`, an import cycle that must still load. Two files also use one
/// commons record, so `handlers.ts` re-exports its rebrand as `type Tag`,
/// which stripping to JavaScript must erase (#1862 review).
const SPLIT_CONTEXT_FILES: &[(&str, &str)] = &[
    (
        "tags.bynk",
        "commons tags\n\ntype Tag = { label: String }\n",
    ),
    (
        "smoke/types.bynk",
        "context smoke\n\ntype Line = { sku: String, qty: Int }\n",
    ),
    (
        "smoke/stamp.bynk",
        r#"context smoke

capability Stamp {
  fn next() -> Effect[Int]
}

provides Stamp = FixedStamp {
  fn next() -> Effect[Int] { 100 }
}
"#,
    ),
    (
        "smoke/book.bynk",
        r#"context smoke

uses tags

agent Book {
  key id: String
  store last: Cell[Option[Line]]
  store count: Cell[Int] = 0

  on call add(line: Line) -> Effect[Int] given Stamp {
    let s <- Stamp.next()
    last := Some(line)
    let _ <- count.update((c) => c + line.qty)
    Effect.pure(count + s)
  }

  on call label(t: Tag) -> Effect[String] { t.label }
}
"#,
    ),
    (
        "smoke/api.bynk",
        r#"context smoke

uses tags

service api from http {
  on GET("/") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }

  on GET("/add") () -> Effect[HttpResult[String]] by v: Visitor {
    let n <- Book("o1").add(Line { sku: "o1", qty: 2 })
    Ok("total=\(n)")
  }

  on GET("/label") () -> Effect[HttpResult[String]] by v: Visitor {
    let l <- Book("o1").label(Tag { label: "x" })
    Ok("label=\(l)")
  }
}
"#,
    ),
];

#[test]
fn a_context_split_across_files_serves_on_workerd() {
    let Some(served) = serve_smoke_files(SPLIT_CONTEXT_FILES, "split", 50000) else {
        return;
    };
    let first = fetch(&served.url, "/add").expect("GET /add reaches the sibling file's agent");
    assert!(
        first.contains("total=102"),
        "the agent adds the line and its provider's stamp: {first}"
    );
    let second = fetch(&served.url, "/add").expect("GET /add again");
    assert!(
        second.contains("total=104"),
        "the agent's state persists across calls: {second}"
    );
    let label = fetch(&served.url, "/label").expect("GET /label passes a commons record");
    assert!(
        label.contains("label=x"),
        "a commons record two files share crosses to the agent: {label}"
    );
}

/// A compiled smoke worker served by `wrangler dev`. Dropping it stops wrangler
/// (and its workerd) and removes the scratch directory.
struct Served {
    url: String,
    child: Option<wrangler::Wrangler>,
    tmp: std::path::PathBuf,
}

impl Drop for Served {
    fn drop(&mut self) {
        // Stop wrangler before removing its directory: a `Drop` body runs
        // before the fields drop, and a live wrangler would recreate its
        // `.wrangler` state under the path after the removal.
        drop(self.child.take());
        let _ = fs::remove_dir_all(&self.tmp);
    }
}

/// Compile `source` (a single-file `smoke` context) for Workers, strip it to JS
/// and serve it with `wrangler dev` on a pid-derived port above `port_base`,
/// with its own inspector port (wrangler's default 9229 is shared otherwise),
/// so the tests in this binary can run concurrently. `None` when the test is
/// skipped: no `npx`/`node`, or wrangler did not boot (`skip` panics instead
/// when `BYNK_REQUIRE_WORKERD` is set).
fn serve_smoke(source: &str, tag: &str, port_base: u16) -> Option<Served> {
    serve_smoke_files(&[("smoke.bynk", source)], tag, port_base)
}

/// [`serve_smoke`] over several source files, each `(path under src/,
/// source)`, which together declare the `smoke` context (#1820).
fn serve_smoke_files(files: &[(&str, &str)], tag: &str, port_base: u16) -> Option<Served> {
    if !tool_exists("npx") && skip("`npx` is not on PATH") {
        return None;
    }
    if !tool_exists("node") && skip("`node` is not on PATH") {
        return None;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-workerd-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let src = tmp.join("src");
    for (path, source) in files {
        let path = src.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }
    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_single(src).target(bynkc::BuildTarget::Workers),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .unwrap_or_else(|e| panic!("the {tag} smoke compiles for Workers: {e:?}"));
    let out = bynkc::strip_project_to_js(out).expect("the smoke strips to JS");
    let out_dir = tmp.join("out");
    bynkc::write_output(&out, &out_dir).unwrap();
    let worker_dir = out_dir.join("workers/smoke");
    assert!(
        worker_dir.join("index.js").is_file() && worker_dir.join("wrangler.toml").is_file(),
        "smoke layout changed — update this test's worker path"
    );

    let port = port_base + (std::process::id() % 10000) as u16;
    let inspector_port = port + 1;
    let child = wrangler::Wrangler::spawn(
        base_command("npx")
            .args([
                "-y",
                WRANGLER,
                "dev",
                "--port",
                &port.to_string(),
                "--inspector-port",
                &inspector_port.to_string(),
            ])
            .current_dir(&worker_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    );
    let child = match child {
        Ok(c) => c,
        Err(e) => {
            if skip(&format!("could not launch npx: {e}")) {
                return None;
            }
            unreachable!()
        }
    };

    let deadline = Instant::now() + Duration::from_secs(180);
    let url = format!("http://127.0.0.1:{port}/");
    let mut last_err = String::new();
    loop {
        if Instant::now() > deadline {
            let mut child = child;
            // Stop first: the pipe reaches EOF only once the whole group is gone.
            child.stop();
            let mut logs = String::new();
            if let Some(mut e) = child.child().stderr.take() {
                let _ = e.read_to_string(&mut logs);
            }
            if skip(&format!(
                "wrangler dev did not serve within the boot window (likely no \
                 network to provision {WRANGLER}); last error: {last_err}\n{logs}"
            )) {
                return None;
            }
            unreachable!()
        }
        std::thread::sleep(Duration::from_millis(500));
        match fetch(&url, "/") {
            Ok(_) => break,
            Err(e) => last_err = e,
        }
    }
    Some(Served {
        url,
        child: Some(child),
        tmp,
    })
}

/// A dependency-free HTTP GET of `path` (the test crate has no HTTP client):
/// one request, HTTP/1.1, connection-close.
fn fetch(url: &str, path: &str) -> Result<String, String> {
    use std::io::Write;
    let addr = url
        .strip_prefix("http://")
        .and_then(|r| r.split('/').next())
        .ok_or("bad url")?;
    let mut stream = std::net::TcpStream::connect_timeout(
        &addr.parse().map_err(|e| format!("{e}"))?,
        Duration::from_secs(2),
    )
    .map_err(|e| e.to_string())?;
    // A route that reaches a Durable Object instantiates it on first use,
    // which takes several seconds on a cold Windows runner; 20s matches the
    // events workerd smokes.
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .map_err(|e| e.to_string())?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| e.to_string())?;
    if !response.starts_with("HTTP/1.1 200") {
        return Err(format!(
            "non-200: {}",
            response.lines().next().unwrap_or("<empty>")
        ));
    }
    Ok(response)
}
