//! A queue message that cannot be decoded, on a real `workerd` (#3878): it is moved to the
//! dead-letter queue at once, unchanged, without running the handler or consuming a retry, and
//! a consumer with no dead-letter queue drops it visibly instead of retrying it.
//!
//! The producer sends a body of the wrong shape to `undec-jobs`, whose consumer has a
//! dead-letter queue. The body does not decode as a `Job`, so the consumer's entry sends it to
//! `undec-jobs-dead` and acks it, and the handler never runs. `undec-jobs-dead` has a Bynk
//! consumer of the same message type, which cannot decode it either and has no dead-letter
//! queue, so it acks and reports it.
//!
//! Skips loudly without the toolchain; a non-empty `BYNK_REQUIRE_WORKERD`
//! turns the skip into a failure.

use bynk_driver::project::BuildTarget;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Stdio;
use std::time::{Duration, Instant};

#[path = "../require/mod.rs"]
mod require;
#[path = "../wrangler/mod.rs"]
mod wrangler;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";
const WRANGLER: &str = wrangler::SPEC;

const SOURCE: &str = r#"context undectest

consumes bynk { Logger }
consumes bynk.cloudflare { Queue }

capability Jobs = Queue @name("undec-jobs")

type Job = { id: String }

type Other = { name: String }

service api from http {
  on GET("/send/:id") (id: String) -> Effect[HttpResult[String]] by v: Visitor given Jobs {
    let _ <- Jobs.send(Json.encode(Other { name: id }))
    Ok("sent")
  }
}

service worker from queue("undec-jobs") {
  delivery {
    deadLetter: "undec-jobs-dead",
    batchWait: 1.second,
  }

  on message(m: Job) -> Effect[QueueResult] by Producer given Logger {
    let _ <- Logger.info("handled \(m.id)")
    Ack
  }
}

service review from queue("undec-jobs-dead") {
  delivery {
    deadLetter: None,
    batchWait: 1.second,
  }

  on message(m: Job) -> Effect[QueueResult] by Producer given Logger {
    let _ <- Logger.warn("reviewed \(m.id)")
    Ack
  }
}
"#;

fn tool_exists(name: &str) -> bool {
    which::which(name).is_ok()
}

fn skip(reason: &str) -> bool {
    eprintln!("\n!!! DETACH (WORKERD) SKIPPED !!!\n{reason}\n");
    if require::is_required(REQUIRE_ENV) {
        panic!("{REQUIRE_ENV} is set but {reason}");
    }
    true
}

struct WranglerGuard(Vec<wrangler::Wrangler>);

impl Drop for WranglerGuard {
    fn drop(&mut self) {
        wrangler::stop_all(&mut self.0);
    }
}

/// One `GET`, HTTP/1.1, connection-close: the status and the body.
fn get(port: u16, path: &str) -> Result<(u16, String), String> {
    let addr = format!("127.0.0.1:{port}");
    let mut stream = TcpStream::connect_timeout(
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
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("no status line: {:?}", response.lines().next()))?;
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Ok((status, body))
}

#[test]
fn an_undecodable_message_is_dead_lettered_and_never_retried_on_workerd() {
    if !tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    let tmp = std::env::temp_dir().join(format!("bynk-undec-workerd-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("undectest.bynk"), SOURCE).unwrap();
    fs::write(
        proj.join("bynk.toml"),
        "[project]\nname = \"undec-smoke\"\nversion = \"0.1.0\"\n\n[unstable]\nallow = [\"bynk.cloudflare.Queue\"]\n",
    )
    .unwrap();
    let out = match bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(&proj).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the undecodable project to workers:\n{}",
            bynk_driver::render::render_project_errors(&failure.flatten())
        ),
    };
    let out =
        bynk_driver::strip::strip_project_to_js(out).expect("the undecodable project strips to JS");
    bynk_driver::write_output(&out, &tmp).unwrap();
    let dir = tmp.join("workers/undectest");
    assert!(dir.join("index.js").is_file(), "expected {dir:?}");

    let [port, inspector] = wrangler::ports::free_ports();
    let log_path = tmp.join("wrangler.log");
    let spawned = (|| -> std::io::Result<wrangler::Wrangler> {
        let out_log = fs::File::create(&log_path)?;
        let err_log = out_log.try_clone()?;
        wrangler::Wrangler::spawn(
            wrangler::npx()
                .args(["dev"])
                .args([
                    "--port",
                    &port.to_string(),
                    "--inspector-port",
                    &inspector.to_string(),
                ])
                .current_dir(&dir)
                .stdout(Stdio::from(out_log))
                .stderr(Stdio::from(err_log)),
            &tmp.join("registry"),
        )
    })();
    let guard = match spawned {
        Ok(w) => WranglerGuard(vec![w]),
        Err(_) => {
            if skip("could not launch npx for the worker") {
                return;
            }
            unreachable!()
        }
    };
    let log = || fs::read_to_string(&log_path).unwrap_or_default();

    if let Err(why) = wrangler::wait_until_ready(&log_path, port, &|| {
        matches!(get(port, "/send/boot"), Ok((200, _)))
    }) {
        let log = log();
        drop(guard);
        let _ = fs::remove_dir_all(&tmp);
        if skip(&format!(
            "{why} (likely no network to provision {WRANGLER})\n{log}"
        )) {
            return;
        }
        unreachable!()
    }
    // Local queues deliver in batches, after a timeout: poll for both consumers' verdicts.
    let end = Instant::now() + Duration::from_secs(90);
    while Instant::now() < end && !log().contains("queue undec-jobs-dead undecodable") {
        std::thread::sleep(Duration::from_millis(250));
    }
    let log = log();
    assert!(
        log.contains("queue undec-jobs undecodable") && log.contains("dead-lettered"),
        "the consumer did not dead-letter the undecodable message:\n{log}"
    );
    assert!(
        log.contains("queue undec-jobs-dead undecodable") && log.contains("dropped"),
        "the dead-letter consumer did not drop the message it cannot decode:\n{log}"
    );
    assert!(
        !log.contains("handled boot") && !log.contains("reviewed boot"),
        "a handler ran for a body that does not decode:\n{log}"
    );
    assert!(
        !log.contains("deserialise failed"),
        "an undecodable message was left to the retry path:\n{log}"
    );
    drop(guard);
    let _ = fs::remove_dir_all(&tmp);
}
