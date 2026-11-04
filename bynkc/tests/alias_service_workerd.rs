//! #2890: a context's service that names a renamed type (`consumes lib { Code as
//! LibCode }`), on a real `workerd`.
//!
//! #2889 checked the aliased context-service paths with `tsc` and the corpus
//! hash guard; this runs them. Three Workers, `edge` -> `app` -> `lib`, joined
//! through wrangler's dev registry as in `callee_faulted_workerd.rs`:
//! `lib` declares `Code`; `app` consumes it as `LibCode` and both calls
//! `lib.flip(LibCode.Alpha)` and serves `echo`, which takes and returns a
//! `LibCode`; `edge` calls `app.echo`. A contract hash that disagreed would
//! answer `409 ContractMismatch`, and a codec naming the wrong type would fault
//! the decode, so each hop answering `200` with the right body is the check.
//!
//! Skips loudly without the toolchain; a non-empty `BYNK_REQUIRE_WORKERD`
//! (Linux CI sets `1`) turns the skip into a failure.

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

const SOURCE_LIB: &str = r#"context alias.lib

exports transparent { Code }

type Code = enum { Alpha, Beta }

service flip {
  on call(c: Code) -> Effect[Code] {
    match c {
      Alpha => Effect.pure(Beta)
      Beta => Effect.pure(Alpha)
    }
  }
}
"#;

const SOURCE_APP: &str = r#"context alias.app

consumes alias.lib { Code as LibCode }

service echo {
  on call(c: LibCode) -> Effect[LibCode] {
    Effect.pure(c)
  }
}

service api from http {
  on GET("/flip") () -> Effect[HttpResult[String]] by v: Visitor {
    let flipped <- alias.lib.flip(LibCode.Alpha)
    match flipped {
      Beta => Ok("beta")
      Alpha => Ok("alpha")
    }
  }
}
"#;

const SOURCE_EDGE: &str = r#"context alias.edge

consumes alias.app
consumes alias.lib { Code }

service api from http {
  on GET("/echo") () -> Effect[HttpResult[String]] by v: Visitor {
    let echoed <- alias.app.echo(Code.Alpha)
    Ok("echoed")
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
    eprintln!("\n!!! ALIASED SERVICE (WORKERD) SKIPPED !!!\n{reason}\n");
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
fn an_aliased_context_service_runs_across_workers() {
    if !tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    if !tool_exists("node") && skip("`node` is not on PATH") {
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-alias-service-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj/alias");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("lib.bynk"), SOURCE_LIB).unwrap();
    fs::write(proj.join("app.bynk"), SOURCE_APP).unwrap();
    fs::write(proj.join("edge.bynk"), SOURCE_EDGE).unwrap();

    let out = match bynkc::compile_project(
        &bynk_testkit::compile_options_single(tmp.join("proj")).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the aliased-service project to workers:\n{}",
            bynkc::render_project_errors(&failure.flatten())
        ),
    };
    let out = bynkc::strip_project_to_js(out).expect("aliased-service project strips to JS");
    bynkc::write_output(&out, &tmp).unwrap();

    let names = ["lib", "app", "edge"];
    let dirs: Vec<_> = names
        .iter()
        .map(|n| tmp.join(format!("workers/alias-{n}")))
        .collect();
    for dir in &dirs {
        assert!(
            dir.join("index.js").is_file() && dir.join("wrangler.toml").is_file(),
            "expected {dir:?}: context dasherisation changed?"
        );
    }

    // Six ports (three servers, three inspectors) in a band clear of the other
    // workerd tests' (20_000 + 2·pid, 30_000 + 4·pid, 40_000 + 2·pid,
    // 45_000 + 4·pid, 55_000 + 2·pid and 63_000 + 4·pid, each pid bounded).
    let base = 50_000 + 6 * (std::process::id() % 600) as u16;
    let logs_paths: Vec<_> = names
        .iter()
        .map(|n| tmp.join(format!("{n}-wrangler.log")))
        .collect();
    let spawn = |i: usize| -> std::io::Result<wrangler::Wrangler> {
        let out_log = fs::File::create(&logs_paths[i])?;
        let err_log = out_log.try_clone()?;
        wrangler::Wrangler::spawn(
            base_command("npx")
                .args(["-y", WRANGLER, "dev"])
                // I2 (#1971): each caller signs its calls under the key for its
                // edge, as under `bynk dev`.
                .args(wrangler::local_edge_vars(&[
                    ("alias.edge", "alias.app"),
                    ("alias.app", "alias.lib"),
                ]))
                .args([
                    "--port",
                    &(base + i as u16).to_string(),
                    "--inspector-port",
                    &(base + 3 + i as u16).to_string(),
                ])
                .current_dir(&dirs[i])
                .stdout(Stdio::from(out_log))
                .stderr(Stdio::from(err_log)),
            &tmp.join("registry"),
        )
    };
    let spawned: Vec<_> = (0..3).map(spawn).collect();
    if spawned.iter().any(|s| s.is_err()) {
        drop(Wranglers(spawned.into_iter().flatten().collect()));
        if skip("could not launch npx for one or more workers") {
            return;
        }
        unreachable!()
    }
    let wranglers = Wranglers(spawned.into_iter().flatten().collect());
    let logs = || {
        let mut text = String::new();
        for (n, p) in names.iter().zip(&logs_paths) {
            text.push_str(&format!("--- {n} log ---\n{}\n", read_log(p)));
        }
        text.push_str(&format!(
            "--- dev registry ---\n{}",
            wrangler::registry_listing(&tmp.join("registry"))
        ));
        text
    };

    let app_port = base + 1;
    let edge_port = base + 2;

    // The first hop (`app` -> `lib`) answers once the Workers are up and the
    // Service Binding has connected, so it doubles as the readiness probe.
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Ok((200, body)) = get(app_port, "/flip") {
            assert_eq!(
                body, "\"beta\"",
                "`app` flips `LibCode.Alpha` through `lib` and gets `Beta` back"
            );
            break;
        }
        if Instant::now() > deadline {
            let logs = logs();
            drop(wranglers);
            let _ = fs::remove_dir_all(&tmp);
            if skip(&format!(
                "the first call did not answer within the boot window (likely \
                 no network to provision {WRANGLER})\n{logs}"
            )) {
                return;
            }
            unreachable!()
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    // `edge` -> `app`: the contract `edge` expects for `echo` is `app`'s own.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match get(edge_port, "/echo") {
            Ok((200, body)) => {
                assert_eq!(body, "\"echoed\"");
                break;
            }
            other if Instant::now() > deadline => {
                panic!(
                    "`edge` -> `app` echo did not answer 200: {other:?}\n{}",
                    logs()
                )
            }
            _ => std::thread::sleep(Duration::from_millis(500)),
        }
    }
    let all = logs();
    assert!(
        !all.contains("ContractMismatch"),
        "an aliased signature must hash the same on both sides\n{all}"
    );

    drop(wranglers);
    let _ = fs::remove_dir_all(&tmp);
}
