//! #3202 — an agent instance runs one handler at a time, end to end on a real
//! `workerd`, through the one path the in-memory tests cannot prove: the events
//! fan-out.
//!
//! A publisher agent's `place` handler loads its state, makes a Durable Object
//! subrequest (a call to another agent, which does not close the input gate),
//! commits, and hands its event to the fan-out. The fan-out Durable Object
//! delivers it over a Service Binding to a subscriber in another Worker, whose
//! handler calls the publisher back (through the publisher's `poke` service) and
//! bumps the same agent instance. Alongside it, plain `bump` handlers run
//! concurrently on the same instance.
//!
//! Every handler adds one to the same count, so the final count is the number of
//! handlers that ran, and a lost update (two handlers loading the same record,
//! the later commit overwriting the earlier) shows as a smaller count. Two
//! separate failures are caught:
//!
//! - no serialisation: concurrent handlers overwrite each other, and the count
//!   settles below the expected total;
//! - serialisation that keeps the instance's place through the fan-out: the
//!   subscriber's call back waits behind the handler that is waiting for it, and
//!   the delivery fails after the queue limit (30 s), so the count never reaches
//!   the total within this test's deadline.
//!
//! Same toolchain-skip and `BYNK_REQUIRE_WORKERD` gate as the other workerd
//! tests; teardown goes through [`wrangler::Wrangler`].

use bynk_driver::project::BuildTarget;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[path = "../require/mod.rs"]
mod require;
#[path = "../wrangler/mod.rs"]
mod wrangler;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";
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
///
/// This used to end "a third workerd-gated test must copy this, not `is_ok()`",
/// which is the instruction that produced the bug twice. There is nothing left
/// to copy: the decision has one implementation now.
fn required() -> bool {
    require::is_required(REQUIRE_ENV)
}

fn skip(reason: &str) -> bool {
    eprintln!("\n!!! AGENT SERIALISATION (WORKERD) SKIPPED !!!\n{reason}\n");
    if required() {
        panic!("{REQUIRE_ENV} is set but {reason}");
    }
    true
}

/// Both `wrangler dev`s, stopped together on every exit path, including a
/// panicking assertion. [`wrangler::stop_all`] signals both before waiting on
/// either, so the two shutdowns overlap.
struct Wranglers(Vec<wrangler::Wrangler>);

impl Drop for Wranglers {
    fn drop(&mut self) {
        wrangler::stop_all(&mut self.0);
    }
}

/// A dependency-free HTTP client (the test crate has no HTTP dep): one
/// request, HTTP/1.1, connection-close. Extends `workers_runtime_smoke.rs`'s
/// GET-only helper with a method, a path, and an optional JSON body, and
/// returns only the response body (split on the blank line) rather than the
/// raw response including headers.
fn http(method: &str, port: u16, path: &str, body: Option<&str>) -> Result<String, String> {
    let addr = format!("127.0.0.1:{port}");
    let mut stream = std::net::TcpStream::connect_timeout(
        &addr.parse().map_err(|e| format!("{e}"))?,
        Duration::from_secs(2),
    )
    .map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .map_err(|e| e.to_string())?;
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(b) = body {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", b.len()));
        request.push_str("\r\n");
        request.push_str(b);
    } else {
        request.push_str("\r\n");
    }
    stream
        .write_all(request.as_bytes())
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
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Ok(body)
}

/// Read back a wrangler child's captured output for a panic/skip message.
/// Piped stdout/stderr that nobody drains fills its OS buffer and blocks the
/// child's next write — across four phases of many requests each, that would
/// eventually stall `wrangler dev` itself, not just this test. Both children
/// log to a file instead, so nothing ever blocks on the pipe, and the file is
/// read back (not streamed) only when there is something to report.
fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| format!("<could not read {path:?}: {e}>"))
}

const SOURCE_PUBLISHER: &str = r#"context ser.publisher

exports transparent { Placed }
consumes bynk { Events }

event Placed = {
  tag: String,
}

agent Probe {
  key id: String
  store seen: Cell[Int] = 0

  on call ping() -> Effect[Int] { Effect.pure(1) }
}

agent Tally {
  key id: String
  store count: Cell[Int] = 0

  on call place(tag: String) -> Effect[()] given Events {
    let p <- Probe("p").ping()
    let _ <- count.update((n) => n + p)
    let _ <- Events.emit[Placed](Placed { tag: tag })
    Effect.pure(())
  }

  on call bump() -> Effect[()] {
    let p <- Probe("p").ping()
    let _ <- count.update((n) => n + p)
    Effect.pure(())
  }

  on call read() -> Effect[Int] {
    Effect.pure(count)
  }
}

service poke {
  on call() -> Effect[()] {
    let _ <- Tally("t").bump()
    Effect.pure(())
  }
}

service api from http {
  on GET("/place/:tag") (tag: String) -> Effect[HttpResult[String]] by v: Visitor {
    let _ <- Tally("t").place(tag)
    Ok("ok")
  }

  on GET("/bump") () -> Effect[HttpResult[String]] by v: Visitor {
    let _ <- Tally("t").bump()
    Ok("ok")
  }

  on GET("/count") () -> Effect[HttpResult[String]] by v: Visitor {
    let n <- Tally("t").read()
    Ok("\(n)")
  }
}
"#;

const SOURCE_SUBSCRIBER: &str = r#"context ser.subscriber

consumes ser.publisher as P

service OnPlaced from Events(Placed) {
  on event(e: Placed) -> Effect[()] {
    let _ <- P.poke()
    Effect.pure(())
  }
}

service ready from http {
  on GET("/ready") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }
}
"#;

/// Poll the publisher's `/count` until `done` accepts it, or the deadline
/// passes: `Err` carries the last observed count.
fn poll_count(
    port: u16,
    deadline: Duration,
    done: impl Fn(i64) -> bool,
) -> Result<i64, Option<i64>> {
    let start = Instant::now();
    let mut last = None;
    loop {
        if let Ok(body) = http("GET", port, "/count", None)
            && let Ok(n) = body.trim().trim_matches('"').parse::<i64>()
        {
            last = Some(n);
            if done(n) {
                return Ok(n);
            }
        }
        if start.elapsed() > deadline {
            return Err(last);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[test]
fn a_subscriber_mutating_the_publisher_during_the_fanout_loses_no_write_on_workerd() {
    if !tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    if !tool_exists("node") && skip("`node` is not on PATH") {
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-agent-serialise-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj/ser");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("publisher.bynk"), SOURCE_PUBLISHER).unwrap();
    fs::write(proj.join("subscriber.bynk"), SOURCE_SUBSCRIBER).unwrap();

    let out = match bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(tmp.join("proj")).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the serialisation project to workers:\n{}",
            bynk_driver::render::render_project_errors(&failure.flatten())
        ),
    };
    let out = bynk_driver::strip::strip_project_to_js(out).expect("the project strips to JS");
    bynk_driver::write_output(&out, &tmp).unwrap();

    let pub_dir = tmp.join("workers/ser-publisher");
    let sub_dir = tmp.join("workers/ser-subscriber");
    assert!(
        pub_dir.join("index.js").is_file() && pub_dir.join("wrangler.toml").is_file(),
        "expected workers/ser-publisher — context dasherisation changed?"
    );
    assert!(
        sub_dir.join("index.js").is_file() && sub_dir.join("wrangler.toml").is_file(),
        "expected workers/ser-subscriber — context dasherisation changed?"
    );

    // Its own port band, clear of the other workerd tests'. Two ports for the
    // Workers and two for their V8 inspectors, as `events_ordering_workerd.rs`
    // explains.
    let base = 6_000 + 4 * (std::process::id() % 1_000) as u16;
    let pub_port = base;
    let sub_port = base + 1;
    let pub_inspector_port = base + 2;
    let sub_inspector_port = base + 3;

    let pub_log = tmp.join("pub-wrangler.log");
    let sub_log = tmp.join("sub-wrangler.log");
    let spawn = |dir: &Path,
                 port: u16,
                 inspector_port: u16,
                 log_path: &Path|
     -> std::io::Result<wrangler::Wrangler> {
        let out_log = fs::File::create(log_path)?;
        let err_log = out_log.try_clone()?;
        wrangler::Wrangler::spawn(
            base_command("npx")
                .args(["-y", WRANGLER, "dev"])
                // The publisher signs each event delivery to the subscriber, and
                // the subscriber signs its call back to the publisher: a key for
                // each edge, on both ends.
                .args(wrangler::local_edge_vars(&[
                    ("ser.publisher", "ser.subscriber"),
                    ("ser.subscriber", "ser.publisher"),
                ]))
                .args([
                    "--port",
                    &port.to_string(),
                    "--inspector-port",
                    &inspector_port.to_string(),
                ])
                .current_dir(dir)
                .stdout(Stdio::from(out_log))
                .stderr(Stdio::from(err_log)),
            &tmp.join("registry"),
        )
    };

    let pub_child = spawn(&pub_dir, pub_port, pub_inspector_port, &pub_log);
    let sub_child = spawn(&sub_dir, sub_port, sub_inspector_port, &sub_log);
    let (pub_child, sub_child) = match (pub_child, sub_child) {
        (Ok(a), Ok(b)) => (a, b),
        (a, b) => {
            let mut stray = Vec::new();
            if let Ok(c) = a {
                stray.push(c);
            }
            if let Ok(c) = b {
                stray.push(c);
            }
            drop(Wranglers(stray));
            if skip("could not launch npx for one or both workers") {
                return;
            }
            unreachable!()
        }
    };
    let wranglers = Wranglers(vec![pub_child, sub_child]);

    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if Instant::now() > deadline {
            let logs = format!(
                "--- publisher log ---\n{}\n--- subscriber log ---\n{}",
                read_log(&pub_log),
                read_log(&sub_log)
            );
            drop(wranglers);
            let _ = fs::remove_dir_all(&tmp);
            if skip(&format!(
                "wrangler dev did not serve within the boot window (likely no \
                 network to provision {WRANGLER})\n{logs}"
            )) {
                return;
            }
            unreachable!()
        }
        let pub_up = http("GET", pub_port, "/count", None).is_ok();
        let sub_up = http("GET", sub_port, "/ready", None).is_ok();
        if pub_up && sub_up {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    let logs = || {
        format!(
            "--- publisher log ---\n{}\n--- subscriber log ---\n{}",
            read_log(&pub_log),
            read_log(&sub_log)
        )
    };

    // Both ports answering is not the Service Binding being connected, and an
    // event emitted before it is not retried. So place events until one is
    // delivered, then wait for the count to stop moving.
    //
    // A delivered probe shows as the count growing by two *across that probe*:
    // the placing handler, and the subscriber's callback. It must not be read
    // off the count's parity. A probe emitted before the binding connected is
    // lost (the fan-out logs `delivery failed: 503` and moves on) after its
    // placing handler has already counted, so the count is one higher for good,
    // every later pair keeps it odd, and a test for an even count never passes
    // however many probes arrive. That is how two Windows runs failed (#3308).
    let start = Instant::now();
    loop {
        if let Ok(base) = poll_count(pub_port, Duration::from_secs(10), |_| true) {
            let _ = http("GET", pub_port, "/place/probe", None);
            if poll_count(pub_port, Duration::from_secs(5), |n| n >= base + 2).is_ok() {
                break;
            }
        }
        assert!(
            start.elapsed() <= Duration::from_secs(120),
            "the subscriber's callback never reached the publisher within 120s\n{}",
            logs()
        );
    }
    let mut stable = poll_count(pub_port, Duration::from_secs(10), |_| true).expect("a count");
    let quiet = Instant::now();
    while quiet.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(250));
        let now = poll_count(pub_port, Duration::from_secs(10), |_| true).expect("a count");
        if now != stable {
            stable = now;
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    let before = stable;

    // One placing handler, whose event brings one callback, and four plain
    // bumps, all at once on the one instance: six handlers in all.
    let results: Vec<Result<String, String>> = std::thread::scope(|scope| {
        let mut handles = vec![scope.spawn(|| http("GET", pub_port, "/place/real", None))];
        for _ in 0..4 {
            handles.push(scope.spawn(|| http("GET", pub_port, "/bump", None)));
        }
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for r in &results {
        assert!(r.is_ok(), "a concurrent request failed: {r:?}\n{}", logs());
    }

    let expected = before + 6;
    let got = poll_count(pub_port, Duration::from_secs(60), |n| n >= expected);
    assert_eq!(
        got.ok(),
        Some(expected),
        "the six handlers must all count (from {before}); last observed {got:?}. A count \
         below {expected} is a lost update; one that never arrives is a callback queued \
         behind the handler waiting for it\n{}",
        logs()
    );
    // And nothing more arrives after: exactly six, no duplicate delivery.
    std::thread::sleep(Duration::from_secs(3));
    let after = poll_count(pub_port, Duration::from_secs(10), |_| true).expect("a count");
    assert_eq!(
        after, expected,
        "the count moved after the six handlers finished"
    );

    drop(wranglers);
    let _ = fs::remove_dir_all(&tmp);
}
