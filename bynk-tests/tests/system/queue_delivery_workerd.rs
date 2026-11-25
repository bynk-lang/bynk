//! A queue consumer's `delivery { }` on a real `workerd` (#3877): a message
//! that is retried to the limit lands on the dead-letter queue, and the Bynk
//! consumer of that queue receives it.
//!
//! The consumer retries every message, with `retries: 1` and a dead-letter queue
//! named in its `delivery { }`. Miniflare reads `max_retries` and
//! `dead_letter_queue` from the generated `wrangler.toml`, so the message is
//! delivered twice (the first attempt and one retry) and then written to the
//! dead-letter queue, where the second consumer logs it.
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

const SOURCE: &str = r#"context dlqtest

consumes bynk { Logger }
consumes bynk.cloudflare { Queue }

capability Jobs = Queue @name("dlq-jobs")

type Job = { id: String }

service api from http {
  on GET("/send/:id") (id: String) -> Effect[HttpResult[String]] by v: Visitor given Jobs {
    let _ <- Jobs.send(Json.encode(Job { id: id }))
    Ok("sent")
  }
}

service worker from queue("dlq-jobs") {
  delivery {
    retries: 1,
    retryDelay: 1.second,
    deadLetter: "dlq-jobs-dead",
    batchWait: 1.second,
  }

  on message(m: Job) -> Effect[QueueResult] by Producer given Logger {
    let _ <- Logger.info("attempt for \(m.id)")
    Retry("never works")
  }
}

service review from queue("dlq-jobs-dead") {
  delivery {
    deadLetter: None,
    batchWait: 1.second,
  }

  on message(m: Job) -> Effect[QueueResult] by Producer given Logger {
    let _ <- Logger.warn("dead-lettered \(m.id)")
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
fn a_message_retried_to_the_limit_reaches_the_dead_letter_consumer_on_workerd() {
    if !tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    let tmp = std::env::temp_dir().join(format!("bynk-dlq-workerd-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("dlqtest.bynk"), SOURCE).unwrap();
    fs::write(
        proj.join("bynk.toml"),
        "[project]\nname = \"dlq-smoke\"\nversion = \"0.1.0\"\n\n[unstable]\nallow = [\"bynk.cloudflare.Queue\"]\n",
    )
    .unwrap();
    let out = match bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(&proj).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the dead-letter project to workers:\n{}",
            bynk_driver::render::render_project_errors(&failure.flatten())
        ),
    };
    let out =
        bynk_driver::strip::strip_project_to_js(out).expect("the dead-letter project strips to JS");
    bynk_driver::write_output(&out, &tmp).unwrap();
    let dir = tmp.join("workers/dlqtest");
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

    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if matches!(get(port, "/send/boot"), Ok((200, _))) {
            break;
        }
        if Instant::now() > deadline {
            let log = log();
            drop(guard);
            let _ = fs::remove_dir_all(&tmp);
            if skip(&format!(
                "the worker did not answer within the boot window (likely no network \
                 to provision {WRANGLER})\n{log}"
            )) {
                return;
            }
            unreachable!()
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    // Local queues deliver in batches, after a timeout, and retry after the delay: poll.
    let end = Instant::now() + Duration::from_secs(90);
    while Instant::now() < end && !log().contains("dead-lettered boot") {
        std::thread::sleep(Duration::from_millis(250));
    }
    let log = log();
    assert!(
        log.contains("dead-lettered boot"),
        "the message never reached the dead-letter consumer:\n{log}"
    );
    let attempts = log.matches("attempt for boot").count();
    assert_eq!(
        attempts, 2,
        "`retries: 1` is the first attempt and one retry:\n{log}"
    );
    drop(guard);
    let _ = fs::remove_dir_all(&tmp);
}
