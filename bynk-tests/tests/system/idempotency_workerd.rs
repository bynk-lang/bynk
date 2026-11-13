//! `Idempotency.claim` against a real `workerd` and its local D1 (#2840).
//!
//! `binding_behaviour.rs` proves the Cloudflare provider's SQL against SQLite.
//! This runs one compiled Worker under `wrangler dev`, whose Miniflare D1 is the
//! platform's own local implementation, with the implicit `D1_IDEMPOTENCY`
//! database and the compiler's own migration applied by `wrangler d1 migrations
//! apply`, as `bynk deploy` applies it. It checks the two things the proposal's
//! "done when" names:
//!
//! 1. concurrent claims of one key give exactly one `Claimed`, and the claim is in
//!    D1, not in the isolate's memory (the row is read back through wrangler);
//! 2. the claim survives a restart of the Worker: after `wrangler dev` is stopped
//!    and started again on the same persisted state, the key is still claimed.
//!
//! The same toolchain skip and `BYNK_REQUIRE_WORKERD` gate as the other workerd
//! smokes; see `events_ordering_workerd.rs`.

#[path = "../require/mod.rs"]
mod require;
#[path = "../tools/mod.rs"]
mod tools;
#[path = "../wrangler/mod.rs"]
mod wrangler;

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";

const SOURCE: &str = "context idem.smoke

consumes bynk { Idempotency, Claim }

service api from http {
\ton GET(\"/claim/:id\") (id: String) -> Effect[HttpResult[String]] by Visitor given Idempotency {
\t\tlet Claim.Claimed <- Idempotency.claim(id, 1.hours) else { Ok(\"duplicate\") }
\t\tOk(\"processed\")
\t}

\ton GET(\"/seen/:id\") (id: String) -> Effect[HttpResult[String]] by Visitor given Idempotency {
\t\tlet seen <- Idempotency.dedup[String](id)
\t\tmatch seen {
\t\t\tSome(v) => Ok(v),
\t\t\tNone => {
\t\t\t\tlet _ <- Idempotency.remember[String](id, \"remembered\", 1.hours)
\t\t\t\tOk(\"fresh\")
\t\t\t},
\t\t}
\t}
}
";

fn skip(reason: &str) -> bool {
    if require::is_required(REQUIRE_ENV) {
        panic!("{REQUIRE_ENV} is set but {reason}");
    }
    eprintln!("\n!!! IDEMPOTENCY (WORKERD) SKIPPED !!!\n{reason}\n");
    true
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .expect("a free local port")
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
        .set_read_timeout(Some(Duration::from_secs(30)))
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
        .map(|(_, b)| b.trim().to_string())
        .unwrap_or_default();
    Ok((status, body))
}

/// `wrangler <args>` from `dir`, with stdin closed; its combined output, or `Err`
/// when it failed.
fn wrangler_run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = tools::base_command("npx")
        .args(["-y", wrangler::SPEC])
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(text)
    }
}

/// Start `wrangler dev` in `dir` and wait until it serves; `None` (after a skip)
/// when it never does.
fn serve(dir: &Path, tmp: &Path, log: &str) -> Option<(wrangler::Wrangler, u16)> {
    let port = free_port();
    let inspector = free_port();
    let log_path = tmp.join(log);
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
                &inspector.to_string(),
            ])
            .current_dir(dir)
            .stdout(Stdio::from(out_log))
            .stderr(Stdio::from(err_log)),
        &tmp.join("registry"),
    );
    let Ok(server) = spawned else {
        skip("could not launch npx");
        return None;
    };
    let deadline = Instant::now() + Duration::from_secs(180);
    while get(port, "/claim/boot-probe").is_err() {
        if Instant::now() > deadline {
            let said = fs::read_to_string(&log_path).unwrap_or_default();
            drop(server);
            skip(&format!(
                "wrangler dev did not serve within the boot window (likely no network \
                 to provision {})\n--- wrangler log ---\n{said}",
                wrangler::SPEC
            ));
            return None;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Some((server, port))
}

/// The `n` of `wrangler d1 execute --json`'s first result row (the JSON array
/// starts at its first line beginning `[`, after any banner).
fn claim_rows(output: &str) -> Option<i64> {
    // The first JSON value from any `[`: wrangler may print a banner before it and
    // a warning after it.
    let parsed = output.match_indices('[').find_map(|(i, _)| {
        serde_json::Deserializer::from_str(&output[i..])
            .into_iter::<serde_json::Value>()
            .next()?
            .ok()
    })?;
    parsed.get(0)?.get("results")?.get(0)?.get("n")?.as_i64()
}

#[test]
fn claims_are_atomic_in_d1_and_survive_a_restart_on_workerd() {
    if !tools::tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    if !tools::tool_exists("node") && skip("`node` is not on PATH") {
        return;
    }
    let tmp = std::env::temp_dir().join(format!("bynk-idempotency-workerd-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("proj/src/idem")).unwrap();
    fs::write(
        tmp.join("proj/bynk.toml"),
        "[project]\nname = \"idem-smoke\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(tmp.join("proj/src/idem/smoke.bynk"), SOURCE).unwrap();
    let compiled = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(["compile", "--target", "workers", "--emit", "js", "-o"])
        .arg(tmp.join("out"))
        .arg(tmp.join("proj"))
        .output()
        .expect("run bynk");
    assert!(
        compiled.status.success(),
        "the idempotency smoke does not compile:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let dir = tmp.join("out/workers/idem-smoke");
    let config = fs::read_to_string(dir.join("wrangler.toml")).unwrap();
    assert!(
        config.contains("D1_IDEMPOTENCY") && config.contains("migrations_dir"),
        "a Worker that calls `claim` declares the implicit database:\n{config}"
    );

    // The compiler's migration, applied as `bynk deploy` applies one (here to the
    // local database).
    if let Err(said) = wrangler_run(
        &dir,
        &["d1", "migrations", "apply", "D1_IDEMPOTENCY", "--local"],
    ) {
        if !skip(&format!(
            "`wrangler d1 migrations apply` failed (no network?):\n{said}"
        )) {
            panic!("`wrangler d1 migrations apply` failed:\n{said}");
        }
        let _ = fs::remove_dir_all(&tmp);
        return;
    }

    let mut failures = Vec::new();
    {
        let Some((server, port)) = serve(&dir, &tmp, "first.log") else {
            let _ = fs::remove_dir_all(&tmp);
            return;
        };
        // 1. Twelve concurrent claims of one key: exactly one wins.
        let answers: Vec<String> = (0..12)
            .map(|_| std::thread::spawn(move || get(port, "/claim/shared")))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| match t.join().unwrap() {
                Ok((200, body)) => body,
                other => format!("unexpected: {other:?}"),
            })
            .collect();
        let won = answers
            .iter()
            .filter(|a| a.as_str() == "\"processed\"")
            .count();
        let lost = answers
            .iter()
            .filter(|a| a.as_str() == "\"duplicate\"")
            .count();
        if won != 1 || lost != 11 {
            failures.push(format!(
                "concurrent claims: {won} processed and {lost} duplicate, expected 1 and 11: {answers:?}"
            ));
        }
        // The remembered value is stored through the same database (`dedup`, then
        // `remember`, then `dedup` again sees it).
        for (path, expected) in [
            ("/seen/durable", "\"fresh\""),
            ("/seen/durable", "\"remembered\""),
        ] {
            match get(port, path) {
                Ok((200, body)) if body == expected => {}
                other => failures.push(format!("{path}: {other:?}, expected {expected}")),
            }
        }
        // 2. The claim is exactly one row in D1, not isolate memory. (The readiness probe
        //    claims `boot-probe`, so count `shared`'s row by its key.)
        match wrangler_run(
            &dir,
            &[
                "d1",
                "execute",
                "D1_IDEMPOTENCY",
                "--local",
                "--json",
                "--command",
                "SELECT count(*) AS n FROM idempotency_claims WHERE key LIKE '%::shared'",
            ],
        ) {
            Ok(text) => match claim_rows(&text) {
                Some(1) => {}
                other => failures.push(format!(
                    "expected exactly one claim row for `shared` in D1, got {other:?} from:\n{text}"
                )),
            },
            Err(text) => failures.push(format!("could not read the claims table:\n{text}")),
        }
        drop(server);
    }
    // 3. After a restart of the Worker on the same local state, the key is still
    //    claimed, and a new key can be.
    {
        let Some((server, port)) = serve(&dir, &tmp, "second.log") else {
            let _ = fs::remove_dir_all(&tmp);
            return;
        };
        for (path, expected) in [
            ("/claim/shared", "\"duplicate\""),
            ("/claim/other", "\"processed\""),
            // A remembered value survives the restart too, decoded as it was written.
            ("/seen/durable", "\"remembered\""),
        ] {
            match get(port, path) {
                Ok((200, body)) if body == expected => {}
                other => failures.push(format!(
                    "after a restart, {path}: {other:?}, expected {expected}"
                )),
            }
        }
        drop(server);
    }
    let first = fs::read_to_string(tmp.join("first.log")).unwrap_or_default();
    let second = fs::read_to_string(tmp.join("second.log")).unwrap_or_default();
    let _ = fs::remove_dir_all(&tmp);
    assert!(
        failures.is_empty(),
        "Idempotency.claim on workerd and local D1:\n{}\n--- first wrangler log ---\n{first}\n--- second wrangler log ---\n{second}",
        failures.join("\n")
    );
}
