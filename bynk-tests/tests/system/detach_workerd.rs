//! `e.detach()` on a real `workerd` (#3437): the two premises its failure route
//! rests on.
//!
//! - **A rejected promise handed to `waitUntil` is reported as an exception** and
//!   does not change the response: a detached effect that faults answers `200`
//!   as usual, and `wrangler dev` records `Uncaught Error: <message>` with the
//!   original stack. That is the mechanism `__bynkReportFault` (#2192) uses for
//!   an entry's own faults. Whether Cloudflare applies an *uploaded* source map
//!   to that exception is the platform's, and only a deployed Worker shows it.
//! - **A detached `Err` is one structured `console.error` record** naming the
//!   site, the error's type and its variant, and not its payload.
//!
//! The 30-second limit on `waitUntil` work after the response is *not* checked
//! here: local `workerd` does not enforce it (a 40 s `waitUntil` ran to the end
//! in a probe), so the guide states no number until it is confirmed against the
//! platform.
//!
//! Skips loudly without the toolchain; a non-empty `BYNK_REQUIRE_WORKERD`
//! turns the skip into a failure.

use bynk_driver::project::BuildTarget;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[path = "../require/mod.rs"]
mod require;
#[path = "../wrangler/mod.rs"]
mod wrangler;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";
const WRANGLER: &str = wrangler::SPEC;

const SOURCE: &str = r#"context detach

consumes bynk

type MailError = | Refused | Bounced(reason: String)

fn refused() -> Effect[Result[(), MailError]] { Err(Bounced("https://x/?key=hunter2")) }

fn faulty() -> Effect[()] { unreachable("detached boom") }

service api from http {
  on GET("/err/:t") (t: String) -> Effect[HttpResult[String]] by v: Visitor {
    do refused().detach()
    Ok("err handed")
  }

  on GET("/fault/:t") (t: String) -> Effect[HttpResult[String]] by v: Visitor {
    do faulty().detach()
    Ok("fault handed")
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
fn a_detached_fault_and_err_are_reported_on_workerd() {
    if !tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    let tmp = std::env::temp_dir().join(format!("bynk-detach-workerd-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("detach.bynk"), SOURCE).unwrap();
    let out = match bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(&proj).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the detach project to workers:\n{}",
            bynk_driver::render::render_project_errors(&failure.flatten())
        ),
    };
    let out =
        bynk_driver::strip::strip_project_to_js(out).expect("the detach project strips to JS");
    bynk_driver::write_output(&out, &tmp).unwrap();
    let dir = tmp.join("workers/detach");
    assert!(dir.join("index.js").is_file(), "expected {dir:?}");

    let base = 10_000 + 2 * (std::process::id() % 4_000) as u16;
    let (port, inspector) = (base, base + 1);
    let log_path = tmp.join("wrangler.log");
    let spawned = (|| -> std::io::Result<wrangler::Wrangler> {
        let out_log = fs::File::create(&log_path)?;
        let err_log = out_log.try_clone()?;
        wrangler::Wrangler::spawn(
            base_command("npx")
                .args(["-y", WRANGLER, "dev"])
                .args(["--port", &port.to_string(), "--inspector-port", &inspector.to_string()])
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
    let (status, body) = loop {
        if let Ok(r) = get(port, "/err/boot") {
            break r;
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
    };
    assert_eq!(status, 200, "a detached Err leaves the response alone\n{}", log());
    assert!(body.contains("err handed"), "{body}");
    let (status, body) = get(port, "/fault/x").expect("the fault route answers");
    assert_eq!(status, 200, "a detached fault leaves the response alone\n{}", log());
    assert!(body.contains("fault handed"), "{body}");

    // `waitUntil` work is reported after the response; give it a moment.
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end
        && !(log().contains("a detached effect returned Err") && log().contains("detached boom"))
    {
        std::thread::sleep(Duration::from_millis(200));
    }
    let log = log();
    assert!(log.contains("a detached effect returned Err"), "{log}");
    assert!(log.contains("MailError") && log.contains("Bounced"), "{log}");
    assert!(!log.contains("hunter2"), "the payload must not be reported:\n{log}");
    assert!(
        log.contains("Uncaught Error") && log.contains("detached boom"),
        "a rejected waitUntil promise is recorded as an exception:\n{log}"
    );
    drop(guard);
    let _ = fs::remove_dir_all(&tmp);
}
