//! The `bynk.cloudflare` binding against a real `workerd`, and the KV fake
//! against the platform (standard-library S2, #1952).
//!
//! `binding_behaviour.rs` tests `WorkersKv` against a fake `KVNamespace` built
//! from Cloudflare's documented contract. That only proves anything if the fake
//! and the platform agree. This test runs one Worker under `wrangler dev`, whose
//! Miniflare KV is the platform's own local implementation, and drives every
//! `Kv` operation through compiled Bynk handlers: `put`, `putTtl`, `get` (present
//! and absent), `list` by prefix (in key order), and `delete`. The answers are
//! the ones the fake gives. S4a (#1966) adds two derived instances
//! (`capability Sessions = Kv`, `capability PriceCache = Kv`): each is its own
//! namespace, so a key written through one is absent through the others.
//!
//! P4a (#2602) adds `WorkersD1`: statements through compiled Bynk handlers
//! against Miniflare's own D1, pinning the shape a blob column arrives in.
//!
//! P5 (#2323) adds `WorkersR2` the same way: every `R2` operation through
//! compiled Bynk handlers against Miniflare's own R2, including both
//! conditions of `putWith` failing as `None` and `list` paging past R2's 1000
//! objects a page. P5b (#2626) adds the streamed reads and writes (a wrong
//! declared size faulting, nothing stored) and multipart uploads (completed
//! from the handle in a later request; R2's minimum part size) to the same
//! smoke, so it keeps band 1.
//!
//! The same toolchain skip and `BYNK_REQUIRE_WORKERD` gate as the events smokes;
//! see `events_ordering_workerd.rs` for why each exists. Teardown is the shared
//! [`wrangler::Wrangler`]'s process-group stop.

mod require;
mod tools;
mod wrangler;

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";

const SOURCE: &str = "context kv.smoke

consumes bynk.cloudflare { Kv }

capability Sessions = Kv
capability PriceCache = Kv

service api from http {
\ton GET(\"/put/:key/:value\") (key: String, value: String) -> Effect[HttpResult[String]] by Visitor given Kv {
\t\tlet _ <- Kv.put(key, value)
\t\tOk(\"put\")
\t}

\ton GET(\"/ttl/:key/:value\") (key: String, value: String) -> Effect[HttpResult[String]] by Visitor given Kv {
\t\tlet _ <- Kv.putTtl(key, value, 120)
\t\tOk(\"ttl\")
\t}

\ton GET(\"/get/:key\") (key: String) -> Effect[HttpResult[String]] by Visitor given Kv {
\t\tlet found <- Kv.get(key)
\t\tmatch found {
\t\t\tSome(v) => Ok(v)
\t\t\tNone => NotFound
\t\t}
\t}

\ton GET(\"/delete/:key\") (key: String) -> Effect[HttpResult[String]] by Visitor given Kv {
\t\tlet _ <- Kv.delete(key)
\t\tOk(\"deleted\")
\t}

\ton GET(\"/list/:prefix\") (prefix: String) -> Effect[HttpResult[List[String]]] by Visitor given Kv {
\t\tlet keys <- Kv.list(Some(prefix))
\t\tOk(keys)
\t}

\ton GET(\"/sessions/put/:key/:value\") (key: String, value: String) -> Effect[HttpResult[String]] by Visitor given Sessions {
\t\tlet _ <- Sessions.put(key, value)
\t\tOk(\"put\")
\t}

\ton GET(\"/sessions/get/:key\") (key: String) -> Effect[HttpResult[String]] by Visitor given Sessions {
\t\tlet found <- Sessions.get(key)
\t\tmatch found {
\t\t\tSome(v) => Ok(v)
\t\t\tNone => NotFound
\t\t}
\t}

\ton GET(\"/prices/get/:key\") (key: String) -> Effect[HttpResult[String]] by Visitor given PriceCache {
\t\tlet found <- PriceCache.get(key)
\t\tmatch found {
\t\t\tSome(v) => Ok(v)
\t\t\tNone => NotFound
\t\t}
\t}

\ton GET(\"/meta/put/:key/:who\") (key: String, who: String) -> Effect[HttpResult[String]] by Visitor given Kv {
\t\tlet _ <- Kv.putWith(key, \"v\", KvPut { expiration: After(5.minutes), metadata: Map.empty().insert(\"user\", who) })
\t\tOk(\"put\")
\t}

\ton GET(\"/meta/get/:key\") (key: String) -> Effect[HttpResult[String]] by Visitor given Kv {
\t\tlet found <- Kv.getWith(key, KvGet { cacheTtl: Some(30.seconds) })
\t\tmatch found {
\t\t\tSome(e) => Ok(\"\\(e.value) \\(e.metadata.get(\"user\").getOrElse(\"?\"))\")
\t\t\tNone => NotFound
\t\t}
\t}

\ton GET(\"/bytes/:key\") (key: String) -> Effect[HttpResult[String]] by Visitor given Sessions {
\t\tlet _ <- Sessions.putBytes(key, Bytes.fromHex(\"00ff07\").getOrElse(Bytes.empty()), KvPut { expiration: Never, metadata: Map.empty() })
\t\tlet back <- Sessions.getBytes(key)
\t\tmatch back {
\t\t\tSome(b) => Ok(b.toHex())
\t\t\tNone => NotFound
\t\t}
\t}

\ton GET(\"/pages/:prefix\") (prefix: String) -> Effect[HttpResult[String]] by Visitor given Kv {
\t\tlet first <- Kv.listPage(KvList { prefix: Some(prefix), cursor: None, limit: Some(2) })
\t\tlet second <- Kv.listPage(KvList { prefix: Some(prefix), cursor: first.cursor, limit: Some(2) })
\t\tlet expiring = first.keys.filter((k) => k.expiration.isSome()).length()
\t\tOk(\"\\(first.keys.length()) \\(first.cursor.isSome()) \\(second.keys.length()) \\(second.cursor.isSome()) \\(expiring)\")
\t}
}
";

fn skip(reason: &str) -> bool {
    if require::is_required(REQUIRE_ENV) {
        panic!("{REQUIRE_ENV} is set but {reason}");
    }
    eprintln!("\n!!! BINDING (WORKERD) SKIPPED !!!\n{reason}\n");
    true
}

/// `GET path` on `port`, as `(status, body)`.
fn get(port: u16, path: &str) -> Result<(u16, String), String> {
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
    let status = response
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("unparseable response: {response:?}"))?;
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Ok((status, body))
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| format!("<could not read {path:?}: {e}>"))
}

/// Each of `steps` (`(path, status, body)`, an empty body not being compared)
/// against the Worker on `port`: what it answered, beside what was expected.
fn check_steps(port: u16, steps: &[(&str, u16, &str)]) -> Vec<String> {
    let mut failures = Vec::new();
    for (path, status, body) in steps {
        match get(port, path) {
            Ok((s, b)) if s == *status && (body.is_empty() || b.trim() == *body) => {}
            Ok((s, b)) => failures.push(format!("{path}: {s} {b:?}, expected {status} {body}")),
            Err(e) => failures.push(format!("{path}: {e}")),
        }
    }
    failures
}

/// One Worker under `wrangler dev`, driven over HTTP. `source` is a context
/// named `<ctx>.smoke` in the project `<project>`, which must answer `GET
/// <boot>` once up; `drive` talks to it on its port and returns what the
/// platform answered differently from the contract. `band` offsets the port
/// within this file's own range so that two smokes can run at once. The
/// disagreements with the wrangler log, or `None` when the smoke was skipped.
fn run_smoke(
    project: &str,
    ctx: &str,
    source: &str,
    boot: &str,
    band: u16,
    drive: &dyn Fn(u16) -> Vec<String>,
) -> Option<(Vec<String>, String)> {
    run_smoke_with(project, ctx, source, &[], &|_| None, boot, band, drive)
}

/// [`run_smoke`] with `files` (project-relative path, content) written into
/// the project beside the source, and `prepare` run on the compiled Worker's
/// directory before `wrangler dev` starts; a `Some` from it is a failure that
/// stops the smoke.
#[allow(clippy::too_many_arguments)]
fn run_smoke_with(
    project: &str,
    ctx: &str,
    source: &str,
    files: &[(&str, &str)],
    prepare: &dyn Fn(&std::path::Path) -> Option<String>,
    boot: &str,
    band: u16,
    drive: &dyn Fn(u16) -> Vec<String>,
) -> Option<(Vec<String>, String)> {
    if !tools::tool_exists("npx") && skip("`npx` is not on PATH") {
        return None;
    }
    if !tools::tool_exists("node") && skip("`node` is not on PATH") {
        return None;
    }

    let tmp = std::env::temp_dir().join(format!(
        "bynk-binding-workerd-{project}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join(format!("proj/src/{ctx}"))).unwrap();
    fs::write(
        tmp.join("proj/bynk.toml"),
        format!(
            "[project]\nname = \"{project}\"\nversion = \"0.1.0\"\n\n[unstable]\nallow = [\"bynk.cloudflare.R2\", \"bynk.cloudflare.RateLimit\", \"bynk.cloudflare.Queue\", \"bynk.cloudflare.Analytics\", \"bynk.Fetch.sendWith\", \"bynk.Fetch.receiveBytes\", \"bynk.Fetch.sendBytes\", \"bynk.Fetch.receiveStream\", \"bynk.Fetch.sendStream\", \"bynk.cloudflare.Kv.getWith\", \"bynk.cloudflare.Kv.putWith\", \"bynk.cloudflare.Kv.getBytes\", \"bynk.cloudflare.Kv.putBytes\", \"bynk.cloudflare.Kv.listPage\"]\n"
        ),
    )
    .unwrap();
    fs::write(tmp.join(format!("proj/src/{ctx}/smoke.bynk")), source).unwrap();
    for (path, content) in files {
        let path = tmp.join("proj").join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    let compiled = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["compile", "--target", "workers", "--emit", "js", "-o"])
        .arg(tmp.join("out"))
        .arg(tmp.join("proj"))
        .output()
        .expect("run bynkc");
    assert!(
        compiled.status.success(),
        "the {project} smoke does not compile:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let dir = tmp.join(format!("out/workers/{project}"));
    assert!(
        dir.join("index.js").is_file() && dir.join("wrangler.toml").is_file(),
        "expected out/workers/{project} — context dasherisation changed?"
    );

    if let Some(failure) = prepare(&dir) {
        let _ = fs::remove_dir_all(&tmp);
        return Some((vec![failure], String::new()));
    }

    // Its own band, clear of the events smokes' (30_000 + 4·pid, 40_000 + 2·pid,
    // 55_000 + 2·pid, each mod 4_000), so they can all run at once. Bands 5
    // and 6 (40_000 to 48_000) share a port with the events smokes for half
    // of all pids, so the Fetch smokes use 8 and 9 (#2604), and the D1
    // migrations smoke 10, the last band below port 65_536 (#2608).
    let port = 20_000 + 2 * (std::process::id() % 2_000) as u16 + 4_000 * band;
    let inspector_port = port + 1;
    let log_path = tmp.join("wrangler.log");
    let out_log = fs::File::create(&log_path).unwrap();
    let err_log = out_log.try_clone().unwrap();
    let spawned = wrangler::Wrangler::spawn(
        tools::base_command("npx")
            .args([
                "-y",
                wrangler::SPEC,
                "dev",
                "--port",
                &port.to_string(),
                "--inspector-port",
                &inspector_port.to_string(),
            ])
            .current_dir(&dir)
            .stdout(Stdio::from(out_log))
            .stderr(Stdio::from(err_log)),
        &tmp.join("registry"),
    );
    let Ok(server) = spawned else {
        skip("could not launch npx");
        return None;
    };

    let deadline = Instant::now() + Duration::from_secs(180);
    while get(port, boot).is_err() {
        if Instant::now() > deadline {
            let log = read_log(&log_path);
            drop(server);
            skip(&format!(
                "wrangler dev did not serve within the boot window (likely no network \
                 to provision {})\n--- wrangler log ---\n{log}",
                wrangler::SPEC
            ));
            return None;
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    let failures = drive(port);
    let log = read_log(&log_path);
    drop(server);
    let _ = fs::remove_dir_all(&tmp);
    Some((failures, log))
}

#[test]
fn workers_kv_round_trips_on_workerd() {
    // Each step, then what the platform answered, beside what the fake answers.
    let steps: &[(&str, u16, &str)] = &[
        ("/put/user:2/b", 200, r#""put""#),
        ("/put/user:1/a", 200, r#""put""#),
        ("/ttl/user:3/c", 200, r#""ttl""#),
        ("/put/order:1/z", 200, r#""put""#),
        ("/get/user:1", 200, r#""a""#),
        ("/get/user:3", 200, r#""c""#),
        ("/get/absent", 404, ""),
        ("/list/user:", 200, r#"["user:1","user:2","user:3"]"#),
        ("/delete/user:1", 200, r#""deleted""#),
        ("/get/user:1", 404, ""),
        ("/delete/never-there", 200, r#""deleted""#),
        ("/list/user:", 200, r#"["user:2","user:3"]"#),
        ("/list/nothing:", 200, "[]"),
        // S4a (#1966): three namespaces, not one.
        ("/sessions/put/user:2/s", 200, r#""put""#),
        ("/sessions/get/user:2", 200, r#""s""#),
        ("/get/user:2", 200, r#""b""#),
        ("/prices/get/user:2", 404, ""),
        ("/sessions/get/user:3", 404, ""),
        // P1 (#2590): metadata and an expiry, binary values, and paging.
        ("/meta/put/user:4/ada", 200, r#""put""#),
        ("/meta/get/user:4", 200, r#""v ada""#),
        ("/meta/get/absent", 404, ""),
        ("/bytes/blob:1", 200, r#""00ff07""#),
        // user:2, user:3 (TTL'd) and user:4 (expiring): two pages.
        ("/pages/user:", 200, r#""2 true 1 false 1""#),
    ];
    let Some((failures, log)) = run_smoke("kv-smoke", "kv", SOURCE, "/get/boot", 0, &|port| {
        check_steps(port, steps)
    }) else {
        return;
    };
    assert!(
        failures.is_empty(),
        "WorkersKv on workerd disagrees with the contract its fake models:\n{}\n--- wrangler log ---\n{log}",
        failures.join("\n")
    );
}

const R2_SOURCE: &str = "context r2.smoke

consumes bynk.cloudflare
consumes bynk.cloudflare { R2 }

capability Archives = R2
capability Indexes = R2

service api from http {
\ton GET(\"/put/:key/:value\") (key: String, value: String) -> Effect[HttpResult[String]] by Visitor given Archives {
\t\tlet o <- Archives.put(key, Bytes.fromUtf8(value))
\t\tOk(o.etag)
\t}

\ton GET(\"/get/:key\") (key: String) -> Effect[HttpResult[String]] by Visitor given Archives {
\t\tlet found <- Archives.get(key)
\t\tmatch found {
\t\t\tSome(b) => Ok(b.toHex())
\t\t\tNone => NotFound
\t\t}
\t}

\ton GET(\"/size/:key\") (key: String) -> Effect[HttpResult[Int]] by Visitor given Archives {
\t\tlet found <- Archives.head(key)
\t\tmatch found {
\t\t\tSome(o) => Ok(o.size)
\t\t\tNone => NotFound
\t\t}
\t}

\ton GET(\"/etag/:key\") (key: String) -> Effect[HttpResult[String]] by Visitor given Archives {
\t\tlet found <- Archives.head(key)
\t\tmatch found {
\t\t\tSome(o) => Ok(o.etag)
\t\t\tNone => NotFound
\t\t}
\t}

\ton GET(\"/meta/:key/:value\") (key: String, value: String) -> Effect[HttpResult[String]] by Visitor given Archives {
\t\tlet _ <- Archives.putWith(key, Bytes.fromUtf8(value), R2Put {
\t\t\tcontentType: Some(\"text/plain\"),
\t\t\tmetadata: Map.empty().insert(\"origin\", value),
\t\t\tonlyIf: Always,
\t\t})
\t\tlet found <- Archives.head(key)
\t\tmatch found {
\t\t\tSome(o) => {
\t\t\t\tlet ct = o.contentType.getOrElse(\"-\")
\t\t\t\tlet origin = o.metadata.get(\"origin\").getOrElse(\"-\")
\t\t\t\tOk(\"\\(ct):\\(origin)\")
\t\t\t}
\t\t\tNone => NotFound
\t\t}
\t}

\ton GET(\"/create/:key/:value\") (key: String, value: String) -> Effect[HttpResult[String]] by Visitor given Archives {
\t\tlet stored <- Archives.putWith(key, Bytes.fromUtf8(value), R2Put {
\t\t\tcontentType: None,
\t\t\tmetadata: Map.empty(),
\t\t\tonlyIf: Absent,
\t\t})
\t\tmatch stored {
\t\t\tSome(_) => Ok(\"created\")
\t\t\tNone => Ok(\"exists\")
\t\t}
\t}

\ton GET(\"/swap/:key/:etag/:value\") (key: String, etag: String, value: String) -> Effect[HttpResult[String]] by Visitor given Archives {
\t\tlet stored <- Archives.putWith(key, Bytes.fromUtf8(value), R2Put {
\t\t\tcontentType: None,
\t\t\tmetadata: Map.empty(),
\t\t\tonlyIf: EtagMatches(etag),
\t\t})
\t\tmatch stored {
\t\t\tSome(_) => Ok(\"swapped\")
\t\t\tNone => Ok(\"stale\")
\t\t}
\t}

\ton GET(\"/delete/:key\") (key: String) -> Effect[HttpResult[String]] by Visitor given Archives {
\t\tlet _ <- Archives.delete(key)
\t\tOk(\"deleted\")
\t}

\ton GET(\"/list/:prefix\") (prefix: String) -> Effect[HttpResult[List[String]]] by Visitor given Archives {
\t\tlet page <- Archives.list(Some(prefix), None)
\t\tOk(page.keys.map((o) => o.key))
\t}

\ton GET(\"/page1/:prefix\") (prefix: String) -> Effect[HttpResult[List[String]]] by Visitor given Archives {
\t\tlet first <- Archives.list(Some(prefix), None)
\t\tOk(first.keys.map((o) => o.key))
\t}

\ton GET(\"/page2/:prefix\") (prefix: String) -> Effect[HttpResult[List[String]]] by Visitor given Archives {
\t\tlet first <- Archives.list(Some(prefix), None)
\t\tlet second <- Archives.list(Some(prefix), first.cursor)
\t\tOk(second.keys.map((o) => o.key))
\t}

\ton GET(\"/indexes/put/:key/:value\") (key: String, value: String) -> Effect[HttpResult[String]] by Visitor given Indexes {
\t\tlet _ <- Indexes.put(key, Bytes.fromUtf8(value))
\t\tOk(\"put\")
\t}

\ton GET(\"/indexes/get/:key\") (key: String) -> Effect[HttpResult[String]] by Visitor given Indexes {
\t\tlet found <- Indexes.get(key)
\t\tmatch found {
\t\t\tSome(b) => Ok(b.toHex())
\t\t\tNone => NotFound
\t\t}
\t}

\ton GET(\"/stream/:key\") (key: String) -> Effect[HttpResult[()]] by Visitor given Archives {
\t\tlet found <- Archives.getStream(key)
\t\tmatch found {
\t\t\tSome(o) => RawStreamed(o.body, o.object.contentType.getOrElse(\"application/octet-stream\"))
\t\t\tNone => NotFound
\t\t}
\t}

\ton GET(\"/copy/:src/:dst\") (src: String, dst: String) -> Effect[HttpResult[Int]] by Visitor given Archives {
\t\tlet found <- Archives.getStream(src)
\t\tmatch found {
\t\t\tSome(o) => {
\t\t\t\tlet copied <- Archives.putStream(dst, o.body, o.object.size)
\t\t\t\tOk(copied.size)
\t\t\t}
\t\t\tNone => NotFound
\t\t}
\t}

\ton GET(\"/sized/:key/:value/:fit\") (key: String, value: String, fit: String) -> Effect[HttpResult[Int]] by Visitor given Archives {
\t\tlet held = 2 * Bytes.fromUtf8(value).length()
\t\tlet size = if fit == \"long\" { held - 1 } else if fit == \"short\" { held + 1 } else { held }
\t\tlet o <- Archives.putStream(key, Stream.of([Bytes.fromUtf8(value), Bytes.fromUtf8(\"\"), Bytes.fromUtf8(value)]), size)
\t\tOk(o.size)
\t}

\ton GET(\"/screate/:key/:value\") (key: String, value: String) -> Effect[HttpResult[String]] by Visitor given Archives {
\t\tlet stored <- Archives.putStreamWith(key, Stream.of([Bytes.fromUtf8(value)]), Bytes.fromUtf8(value).length(), R2Put {
\t\t\tcontentType: Some(\"text/plain\"),
\t\t\tmetadata: Map.empty(),
\t\t\tonlyIf: Absent,
\t\t})
\t\tmatch stored {
\t\t\tSome(_) => Ok(\"created\")
\t\t\tNone => Ok(\"exists\")
\t\t}
\t}

\ton GET(\"/mp/begin/:key\") (key: String) -> Effect[HttpResult[String]] by Visitor given Archives {
\t\tlet u <- Archives.createUpload(key, R2UploadOptions { contentType: Some(\"text/plain\"), metadata: Map.empty().insert(\"by\", \"smoke\") })
\t\tOk(u.uploadId)
\t}

\ton GET(\"/mp/finish/:key/:id\") (key: String, id: String) -> Effect[HttpResult[String]] by Visitor given Archives {
\t\tlet u = R2Upload { key: key, uploadId: id }
\t\tlet tail <- Archives.uploadPart(u, 2, Bytes.fromUtf8(\"tail\"))
\t\tlet head <- Archives.uploadPart(u, 1, grow(Bytes.fromUtf8(\"x\"), 23))
\t\tlet o <- Archives.completeUpload(u, [tail, head])
\t\tOk(\"\\(o.size) \\(o.contentType.getOrElse(\"-\")) \\(o.metadata.get(\"by\").getOrElse(\"-\"))\")
\t}

\ton GET(\"/mp/small/:key\") (key: String) -> Effect[HttpResult[Int]] by Visitor given Archives {
\t\tlet u <- Archives.createUpload(key, R2UploadOptions { contentType: None, metadata: Map.empty() })
\t\tlet a <- Archives.uploadPart(u, 1, Bytes.fromUtf8(\"a\"))
\t\tlet b <- Archives.uploadPart(u, 2, Bytes.fromUtf8(\"b\"))
\t\tlet o <- Archives.completeUpload(u, [a, b])
\t\tOk(o.size)
\t}

\ton GET(\"/mp/abort/:key\") (key: String) -> Effect[HttpResult[String]] by Visitor given Archives {
\t\tlet u <- Archives.createUpload(key, R2UploadOptions { contentType: None, metadata: Map.empty() })
\t\tlet _ <- Archives.uploadPart(u, 1, Bytes.fromUtf8(\"a\"))
\t\tlet _ <- Archives.abortUpload(u)
\t\tOk(\"aborted\")
\t}
}

// 2^n copies of `b`: 8 MiB from one byte at n = 23, over R2's 5 MiB minimum part.
fn grow(b: Bytes, n: Int) -> Bytes {
\tif n == 0 {
\t\tb
\t} else {
\t\tgrow(b.concat(b), n - 1)
\t}
}
";

#[test]
fn workers_r2_round_trips_on_workerd() {
    // Each step, then what the platform answered, beside what the fake answers.
    // "hi" is 6869 in hex, "yo" 796f.
    let steps: &[(&str, u16, &str)] = &[
        // get / head / put, present and absent
        ("/put/a/hi", 200, ""),
        ("/get/a", 200, r#""6869""#),
        ("/get/absent", 404, ""),
        ("/size/a", 200, "2"),
        ("/size/absent", 404, ""),
        // content type and custom metadata survive a write
        ("/meta/m/yo", 200, r#""text/plain:yo""#),
        // Absent: create only; an existing key is None and unchanged
        ("/create/c/hi", 200, r#""created""#),
        ("/create/c/yo", 200, r#""exists""#),
        ("/get/c", 200, r#""6869""#),
        // EtagMatches: a stale etag is None and unchanged
        ("/swap/a/not-the-etag/yo", 200, r#""stale""#),
        ("/get/a", 200, r#""6869""#),
        ("/swap/never-written/not-the-etag/yo", 200, r#""stale""#),
        ("/get/never-written", 404, ""),
        // delete is idempotent
        ("/delete/a", 200, r#""deleted""#),
        ("/delete/a", 200, r#""deleted""#),
        ("/get/a", 404, ""),
        // list by prefix, in key order
        ("/put/p~2/x", 200, ""),
        ("/put/p~1/x", 200, ""),
        ("/put/q~1/x", 200, ""),
        ("/list/p~", 200, r#"["p~1","p~2"]"#),
        ("/list/nothing~", 200, "[]"),
        // two buckets, not one
        ("/indexes/put/p~1/z", 200, r#""put""#),
        ("/indexes/get/p~1", 200, r#""7a""#),
        ("/indexes/get/c", 404, ""),
        // P5b (#2626): a streamed write of its declared size, through
        // `FixedLengthStream`; a size the stream doesn't hold faults, and
        // nothing is stored
        ("/sized/s/hi/exact", 200, "4"),
        ("/get/s", 200, r#""68696869""#),
        ("/sized/long/hi/long", 500, ""),
        ("/get/long", 404, ""),
        ("/sized/short/hi/short", 500, ""),
        ("/get/short", 404, ""),
        // a streamed read copied into a streamed write
        ("/copy/s/s2", 200, "4"),
        ("/get/s2", 200, r#""68696869""#),
        ("/copy/absent/x", 404, ""),
        ("/stream/absent", 404, ""),
        // putStreamWith honours its condition
        ("/screate/sc/hi", 200, r#""created""#),
        ("/screate/sc/yo", 200, r#""exists""#),
        ("/get/sc", 200, r#""6869""#),
        // an aborted upload stores nothing
        ("/mp/abort/ab", 200, r#""aborted""#),
        ("/get/ab", 404, ""),
    ];
    let Some((failures, log)) = run_smoke("r2-smoke", "r2", R2_SOURCE, "/get/boot", 1, &|port| {
        let mut failures = check_steps(port, steps);
        // Compare-and-swap on the etag R2 minted: it swaps once, and the same
        // etag is then stale.
        match get(port, "/etag/c") {
            Ok((200, body)) => {
                let etag = body.trim().trim_matches('"').to_string();
                failures.extend(check_steps(
                    port,
                    &[
                        (&format!("/swap/c/{etag}/yo"), 200, r#""swapped""#),
                        ("/get/c", 200, r#""796f""#),
                        (&format!("/swap/c/{etag}/again"), 200, r#""stale""#),
                        ("/get/c", 200, r#""796f""#),
                    ],
                ));
            }
            other => failures.push(format!("/etag/c: {other:?}")),
        }
        // P5b (#2626): a streamed body is served as the object's bytes.
        match get(port, "/stream/sc") {
            Ok((200, body)) if body.contains("hi") => {}
            other => failures.push(format!("/stream/sc: {other:?}, expected 200 with hi")),
        }
        // A multipart upload begun in one request and completed in another,
        // from its handle alone: an 8 MiB part and a short last one.
        match get(port, "/mp/begin/big") {
            Ok((200, body)) => {
                let id = body.trim().trim_matches('"').to_string();
                failures.extend(check_steps(
                    port,
                    &[
                        (
                            &format!("/mp/finish/big/{id}"),
                            200,
                            r#""8388612 text/plain smoke""#,
                        ),
                        ("/size/big", 200, "8388612"),
                    ],
                ));
            }
            other => failures.push(format!("/mp/begin/big: {other:?}")),
        }
        // Parts under R2's 5 MiB minimum, but for the last, are refused.
        failures.extend(check_steps(
            port,
            &[("/mp/small/sm", 500, ""), ("/get/sm", 404, "")],
        ));
        // R2 lists at most 1000 objects a page (fewer, when asked for their
        // metadata, as `list` does), so 1001 span at least two: the cursor
        // `list` returns is the one the platform accepts, and the second page
        // carries on in key order after the first.
        for i in 0..1001 {
            if !matches!(get(port, &format!("/put/bulk-{i:04}/x")), Ok((200, _))) {
                failures.push(format!("seeding bulk-{i:04} failed"));
                return failures;
            }
        }
        let page = |path: &str| -> Result<Vec<String>, String> {
            let (status, body) = get(port, path)?;
            let keys: serde_json::Value =
                serde_json::from_str(body.trim()).map_err(|e| format!("{path}: {e}"))?;
            match (status, keys.as_array()) {
                (200, Some(a)) => Ok(a
                    .iter()
                    .filter_map(|k| k.as_str().map(str::to_string))
                    .collect()),
                _ => Err(format!("{path}: {status} {body:?}")),
            }
        };
        match (page("/page1/bulk-"), page("/page2/bulk-")) {
            (Ok(first), Ok(second)) => {
                let ordered = first.windows(2).all(|w| w[0] < w[1])
                    && second.windows(2).all(|w| w[0] < w[1])
                    && first.last() < second.first();
                if first.is_empty() || first.len() > 1000 || second.is_empty() || !ordered {
                    failures.push(format!(
                        "pages of {} and {} keys, not two disjoint pages in key order",
                        first.len(),
                        second.len()
                    ));
                }
            }
            (a, b) => failures.push(format!("paging: {a:?} {b:?}")),
        }
        failures
    }) else {
        return;
    };
    assert!(
        failures.is_empty(),
        "WorkersR2 on workerd disagrees with the contract its fake models:\n{}\n--- wrangler log ---\n{log}",
        failures.join("\n")
    );
}

const RATELIMIT_SOURCE: &str = "context rl.smoke

consumes bynk.cloudflare
consumes bynk.cloudflare { RateLimit }

capability Login = RateLimit @limit(2) @period(60.seconds)
capability Reads = RateLimit @limit(1) @period(10.seconds)

service api from http {
\ton GET(\"/login/:key\") (key: String) -> Effect[HttpResult[Bool]] by Visitor given Login {
\t\tlet allowed <- Login.allow(key)
\t\tOk(allowed)
\t}

\ton GET(\"/reads/:key\") (key: String) -> Effect[HttpResult[Bool]] by Visitor given Reads {
\t\tlet allowed <- Reads.allow(key)
\t\tOk(allowed)
\t}
}
";

#[test]
fn workers_ratelimit_counts_on_workerd() {
    // P3 (#2342): Miniflare simulates `[[ratelimits]]` from the emitted stanza
    // — its placeholder `namespace_id` included — and counts per key, per
    // limiter, as the fakes do.
    let steps: &[(&str, u16, &str)] = &[
        ("/login/a", 200, "true"),
        ("/login/a", 200, "true"),
        ("/login/a", 200, "false"),
        ("/login/b", 200, "true"),
        // Another limiter counts apart: the same key is fresh there.
        ("/reads/a", 200, "true"),
        ("/reads/a", 200, "false"),
    ];
    let Some((failures, log)) = run_smoke(
        "rl-smoke",
        "rl",
        RATELIMIT_SOURCE,
        "/reads/boot",
        2,
        &|port| check_steps(port, steps),
    ) else {
        return;
    };
    assert!(
        failures.is_empty(),
        "WorkersRateLimit on workerd disagrees with the contract its fake models:\n{}\n--- wrangler log ---\n{log}",
        failures.join("\n")
    );
}

const QUEUE_SOURCE: &str = "context q.smoke

consumes bynk
consumes bynk.cloudflare { Kv, Queue }

type Placed = {
\tid: String,
\ttotal: Int,
}

capability Orders = Queue @name(\"q-smoke-orders\")

service api from http {
\ton GET(\"/send/:id\") (id: String) -> Effect[HttpResult[String]] by Visitor given Orders {
\t\tlet _ <- Orders.send(Json.encode(Placed { id: id, total: 7 }))
\t\tOk(\"sent\")
\t}

\ton GET(\"/later/:id\") (id: String) -> Effect[HttpResult[String]] by Visitor given Orders {
\t\tlet _ <- Orders.sendWith(Json.encode(Placed { id: id, total: 8 }), QueueSend { delay: Some(1.seconds) })
\t\tOk(\"sent\")
\t}

\ton GET(\"/batch/:a/:b\") (a: String, b: String) -> Effect[HttpResult[String]] by Visitor given Orders {
\t\tlet _ <- Orders.sendBatch([Json.encode(Placed { id: a, total: 1 }), Json.encode(Placed { id: b, total: 2 })])
\t\tOk(\"sent\")
\t}

\ton GET(\"/value/:id\") (id: String) -> Effect[HttpResult[String]] by Visitor given Orders {
\t\tlet _ <- Orders.sendValue(Placed { id: id, total: 9 })
\t\tOk(\"sent\")
\t}

\ton GET(\"/raw/:text\") (text: String) -> Effect[HttpResult[String]] by Visitor given Orders {
\t\tlet _ <- Orders.send(text)
\t\tOk(\"sent\")
\t}

\ton GET(\"/seen/:id\") (id: String) -> Effect[HttpResult[String]] by Visitor given Kv {
\t\tlet found <- Kv.get(id)
\t\tmatch found {
\t\t\tSome(total) => Ok(total)
\t\t\tNone => NotFound
\t\t}
\t}
}

service fulfilment from queue(\"q-smoke-orders\") {
\ton message(m: Placed) -> Effect[QueueResult] given Kv {
\t\tlet _ <- Kv.put(m.id, m.total.toString())
\t\tAck
\t}
}
";

#[test]
fn workers_queue_delivers_to_a_bynk_consumer_on_workerd() {
    // P2 (#2578): Miniflare simulates `[[queues.producers]]` and
    // `[[queues.consumers]]` in one Worker. A body sent as parsed JSON reaches
    // the generated consumer as the value its decoder expects, which is the
    // contract `WorkersQueue` exists to keep; text that is not JSON is refused
    // before anything is sent.
    let Some((failures, log)) = run_smoke("q-smoke", "q", QUEUE_SOURCE, "/seen/boot", 3, &|port| {
        let mut failures = check_steps(
            port,
            &[
                ("/send/o-1", 200, r#""sent""#),
                ("/later/o-2", 200, r#""sent""#),
                ("/batch/o-3/o-4", 200, r#""sent""#),
                // `sendValue` encodes at the call site and must reach the same
                // consumer, decoded the same way, as `send(Json.encode(…))`.
                ("/value/o-5", 200, r#""sent""#),
                ("/raw/not-json", 500, ""),
            ],
        );
        // Local queues deliver in batches, after a timeout: poll.
        let expected = [
            ("o-1", "7"),
            ("o-2", "8"),
            ("o-3", "1"),
            ("o-4", "2"),
            ("o-5", "9"),
        ];
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let seen = expected
                .iter()
                .map(|(id, _)| get(port, &format!("/seen/{id}")))
                .collect::<Vec<_>>();
            let all = seen.iter().zip(&expected).all(
                |(r, (_, total))| matches!(r, Ok((200, b)) if b.trim() == format!("\"{total}\"")),
            );
            if all || Instant::now() > deadline {
                if !all {
                    failures.push(format!("not every message was consumed: {seen:?}"));
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        failures
    }) else {
        return;
    };
    assert!(
        failures.is_empty(),
        "WorkersQueue on workerd disagrees with the contract its fake models:\n{}\n--- wrangler log ---\n{log}",
        failures.join("\n")
    );
}

const ANALYTICS_SOURCE: &str = "context a.smoke

consumes bynk
consumes bynk.cloudflare { Analytics }

capability Signups = Analytics

service api from http {
\ton GET(\"/write/:plan\") (plan: String) -> Effect[HttpResult[String]] by Visitor given Signups {
\t\tlet _ <- Signups.write(DataPoint { index: Some(plan), blobs: [plan, \"web\"], doubles: [1.0, 2.5] })
\t\tOk(\"written\")
\t}

\ton GET(\"/bare\") () -> Effect[HttpResult[String]] by Visitor given Signups {
\t\tlet _ <- Signups.write(DataPoint { index: None, blobs: [], doubles: [] })
\t\tOk(\"written\")
\t}

\ton GET(\"/wide\") () -> Effect[HttpResult[String]] by Visitor given Signups {
\t\tlet _ <- Signups.write(DataPoint { index: None, blobs: [], doubles: [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0] })
\t\tOk(\"written\")
\t}
}
";

#[test]
fn workers_analytics_writes_on_workerd() {
    // P3b (#2582): `wrangler dev` binds `[[analytics_engine_datasets]]` and
    // drops the writes, so this shows the stanza and the binding agree and a
    // write within the limits goes through; a point over them is refused
    // before the platform sees it.
    let Some((failures, log)) = run_smoke("a-smoke", "a", ANALYTICS_SOURCE, "/bare", 4, &|port| {
        check_steps(
            port,
            &[
                ("/write/pro", 200, r#""written""#),
                ("/bare", 200, r#""written""#),
                ("/wide", 500, ""),
            ],
        )
    }) else {
        return;
    };
    assert!(
        failures.is_empty(),
        "WorkersAnalytics on workerd disagrees with the contract its fake models:\n{}\n--- wrangler log ---\n{log}",
        failures.join("\n")
    );
}

const D1_SOURCE: &str = "context d.smoke

consumes bynk
consumes bynk.cloudflare { D1 }

capability Orders = D1

type Order = { id: String, total: Int, paid: Int, receipt: Bytes, note: Option[String], weight: Float }

service api from http {
\ton GET(\"/setup\") () -> Effect[HttpResult[String]] by Visitor given Orders {
\t\tlet made <- Orders.run(\"CREATE TABLE IF NOT EXISTS orders (id TEXT PRIMARY KEY, total INTEGER NOT NULL CHECK (total BETWEEN 0 AND 100), paid INTEGER NOT NULL, receipt BLOB NOT NULL, note TEXT, weight REAL)\", [])
\t\tmatch made {
\t\t\tOk(_) => Ok(\"ready\")
\t\t\tErr(e) => ServerError(\"setup failed\")
\t\t}
\t}

\ton GET(\"/put/:id\") (id: String) -> Effect[HttpResult[String]] by Visitor given Orders {
\t\tlet total = if id == \"c\" { 5 } else { 95 }
\t\tlet made <- Orders.run(\"INSERT INTO orders VALUES (?1, ?2, ?3, ?4, ?5, ?6)\", [Text(id), Integer(total), Boolean(true), Blob(Bytes.fromUtf8(id)), Null, Real(1.5)])
\t\tmatch made {
\t\t\tOk(run) => Ok(\"changed \\(run.changes)\")
\t\t\tErr(Constraint(_)) => Conflict(\"exists\")
\t\t\tErr(_) => ServerError(\"insert failed\")
\t\t}
\t}

\ton GET(\"/get/:id\") (id: String) -> Effect[HttpResult[String]] by Visitor given Orders {
\t\tlet row <- Orders.first(\"SELECT * FROM orders WHERE id = ?1\", [Text(id)])
\t\tmatch row {
\t\t\tOk(Some(text)) => match Json.decode[Order](text) {
\t\t\t\tOk(o) => Ok(\"\\(o.id) \\(o.total) \\(o.paid) \\(o.receipt.toHex()) \\(o.weight)\")
\t\t\t\tErr(_) => ServerError(\"did not decode: \\(text)\")
\t\t\t}
\t\t\tOk(None) => NotFound
\t\t\tErr(_) => ServerError(\"query failed\")
\t\t}
\t}

\ton GET(\"/count\") () -> Effect[HttpResult[String]] by Visitor given Orders {
\t\tlet rows <- Orders.all(\"SELECT * FROM orders ORDER BY id\", [])
\t\tmatch rows {
\t\t\tOk(text) => match Json.decode[List[Order]](text) {
\t\t\t\tOk(os) => Ok(\"\\(os.length())\")
\t\t\t\tErr(_) => ServerError(\"did not decode: \\(text)\")
\t\t\t}
\t\t\tErr(_) => ServerError(\"query failed\")
\t\t}
\t}

\ton GET(\"/move/:a/:b\") (a: String, b: String) -> Effect[HttpResult[String]] by Visitor given Orders {
\t\tlet ran <- Orders.batch([
\t\t\tSqlStatement { sql: \"UPDATE orders SET total = total - 10 WHERE id = ?1\", params: [Text(a)] },
\t\t\tSqlStatement { sql: \"UPDATE orders SET total = total + 10 WHERE id = ?1\", params: [Text(b)] }
\t\t])
\t\tmatch ran {
\t\t\tOk(runs) => Ok(\"moved \\(runs.length())\")
\t\t\tErr(Constraint(_)) => Conflict(\"refused\")
\t\t\tErr(_) => ServerError(\"batch failed\")
\t\t}
\t}

\ton GET(\"/bad\") () -> Effect[HttpResult[String]] by Visitor given Orders {
\t\tlet r <- Orders.all(\"SELEC 1\", [])
\t\tmatch r {
\t\t\tErr(Failed(_)) => Ok(\"failed\")
\t\t\t_ => ServerError(\"not a failure\")
\t\t}
\t}
}
";

#[test]
fn workers_d1_round_trips_on_workerd() {
    // P4a (#2602): Miniflare's local D1 behind the generated `[[d1_databases]]`
    // stanza (its placeholder id is a local name). Every `SqlValue` binds, a
    // blob column decodes as `Bytes` (so the binding reads D1's blob shape), a
    // UNIQUE or CHECK breach is `Constraint`, a batch that breaks one rolls
    // back, and a syntax error is `Failed`.
    let Some((failures, log)) = run_smoke("d-smoke", "d", D1_SOURCE, "/setup", 7, &|port| {
        check_steps(
            port,
            &[
                ("/put/a", 200, r#""changed 1""#),
                ("/put/a", 409, ""),
                ("/put/b", 200, r#""changed 1""#),
                ("/put/c", 200, r#""changed 1""#),
                ("/get/a", 200, r#""a 95 1 61 1.5""#),
                ("/get/z", 404, ""),
                ("/count", 200, r#""3""#),
                ("/move/a/c", 200, r#""moved 2""#),
                // `a` drops to 75, then `b` would pass 100: neither stays.
                ("/move/a/b", 409, ""),
                ("/get/a", 200, r#""a 85 1 61 1.5""#),
                ("/get/b", 200, r#""b 95 1 62 1.5""#),
                ("/bad", 200, r#""failed""#),
            ],
        )
    }) else {
        return;
    };
    assert!(
        failures.is_empty(),
        "WorkersD1 on workerd disagrees with the contract its fake models:\n{}\n--- wrangler log ---\n{log}",
        failures.join("\n")
    );
}

const D1_MIGRATIONS_SOURCE: &str = "context m.smoke

consumes bynk
consumes bynk.cloudflare { D1 }

capability Items = D1

type Item = { id: String, label: String }

service api from http {
\ton GET(\"/items\") () -> Effect[HttpResult[String]] by Visitor given Items {
\t\tlet rows <- Items.all(\"SELECT id, label FROM items ORDER BY id\", [])
\t\tmatch rows {
\t\t\tOk(text) => match Json.decode[List[Item]](text) {
\t\t\t\tOk(items) => Ok(\"\\(items.length())\")
\t\t\t\tErr(_) => ServerError(\"did not decode: \\(text)\")
\t\t\t}
\t\t\tErr(_) => ServerError(\"query failed\")
\t\t}
\t}

\ton GET(\"/label/:id\") (id: String) -> Effect[HttpResult[String]] by Visitor given Items {
\t\tlet row <- Items.first(\"SELECT id, label FROM items WHERE id = ?1\", [Text(id)])
\t\tmatch row {
\t\t\tOk(Some(text)) => match Json.decode[Item](text) {
\t\t\t\tOk(item) => Ok(item.label)
\t\t\t\tErr(_) => ServerError(\"did not decode: \\(text)\")
\t\t\t}
\t\t\tOk(None) => NotFound
\t\t\tErr(_) => ServerError(\"query failed\")
\t\t}
\t}
}
";

#[test]
fn d1_migrations_apply_to_the_emitted_config_on_workerd() {
    // P4b (#2608): the copied migrations and the stanza's `migrations_dir`
    // are what real wrangler applies: with stdin closed it takes its "yes"
    // fallback rather than asking, and the Worker then reads the table the
    // first migration made and the column the second added.
    let files = [
        (
            "migrations/m-smoke-items/0001_create_items.sql",
            "CREATE TABLE items (id TEXT PRIMARY KEY);\nINSERT INTO items (id) VALUES ('a');\n",
        ),
        (
            "migrations/m-smoke-items/0002_add_label.sql",
            "ALTER TABLE items ADD COLUMN label TEXT NOT NULL DEFAULT 'none';\nINSERT INTO items (id, label) VALUES ('b', 'bee');\n",
        ),
    ];
    let apply = |dir: &std::path::Path| -> Option<String> {
        let out = tools::base_command("npx")
            .args([
                "-y",
                wrangler::SPEC,
                "d1",
                "migrations",
                "apply",
                "D1_ITEMS",
                "--local",
            ])
            .current_dir(dir)
            .stdin(Stdio::null())
            .output()
            .expect("run wrangler");
        (!out.status.success()).then(|| {
            format!(
                "`wrangler d1 migrations apply` failed:\n{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        })
    };
    let Some((failures, log)) = run_smoke_with(
        "m-smoke",
        "m",
        D1_MIGRATIONS_SOURCE,
        &files,
        &apply,
        "/items",
        10,
        &|port| {
            check_steps(
                port,
                &[
                    ("/items", 200, r#""2""#),
                    ("/label/a", 200, r#""none""#),
                    ("/label/b", 200, r#""bee""#),
                ],
            )
        },
    ) else {
        return;
    };
    assert!(
        failures.is_empty(),
        "D1 migrations on workerd disagree with the emitted config:\n{}\n--- wrangler log ---\n{log}",
        failures.join("\n")
    );
}

/// K5a (#2586): an upstream on `127.0.0.1` for the Fetch smoke. `/redirect`
/// answers `302` to `/landed`, `/landed` answers `200 landed`, and `/hang`
/// reads the request and never answers. Each connection gets its own thread,
/// so a hanging one blocks nothing else.
fn upstream() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the upstream");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]);
                let path = head.split_whitespace().nth(1).unwrap_or("/");
                let reply = match path {
                    "/redirect" => "HTTP/1.1 302 Found\r\nlocation: /landed\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string(),
                    "/landed" => "HTTP/1.1 200 OK\r\ncontent-length: 6\r\nconnection: close\r\n\r\nlanded".to_string(),
                    _ => {
                        std::thread::sleep(Duration::from_secs(30));
                        return;
                    }
                };
                let _ = stream.write_all(reply.as_bytes());
            });
        }
    });
    port
}

fn fetch_source(upstream: u16) -> String {
    format!(
        "context f.smoke

consumes bynk {{ Fetch }}

service api from http {{
\ton GET(\"/go/:path/:manual/:wait\") (path: String, manual: String, wait: String) -> Effect[HttpResult[String]] by Visitor given Fetch {{
\t\tlet req = Request {{ method: Get, url: \"http://127.0.0.1:{upstream}/\\(path)\", contentType: None, authorization: None, headers: Map.empty(), body: None }}
\t\tlet redirect = if manual == \"manual\" {{ Manual }} else {{ Follow }}
\t\tlet timeout = if wait == \"short\" {{ 300.milliseconds }} else {{ 5.seconds }}
\t\tlet res <- Fetch.sendWith(req, FetchOptions {{ timeout: Some(timeout), redirect: redirect }})
\t\tmatch res {{
\t\t\tOk(r) => Ok(\"\\(r.status) \\(r.body)\")
\t\t\tErr(Timeout) => Ok(\"timeout\")
\t\t\tErr(_) => Ok(\"other\")
\t\t}}
\t}}

\ton GET(\"/ready\") () -> Effect[HttpResult[String]] by Visitor {{
\t\tOk(\"ready\")
\t}}
}}
"
    )
}

#[test]
fn fetch_send_with_times_out_and_returns_a_manual_redirect_on_workerd() {
    // K5a (#2586): `sendWith`'s two options against workerd's own `fetch`,
    // which is what `AbortSignal.timeout` and `redirect: "manual"` have to
    // agree with: a followed redirect lands, a manual one comes back as the
    // `302`, and an upstream that never answers is `Err(Timeout)`.
    let upstream = upstream();
    let source = fetch_source(upstream);
    let Some((failures, log)) = run_smoke("f-smoke", "f", &source, "/ready", 9, &|port| {
        check_steps(
            port,
            &[
                ("/go/redirect/follow/long", 200, r#""200 landed""#),
                ("/go/redirect/manual/long", 200, r#""302 ""#),
                ("/go/hang/follow/short", 200, r#""timeout""#),
            ],
        )
    }) else {
        return;
    };
    assert!(
        failures.is_empty(),
        "Fetch.sendWith on workerd disagrees with the contract its fake models:\n{}\n--- wrangler log ---\n{log}",
        failures.join("\n")
    );
}

/// K5b (#2604): every byte value 0–255, base64 — 0x80 and up alone are not
/// UTF-8, so a `String` body would have lost them.
const ALL_BYTES_BASE64: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0+P0BBQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWltcXV5fYGFiY2RlZmdoaWprbG1ub3BxcnN0dXZ3eHl6e3x9fn+AgYKDhIWGh4iJiouMjY6PkJGSk5SVlpeYmZqbnJ2en6ChoqOkpaanqKmqq6ytrq+wsbKztLW2t7i5uru8vb6/wMHCw8TFxsfIycrLzM3Oz9DR0tPU1dbX2Nna29zd3t/g4eLj5OXm5+jp6uvs7e7v8PHy8/T19vf4+fr7/P3+/w==";

/// K5b (#2604): an upstream on `127.0.0.1` for the byte-body smoke. `/bin`
/// answers the bytes 0–255; `/echo` answers the request body it was sent,
/// with the request's Content-Type as `x-seen-type`.
fn bytes_upstream() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the upstream");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut stream = stream;
                let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                // The head, then as much body as its content-length says.
                let mut got = Vec::new();
                let mut buf = [0u8; 4096];
                let split = loop {
                    if let Some(i) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => got.extend_from_slice(&buf[..n]),
                    }
                };
                let head = String::from_utf8_lossy(&got[..split]).to_string();
                let header = |name: &str| {
                    head.lines().find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
                    })
                };
                let length: usize = header("content-length")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                while got.len() < split + length {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => got.extend_from_slice(&buf[..n]),
                    }
                }
                let path = head.split_whitespace().nth(1).unwrap_or("/");
                let (extra, body): (String, Vec<u8>) = match path {
                    "/bin" => (String::new(), (0..=255u8).collect()),
                    _ => (
                        format!(
                            "x-seen-type: {}\r\n",
                            header("content-type").unwrap_or_else(|| "none".into())
                        ),
                        got[split..].to_vec(),
                    ),
                };
                let mut reply = format!(
                    "HTTP/1.1 200 OK\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                reply.extend_from_slice(&body);
                let _ = stream.write_all(&reply);
            });
        }
    });
    port
}

/// K5c (#2627): an upstream on `127.0.0.1` for the streamed-body smoke.
/// `/chunks` answers the bytes 0–255 chunked, in four parts 150 ms apart, so
/// the body outlasts a 300 ms head timeout; `/text` answers `alpha beta
/// gamma` chunked; `/echo` answers the request body it was sent — read chunked
/// or by content-length — with how it was framed as `x-seen-te` and
/// `x-seen-length`.
fn streams_upstream() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the upstream");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut stream = stream;
                let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                let mut got = Vec::new();
                let fill = |got: &mut Vec<u8>, stream: &mut std::net::TcpStream| -> bool {
                    let mut buf = [0u8; 4096];
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => false,
                        Ok(n) => {
                            got.extend_from_slice(&buf[..n]);
                            true
                        }
                    }
                };
                let split = loop {
                    if let Some(i) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                    if !fill(&mut got, &mut stream) {
                        return;
                    }
                };
                let head = String::from_utf8_lossy(&got[..split]).to_string();
                let header = |name: &str| {
                    head.lines().find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
                    })
                };
                let te = header("transfer-encoding");
                let length = header("content-length");
                let mut body = Vec::new();
                if te
                    .as_deref()
                    .is_some_and(|t| t.eq_ignore_ascii_case("chunked"))
                {
                    // `<hex size>\r\n<data>\r\n`, until a size of zero.
                    let mut at = split;
                    loop {
                        let line_end = loop {
                            if let Some(i) = got[at..].windows(2).position(|w| w == b"\r\n") {
                                break at + i;
                            }
                            if !fill(&mut got, &mut stream) {
                                return;
                            }
                        };
                        let size_text = String::from_utf8_lossy(&got[at..line_end]).to_string();
                        let size =
                            usize::from_str_radix(size_text.split(';').next().unwrap().trim(), 16)
                                .unwrap_or(0);
                        if size == 0 {
                            break;
                        }
                        while got.len() < line_end + 2 + size + 2 {
                            if !fill(&mut got, &mut stream) {
                                return;
                            }
                        }
                        body.extend_from_slice(&got[line_end + 2..line_end + 2 + size]);
                        at = line_end + 2 + size + 2;
                    }
                } else {
                    let n: usize = length.as_deref().and_then(|v| v.parse().ok()).unwrap_or(0);
                    while got.len() < split + n {
                        if !fill(&mut got, &mut stream) {
                            break;
                        }
                    }
                    body.extend_from_slice(&got[split..(split + n).min(got.len())]);
                }
                let path = head.split_whitespace().nth(1).unwrap_or("/");
                let chunked = |stream: &mut std::net::TcpStream, parts: Vec<Vec<u8>>, gap: u64| {
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
                    );
                    for part in parts {
                        let mut chunk = format!("{:x}\r\n", part.len()).into_bytes();
                        chunk.extend_from_slice(&part);
                        chunk.extend_from_slice(b"\r\n");
                        let _ = stream.write_all(&chunk);
                        let _ = stream.flush();
                        std::thread::sleep(Duration::from_millis(gap));
                    }
                    let _ = stream.write_all(b"0\r\n\r\n");
                };
                match path {
                    "/chunks" => {
                        let all: Vec<u8> = (0..=255u8).collect();
                        chunked(
                            &mut stream,
                            all.chunks(64).map(<[u8]>::to_vec).collect(),
                            150,
                        );
                    }
                    "/text" => chunked(
                        &mut stream,
                        vec![b"alpha ".to_vec(), b"beta ".to_vec(), b"gamma".to_vec()],
                        50,
                    ),
                    _ => {
                        let mut reply = format!(
                            "HTTP/1.1 200 OK\r\nx-seen-te: {}\r\nx-seen-length: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                            te.unwrap_or_else(|| "none".into()),
                            length.unwrap_or_else(|| "none".into()),
                            body.len()
                        )
                        .into_bytes();
                        reply.extend_from_slice(&body);
                        let _ = stream.write_all(&reply);
                    }
                }
            });
        }
    });
    port
}

/// K5c (#2627): the payload of a chunked HTTP/1.1 body, as [`get`] returns it
/// undecoded.
fn dechunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size, after)) = rest.split_once("\r\n") {
        let Ok(n) = usize::from_str_radix(size.trim(), 16) else {
            break;
        };
        if n == 0 || after.len() < n {
            break;
        }
        out.push_str(&after[..n]);
        rest = after[n..].trim_start_matches("\r\n");
    }
    out
}

fn fetch_bytes_source(upstream: u16, streams: u16) -> String {
    format!(
        "context fb.smoke

consumes bynk {{ Fetch }}

service api from http {{
\ton GET(\"/round\") () -> Effect[HttpResult[String]] by Visitor given Fetch {{
\t\tlet options = FetchOptions {{ timeout: Some(5.seconds), redirect: Follow }}
\t\tlet down = Request {{ method: Get, url: \"http://127.0.0.1:{upstream}/bin\", contentType: None, authorization: None, headers: Map.empty(), body: None }}
\t\tlet got <- Fetch.receiveBytes(down, options)
\t\tmatch got {{
\t\t\tOk(d) => {{
\t\t\t\tlet up = Request {{ method: Post, url: \"http://127.0.0.1:{upstream}/echo\", contentType: Some(\"application/octet-stream\"), authorization: None, headers: Map.empty(), body: None }}
\t\t\t\tlet echoed <- Fetch.sendBytes(up, d.body, options)
\t\t\t\tmatch echoed {{
\t\t\t\t\tOk(e) => Ok(\"\\(d.body.toBase64()) \\(e.body.toBase64()) \\(e.headers.get(\"x-seen-type\").getOrElse(\"\"))\")
\t\t\t\t\tErr(_) => Ok(\"upload failed\")
\t\t\t\t}}
\t\t\t}}
\t\t\tErr(_) => Ok(\"download failed\")
\t\t}}
\t}}

\t// K5c (#2627): a platform stream, piped from one fetch into the next. The
\t// 300 ms timeout bounds the head alone; the body takes about 600 ms.
\ton GET(\"/stream-round\") () -> Effect[HttpResult[String]] by Visitor given Fetch {{
\t\tlet options = FetchOptions {{ timeout: Some(Duration.millis(300)), redirect: Follow }}
\t\tlet down = Request {{ method: Get, url: \"http://127.0.0.1:{streams}/chunks\", contentType: None, authorization: None, headers: Map.empty(), body: None }}
\t\tlet got <- Fetch.receiveStream(down, options)
\t\tmatch got {{
\t\t\tOk(d) => {{
\t\t\t\tlet up = Request {{ method: Post, url: \"http://127.0.0.1:{streams}/echo\", contentType: None, authorization: None, headers: Map.empty(), body: None }}
\t\t\t\tlet echoed <- Fetch.sendStream(up, d.body, FetchOptions {{ timeout: Some(5.seconds), redirect: Follow }})
\t\t\t\tmatch echoed {{
\t\t\t\t\tOk(e) => {{
\t\t\t\t\t\tlet parts <- e.body.collect()
\t\t\t\t\t\tlet all = parts.fold(Bytes.empty(), (acc, b) => acc.concat(b))
\t\t\t\t\t\tOk(\"\\(all.toBase64()) \\(e.headers.get(\"x-seen-te\").getOrElse(\"\")) \\(e.headers.get(\"x-seen-length\").getOrElse(\"\"))\")
\t\t\t\t\t}}
\t\t\t\t\tErr(_) => Ok(\"upload failed\")
\t\t\t\t}}
\t\t\t}}
\t\t\tErr(_) => Ok(\"download failed\")
\t\t}}
\t}}

\ton GET(\"/proxy\") () -> Effect[HttpResult[()]] by Visitor given Fetch {{
\t\tlet down = Request {{ method: Get, url: \"http://127.0.0.1:{streams}/text\", contentType: None, authorization: None, headers: Map.empty(), body: None }}
\t\tlet got <- Fetch.receiveStream(down, FetchOptions {{ timeout: Some(5.seconds), redirect: Follow }})
\t\tmatch got {{
\t\t\tOk(d) => RawStreamed(d.body, \"text/plain\")
\t\t\tErr(_) => BadGateway(\"download failed\")
\t\t}}
\t}}

\ton GET(\"/stream-twice\") () -> Effect[HttpResult[String]] by Visitor given Fetch {{
\t\tlet down = Request {{ method: Get, url: \"http://127.0.0.1:{streams}/text\", contentType: None, authorization: None, headers: Map.empty(), body: None }}
\t\tlet got <- Fetch.receiveStream(down, FetchOptions {{ timeout: Some(5.seconds), redirect: Follow }})
\t\tmatch got {{
\t\t\tOk(d) => {{
\t\t\t\tlet first <- d.body.collect()
\t\t\t\tlet second <- d.body.collect()
\t\t\t\tOk(\"read twice: \\(first.length() + second.length())\")
\t\t\t}}
\t\t\tErr(_) => Ok(\"download failed\")
\t\t}}
\t}}

\ton GET(\"/ready\") () -> Effect[HttpResult[String]] by Visitor {{
\t\tOk(\"ready\")
\t}}
}}
"
    )
}

#[test]
fn fetch_bytes_round_trip_every_byte_on_workerd() {
    // K5b (#2604): `receiveBytes` and `sendBytes` against workerd's own
    // `fetch`: bytes 0–255 come down and go back up unchanged, and the
    // upload's Content-Type is the one the request set.
    //
    // K5c (#2627), in the same Worker (the port bands are all taken):
    // `receiveStream` hands out a platform stream that outlasts its head
    // timeout, `sendStream` pipes it upstream chunked with no content-length,
    // `RawStreamed` re-serves one, and a second read of one faults.
    let upstream = bytes_upstream();
    let streams = streams_upstream();
    let source = fetch_bytes_source(upstream, streams);
    let expected = format!(r#""{ALL_BYTES_BASE64} {ALL_BYTES_BASE64} application/octet-stream""#);
    let streamed = format!(r#""{ALL_BYTES_BASE64} chunked none""#);
    let Some((failures, log)) = run_smoke("fb-smoke", "fb", &source, "/ready", 8, &|port| {
        let mut failures = check_steps(
            port,
            &[
                ("/round", 200, &expected),
                ("/stream-round", 200, &streamed),
                ("/stream-twice", 500, ""),
            ],
        );
        // A `RawStreamed` body has no content-length, so it arrives chunked.
        match get(port, "/proxy") {
            Ok((200, b)) if dechunk(&b) == "alpha beta gamma" => {}
            Ok((s, b)) => {
                failures.push(format!("/proxy: {s} {b:?}, expected 200 alpha beta gamma"))
            }
            Err(e) => failures.push(format!("/proxy: {e}")),
        }
        failures
    }) else {
        return;
    };
    assert!(
        failures.is_empty(),
        "Fetch.receiveBytes/sendBytes/receiveStream/sendStream on workerd disagree with the contract their fake models:\n{}\n--- wrangler log ---\n{log}",
        failures.join("\n")
    );
}
