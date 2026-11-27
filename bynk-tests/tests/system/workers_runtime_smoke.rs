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
use std::time::Duration;

#[path = "../require/mod.rs"]
mod require;
#[path = "../wrangler/mod.rs"]
mod wrangler;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";

/// Pinned provisioning, per the repo's npx convention — the spec itself lives in
/// [`wrangler`], shared with the other smokes and with CI's pre-warm step.
const WRANGLER: &str = wrangler::SPEC;

fn tool_exists(name: &str) -> bool {
    which::which(name).is_ok()
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

/// Wait for the Worker `child` serves on `port` (logging to `log_path`) to log its Ready line and
/// answer `GET /` at `url`: the body it answered. `Err` says why it did not, with what wrangler
/// logged. #4174: a wrangler that has already exited will never serve, so that ends the wait at
/// once; one that could not bind its port is a failure, not a skip.
fn boot_answer(
    child: &mut wrangler::Wrangler,
    log_path: &Path,
    port: u16,
    url: &str,
) -> Result<String, String> {
    let body = std::cell::RefCell::new(None);
    // What the last probe said, as the old loops kept it: a Ready Worker that never answers `200`
    // is told apart from one that never answered at all by that status line.
    let last_err = std::cell::RefCell::new(String::new());
    wrangler::wait_until_ready_or_exit(
        log_path,
        port,
        &|| match fetch(url, "/") {
            Ok(answer) => {
                *body.borrow_mut() = Some(answer);
                true
            }
            Err(e) => {
                *last_err.borrow_mut() = e;
                false
            }
        },
        &mut || {
            child
                .child()
                .try_wait()
                .ok()
                .flatten()
                .map(|status| status.to_string())
        },
    )
    .map(|()| body.into_inner().expect("the probe answered"))
    .map_err(|why| {
        // Stop first: the log is read once the whole group is gone.
        child.stop();
        let logs = fs::read_to_string(log_path).unwrap_or_default();
        assert!(
            !logs.contains("Address already in use"),
            "wrangler dev could not bind its ports ({why}): {logs}"
        );
        format!("{why}; last probe: {}\n{logs}", last_err.borrow())
    })
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
    let paths =
        bynk_driver::project::try_read_project_paths(&root).expect("well-formed fixture manifest");
    let out = bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_split(root, paths)
            .target(bynk_driver::project::BuildTarget::Workers),
    )
    .map_err(bynk_driver::project::ProjectFailure::flatten)
    .expect("hello-world compiles for Workers");
    let out = bynk_driver::strip::strip_project_to_js(out).expect("hello-world strips to JS");

    let tmp = std::env::temp_dir().join(format!("bynk-workerd-smoke-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    bynk_driver::write_output(&out, &tmp).unwrap();

    let worker_dir = tmp.join("workers/hello-web");
    assert!(
        worker_dir.join("index.js").is_file() && worker_dir.join("wrangler.toml").is_file(),
        "example layout changed — update this test's worker path"
    );

    // #4174: a port the OS has just handed out, as for the other smokes (`wrangler::ports`).
    let [port, inspector_port] = wrangler::ports::free_ports();
    let log_path = tmp.join("wrangler.log");
    let out_log = fs::File::create(&log_path).unwrap();
    let err_log = out_log.try_clone().unwrap();
    let child = wrangler::Wrangler::spawn(
        wrangler::npx()
            .args([
                "dev",
                "--port",
                &port.to_string(),
                // wrangler's default inspector port is shared by every `wrangler dev` otherwise.
                "--inspector-port",
                &inspector_port.to_string(),
            ])
            .current_dir(&worker_dir)
            .stdout(Stdio::from(out_log))
            .stderr(Stdio::from(err_log)),
        &tmp.join("registry"),
    );
    let mut child = match child {
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
    let url = format!("http://127.0.0.1:{port}/");
    let body = match boot_answer(&mut child, &log_path, port, &url) {
        Ok(body) => body,
        Err(why) => {
            if skip(&format!(
                "{why} (likely no network to provision {WRANGLER})"
            )) {
                return;
            }
            unreachable!()
        }
    };
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
    let Some(served) = serve_smoke(AGENT_STATE_SOURCE, "state") else {
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

/// State-migrations S2 (#2224): a `migrate` step carries a record that a
/// previous build wrote to workerd's real Durable Object storage. The first
/// build writes `owner`; the second renames it to `ownerName` with a step, and
/// reads the migrated value back after a restart on the same storage.
const MIGRATE_SMOKE_V1: &str = r#"context smoke

agent Cart {
  key id: String
  store owner: Cell[String]

  on call setOwner(o: String) -> Effect[()] { owner := o }
  on call getOwner() -> Effect[String] { owner }
}

service api from http {
  on GET("/") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }

  on GET("/set") () -> Effect[HttpResult[String]] by v: Visitor {
    do Cart("k").setOwner("ada")
    Ok("set")
  }
}
"#;

const MIGRATE_SMOKE_V2: &str = r#"context smoke

type CartV1 = { owner: String }

agent Cart {
  key id: String
  store ownerName: Cell[String]

  migrate from 1 (old: CartV1) {
    ownerName := old.owner
  }

  on call getOwner() -> Effect[String] { ownerName }
}

service api from http {
  on GET("/") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }

  on GET("/get") () -> Effect[HttpResult[String]] by v: Visitor {
    let o <- Cart("k").getOwner()
    Ok("owner=\(o)")
  }
}
"#;

#[test]
fn a_migrate_step_carries_state_written_by_an_earlier_build_on_workerd() {
    let persist = std::env::temp_dir().join(format!("bynk-workerd-migrate-{}", std::process::id()));
    let _ = fs::remove_dir_all(&persist);
    fs::create_dir_all(&persist).unwrap();
    // The lock the first build records: `Cart` at version 1.
    let lock = {
        let dir = persist.join("lock-src");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("smoke.bynk"), MIGRATE_SMOKE_V1).unwrap();
        bynk_driver::project::compile_project(
            &bynk_testkit::compile_options_single(&dir)
                .target(bynk_driver::project::BuildTarget::Workers)
                .schema_registry(bynk_driver::project::SchemaLock::On { existing: None }),
        )
        .map_err(bynk_driver::project::ProjectFailure::flatten)
        .expect("the first build compiles")
        .schema_lock
    };
    let store = persist.join("state");
    {
        let Some(served) = serve_smoke_with(
            &[("smoke.bynk", MIGRATE_SMOKE_V1)],
            "migrate-v1",
            None,
            Some(&store),
        ) else {
            let _ = fs::remove_dir_all(&persist);
            return;
        };
        let set = fetch(&served.url, "/set").expect("GET /set writes v1 state");
        assert!(set.contains("set"), "unexpected /set body: {set}");
    }
    let Some(served) = serve_smoke_with(
        &[("smoke.bynk", MIGRATE_SMOKE_V2)],
        "migrate-v2",
        lock,
        Some(&store),
    ) else {
        let _ = fs::remove_dir_all(&persist);
        return;
    };
    let got = fetch(&served.url, "/get").expect("GET /get reads the migrated state");
    drop(served);
    let _ = fs::remove_dir_all(&persist);
    assert!(
        got.contains("owner=ada"),
        "the step must carry the v1 `owner` into `ownerName`: {got}"
    );
}

/// State-migrations S4 (#2259 Decision D): `bynk migrate sweep`'s wake on the
/// real runtime. A v1 build stores `owner`; the v2 build (with a step) is
/// served with the sweep's key set, and the signed `/_bynk/migrate/Cart`
/// route wakes the stored instance by its id (read from the persisted
/// storage, as Cloudflare's listing would give it): the record migrates and
/// commits at version 2, and a second wake reads it without writing.
#[test]
fn the_migrate_route_wakes_a_stored_instance_on_workerd() {
    use bynk_check::wire_format as wf;
    let persist = std::env::temp_dir().join(format!("bynk-workerd-sweep-{}", std::process::id()));
    let _ = fs::remove_dir_all(&persist);
    fs::create_dir_all(&persist).unwrap();
    let lock = {
        let dir = persist.join("lock-src");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("smoke.bynk"), MIGRATE_SMOKE_V1).unwrap();
        bynk_driver::project::compile_project(
            &bynk_testkit::compile_options_single(&dir)
                .target(bynk_driver::project::BuildTarget::Workers)
                .schema_registry(bynk_driver::project::SchemaLock::On { existing: None }),
        )
        .map_err(bynk_driver::project::ProjectFailure::flatten)
        .expect("the first build compiles")
        .schema_lock
    };
    let store = persist.join("state");
    {
        let Some(served) = serve_smoke_with(
            &[("smoke.bynk", MIGRATE_SMOKE_V1)],
            "sweep-v1",
            None,
            Some(&store),
        ) else {
            let _ = fs::remove_dir_all(&persist);
            return;
        };
        fetch(&served.url, "/set").expect("GET /set writes v1 state");
    }
    let id = stored_object_ids(&store.join("v3/do/smoke-Cart"))
        .into_iter()
        .next()
        .expect("the v1 run persisted one Durable Object");
    let key = "sweep-smoke-key";
    let Some(served) = serve_smoke_with_args(
        &[("smoke.bynk", MIGRATE_SMOKE_V2)],
        "sweep-v2",
        lock,
        Some(&store),
        &["--var".to_string(), format!("{}:{key}", wf::MIGRATE_KEY)],
    ) else {
        let _ = fs::remove_dir_all(&persist);
        return;
    };
    let path = format!("{}Cart", wf::MIGRATE_PATH_PREFIX);
    let body = format!("{{\"ids\":[\"{id}\"]}}");
    let unsigned = post(&served.url, &path, &[], &body);
    let first = post(
        &served.url,
        &path,
        &migrate_signed_headers(key, &path, &body),
        &body,
    );
    let second = post(
        &served.url,
        &path,
        &migrate_signed_headers(key, &path, &body),
        &body,
    );
    let got = fetch(&served.url, "/get");
    drop(served);
    let _ = fs::remove_dir_all(&persist);
    assert!(
        unsigned.starts_with("HTTP/1.1 401"),
        "an unsigned wake is refused: {unsigned}"
    );
    assert!(
        first.contains(&format!(
            "{{\"current\":2,\"layout\":2,\"results\":[{{\"id\":\"{id}\",\"before\":1,\"after\":2,\"layoutBefore\":2,\"layoutAfter\":2}}]}}"
        )),
        "the wake migrates the v1 record: {first}"
    );
    assert!(
        second.contains("\"before\":2,\"after\":2"),
        "the second wake finds it committed at version 2: {second}"
    );
    let got = got.expect("GET /get reads the migrated state");
    assert!(
        got.contains("owner=ada"),
        "the record carried `owner`: {got}"
    );
}

/// #4390: `@unique(by: email)` added to a `Map` that already holds duplicates. The v1 build stores
/// six people over three emails (held by 2, 3 and 1 key); the v2 build declares the constraint, and the
/// sweep's wake reports the values more than one key holds, as holder counts: never a value.
const UNIQUE_SWEEP_V1: &str = r#"context smoke

type Person = { id: String, email: String }

agent Directory {
  key id: String
  store people: Map[String, Person]

  on call seed() -> Effect[()] {
    let _ <- people.put("k1", Person { id: "k1", email: "a@x" })
    let _ <- people.put("k2", Person { id: "k2", email: "a@x" })
    let _ <- people.put("k3", Person { id: "k3", email: "b@x" })
    let _ <- people.put("k4", Person { id: "k4", email: "b@x" })
    let _ <- people.put("k5", Person { id: "k5", email: "b@x" })
    let _ <- people.put("k6", Person { id: "k6", email: "c@x" })
  }
}

service api from http {
  on GET("/") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }

  on GET("/seed") () -> Effect[HttpResult[String]] by v: Visitor {
    do Directory("d").seed()
    Ok("seeded")
  }
}
"#;

const UNIQUE_SWEEP_V2: &str = r#"context smoke

type Person = { id: String, email: String }

agent Directory {
  key id: String
  store people: Map[String, Person] @unique(by: email)

  on call size() -> Effect[Int] { people.size() }
}

service api from http {
  on GET("/") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }
}
"#;

#[test]
fn the_migrate_route_reports_unique_duplicates_already_stored_on_workerd() {
    use bynk_check::wire_format as wf;
    let persist = std::env::temp_dir().join(format!("bynk-workerd-unique-{}", std::process::id()));
    let _ = fs::remove_dir_all(&persist);
    fs::create_dir_all(&persist).unwrap();
    let lock = {
        let dir = persist.join("lock-src");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("smoke.bynk"), UNIQUE_SWEEP_V1).unwrap();
        bynk_driver::project::compile_project(
            &bynk_testkit::compile_options_single(&dir)
                .target(bynk_driver::project::BuildTarget::Workers)
                .schema_registry(bynk_driver::project::SchemaLock::On { existing: None }),
        )
        .map_err(bynk_driver::project::ProjectFailure::flatten)
        .expect("the first build compiles")
        .schema_lock
    };
    let store = persist.join("state");
    {
        let Some(served) = serve_smoke_with(
            &[("smoke.bynk", UNIQUE_SWEEP_V1)],
            "unique-v1",
            None,
            Some(&store),
        ) else {
            let _ = fs::remove_dir_all(&persist);
            return;
        };
        fetch(&served.url, "/seed").expect("GET /seed stores the duplicates");
    }
    let id = stored_object_ids(&store.join("v3/do/smoke-Directory"))
        .into_iter()
        .next()
        .expect("the v1 run persisted one Durable Object");
    let key = "sweep-unique-key";
    let Some(served) = serve_smoke_with_args(
        &[("smoke.bynk", UNIQUE_SWEEP_V2)],
        "unique-v2",
        lock,
        Some(&store),
        &["--var".to_string(), format!("{}:{key}", wf::MIGRATE_KEY)],
    ) else {
        let _ = fs::remove_dir_all(&persist);
        return;
    };
    let path = format!("{}Directory", wf::MIGRATE_PATH_PREFIX);
    let body = format!("{{\"ids\":[\"{id}\"]}}");
    let wake = post(
        &served.url,
        &path,
        &migrate_signed_headers(key, &path, &body),
        &body,
    );
    drop(served);
    let _ = fs::remove_dir_all(&persist);
    assert!(
        wake.contains("\"unique\":[{\"map\":\"people\",\"by\":\"email\",\"holders\":[2,3]}]"),
        "the wake reports the two shared values by their holders: {wake}"
    );
    assert!(
        !wake.contains("@x") && !wake.contains("k3"),
        "no stored value or key is named: {wake}"
    );
}

/// The hex ids of the Durable Objects `wrangler dev --persist-to` stored in one
/// namespace's directory, `dir`, from their SQLite files' names (as
/// Miniflare's own objects listing reads them).
fn stored_object_ids(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "sqlite"))
        .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_string))
        .filter(|stem| stem.len() == 64 && stem.bytes().all(|b| b.is_ascii_hexdigit()))
        .collect()
}

/// The headers a sweep signs a wake with, under `key`, computed by Node's
/// `crypto` from the published signing input.
fn migrate_signed_headers(key: &str, path: &str, body: &str) -> Vec<(String, String)> {
    use bynk_check::wire_format as wf;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs()
        .to_string();
    let script = r#"const c = require("node:crypto");
const [key, input, body] = process.argv.slice(1);
const digest = c.createHash("sha256").update(body, "utf8").digest("hex");
process.stdout.write(c.createHmac("sha256", key).update(input + digest, "utf8").digest("hex"));"#;
    let input = wf::signing_input(
        wf::MIGRATE_CALLER,
        "smoke",
        "POST",
        path,
        "",
        "1",
        &timestamp,
        "",
    );
    let out = Command::new("node")
        .args(["-e", script, key, &input, body])
        .output()
        .expect("node runs");
    assert!(out.status.success(), "node could not sign: {out:?}");
    vec![
        (
            wf::CALLER_HEADER.to_string(),
            wf::MIGRATE_CALLER.to_string(),
        ),
        (wf::TIMESTAMP_HEADER.to_string(), timestamp),
        (
            wf::SIGNATURE_HEADER.to_string(),
            String::from_utf8(out.stdout).expect("hex"),
        ),
    ]
}

/// A dependency-free HTTP POST, returning the whole response (status line
/// included) whatever its status.
fn post(url: &str, path: &str, headers: &[(String, String)], body: &str) -> String {
    use std::io::Write;
    let addr = url
        .strip_prefix("http://")
        .and_then(|r| r.split('/').next())
        .expect("a smoke url");
    let mut stream = std::net::TcpStream::connect(addr).expect("the smoke is listening");
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
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
    let Some(served) = serve_smoke(AGENT_CODEC_SOURCE, "codec") else {
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
      Some(x) => match x.slice(0, 1).compare(x.slice(1, 2)) {
        Less => -1
        Equal => 0
        Greater => 1
      }
      None => 0
    }
    Ok("hex=\(hex) url=\(url) back=\(back) order=\(parsed)")
  }
}
"#;

#[test]
fn bytes_operations_run_on_workerd() {
    let Some(served) = serve_smoke(BYTES_OPS_SOURCE, "bytes") else {
        return;
    };
    let body = fetch(&served.url, "/bytes").expect("GET /bytes runs the Bytes operations");
    assert!(
        body.contains("hex=76313a6869 url=Zm8 back=true order=1"),
        "each operation gives the same answer on workerd as under Node: {body}"
    );
}

/// #2355: a context-local provider whose body calls a local agent. The provider
/// class used to read `deps.env`, which nothing in the class binds, so the
/// call faulted with `ReferenceError: deps is not defined`. Compose now hands
/// the provider the Worker's `env`, and the call reaches the Durable Object.
/// `AgentYanks` has a `given` of its own; `AgentUnyank` has none.
const PROVIDER_AGENT_SOURCE: &str = r#"context smoke

agent PackageIndex {
  key name: String
  store yanked: Cell[Bool]

  on call setYanked(value: Bool) -> Effect[Bool] {
    let changed = yanked != value
    yanked := value
    Effect.pure(changed)
  }
}

capability Tokens { fn check() -> Effect[Bool] }
capability Yanks { fn set(value: Bool) -> Effect[Bool] }
capability Unyank { fn clear() -> Effect[Bool] }

provides Tokens = AllowAll {
  fn check() -> Effect[Bool] { Effect.pure(true) }
}

provides Yanks = AgentYanks given Tokens {
  fn set(value: Bool) -> Effect[Bool] {
    let ok <- Tokens.check()
    let changed <- PackageIndex("x").setYanked(value)
    Effect.pure(ok && changed)
  }
}

provides Unyank = AgentUnyank {
  fn clear() -> Effect[Bool] {
    PackageIndex("x").setYanked(false)
  }
}

service api from http {
  on GET("/") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }

  on GET("/yank") () -> Effect[HttpResult[String]] by v: Visitor given Yanks, Unyank {
    let first <- Yanks.set(true)
    let again <- Yanks.set(true)
    let cleared <- Unyank.clear()
    Ok("first=\(first) again=\(again) cleared=\(cleared)")
  }
}
"#;

#[test]
fn a_provider_calls_an_agent_on_workerd() {
    let Some(served) = serve_smoke(PROVIDER_AGENT_SOURCE, "provider-agent") else {
        return;
    };
    let body = fetch(&served.url, "/yank").expect("GET /yank calls the agent through providers");
    assert!(
        body.contains("first=true again=false cleared=true"),
        "both providers must reach the same Durable Object: {body}"
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
    let Some(served) = serve_smoke_files(SPLIT_CONTEXT_FILES, "split") else {
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
/// and serve it with `wrangler dev` on ports the OS hands out (`wrangler::ports`), with its
/// own inspector port (wrangler's default 9229 is shared otherwise), so the tests in this
/// binary can run concurrently. `None` when the test is
/// skipped: no `npx`/`node`, or wrangler did not boot (`skip` panics instead
/// when `BYNK_REQUIRE_WORKERD` is set).
fn serve_smoke(source: &str, tag: &str) -> Option<Served> {
    serve_smoke_files(&[("smoke.bynk", source)], tag)
}

/// [`serve_smoke`] over several source files, each `(path under src/,
/// source)`, which together declare the `smoke` context (#1820).
fn serve_smoke_files(files: &[(&str, &str)], tag: &str) -> Option<Served> {
    serve_smoke_with(files, tag, None, None)
}

/// [`serve_smoke_files`] against a committed schema lock (`lock`, `None` for a
/// fresh one) and, when `persist` is set, Durable Object storage kept in that
/// directory across runs — so a second build can load what a first one wrote
/// (state-migrations S2, #2224).
fn serve_smoke_with(
    files: &[(&str, &str)],
    tag: &str,
    lock: Option<String>,
    persist: Option<&Path>,
) -> Option<Served> {
    serve_smoke_with_args(files, tag, lock, persist, &[])
}

/// [`serve_smoke_with`], passing `extra` to `wrangler dev` (state-migrations
/// S4, #2259: a `--var` for the sweep's key).
fn serve_smoke_with_args(
    files: &[(&str, &str)],
    tag: &str,
    lock: Option<String>,
    persist: Option<&Path>,
    extra: &[String],
) -> Option<Served> {
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
    // State-migrations S1 (#2211): build as `bynk deploy` does, with the schema
    // lock (a fresh one), so agents stamp and check the `__v` marker — the
    // agent-state round trip below then proves it survives workerd's own
    // storage.
    let out = bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(src)
            .target(bynk_driver::project::BuildTarget::Workers)
            .schema_registry(bynk_driver::project::SchemaLock::On { existing: lock }),
    )
    .map_err(bynk_driver::project::ProjectFailure::flatten)
    .unwrap_or_else(|e| panic!("the {tag} smoke compiles for Workers: {e:?}"));
    let out = bynk_driver::strip::strip_project_to_js(out).expect("the smoke strips to JS");
    let out_dir = tmp.join("out");
    bynk_driver::write_output(&out, &out_dir).unwrap();
    let worker_dir = out_dir.join("workers/smoke");
    assert!(
        worker_dir.join("index.js").is_file() && worker_dir.join("wrangler.toml").is_file(),
        "smoke layout changed — update this test's worker path"
    );

    // #4174: ports the OS has just handed out, not a pid-derived number. A pid-derived `base +
    // pid % 10000` collided with another test's (`Address already in use`), wrangler exited
    // at once, and the wait below polled the dead process for the whole boot window.
    let [port, inspector_port] = wrangler::ports::free_ports();
    let log_path = tmp.join("wrangler.log");
    let out_log = fs::File::create(&log_path).unwrap();
    let err_log = out_log.try_clone().unwrap();
    let mut args: Vec<String> = [
        "dev",
        "--port",
        &port.to_string(),
        "--inspector-port",
        &inspector_port.to_string(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(dir) = persist {
        args.push("--persist-to".to_string());
        args.push(dir.display().to_string());
    }
    args.extend(extra.iter().cloned());
    let child = wrangler::Wrangler::spawn(
        wrangler::npx()
            .args(&args)
            .current_dir(&worker_dir)
            .stdout(Stdio::from(out_log))
            .stderr(Stdio::from(err_log)),
        &tmp.join("registry"),
    );
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            if skip(&format!("could not launch npx: {e}")) {
                return None;
            }
            unreachable!()
        }
    };

    let url = format!("http://127.0.0.1:{port}/");
    if let Err(why) = boot_answer(&mut child, &log_path, port, &url) {
        if skip(&format!(
            "{why} (likely no network to provision {WRANGLER})"
        )) {
            return None;
        }
        unreachable!()
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

/// #2966: a broadcast reaches WebSocket connections opened through the Workers
/// upgrade. #2959 had two bugs where the Worker passed `bynk check` and `tsc`
/// but routed the upgrade to the wrong Durable Object, so a broadcast to
/// `Board("ci")` reached no one. Only a run-time test sees a mismatch between
/// the edge's `idFromName(...)` for the upgrade and the `__make<Agent>(key,
/// env)` path an ordinary agent call takes.
///
/// The `on open` transfers to `Board("ci")` while carrying a route parameter
/// (`user`), the #2959 misroute shape: the literal key must stay the literal.
const WS_LITERAL_KEY_SOURCE: &str = r#"context smoke

type Frame = { text: String }

service live from websocket(in: Frame, out: Frame) {
  on open (user: String) -> Effect[()] by v: Visitor {
    let _ <- Board("ci").join(user, connection)
    ()
  }

  on close (user: String) -> Effect[()] by v: Visitor {
    ()
  }
}

agent Board {
  key id: String
  store conns: Map[String, Connection[Frame]]

  on call join(user: String, conn: Connection[Frame]) -> Effect[()] {
    let _ <- conns.put(user, conn)
    ()
  }

  on call post(text: String) -> Effect[()] {
    let _ <- conns.parForEach((c: Connection[Frame]) => c.send(Frame { text: text }))
    ()
  }
}

service api from http {
  on GET("/") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }

  on GET("/post") () -> Effect[HttpResult[String]] by v: Visitor {
    do Board("ci").post("hello")
    Ok("posted")
  }
}
"#;

/// The route-parameter-keyed shape: the upgrade carries `room` and the
/// connection is transferred to `Room(room)`; the broadcast reaches the same
/// instance through the HTTP route's own `Room("r1")`.
const WS_ROUTE_KEY_SOURCE: &str = r#"context smoke

type Frame = { text: String }

service live from websocket(in: Frame, out: Frame) {
  on open (room: String, user: String) -> Effect[()] by v: Visitor {
    let _ <- Room(room).join(user, connection)
    ()
  }

  on close (room: String, user: String) -> Effect[()] by v: Visitor {
    ()
  }
}

agent Room {
  key id: String
  store conns: Map[String, Connection[Frame]]

  on call join(user: String, conn: Connection[Frame]) -> Effect[()] {
    let _ <- conns.put(user, conn)
    ()
  }

  on call post(text: String) -> Effect[()] {
    let _ <- conns.parForEach((c: Connection[Frame]) => c.send(Frame { text: text }))
    ()
  }
}

service api from http {
  on GET("/") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }

  on GET("/post") () -> Effect[HttpResult[String]] by v: Visitor {
    do Room("r1").post("hello")
    Ok("posted")
  }
}
"#;

/// Real WebSocket clients (Node's built-in `WebSocket`, so no npm dependency)
/// open upgrades at `ws://127.0.0.1:<port>/?<query>`, one per query. The test
/// triggers the broadcast over HTTP; the first `expected` clients must each
/// receive it, and any further clients (opened on another key) must not, which
/// is checked after a short bounded wait. Prints the frames each client
/// received, a JSON array of arrays in client order. Every await is bounded, so
/// a misrouted upgrade fails fast with a message rather than hanging.
const WS_CLIENTS_JS: &str = r#"
const [port, expectedArg, ...queries] = process.argv.slice(2);
const expected = Number(expectedArg);
const within = (p, ms, what) => Promise.race([
  p,
  new Promise((_, reject) => setTimeout(() => reject(new Error(`timed out waiting for ${what}`)), ms)),
]);
const open = (query) => within(new Promise((resolve, reject) => {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/?${query}`);
  const frames = [];
  const got = new Promise((done) => {
    ws.addEventListener("message", (e) => { frames.push(String(e.data)); done(); });
  });
  ws.addEventListener("open", () => resolve({ ws, frames, got }));
  ws.addEventListener("error", () => reject(new Error(`upgrade failed: ${query}`)));
}), 15000, `the upgrade for ${query}`);
(async () => {
  const clients = [];
  for (const query of queries) clients.push(await open(query));
  const res = await within(fetch(`http://127.0.0.1:${port}/post`), 15000, "the /post route");
  if (!res.ok) throw new Error(`POST route answered ${res.status}`);
  await within(Promise.all(clients.slice(0, expected).map((c) => c.got)), 15000, "the broadcast on the expected clients");
  // The clients on another key get nothing: give a stray frame time to arrive.
  if (clients.length > expected) await new Promise((r) => setTimeout(r, 1500));
  console.log(JSON.stringify(clients.map((c) => c.frames)));
  clients.forEach((c) => c.ws.close());
  process.exit(0);
})().catch((e) => { console.error(String(e)); process.exit(1); });
"#;

/// Serve `source`, run the clients (`queries`; the first `expected` share the
/// broadcast's instance), and assert the frames each received: the one
/// `hello` frame for those, none for the rest.
fn broadcast_reaches_clients(source: &str, tag: &str, expected: usize, queries: &[&str]) {
    let Some(served) = serve_smoke(source, tag) else {
        return;
    };
    let port = served
        .url
        .rsplit(':')
        .next()
        .and_then(|p| p.trim_end_matches('/').parse::<u16>().ok())
        .expect("the served url carries a port");
    // A script file run by `node` itself: `base_command` would route through
    // `cmd /C` on Windows, which ends a `-e` argument at its first newline and
    // runs nothing (exit 0, empty stdout). `node` is an executable, not an npm
    // `.cmd` shim, so it needs no such routing.
    let script = served.tmp.join("ws-clients.js");
    fs::write(&script, WS_CLIENTS_JS).expect("write the client script");
    let out = Command::new("node")
        .arg(&script)
        .arg(port.to_string())
        .arg(expected.to_string())
        .args(queries)
        .output()
        .expect("node runs");
    let report = format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "the clients must receive the broadcast:\n{report}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let frame = r#""{\"text\":\"hello\"}""#;
    let want: Vec<String> = (0..queries.len())
        .map(|i| {
            if i < expected {
                format!("[{frame}]")
            } else {
                "[]".to_string()
            }
        })
        .collect();
    assert_eq!(
        stdout.trim(),
        format!("[{}]", want.join(",")),
        "the clients on the broadcast's key receive the one frame; the others none\n{report}"
    );
}

#[test]
fn a_broadcast_reaches_websockets_opened_through_a_literal_keyed_upgrade_on_workerd() {
    broadcast_reaches_clients(
        WS_LITERAL_KEY_SOURCE,
        "ws-literal",
        2,
        &["user=alice", "user=bob"],
    );
}

#[test]
fn a_broadcast_reaches_websockets_opened_through_a_route_keyed_upgrade_on_workerd() {
    // `carol` is in room `r2`: the route key separates instances, so the
    // broadcast to `Room("r1")` must not reach her.
    broadcast_reaches_clients(
        WS_ROUTE_KEY_SOURCE,
        "ws-route",
        2,
        &[
            "room=r1&user=alice",
            "room=r1&user=bob",
            "room=r2&user=carol",
        ],
    );
}

/// #3902: a client leaving is released by `on close`, with the close code the client sent, and the
/// close goes out before `on close` runs (so the client sees its own code echoed). `join` closes a banned user's connection with a code and
/// reason of the agent's choosing; `leave` removes a departed client and records the code `on close`
/// was given. A room that kept a dead connection (no `on close`) would raise on the next broadcast.
const WS_CLOSE_SOURCE: &str = r#"context smoke

consumes bynk { CloseInfo }

type Frame = { text: String }

service live from websocket(in: Frame, out: Frame) {
  on open (room: String, user: String) -> Effect[()] by v: Visitor {
    let _ <- Room(room).join(user, connection)
    ()
  }

  on close (room: String, user: String, close: CloseInfo) -> Effect[()] by v: Visitor {
    let _ <- Room(room).leave(user, close.code)
    ()
  }
}

agent Room {
  key id: String
  store conns: Map[String, Connection[Frame]]
  store lastCode: Cell[Int]

  on call join(user: String, conn: Connection[Frame]) -> Effect[()] {
    if user == "banned" {
      let _ <- conn.close(4001, "banned")
      ()
    } else {
      let _ <- conns.put(user, conn)
      ()
    }
  }

  on call leave(user: String, code: Int) -> Effect[()] {
    let _ <- conns.remove(user)
    lastCode := code
    ()
  }

  on call post(text: String) -> Effect[()] {
    let _ <- conns.parForEach((c: Connection[Frame]) => c.send(Frame { text: text }))
    ()
  }

  on call last() -> Effect[Int] {
    lastCode
  }
}

service api from http {
  on GET("/") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }

  on GET("/post") () -> Effect[HttpResult[String]] by v: Visitor {
    do Room("r1").post("hello")
    Ok("posted")
  }

  on GET("/last") () -> Effect[HttpResult[String]] by v: Visitor {
    let code <- Room("r1").last()
    Ok("\(code)")
  }
}
"#;

/// Real clients: `banned` is closed by the server with `4001 "banned"` and sees exactly that; `alice`
/// and `bob` join; `alice` closes with `4002 "bye"` and, once the Durable Object has run `on close`,
/// sees `4002 "bye"` echoed back; the room then records `4002`, and a broadcast reaches
/// only `bob` (a dead connection would have raised on the way). Prints the observations as JSON.
const WS_CLOSE_CLIENTS_JS: &str = r#"
const [port] = process.argv.slice(2);
const within = (p, ms, what) => Promise.race([
  p,
  new Promise((_, reject) => setTimeout(() => reject(new Error(`timed out waiting for ${what}`)), ms)),
]);
const open = (user) => within(new Promise((resolve, reject) => {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/?room=r1&user=${user}`);
  const frames = [];
  const closed = new Promise((done) => {
    ws.addEventListener("close", (e) => done({ code: e.code, reason: e.reason }));
  });
  ws.addEventListener("message", (e) => frames.push(String(e.data)));
  ws.addEventListener("open", () => resolve({ ws, frames, closed }));
  ws.addEventListener("error", () => reject(new Error(`upgrade failed: ${user}`)));
}), 15000, `the upgrade for ${user}`);
(async () => {
  const banned = await open("banned");
  const bannedClose = await within(banned.closed, 15000, "the server's close of the banned client");
  const alice = await open("alice");
  const bob = await open("bob");
  alice.ws.close(4002, "bye");
  const aliceClose = await within(alice.closed, 15000, "the close handshake reply for alice");
  const last = await within(fetch(`http://127.0.0.1:${port}/last`).then((r) => r.text()), 15000, "/last");
  const post = await within(fetch(`http://127.0.0.1:${port}/post`), 15000, "/post");
  if (!post.ok) throw new Error(`the broadcast answered ${post.status}`);
  await within(new Promise((resolve) => {
    const tick = () => (bob.frames.length > 0 ? resolve() : setTimeout(tick, 50));
    tick();
  }), 15000, "the broadcast on bob");
  console.log(JSON.stringify({ bannedClose, aliceClose, last, bob: bob.frames, alice: alice.frames }));
  bob.ws.close();
  process.exit(0);
})().catch((e) => { console.error(String(e)); process.exit(1); });
"#;

#[test]
fn a_departing_client_runs_on_close_with_its_code_and_the_handshake_completes_on_workerd() {
    let Some(served) = serve_smoke(WS_CLOSE_SOURCE, "ws-close") else {
        return;
    };
    let port = served
        .url
        .rsplit(':')
        .next()
        .and_then(|p| p.trim_end_matches('/').parse::<u16>().ok())
        .expect("the served url carries a port");
    let script = served.tmp.join("ws-close-clients.js");
    fs::write(&script, WS_CLOSE_CLIENTS_JS).expect("write the client script");
    let out = Command::new("node")
        .arg(&script)
        .arg(port.to_string())
        .output()
        .expect("node runs");
    let report = format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "the close clients must run green:\n{report}"
    );
    let seen: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("report is JSON ({e}):\n{report}"));
    assert_eq!(
        seen["bannedClose"],
        serde_json::json!({ "code": 4001, "reason": "banned" }),
        "`connection.close(4001, \"banned\")` reaches the client with that code and reason\n{report}"
    );
    // The close goes out before `on close` runs, so the release `on close` performs (`conns.remove`
    // closes the held connection with no code) cannot replace it: the client sees the code and reason
    // it closed with echoed back (#4491).
    assert_eq!(
        seen["aliceClose"],
        serde_json::json!({ "code": 4002, "reason": "bye" }),
        "the close reply carries the client's own code and reason\n{report}"
    );
    assert_eq!(
        seen["last"],
        serde_json::json!("\"4002\""),
        "`on close` ran with the code the client closed with\n{report}"
    );
    assert_eq!(
        seen["bob"],
        serde_json::json!(["{\"text\":\"hello\"}"]),
        "a later broadcast reaches the remaining client\n{report}"
    );
    assert_eq!(
        seen["alice"],
        serde_json::json!([]),
        "and not the one that left\n{report}"
    );
}

/// #3903: a connection is closed when its credential expires. The upgrade records the Bearer token's
/// `exp`, and a frame arriving after it closes the socket with `1008 authentication expired` and runs
/// `on close` (once), without running `on message` for that frame. `heard` counts the frames
/// `on message` ran for, and `lastCode` is the code `on close` was given.
const WS_EXPIRY_SOURCE: &str = r#"context smoke

consumes bynk { CloseInfo }

type UserId = String where NonEmpty
type Frame = { text: String }

actor Member { auth = Bearer(secret = "WS_SECRET"), identity = UserId }

service live from websocket(in: Frame, out: Frame) {
  on open (room: String) -> Effect[()] by user: Member {
    let _ <- Room(room).join(connection)
    ()
  }

  on message (room: String, frame: Frame) -> Effect[()] by user: Member {
    let _ <- Room(room).heard()
    ()
  }

  on close (room: String, close: CloseInfo) -> Effect[()] by user: Member {
    let _ <- Room(room).left(close.code)
    ()
  }
}

agent Room {
  key id: String
  store conns: Map[String, Connection[Frame]]
  store count: Cell[Int]
  store lastCode: Cell[Int]

  on call join(conn: Connection[Frame]) -> Effect[()] {
    let _ <- conns.put("c", conn)
    ()
  }

  on call heard() -> Effect[()] {
    let _ <- count.update((c) => c + 1)
    ()
  }

  on call left(code: Int) -> Effect[()] {
    let _ <- conns.remove("c")
    lastCode := code
    ()
  }

  on call snapshot() -> Effect[String] {
    "\(count):\(lastCode)"
  }
}

service api from http {
  on GET("/") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }

  on GET("/state") () -> Effect[HttpResult[String]] by v: Visitor {
    let s <- Room("r1").snapshot()
    Ok(s)
  }
}
"#;

/// A real client: it signs a token that expires a few seconds from now, opens the upgrade with
/// `["bynk.bearer", token]`, sends one frame before the expiry (handled), waits past it, and sends
/// another, which closes the connection. Prints the observations as JSON.
const WS_EXPIRY_CLIENT_JS: &str = r#"
const crypto = require("node:crypto");
const [port, secret, ttl] = process.argv.slice(2);
const b64 = (o) => Buffer.from(typeof o === "string" ? o : JSON.stringify(o)).toString("base64url");
const exp = Math.floor(Date.now() / 1000) + Number(ttl);
const head = b64({ alg: "HS256", typ: "JWT" });
const body = b64({ sub: "ada", exp });
const sig = crypto.createHmac("sha256", secret).update(`${head}.${body}`).digest("base64url");
const token = `${head}.${body}.${sig}`;
const within = (p, ms, what) => Promise.race([
  p,
  new Promise((_, reject) => setTimeout(() => reject(new Error(`timed out waiting for ${what}`)), ms)),
]);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const state = () => fetch(`http://127.0.0.1:${port}/state`).then((r) => r.text());
(async () => {
  const ws = await within(new Promise((resolve, reject) => {
    const w = new WebSocket(`ws://127.0.0.1:${port}/?room=r1`, ["bynk.bearer", token]);
    w.addEventListener("open", () => resolve(w));
    w.addEventListener("error", () => reject(new Error("upgrade failed")));
  }), 15000, "the upgrade");
  const closed = new Promise((done) => ws.addEventListener("close", (e) => done({ code: e.code, reason: e.reason })));
  ws.send(JSON.stringify({ text: "early" }));
  await sleep(1000);
  const before = await state();
  // Past the credential's expiry (`exp < now`), plus a margin for clock rounding.
  await sleep((exp + 2) * 1000 - Date.now());
  ws.send(JSON.stringify({ text: "late" }));
  const close = await within(closed, 15000, "the close after expiry");
  const after = await state();
  console.log(JSON.stringify({ before, close, after }));
  process.exit(0);
})().catch((e) => { console.error(String(e)); process.exit(1); });
"#;

#[test]
fn a_frame_after_the_credential_expires_closes_the_connection_on_workerd() {
    let secret = "expiry-smoke-secret";
    let Some(served) = serve_smoke_with_args(
        &[("smoke.bynk", WS_EXPIRY_SOURCE)],
        "ws-expiry",
        None,
        None,
        &["--var".to_string(), format!("WS_SECRET:{secret}")],
    ) else {
        return;
    };
    let port = served
        .url
        .rsplit(':')
        .next()
        .and_then(|p| p.trim_end_matches('/').parse::<u16>().ok())
        .expect("the served url carries a port");
    let script = served.tmp.join("ws-expiry-client.js");
    fs::write(&script, WS_EXPIRY_CLIENT_JS).expect("write the client script");
    let out = Command::new("node")
        .arg(&script)
        .arg(port.to_string())
        .arg(secret)
        .arg("5")
        .output()
        .expect("node runs");
    let report = format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "the expiry client must run green:\n{report}"
    );
    let seen: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("report is JSON ({e}):\n{report}"));
    assert_eq!(
        seen["before"],
        serde_json::json!("\"1:0\""),
        "a frame before the expiry is handled as usual, and nothing has closed\n{report}"
    );
    assert_eq!(
        seen["close"],
        serde_json::json!({ "code": 1008, "reason": "authentication expired" }),
        "a frame after the expiry closes the socket with 1008\n{report}"
    );
    assert_eq!(
        seen["after"],
        serde_json::json!("\"1:1008\""),
        "`on message` did not run for the late frame, and `on close` ran with 1008\n{report}"
    );
}

/// #3904: a cookie-authenticated upgrade through the deployed Worker. The service lists one origin in
/// `cors`; `joined` counts the connections the Durable Object accepted, so a refused upgrade that
/// reached it would show.
const WS_COOKIE_SOURCE: &str = r#"context smoke

type UserId = String where NonEmpty
type Frame = { text: String }

actor Member { auth = SignedCookie(secret = "SESSION_KEY", ttl = 1.hour), identity = UserId }

service live from websocket(in: Frame, out: Frame) {
  cors { origins: ["https://app.example.com"], credentials: true }

  on open (room: String) -> Effect[()] by user: Member {
    let _ <- Room(room).join(connection)
    ()
  }

  on close (room: String) -> Effect[()] by user: Member {
    let _ <- Room(room).left()
    ()
  }
}

agent Room {
  key id: String
  store conns: Map[String, Connection[Frame]]
  store joined: Cell[Int]

  on call join(conn: Connection[Frame]) -> Effect[()] {
    let _ <- conns.put("c", conn)
    let _ <- joined.update((c) => c + 1)
    ()
  }

  on call left() -> Effect[()] {
    let _ <- conns.remove("c")
    ()
  }

  on call snapshot() -> Effect[String] {
    "\(joined)"
  }
}

service api from http {
  on GET("/") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }

  on GET("/state") () -> Effect[HttpResult[String]] by v: Visitor {
    let s <- Room("r1").snapshot()
    Ok(s)
  }
}
"#;

/// A real client that sends raw upgrade requests, so it can set `Origin` and `Cookie` as a browser
/// would, and reports the status of each. A `101` is the upgrade accepted.
const WS_COOKIE_CLIENT_JS: &str = r#"
const crypto = require("node:crypto");
const http = require("node:http");
const [port, keyB64] = process.argv.slice(2);
const b64 = (o) => Buffer.from(typeof o === "string" ? o : JSON.stringify(o)).toString("base64url");
const key = Buffer.from(keyB64, "base64url");
const kid = crypto.createHash("sha256").update(key).digest().subarray(0, 6).toString("base64url");
const cookie = (aud, ttl, secret = key) => {
  const iat = Math.floor(Date.now() / 1000);
  const head = b64({ alg: "HS256", typ: "bynk+session", kid });
  const body = b64({ sub: "ada", aud, iat, exp: iat + ttl });
  const mac = crypto.createHmac("sha256", secret).update(`${head}.${body}`).digest("base64url");
  return `${head}.${body}.${mac}`;
};
const upgrade = (origin, cookieValue) => new Promise((resolve, reject) => {
  const headers = {
    Connection: "Upgrade",
    Upgrade: "websocket",
    "Sec-WebSocket-Version": "13",
    "Sec-WebSocket-Key": crypto.randomBytes(16).toString("base64"),
  };
  if (origin !== null) headers.Origin = origin;
  if (cookieValue !== null) headers.Cookie = `__Host-session=${cookieValue}`;
  const req = http.request({ host: "127.0.0.1", port, path: "/?room=r1", headers });
  req.on("upgrade", (res, socket) => { socket.destroy(); resolve(res.statusCode); });
  req.on("response", (res) => { res.resume(); resolve(res.statusCode); });
  req.on("error", reject);
  req.end();
});
const state = () => fetch(`http://127.0.0.1:${port}/state`).then((r) => r.text());
(async () => {
  const good = cookie("Member", 3600);
  const out = {};
  out.listed = await upgrade("https://app.example.com", good);
  await new Promise((r) => setTimeout(r, 500));
  out.afterListed = await state();
  out.noOrigin = await upgrade(null, good);
  out.otherOrigin = await upgrade("https://evil.example", good);
  out.nearOrigin = await upgrade("https://app.example.com.evil.example", good);
  out.noOriginNoCookie = await upgrade(null, null);
  out.noCookie = await upgrade("https://app.example.com", null);
  out.foreignKey = await upgrade("https://app.example.com", cookie("Member", 3600, crypto.randomBytes(32)));
  out.otherActor = await upgrade("https://app.example.com", cookie("Other", 3600));
  await new Promise((r) => setTimeout(r, 500));
  out.afterRefused = await state();
  console.log(JSON.stringify(out));
  process.exit(0);
})().catch((e) => { console.error(String(e)); process.exit(1); });
"#;

#[test]
fn a_cookie_upgrade_needs_a_listed_origin_through_the_worker_on_workerd() {
    let key = "c2Vzc2lvbi1rZXktZm9yLXRoZS13b3JrZXJkLXNtb2tlLXRlc3Q";
    let Some(served) = serve_smoke_with_args(
        &[("smoke.bynk", WS_COOKIE_SOURCE)],
        "ws-cookie",
        None,
        None,
        &["--var".to_string(), format!("SESSION_KEY:{key}")],
    ) else {
        return;
    };
    let port = served
        .url
        .rsplit(':')
        .next()
        .and_then(|p| p.trim_end_matches('/').parse::<u16>().ok())
        .expect("the served url carries a port");
    let script = served.tmp.join("ws-cookie-client.js");
    fs::write(&script, WS_COOKIE_CLIENT_JS).expect("write the client script");
    let out = Command::new("node")
        .arg(&script)
        .arg(port.to_string())
        .arg(key)
        .output()
        .expect("node runs");
    let report = format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "the cookie client must run green:\n{report}"
    );
    let seen: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("report is JSON ({e}):\n{report}"));
    assert_eq!(
        seen["listed"], 101,
        "a listed Origin with a valid cookie upgrades\n{report}"
    );
    assert_eq!(
        seen["afterListed"],
        serde_json::json!("\"1\""),
        "the Durable Object accepted it\n{report}"
    );
    for case in ["noOrigin", "otherOrigin", "nearOrigin", "noOriginNoCookie"] {
        assert_eq!(
            seen[case], 403,
            "{case}: a missing or unlisted Origin is 403\n{report}"
        );
    }
    for case in ["noCookie", "foreignKey", "otherActor"] {
        assert_eq!(
            seen[case], 401,
            "{case}: a listed Origin with a bad cookie is 401\n{report}"
        );
    }
    assert_eq!(
        seen["afterRefused"],
        serde_json::json!("\"1\""),
        "no refused upgrade reached the Durable Object\n{report}"
    );
}

/// #3905: frame limits and binary frames through the deployed Worker. `heard` counts the frames
/// `on message` ran for, so a frame refused before decoding shows as not heard. One service per
/// source (the Worker routes an upgrade to the first), in a `Bytes` and a JSON form.
fn ws_frame_source(in_ty: &str, handler_frame: &str, echo: &str) -> String {
    format!(
        r#"context smoke

type UserId = String where NonEmpty
type Frame = {{ text: String }}

actor Member {{ auth = Bearer(secret = "WS_SECRET"), identity = UserId }}

service live from websocket(in: {in_ty}, out: {in_ty}) {{
  limits {{ maxFrame: 64 }}

  on open (room: String) -> Effect[()] by user: Member {{
    let _ <- Room(room).join(connection)
    ()
  }}

  on message (room: String, frame: {handler_frame}) -> Effect[()] by user: Member {{
    let _ <- Room(room).heard()
    let _ <- connection.send({echo})
    ()
  }}

  on close (room: String) -> Effect[()] by user: Member {{
    let _ <- Room(room).left()
    ()
  }}
}}

agent Room {{
  key id: String
  store conns: Map[String, Connection[{in_ty}]]
  store count: Cell[Int]

  on call join(conn: Connection[{in_ty}]) -> Effect[()] {{
    let _ <- conns.put("c", conn)
    ()
  }}

  on call heard() -> Effect[()] {{
    let _ <- count.update((c) => c + 1)
    ()
  }}

  on call left() -> Effect[()] {{
    let _ <- conns.remove("c")
    ()
  }}

  on call snapshot() -> Effect[String] {{
    "\(count)"
  }}
}}

service api from http {{
  on GET("/") () -> Effect[HttpResult[String]] by v: Visitor {{
    Ok("up")
  }}

  on GET("/state") () -> Effect[HttpResult[String]] by v: Visitor {{
    let s <- Room("r1").snapshot()
    Ok(s)
  }}
}}
"#
    )
}

/// A real client: it opens a socket with a Bearer pair, sends the frames its mode names and reports
/// what came back (an echoed frame's kind and size, or the close code), then the service's `heard`.
const WS_FRAMES_CLIENT_JS: &str = r#"
const crypto = require("node:crypto");
const [port, secret, mode] = process.argv.slice(2);
const b64 = (o) => Buffer.from(typeof o === "string" ? o : JSON.stringify(o)).toString("base64url");
const exp = Math.floor(Date.now() / 1000) + 3600;
const head = b64({ alg: "HS256", typ: "JWT" });
const body = b64({ sub: "ada", exp });
const sig = crypto.createHmac("sha256", secret).update(`${head}.${body}`).digest("base64url");
const token = `${head}.${body}.${sig}`;
const within = (p, ms, what) => Promise.race([
  p,
  new Promise((_, reject) => setTimeout(() => reject(new Error(`timed out waiting for ${what}`)), ms)),
]);
const state = () => fetch(`http://127.0.0.1:${port}/state`).then((r) => r.text());
// Open a socket, send `frame`, and report the first thing that happens: a frame back or the close.
async function trial(frame) {
  const ws = await within(new Promise((resolve, reject) => {
    const w = new WebSocket(`ws://127.0.0.1:${port}/?room=r1`, ["bynk.bearer", token]);
    w.binaryType = "arraybuffer";
    w.addEventListener("open", () => resolve(w));
    w.addEventListener("error", () => reject(new Error("upgrade failed")));
  }), 15000, "the upgrade");
  const outcome = within(new Promise((done) => {
    ws.addEventListener("message", (e) => done(typeof e.data === "string"
      ? { text: e.data.length }
      : { binary: e.data.byteLength }));
    ws.addEventListener("close", (e) => done({ close: e.code, reason: e.reason }));
  }), 15000, "a reply");
  ws.send(frame);
  const result = await outcome;
  try { ws.close(); } catch {}
  await new Promise((r) => setTimeout(r, 300));
  return result;
}
(async () => {
  const out = {};
  if (mode === "bytes") {
    out.small = await trial(new Uint8Array(10).fill(7));
    out.tooBig = await trial(new Uint8Array(65).fill(7));
    out.text = await trial(JSON.stringify({ text: "hi" }));
  } else {
    out.small = await trial(JSON.stringify({ text: "hi" }));
    out.tooBig = await trial(JSON.stringify({ text: "x".repeat(100) }));
    out.binary = await trial(new Uint8Array(10).fill(7));
  }
  out.heard = await state();
  console.log(JSON.stringify(out));
  process.exit(0);
})().catch((e) => { console.error(String(e)); process.exit(1); });
"#;

fn run_ws_frames(source: &str, tag: &str, mode: &str) -> Option<serde_json::Value> {
    let secret = "frames-smoke-secret";
    let served = serve_smoke_with_args(
        &[("smoke.bynk", source)],
        tag,
        None,
        None,
        &["--var".to_string(), format!("WS_SECRET:{secret}")],
    )?;
    let port = served
        .url
        .rsplit(':')
        .next()
        .and_then(|p| p.trim_end_matches('/').parse::<u16>().ok())
        .expect("the served url carries a port");
    let script = served.tmp.join("ws-frames-client.js");
    fs::write(&script, WS_FRAMES_CLIENT_JS).expect("write the client script");
    let out = Command::new("node")
        .arg(&script)
        .arg(port.to_string())
        .arg(secret)
        .arg(mode)
        .output()
        .expect("node runs");
    let report = format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "the frames client must run green:\n{report}"
    );
    Some(
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("report is JSON ({e}):\n{report}")),
    )
}

#[test]
fn a_bytes_service_echoes_binary_frames_and_refuses_text_and_oversize_on_workerd() {
    let source = ws_frame_source("Bytes", "Bytes", "frame");
    let Some(seen) = run_ws_frames(&source, "ws-bytes", "bytes") else {
        return;
    };
    assert_eq!(
        seen["small"],
        serde_json::json!({ "binary": 10 }),
        "a binary frame is echoed as binary\n{seen}"
    );
    assert_eq!(
        seen["tooBig"],
        serde_json::json!({ "close": 1009, "reason": "message too large" }),
        "a frame over maxFrame is closed with 1009\n{seen}"
    );
    assert_eq!(
        seen["text"],
        serde_json::json!({ "close": 1003, "reason": "text frame not accepted" }),
        "a text frame to a Bytes service is closed with 1003\n{seen}"
    );
    assert_eq!(
        seen["heard"],
        serde_json::json!("\"1\""),
        "only the one admitted frame reached `on message`\n{seen}"
    );
}

#[test]
fn a_json_service_refuses_binary_and_oversize_frames_before_parsing_on_workerd() {
    let source = ws_frame_source("Frame", "Frame", "frame");
    let Some(seen) = run_ws_frames(&source, "ws-json-frames", "json") else {
        return;
    };
    assert_eq!(
        seen["small"],
        serde_json::json!({ "text": 13 }),
        "a JSON frame is echoed as text\n{seen}"
    );
    assert_eq!(
        seen["tooBig"],
        serde_json::json!({ "close": 1009, "reason": "message too large" }),
        "a frame over maxFrame is closed with 1009\n{seen}"
    );
    assert_eq!(
        seen["binary"],
        serde_json::json!({ "close": 1003, "reason": "binary frame not accepted" }),
        "a binary frame to a JSON service is closed with 1003\n{seen}"
    );
    assert_eq!(
        seen["heard"],
        serde_json::json!("\"1\""),
        "only the one admitted frame reached `on message`\n{seen}"
    );
}

/// #3202: an agent instance runs the handlers that reach it one at a time. `bump`
/// loads its state, awaits a call to another agent (a Durable Object subrequest,
/// which does not close the input gate), then commits `n + 1`. Before the fix, five
/// concurrent `bump`s on one instance all read the same `n` and the later commits
/// overwrote the earlier ones, so the count came out at 1 or 2, not 5.
const SERIALISE_SOURCE: &str = r#"context smoke

agent Probe {
  key id: String
  store seen: Cell[Int]

  on call ping() -> Effect[Int] { 1 }
}

agent Tally {
  key id: String
  store n: Cell[Int]

  on call bump() -> Effect[Int] {
    let p <- Probe("p").ping()
    let before = n
    n := before + p
    n
  }

  on call count() -> Effect[Int] { n }
}

service api from http {
  on GET("/") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }

  on GET("/bump") () -> Effect[HttpResult[String]] by v: Visitor {
    let n <- Tally("k").bump()
    Ok("bumped=\(n)")
  }

  on GET("/count") () -> Effect[HttpResult[String]] by v: Visitor {
    let n <- Tally("k").count()
    Ok("count=\(n)")
  }
}
"#;

#[test]
fn concurrent_handlers_on_one_agent_instance_all_count_on_workerd() {
    let Some(served) = serve_smoke(SERIALISE_SOURCE, "serialise") else {
        return;
    };
    let url = served.url.clone();
    // Warm the instances, so the burst below races the handlers and not the cold start.
    fetch(&url, "/count").expect("GET /count before the burst");
    let threads: Vec<_> = (0..5)
        .map(|_| {
            let url = url.clone();
            std::thread::spawn(move || fetch(&url, "/bump"))
        })
        .collect();
    for t in threads {
        let body = t.join().expect("a bump thread").expect("GET /bump");
        assert!(body.contains("bumped="), "unexpected /bump body: {body}");
    }
    let count = fetch(&url, "/count").expect("GET /count after the burst");
    assert!(
        count.contains("count=5"),
        "five concurrent bumps on one instance must count five, none lost to an \
         interleaved commit: {count}"
    );
}
