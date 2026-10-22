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

#[test]
fn workers_kv_round_trips_on_workerd() {
    if !tools::tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    if !tools::tool_exists("node") && skip("`node` is not on PATH") {
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-binding-workerd-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("proj/src/kv")).unwrap();
    fs::write(
        tmp.join("proj/bynk.toml"),
        "[project]\nname = \"kv-smoke\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(tmp.join("proj/src/kv/smoke.bynk"), SOURCE).unwrap();
    let compiled = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["compile", "--target", "workers", "--emit", "js", "-o"])
        .arg(tmp.join("out"))
        .arg(tmp.join("proj"))
        .output()
        .expect("run bynkc");
    assert!(
        compiled.status.success(),
        "the KV smoke does not compile:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let dir = tmp.join("out/workers/kv-smoke");
    assert!(
        dir.join("index.js").is_file() && dir.join("wrangler.toml").is_file(),
        "expected out/workers/kv-smoke — context dasherisation changed?"
    );

    // Its own band, clear of the events smokes' (30_000 + 4·pid, 40_000 + 2·pid,
    // 55_000 + 2·pid, each mod 4_000), so they can all run at once.
    let port = 20_000 + 2 * (std::process::id() % 4_000) as u16;
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
        return;
    };

    let deadline = Instant::now() + Duration::from_secs(180);
    while get(port, "/get/boot").is_err() {
        if Instant::now() > deadline {
            let log = read_log(&log_path);
            drop(server);
            skip(&format!(
                "wrangler dev did not serve within the boot window (likely no network \
                 to provision {})\n--- wrangler log ---\n{log}",
                wrangler::SPEC
            ));
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }

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
    let mut failures = Vec::new();
    for (path, status, body) in steps {
        match get(port, path) {
            Ok((s, b)) if s == *status && (body.is_empty() || b.trim() == *body) => {}
            Ok((s, b)) => failures.push(format!("{path}: {s} {b:?}, expected {status} {body}")),
            Err(e) => failures.push(format!("{path}: {e}")),
        }
    }
    let log = read_log(&log_path);
    drop(server);
    assert!(
        failures.is_empty(),
        "WorkersKv on workerd disagrees with the contract its fake models:\n{}\n--- wrangler log ---\n{log}",
        failures.join("\n")
    );
    let _ = fs::remove_dir_all(&tmp);
}
