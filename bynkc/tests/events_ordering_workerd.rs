//! Events track (spine #936), `design/tracks/events.md` §3.4 — empirical
//! per-publisher FIFO ordering on a real Cloudflare Durable Object.
//!
//! Slice 0 (#939, PR #951) shipped the fan-out DO but explicitly left §3.4
//! open: the ordering contract in `design/bynk-design-notes.md` §7 ("events
//! from the same publishing agent are delivered to each subscriber in
//! emission order") was asserted, never measured against real `workerd`
//! (`events_workers_wiring.rs` proves only `tsc --strict` + generated-file
//! content; `events_behaviour.rs` proves Bundle-target delivery, which has no
//! Durable Object in its path at all).
//!
//! Tracing the delivery chain shows the guarantee is narrower than asserted.
//! The fan-out DO (`bynk-emit/src/emitter/events_fanout.rs`) is a **stateless
//! router**: its constructor discards `DurableObjectState`, it performs no
//! storage operation, and there is no `blockConcurrencyWhile` anywhere in it
//! or in the runtime. Cloudflare's DO input gate closes only around storage
//! operations, so the DO's delivery loop *yields* at every `await
//! deliverEvent(...)`. Within one batch this is harmless — one handler's
//! flush is one array, one `fetch`, one sequential loop, and yielding mid-loop
//! cannot reorder that loop's own resumptions. Across batches it is not: the
//! publisher flushes *after* `commitState` (`bynk-emit/src/emitter/emit.rs`),
//! so the agent's own storage gate has already reopened, and two overlapping
//! invocations of the same agent can have both flushes in flight
//! simultaneously with nothing to serialise them.
//!
//! This file measures exactly that, in three cases:
//!
//! - (a) several emits inside **one** handler body — expected to hold, and
//!   asserted as a hard ordering check.
//! - (b) several **successive, sequentially-awaited** invocations of one
//!   agent — expected to hold (no concurrency to interleave), also a hard
//!   ordering check.
//! - (c) two **overlapping** invocations of one agent, issued from separate
//!   threads. Whether an interleave manifests is a scheduling race, so this
//!   is not an order assertion in either direction — asserting "must
//!   interleave" or "must stay ordered" would both be flaky. What CI asserts
//!   instead: every emitted event is still delivered exactly once
//!   (completeness — no loss, no duplication), and each burst's own relative
//!   order survives regardless of interleaving (a single burst is still one
//!   sequential loop; only the two loops' *relative* interleaving is
//!   unconstrained). The interleave observation itself belongs in the
//!   accompanying ADR (`design/pending/events-per-publisher-fifo-scope.md`)
//!   as a recorded experiment, not a CI gate — see that file for the
//!   observed result.
//!
//! What this proves, precisely: order **as observed at the subscriber's
//! recording point**. The recorder (`Trace`) is a single agent instance (one
//! Durable Object, single-threaded) and its `on event` handler awaits the
//! append before returning, so the recorder itself cannot introduce a
//! permutation of what it is asked to note.
//!
//! Like the other workerd-gated tests, this skips loudly when the toolchain
//! is unavailable; a *non-empty* `BYNK_REQUIRE_WORKERD` (Linux CI sets `1`,
//! `.github/workflows/ci.yml`) turns the skip into a failure. Empty counts as
//! unset — see `required`.
//!
//! Teardown goes through [`wrangler::Wrangler`], which stops each `wrangler
//! dev` together with the `workerd`s under it (#1686). Both are stopped when
//! the guards drop, so a panicking assertion still tears them down. Both
//! are signalled before either is waited on.

use bynkc::BuildTarget;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

mod require;
mod wrangler;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";
/// Shared with the other smokes via [`wrangler`], which also documents why the
/// two workers this test starts must not be the ones to install it.
const WRANGLER: &str = wrangler::SPEC;

const SOURCE_PUBLISHER: &str = r#"context fifo.publisher

exports transparent { Tick }
consumes bynk { Events }

event Tick = {
  tag: String,
}

agent Emitter {
  key id: String
  store count: Cell[Int] = 0

  on call burst(p: String) -> Effect[()] given Events {
    let _ <- count.update((n) => n + 1)
    let _ <- Events.emit[Tick](Tick { tag: "\(p)0" })
    let _ <- Events.emit[Tick](Tick { tag: "\(p)1" })
    let _ <- Events.emit[Tick](Tick { tag: "\(p)2" })
    let _ <- Events.emit[Tick](Tick { tag: "\(p)3" })
    let _ <- Events.emit[Tick](Tick { tag: "\(p)4" })
    let _ <- Events.emit[Tick](Tick { tag: "\(p)5" })
    let _ <- Events.emit[Tick](Tick { tag: "\(p)6" })
    let _ <- Events.emit[Tick](Tick { tag: "\(p)7" })
    Effect.pure(())
  }

  on call one(tag: String) -> Effect[()] given Events {
    let _ <- count.update((n) => n + 1)
    let _ <- Events.emit[Tick](Tick { tag: tag })
    Effect.pure(())
  }
}

service trigger from http {
  // (a) eight emissions from ONE handler body: one batch, one flush.
  on GET("/burst/:p") (p: String) -> Effect[HttpResult[String]] by v: Visitor {
    let e = Emitter("solo")
    let _ <- e.burst(p)
    Ok("ok")
  }

  // (b) eight SUCCESSIVE, sequentially-awaited invocations of the same
  // agent: eight batches, each flushed to completion before the next begins.
  on GET("/successive/:p") (p: String) -> Effect[HttpResult[String]] by v: Visitor {
    let e = Emitter("solo")
    let _ <- e.one("\(p)0")
    let _ <- e.one("\(p)1")
    let _ <- e.one("\(p)2")
    let _ <- e.one("\(p)3")
    let _ <- e.one("\(p)4")
    let _ <- e.one("\(p)5")
    let _ <- e.one("\(p)6")
    let _ <- e.one("\(p)7")
    Ok("ok")
  }
}
"#;

const SOURCE_SUBSCRIBER: &str = r#"context fifo.subscriber

consumes fifo.publisher

agent Trace {
  key id: String
  store seen: Cell[String] = ""

  on call note(tag: String) -> Effect[()] {
    let _ <- seen.update((s) => s.concat(tag))
    Effect.pure(())
  }

  on call read() -> Effect[String] {
    Effect.pure(seen)
  }
}

service OnTick from Events(Tick) {
  on event(e: Tick) -> Effect[()] {
    let _ <- Trace("main").note(e.tag)
    Effect.pure(())
  }
}

service traceApi from http {
  on GET("/trace") () -> Effect[HttpResult[String]] by v: Visitor {
    let s <- Trace("main").read()
    Ok(s)
  }
}
"#;

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
    eprintln!("\n!!! EVENTS ORDERING (WORKERD) SKIPPED !!!\n{reason}\n");
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

/// A bare TCP-connect probe, with no HTTP semantics and no side effects —
/// used for the publisher's boot check so it does not itself trigger an
/// emission (the subscriber's `/trace` route is already side-effect-free and
/// is used directly).
fn port_open(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(500),
    )
    .is_ok()
}

/// Strip the JSON string quoting the boundary codec wraps a bare `String`
/// return value in — `HttpResult[String]` serialises over the wire as a JSON
/// string literal (`"a0a1a2..."`), not a raw string, so the body arrives
/// quoted. The fixture's tags are plain alphanumerics, so stripping the
/// surrounding quotes is enough; no escape decoding is needed.
fn unquote_json_string(s: &str) -> String {
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s)
        .to_string()
}

/// Extract the two-character tags with the given prefix, in the order they
/// appear in the (already unquoted) trace string (`"a0a1a2..."`, from
/// `Trace`'s `Cell[String]` concatenation).
fn tags_with_prefix(trace: &str, prefix: char) -> Vec<String> {
    let chars: Vec<char> = trace.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < chars.len() {
        if chars[i] == prefix {
            out.push(format!("{}{}", chars[i], chars[i + 1]));
        }
        i += 2;
    }
    out
}

fn expected_run(prefix: char) -> Vec<String> {
    (0..8).map(|n| format!("{prefix}{n}")).collect()
}

/// Poll `GET /trace` on the subscriber until the given prefix has collected
/// at least `n` tags, or the deadline passes: `Err` carries the last observed
/// trace. Never a fixed sleep-then-assert-once: the two `wrangler dev`
/// processes discover each other through wrangler's dev registry
/// (`bynk/src/dev.rs` — verified, a Service Binding starts `[not connected]`
/// and converges once its callee is up), so a slow convergence must show up as
/// a bounded retry, never a false ordering failure.
fn try_poll_until_len(
    sub_port: u16,
    prefix: char,
    n: usize,
    deadline: Duration,
) -> Result<Vec<String>, String> {
    let start = Instant::now();
    let mut last = String::new();
    loop {
        if let Ok(body) = http("GET", sub_port, "/trace", None) {
            let trace = unquote_json_string(&body);
            last = trace.clone();
            let got = tags_with_prefix(&trace, prefix);
            if got.len() >= n {
                return Ok(got);
            }
        }
        if start.elapsed() > deadline {
            return Err(last);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// [`try_poll_until_len`], failing the test with both workers' logs when the
/// deadline passes. The fan-out DO logs `EventsFanout delivery failed` on a
/// caught `deliverEvent` error (a not-yet-connected Service Binding, most
/// likely) — exactly what distinguishes "binding not connected" from "emit
/// never fired" from "DO never instantiated", so both logs go in the panic
/// message rather than only the last observed trace.
fn poll_until_len(
    sub_port: u16,
    prefix: char,
    n: usize,
    deadline: Duration,
    pub_log: &Path,
    sub_log: &Path,
) -> Vec<String> {
    try_poll_until_len(sub_port, prefix, n, deadline).unwrap_or_else(|last| {
        panic!(
            "delivery did not reach {n} '{prefix}'-tagged events within {deadline:?}; \
             last observed trace: {last:?}\n\
             --- publisher log ---\n{}\n\
             --- subscriber log ---\n{}",
            read_log(pub_log),
            read_log(sub_log)
        )
    })
}

/// Wait until the publisher's Service Binding to the subscriber is live, by
/// emitting a probe burst (tag prefix `w`, which no phase asserts on) until one
/// reaches the subscriber, then until the trace stops growing so no probe
/// delivery is still in flight when phase (a) starts.
///
/// Both ports answering does not mean the binding has connected, and an event
/// emitted before it has is not retried: the fan-out catches the failed
/// delivery, logs it and moves on. Polling the trace after a single trigger
/// cannot recover that, which is how a Windows run saw an empty trace for the
/// whole window. So the readiness probe has to emit repeatedly, not just poll.
fn await_binding(pub_port: u16, sub_port: u16, deadline: Duration, pub_log: &Path, sub_log: &Path) {
    let start = Instant::now();
    loop {
        let _ = http("GET", pub_port, "/burst/w", None);
        if try_poll_until_len(sub_port, 'w', 1, Duration::from_secs(5)).is_ok() {
            break;
        }
        assert!(
            start.elapsed() <= deadline,
            "the publisher's Service Binding never delivered a probe event within {deadline:?}\n\
             --- publisher log ---\n{}\n\
             --- subscriber log ---\n{}",
            read_log(pub_log),
            read_log(sub_log)
        );
    }
    let mut last = String::new();
    let mut quiet_since = Instant::now();
    while quiet_since.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_millis(200));
        if let Ok(body) = http("GET", sub_port, "/trace", None)
            && body != last
        {
            last = body;
            quiet_since = Instant::now();
        }
    }
}

#[test]
fn per_publisher_fifo_on_workerd() {
    if !tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    if !tool_exists("node") && skip("`node` is not on PATH") {
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-events-fifo-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj/fifo");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("publisher.bynk"), SOURCE_PUBLISHER).unwrap();
    fs::write(proj.join("subscriber.bynk"), SOURCE_SUBSCRIBER).unwrap();

    let out = match bynkc::compile_project(
        &bynk_testkit::compile_options_single(tmp.join("proj")).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the fifo-ordering project to workers:\n{}",
            bynkc::render_project_errors(&failure.flatten())
        ),
    };
    let out = bynkc::strip_project_to_js(out).expect("fifo-ordering project strips to JS");
    bynkc::write_output(&out, &tmp).unwrap();

    let pub_dir = tmp.join("workers/fifo-publisher");
    let sub_dir = tmp.join("workers/fifo-subscriber");
    assert!(
        pub_dir.join("index.js").is_file() && pub_dir.join("wrangler.toml").is_file(),
        "expected workers/fifo-publisher — context dasherisation changed?"
    );
    assert!(
        sub_dir.join("index.js").is_file() && sub_dir.join("wrangler.toml").is_file(),
        "expected workers/fifo-subscriber — context dasherisation changed?"
    );

    // Two ports, offset from `workers_runtime_smoke.rs`'s single-port band
    // (20000 + pid%10000) so the two tests cannot collide when run together.
    // Two MORE ports for the V8 inspector: `wrangler dev` binds one whether
    // or not `--inspector-port` is passed (default 9229), so two instances
    // spawned by this test collide with each other on every run unless both
    // are given distinct explicit ports — `bynk/src/dev.rs`'s multi-context
    // `serve_args` does exactly this for the same reason. Confirmed the hard
    // way: an un-pinned run hit `kj::Exception: …bind… Address already in
    // use; …127.0.0.1:9229` and stalled past the boot deadline.
    let base = 30_000 + 4 * (std::process::id() % 4_000) as u16;
    let pub_port = base;
    let sub_port = base + 1;
    let pub_inspector_port = base + 2;
    let sub_inspector_port = base + 3;

    // Log to a file, not a pipe: a piped stdout/stderr nobody drains fills
    // its OS buffer and blocks the child's next write once enough of the
    // suite's many requests have been logged — the worker then stops
    // answering mid-test, which reads exactly like a Service Binding that
    // never connected. `wrangler dev` runs for the whole test, unlike
    // `workers_runtime_smoke.rs`'s single request, so this is reached in
    // practice, not just in theory.
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
                // I2 (#1971): the publisher signs each delivery under the key
                // for its edge to the subscriber, as under `bynk dev`.
                .args(wrangler::local_edge_vars(&[(
                    "fifo.publisher",
                    "fifo.subscriber",
                )]))
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

    // Both processes must be answering before any trigger fires. That is
    // necessary, not sufficient: the Service Binding between them converges
    // later, which `await_binding` waits out.
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
        let pub_up = port_open(pub_port);
        let sub_up = http("GET", sub_port, "/trace", None).is_ok();
        if pub_up && sub_up {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    // Both answering is not the binding being connected: see `await_binding`.
    await_binding(
        pub_port,
        sub_port,
        Duration::from_secs(120),
        &pub_log,
        &sub_log,
    );

    // (a) eight emissions from ONE handler body.
    http("GET", pub_port, "/burst/a", None).expect("trigger phase (a)");
    let got_a = poll_until_len(
        sub_port,
        'a',
        8,
        Duration::from_secs(30),
        &pub_log,
        &sub_log,
    );
    assert_eq!(
        got_a,
        expected_run('a'),
        "phase (a) — emissions within one handler body must arrive in order"
    );

    // (b) eight successive, sequentially-awaited invocations of one agent.
    http("GET", pub_port, "/successive/s", None).expect("trigger phase (b)");
    let got_b = poll_until_len(
        sub_port,
        's',
        8,
        Duration::from_secs(30),
        &pub_log,
        &sub_log,
    );
    assert_eq!(
        got_b,
        expected_run('s'),
        "phase (b) — successive non-overlapping invocations of one agent \
         must arrive in order"
    );

    // (c) two overlapping invocations of the same agent, issued concurrently
    // from separate threads (the blocking raw-TCP client cannot overlap two
    // requests on one thread). This is NOT an ordering assertion in either
    // direction — see the module doc and the accompanying ADR
    // (design/pending/events-per-publisher-fifo-scope.md) for why, and for
    // the recorded interleaving observation.
    let (p_result, q_result) = std::thread::scope(|scope| {
        let p = scope.spawn(|| http("GET", pub_port, "/burst/p", None));
        let q = scope.spawn(|| http("GET", pub_port, "/burst/q", None));
        (p.join().unwrap(), q.join().unwrap())
    });
    p_result.expect("trigger phase (c) burst p");
    q_result.expect("trigger phase (c) burst q");
    let trace_c = {
        // Wait for both bursts to fully land, then read once for inspection.
        let _ = poll_until_len(
            sub_port,
            'p',
            8,
            Duration::from_secs(30),
            &pub_log,
            &sub_log,
        );
        let _ = poll_until_len(
            sub_port,
            'q',
            8,
            Duration::from_secs(30),
            &pub_log,
            &sub_log,
        );
        let body = http("GET", sub_port, "/trace", None).expect("read trace after phase (c)");
        unquote_json_string(&body)
    };
    let got_p = tags_with_prefix(&trace_c, 'p');
    let got_q = tags_with_prefix(&trace_c, 'q');

    // Completeness: every emitted event arrived exactly once, on each side.
    assert_eq!(
        got_p,
        expected_run('p'),
        "phase (c) — burst p's own relative order must survive regardless \
         of interleaving with burst q (one burst is one sequential delivery \
         loop; only cross-burst interleaving is unconstrained)"
    );
    assert_eq!(
        got_q,
        expected_run('q'),
        "phase (c) — burst q's own relative order must survive regardless \
         of interleaving with burst p"
    );

    // Record, don't assert, whether the two bursts interleaved — see the
    // module doc: this is a scheduling race, not a pass/fail gate. The ADR's
    // evidence comes from repeating this phase many times outside CI. Not
    // interleaved means one burst's 8 deliveries are contiguous and finish
    // entirely before the other's first delivery (in either order).
    let seq: Vec<char> = trace_c
        .chars()
        .collect::<Vec<_>>()
        .chunks(2)
        .filter(|c| c.len() == 2 && (c[0] == 'p' || c[0] == 'q'))
        .map(|c| c[0])
        .collect();
    let interleaved = match (
        seq.iter().position(|&c| c == 'p'),
        seq.iter().rposition(|&c| c == 'p'),
        seq.iter().position(|&c| c == 'q'),
        seq.iter().rposition(|&c| c == 'q'),
    ) {
        (Some(_), Some(last_p), Some(first_q), Some(_)) if last_p < first_q => false,
        (Some(first_p), Some(_), Some(_), Some(last_q)) if last_q < first_p => false,
        _ => true,
    };
    eprintln!(
        "events_ordering_workerd: phase (c) interleaved = {interleaved} \
         (p/q burst-level trace: {seq:?})"
    );

    drop(wranglers);
    let _ = fs::remove_dir_all(&tmp);
}
