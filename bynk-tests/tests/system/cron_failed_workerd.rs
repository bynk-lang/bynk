//! A failed cron run on a real `workerd` (#3872): an `Err` run and a throwing run
//! each show as failed, an `Ok` run as a success, via the local scheduled
//! trigger (`/cdn-cgi/handler/scheduled?cron=...`, which answers `200 ok` or
//! `500 exception`).
//!
//! - **An `Err` run** is logged as one structured record naming the cron, the
//!   service, the error's type and variant (never the payload) and is reported
//!   through `waitUntil`, so the platform records it as an exception.
//! - **A throwing run** is logged and reported, then rethrown.
//! - **An unmatched expression** is named in one warning.
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

const SOURCE: &str = r#"context cronfail

type Refused = | Overdue(note: String) | Stale

type Detail = { tag: String }

service sweeper from cron {
  on schedule("*/5 * * * *") (at: Instant) -> Effect[Result[(), Refused]] {
    Err(Overdue("https://x/?key=hunter2"))
  }

  on schedule("0 * * * *") (at: Instant) -> Effect[Result[(), Refused]] {
    Ok(())
  }

  on schedule("30 * * * *") (at: Instant) -> Effect[Result[(), Refused]] {
    unreachable("sweep boom")
  }
}

// A record error with a field called `tag` is user data, not a variant.
service audit from cron {
  on schedule("15 * * * *") () -> Effect[Result[(), Detail]] {
    Err(Detail { tag: "hunter3" })
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
fn a_failed_cron_run_is_reported_on_workerd() {
    if !tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    let tmp = std::env::temp_dir().join(format!("bynk-cronfail-workerd-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("cronfail.bynk"), SOURCE).unwrap();
    let out = match bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(&proj).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the cron project to workers:\n{}",
            bynk_driver::render::render_project_errors(&failure.flatten())
        ),
    };
    let out = bynk_driver::strip::strip_project_to_js(out).expect("the cron project strips to JS");
    bynk_driver::write_output(&out, &tmp).unwrap();
    let dir = tmp.join("workers/cronfail");
    assert!(dir.join("index.js").is_file(), "expected {dir:?}");

    let [port, inspector] = wrangler::ports::free_ports();
    let log_path = tmp.join("wrangler.log");
    let spawned = (|| -> std::io::Result<wrangler::Wrangler> {
        let out_log = fs::File::create(&log_path)?;
        let err_log = out_log.try_clone()?;
        wrangler::Wrangler::spawn(
            wrangler::npx()
                .args(["dev", "--test-scheduled"])
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
    let trigger = |cron: &str| get(port, &format!("/cdn-cgi/handler/scheduled?cron={cron}"));

    let booted = std::cell::RefCell::new(None);
    if let Err(why) = wrangler::wait_until_ready(&log_path, port, &|| match trigger("0+*+*+*+*") {
        Ok(r) => {
            *booted.borrow_mut() = Some(r);
            true
        }
        Err(_) => false,
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
    let ok = booted.into_inner().expect("the probe answered");
    assert_eq!(ok.0, 200, "an Ok run is a success: {ok:?}\n{}", log());
    assert!(
        ok.1.eq_ignore_ascii_case("ok"),
        "an Ok run is a success: {ok:?}"
    );

    let err = trigger("*%2F5+*+*+*+*").expect("the Err run answers");
    assert!(
        err.0 == 500 && err.1 == "exception",
        "an Err run is recorded as failed: {err:?}\n{}",
        log()
    );
    let threw = trigger("30+*+*+*+*").expect("the throwing run answers");
    assert!(
        threw.0 == 500 && threw.1 == "exception",
        "a throwing run is recorded as failed: {threw:?}\n{}",
        log()
    );
    let record = trigger("15+*+*+*+*").expect("the record-error run answers");
    assert!(
        record.0 == 500 && record.1 == "exception",
        "{record:?}\n{}",
        log()
    );
    let _ = trigger("1+1+1+1+1");

    let end = Instant::now() + Duration::from_secs(20);
    let settled = |l: &str| {
        [
            "sweep boom",
            "a cron run returned Err",
            "matches no `on schedule`",
        ]
        .iter()
        .all(|m| l.contains(m))
    };
    while Instant::now() < end && !settled(&log()) {
        std::thread::sleep(Duration::from_millis(200));
    }
    let log = log();
    assert!(log.contains("a cron run returned Err"), "{log}");
    assert!(
        log.contains("Refused") && log.contains("Overdue"),
        "the record names the type and variant:\n{log}"
    );
    assert!(
        !log.contains("hunter2"),
        "the payload must not be logged:\n{log}"
    );
    assert!(
        log.contains("Detail") && !log.contains("hunter3"),
        "a record error's own `tag` field is not a variant:\n{log}"
    );
    assert!(
        log.contains("cron `30 * * * *` (sweeper) faulted") && log.contains("sweep boom"),
        "a throw is labelled:\n{log}"
    );
    assert!(
        log.contains("matches no `on schedule` handler"),
        "an unmatched expression is warned about:\n{log}"
    );
    drop(guard);
    let _ = fs::remove_dir_all(&tmp);
}
