//! #2193: a callee's fault reaches its caller as `CalleeFaulted`, on a real
//! `workerd`.
//!
//! The runtime's unit tests (`bynk-emit/runtime/test/boundary.test.ts`) prove
//! `callService` reads a marked `500` and `faultResponse` marks only an
//! `on call` route's. Only a real call proves the emitted entries put the two
//! together: that a handler fault in the callee's `/_bynk/call/` dispatch
//! answers the marked `500`, and that the caller, reaching it over a Service
//! Binding, logs `CalleeFaulted <caller> -> <callee> call <service>` and faults
//! with `BoundaryError: CalleeFaulted` rather than `Transport`.
//!
//! Two `wrangler dev`s, one per context, joined through wrangler's dev
//! registry as in `events_ordering_workerd.rs`. The callee's `square` returns
//! `n * n`; at `n = 100000000` the result is past the safe-integer range the
//! wire format carries, so encoding it faults the handler. A small `n` is the
//! control: the same path answers `200`.
//!
//! Skips loudly without the toolchain; a non-empty `BYNK_REQUIRE_WORKERD`
//! (Linux CI sets `1`) turns the skip into a failure. Both `wrangler dev`s are
//! stopped, with their `workerd`s, when the guard drops (#1686).

use bynkc::BuildTarget;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

mod require;
mod wrangler;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";
const WRANGLER: &str = wrangler::SPEC;

const SOURCE_CALLEE: &str = r#"context fault.callee

service square {
  on call(n: Int) -> Effect[Int] {
    Effect.pure(n * n)
  }
}
"#;

const SOURCE_CALLER: &str = r#"context fault.caller

consumes fault.callee

service api from http {
  on GET("/square/:n") (n: String) -> Effect[HttpResult[String]] by v: Visitor {
    match Int.parse(n) {
      Some(i) => {
        let r <- fault.callee.square(i)
        Ok("\(r)")
      },
      None => BadRequest("not an int"),
    }
  }
}
"#;

fn tool_exists(name: &str) -> bool {
    which::which(name).is_ok()
}

/// Route through `cmd /C` on Windows so npm's `npx.cmd` shim resolves.
fn base_command(program: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        Command::new(program)
    }
}

fn skip(reason: &str) -> bool {
    eprintln!("\n!!! CALLEE FAULTED (WORKERD) SKIPPED !!!\n{reason}\n");
    if require::is_required(REQUIRE_ENV) {
        panic!("{REQUIRE_ENV} is set but {reason}");
    }
    true
}

/// Both `wrangler dev`s, stopped together on every exit path.
struct Wranglers(Vec<wrangler::Wrangler>);

impl Drop for Wranglers {
    fn drop(&mut self) {
        wrangler::stop_all(&mut self.0);
    }
}

/// One `GET`, HTTP/1.1, connection-close: the status and the body.
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

/// Each child logs to a file, not an undrained pipe, so it never blocks on a
/// full buffer; the file is read back to assert on and for failure messages.
fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| format!("<could not read {path:?}: {e}>"))
}

#[test]
fn a_callee_fault_reaches_the_caller_as_callee_faulted() {
    if !tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    if !tool_exists("node") && skip("`node` is not on PATH") {
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-callee-faulted-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj/fault");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("callee.bynk"), SOURCE_CALLEE).unwrap();
    fs::write(proj.join("caller.bynk"), SOURCE_CALLER).unwrap();

    let out = match bynkc::compile_project(
        &bynk_testkit::compile_options_single(tmp.join("proj")).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the callee-faulted project to workers:\n{}",
            bynkc::render_project_errors(&failure.flatten())
        ),
    };
    let out = bynkc::strip_project_to_js(out).expect("callee-faulted project strips to JS");
    bynkc::write_output(&out, &tmp).unwrap();

    let callee_dir = tmp.join("workers/fault-callee");
    let caller_dir = tmp.join("workers/fault-caller");
    for dir in [&callee_dir, &caller_dir] {
        assert!(
            dir.join("index.js").is_file() && dir.join("wrangler.toml").is_file(),
            "expected {dir:?}: context dasherisation changed?"
        );
    }

    // Four ports (two servers, two inspectors) in a band clear of the other
    // workerd tests' (20_000 + 2·pid, 30_000 + 4·pid, 40_000 + 2·pid,
    // 45_000 + 4·pid and 55_000 + 2·pid, each pid bounded), so they can all
    // run at once.
    let base = 63_000 + 4 * (std::process::id() % 600) as u16;
    let callee_port = base;
    let caller_port = base + 1;
    let callee_log = tmp.join("callee-wrangler.log");
    let caller_log = tmp.join("caller-wrangler.log");
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
                // I2 (#1971): the caller signs each call under the key for its
                // edge to the callee, as under `bynk dev`.
                .args(wrangler::local_edge_vars(&[(
                    "fault.caller",
                    "fault.callee",
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
        )
    };

    let callee = spawn(&callee_dir, callee_port, base + 2, &callee_log);
    let caller = spawn(&caller_dir, caller_port, base + 3, &caller_log);
    let wranglers = match (callee, caller) {
        (Ok(a), Ok(b)) => Wranglers(vec![a, b]),
        (a, b) => {
            drop(Wranglers(a.into_iter().chain(b).collect()));
            if skip("could not launch npx for one or both workers") {
                return;
            }
            unreachable!()
        }
    };
    let logs = || {
        format!(
            "--- caller log ---\n{}\n--- callee log ---\n{}",
            read_log(&caller_log),
            read_log(&callee_log)
        )
    };

    // The control call answers once both Workers are up and the Service
    // Binding has connected (it starts `[not connected]` and converges), so
    // it doubles as the readiness probe.
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Ok((200, body)) = get(caller_port, "/square/3") {
            assert_eq!(body, "\"9\"", "the control call answers the square");
            break;
        }
        if Instant::now() > deadline {
            let logs = logs();
            drop(wranglers);
            let _ = fs::remove_dir_all(&tmp);
            if skip(&format!(
                "the control call did not answer within the boot window (likely \
                 no network to provision {WRANGLER})\n{logs}"
            )) {
                return;
            }
            unreachable!()
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    // 100000000² is past the safe-integer range, so the callee's handler
    // faults encoding it, and the caller faults in turn. Only the caller's log
    // from each attempt's start is read: before the Service Binding connects, a
    // control call can fail as a genuine `Transport` (the callee is not
    // reachable yet), which is the right report for that, not this.
    //
    // The callee's line is a `console.error` in the callee Worker, which that
    // `wrangler dev` forwards to its log on its own schedule, and on Windows it
    // has been missing for a whole window while the caller's lines (and the
    // 500) were all there. The fault is deterministic, so a missing line is a
    // lost log, not a missing fault: the call is made again, up to `ATTEMPTS`
    // times, each with its own window, and the test fails only when no attempt
    // got the line out. Every attempt still has to fault as `CalleeFaulted`.
    let wanted = [
        "CalleeFaulted fault.caller -> fault.callee call square",
        "BoundaryError: CalleeFaulted",
    ];
    let callee_wanted = "fault.callee call square faulted";
    const ATTEMPTS: u32 = 3;
    let mut outcome = String::new();
    for attempt in 1..=ATTEMPTS {
        let caller_mark = read_log(&caller_log).len();
        let (status, _) = get(caller_port, "/square/100000000").expect("the faulting call answers");
        assert_eq!(
            status,
            500,
            "the caller faults when its callee does (attempt {attempt})\n{}",
            logs()
        );

        // The caller's log names the callee, and the callee's keeps the cause:
        // its own fault line, which the caller's never carries. Both Workers'
        // lines arrive asynchronously, so poll for them rather than read once.
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let caller_text = read_log(&caller_log)
                .get(caller_mark..)
                .unwrap_or_default()
                .to_string();
            let caller_named = wanted.iter().all(|w| caller_text.contains(w));
            let callee_logged = read_log(&callee_log).contains(callee_wanted);
            if caller_named && callee_logged {
                assert!(
                    !caller_text.contains("BoundaryError: Transport"),
                    "a marked fault must not also read as a transport failure\n{}",
                    logs()
                );
                outcome.clear();
                break;
            }
            if Instant::now() > deadline {
                outcome = format!(
                    "attempt {attempt}/{ATTEMPTS}: the caller's log names the callee's fault \
                     ({wanted:?}: {caller_named}), the callee's logs its own \
                     ({callee_wanted:?}: {callee_logged})"
                );
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        if outcome.is_empty() {
            break;
        }
        eprintln!("callee_faulted_workerd: {outcome}; retrying");
    }
    assert!(outcome.is_empty(), "{outcome}\n{}", logs());

    drop(wranglers);
    let _ = fs::remove_dir_all(&tmp);
}
