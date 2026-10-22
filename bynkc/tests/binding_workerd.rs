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
//! P5 (#2323) adds `WorkersR2` the same way: every `R2` operation through
//! compiled Bynk handlers against Miniflare's own R2, including both
//! conditions of `putWith` failing as `None` and `list` paging past R2's 1000
//! objects a page.
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
    if !tools::tool_exists("npx") && skip("`npx` is not on PATH") {
        return None;
    }
    if !tools::tool_exists("node") && skip("`node` is not on PATH") {
        return None;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-binding-workerd-{project}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join(format!("proj/src/{ctx}"))).unwrap();
    fs::write(
        tmp.join("proj/bynk.toml"),
        format!(
            "[project]\nname = \"{project}\"\nversion = \"0.1.0\"\n\n[unstable]\nallow = [\"bynk.cloudflare.R2\"]\n"
        ),
    )
    .unwrap();
    fs::write(tmp.join(format!("proj/src/{ctx}/smoke.bynk")), source).unwrap();
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

    // Its own band, clear of the events smokes' (30_000 + 4·pid, 40_000 + 2·pid,
    // 55_000 + 2·pid, each mod 4_000), so they can all run at once.
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
            let keys: serde_json::Value = serde_json::from_str(body.trim()).map_err(|e| format!("{path}: {e}"))?;
            match (status, keys.as_array()) {
                (200, Some(a)) => Ok(a.iter().filter_map(|k| k.as_str().map(str::to_string)).collect()),
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
