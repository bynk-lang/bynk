//! #3580 (the dev request-traces track): per-request traces on a real `workerd`.
//!
//! The same two-context project is served twice under `wrangler dev`: once as
//! `bynk dev` serves it by default, and once built to record traces
//! (`trace_requests`, plus the `nodejs_als` compatibility flag `--trace-events`
//! adds). Each is sent the same requests: a call that succeeds, and one whose
//! callee faults. The two must answer identically, which is the proof that
//! recording only observes. The recording run must also have printed, for each
//! request, one `__bynk_trace:` line carrying the route, the status and the
//! ordered, spanned steps; the plain run must have printed none. That proves
//! `AsyncLocalStorage` finds the request's record under workerd, which the
//! runtime's Node tests cannot.
//!
//! The "off" side runs without the flag, so it also shows the flag itself
//! changes nothing.
//!
//! Skips loudly without the toolchain; a non-empty `BYNK_REQUIRE_WORKERD`
//! (Linux CI sets `1`) turns the skip into a failure.

use bynk_driver::project::BuildTarget;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

#[path = "../require/mod.rs"]
mod require;
#[path = "../wrangler/mod.rs"]
mod wrangler;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";
const PREFIX: &str = "__bynk_trace:";

const SOURCE_CALLEE: &str = r#"context tr.callee

service square {
  on call(n: Int) -> Effect[Int] {
    Effect.pure(n * n)
  }
}
"#;

const SOURCE_CALLER: &str = r#"context tr.caller

consumes tr.callee

type Never = enum { Never }

agent Counter {
  key id: String

  store hits: Cell[Int]

  on call bump() -> Effect[Result[Int, Never]] {
    let m = hits + 1
    hits := m
    Ok(m)
  }
}

service api from http {
  on GET("/square/:n") (n: String) -> Effect[HttpResult[String]] by v: Visitor {
    match Int.parse(n) {
      Some(i) => {
        let counter = Counter("k")
        let bumped <- counter.bump()
        let r <- tr.callee.square(i)
        match bumped {
          Ok(_) => Ok("\(r)"),
          Err(_) => BadRequest("never"),
        }
      },
      None => BadRequest("not an int"),
    }
  }
}
"#;

fn tool_exists(name: &str) -> bool {
    which::which(name).is_ok()
}

fn skip(reason: &str) -> bool {
    eprintln!("\n!!! TRACE REQUESTS (WORKERD) SKIPPED !!!\n{reason}\n");
    if require::is_required(REQUIRE_ENV) {
        panic!("{REQUIRE_ENV} is set but {reason}");
    }
    true
}

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

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| format!("<could not read {path:?}: {e}>"))
}

/// What one served run produced: each request's answer, and the caller's and
/// callee's logs.
struct Run {
    answers: Vec<(u16, String)>,
    caller_log: String,
    callee_log: String,
}

/// The requests both runs are sent, in order.
const REQUESTS: [&str; 3] = ["/square/3", "/square/4", "/square/100000000"];

/// Build the project (traced or not), serve both Workers, send [`REQUESTS`], and
/// return the answers and logs. `None` when the toolchain is unavailable.
fn serve(tmp: &Path, traced: bool, base: u16) -> Option<Run> {
    let proj = tmp.join("proj/tr");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("callee.bynk"), SOURCE_CALLEE).unwrap();
    fs::write(proj.join("caller.bynk"), SOURCE_CALLER).unwrap();
    let mut options =
        bynk_testkit::compile_options_single(tmp.join("proj")).target(BuildTarget::Workers);
    if traced {
        options = options
            .trace_requests(true)
            .compatibility_flags(vec!["nodejs_als".to_string()]);
    }
    let out = match bynk_driver::project::compile_project(&options) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the trace project:\n{}",
            bynk_driver::render::render_project_errors(&failure.flatten())
        ),
    };
    let out = bynk_driver::strip::strip_project_to_js(out).expect("the project strips to JS");
    bynk_driver::write_output(&out, tmp).unwrap();

    let callee_dir = tmp.join("workers/tr-callee");
    let caller_dir = tmp.join("workers/tr-caller");
    let callee_log = tmp.join("callee-wrangler.log");
    let caller_log = tmp.join("caller-wrangler.log");
    let spawn = |dir: &Path, port: u16, inspector: u16, log: &Path| {
        let out_log = fs::File::create(log)?;
        let err_log = out_log.try_clone()?;
        wrangler::Wrangler::spawn(
            wrangler::npx()
                .args(["dev"])
                .args(wrangler::local_edge_vars(&[("tr.caller", "tr.callee")]))
                .args([
                    "--port",
                    &port.to_string(),
                    "--inspector-port",
                    &inspector.to_string(),
                ])
                .current_dir(dir)
                .stdout(Stdio::from(out_log))
                .stderr(Stdio::from(err_log)),
            &tmp.join("registry"),
        )
    };
    let callee = spawn(&callee_dir, base, base + 2, &callee_log);
    let caller = spawn(&caller_dir, base + 1, base + 3, &caller_log);
    let wranglers = match (callee, caller) {
        (Ok(a), Ok(b)) => Wranglers(vec![a, b]),
        (a, b) => {
            drop(Wranglers(a.into_iter().chain(b).collect()));
            return None;
        }
    };
    // The first call answers once both Workers are up and the Service Binding has
    // connected, so it is the readiness probe. It is sent again below, so the
    // probe's answer is not one of the compared ones.
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Ok((200, _)) = get(base + 1, "/square/2") {
            break;
        }
        if Instant::now() > deadline {
            drop(wranglers);
            return None;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let answers = REQUESTS
        .iter()
        .map(|p| get(base + 1, p).expect("the request answers"))
        .collect();
    // Trace lines reach the log file after the response; give them a moment.
    std::thread::sleep(Duration::from_secs(2));
    let run = Run {
        answers,
        caller_log: read_log(&caller_log),
        callee_log: read_log(&callee_log),
    };
    drop(wranglers);
    Some(run)
}

/// The trace lines in `log`, parsed.
fn traces(log: &str) -> Vec<serde_json::Value> {
    log.lines()
        .filter_map(|l| l.find(PREFIX).map(|at| &l[at + PREFIX.len()..]))
        .map(|j| serde_json::from_str(j.trim()).unwrap_or_else(|e| panic!("trace line {j:?}: {e}")))
        .collect()
}

fn step_kinds(t: &serde_json::Value) -> Vec<(String, String, String)> {
    t["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["kind"].as_str().unwrap().to_string(),
                s["target"].as_str().unwrap().to_string(),
                s["outcome"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn recording_changes_no_outcome_and_prints_one_spanned_trace_per_request() {
    if !tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    if !tool_exists("node") && skip("`node` is not on PATH") {
        return;
    }
    let tmp = std::env::temp_dir().join(format!("bynk-trace-requests-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    // Eight ports (two runs of two servers and two inspectors). 53_000..55_000 is
    // the span the other workerd tests' bands (20_000 to 52_999 and 55_000 up)
    // leave free; as with theirs, a pid-derived offset keeps parallel runs apart.
    let base = 53_000 + 8 * (std::process::id() % 250) as u16;
    let plain = match serve(&tmp.join("plain"), false, base) {
        Some(r) => r,
        None if skip("the plain run did not answer (likely no network to provision wrangler)") => {
            return;
        }
        None => unreachable!(),
    };
    let traced = match serve(&tmp.join("traced"), true, base + 4) {
        Some(r) => r,
        None if skip("the traced run did not answer (likely no network to provision wrangler)") => {
            return;
        }
        None => unreachable!(),
    };
    let logs = || {
        format!(
            "--- traced caller log ---\n{}\n--- traced callee log ---\n{}",
            traced.caller_log, traced.callee_log
        )
    };

    // Recording changes no outcome.
    assert_eq!(
        plain.answers,
        traced.answers,
        "a traced build answers differently from a plain one\n{}",
        logs()
    );
    assert_eq!(plain.answers[0].0, 200, "{:?}", plain.answers);
    assert_eq!(
        plain.answers[2].0, 500,
        "the faulting call answers 500: {:?}",
        plain.answers
    );

    // The plain run records nothing.
    assert!(
        !plain.caller_log.contains(PREFIX) && !plain.callee_log.contains(PREFIX),
        "a build that does not ask for traces printed one"
    );

    // The traced caller printed one line per request (and one for the readiness probe).
    let caller = traces(&traced.caller_log);
    // Readiness retries add lines of their own, so each request is found by its
    // route, and the last line for it is the one the comparison above answered.
    let by_route = |route: &str| {
        caller
            .iter()
            .rfind(|t| t["route"] == route)
            .unwrap_or_else(|| panic!("no trace for {route}\n{}", logs()))
    };
    let ok = by_route("GET /square/3");
    assert_eq!(ok["context"], "tr.caller");
    assert_eq!(ok["route"], "GET /square/3");
    assert_eq!(ok["status"], 200);
    assert_eq!(ok["fault"], serde_json::Value::Null);
    assert_eq!(ok["truncated"], false);
    assert_eq!(
        step_kinds(ok),
        [
            (
                "agent".to_string(),
                "Counter.bump".to_string(),
                "ok".to_string()
            ),
            (
                "call".to_string(),
                "tr.callee.square".to_string(),
                "ok".to_string()
            ),
        ],
        "the steps are in call order\n{}",
        logs()
    );
    for step in ok["steps"].as_array().unwrap() {
        assert!(
            step["span"]["file"]
                .as_str()
                .unwrap()
                .ends_with("caller.bynk"),
            "{step}"
        );
        assert!(step["span"]["line"].as_u64().unwrap() > 0, "{step}");
        assert!(step["ms"].as_f64().unwrap() >= 0.0, "{step}");
    }

    // The faulting request is a 500 whose trace records the failed call.
    let faulted = by_route("GET /square/100000000");
    assert_eq!(faulted["status"], 500);
    assert!(faulted["fault"].is_string(), "{faulted}\n{}", logs());
    assert!(
        step_kinds(faulted)
            .iter()
            .any(|(k, t, o)| k == "call" && t == "tr.callee.square" && o == "fault"),
        "{faulted}\n{}",
        logs()
    );

    // The callee, serving the internal call, prints a trace of its own, and it
    // names the caller's request as its parent: the caller's `call` step carries
    // the callee's request id, and the callee's record carries the caller's.
    let callee = traces(&traced.callee_log);
    let call_step = ok["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["kind"] == "call")
        .unwrap();
    let child_id = call_step["request"]
        .as_str()
        .expect("a call step names its callee's request");
    let child = callee
        .iter()
        .find(|t| t["id"] == child_id)
        .unwrap_or_else(|| panic!("no callee trace with id {child_id}\n{}", logs()));
    assert_eq!(child["context"], "tr.callee");
    assert_eq!(child["route"], "POST /_bynk/call/square");
    assert_eq!(
        child["parent"],
        ok["id"],
        "the callee names its caller's request\n{}",
        logs()
    );
    // The caller's own request came from outside, so it has no parent.
    assert_eq!(ok["parent"], serde_json::Value::Null);
    // The faulting call is stitched too: its callee's record is the 500.
    let fault_child_id = faulted["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["kind"] == "call")
        .and_then(|s| s["request"].as_str())
        .expect("the faulting call step names its callee's request");
    let fault_child = callee
        .iter()
        .find(|t| t["id"] == fault_child_id)
        .unwrap_or_else(|| panic!("no callee trace with id {fault_child_id}\n{}", logs()));
    assert_eq!(fault_child["status"], 500);
    assert_eq!(fault_child["parent"], faulted["id"]);
    let _ = fs::remove_dir_all(&tmp);
}
