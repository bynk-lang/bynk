//! #3895 (a request's trace across contexts): one request on a real `workerd`.
//!
//! An HTTP route in `tp.web` calls `tp.orders`, which logs, emits an event and
//! returns; `tp.audit` subscribes to the event and logs. Each of the three logs
//! a `Logger.log` record, and the test reads the `trace` each carries:
//!
//! - one request leaves the same `trace` in all three Workers' logs;
//! - a second request leaves a different one;
//! - a request that arrives with a client `traceparent` is not given the
//!   client's trace: a public request's header is never read.
//!
//! The trace id reaches `tp.orders` as a signed call's `traceparent` and
//! reaches `tp.audit` through the fan-out Durable Object, as the envelope's
//! `trace`, so this covers both hops. `nodejs_als` is the compatibility flag
//! every generated Worker now carries, so it is not added here.
//!
//! Skips loudly without the toolchain; a non-empty `BYNK_REQUIRE_WORKERD`
//! (Linux CI sets `1`) turns the skip into a failure.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use bynk_driver::project::BuildTarget;

#[path = "../require/mod.rs"]
mod require;
#[path = "../wrangler/mod.rs"]
mod wrangler;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";

const MANIFEST: &str = r#"[project]
name = "trace-propagation"
version = "0.1.0"

[unstable]
allow = ["bynk.Logger.log"]
"#;

const SOURCE_WEB: &str = r#"context tp.web

consumes tp.orders
consumes bynk { Logger, LogLevel }

service api from http {
  on GET("/order/:n") (n: String) -> Effect[HttpResult[String]] by v: Visitor given Logger {
    let _ <- Logger.log(Info, "web", Map.empty().insert("n", n))
    let r <- tp.orders.place(n)
    Ok(r)
  }
}
"#;

const SOURCE_ORDERS: &str = r#"context tp.orders

exports transparent { Placed }
consumes bynk { Events, Logger, LogLevel }

event Placed = { n: String }

service place {
  on call(n: String) -> Effect[String] by Caller given Events, Logger {
    let _ <- Logger.log(Info, "orders", Map.empty().insert("n", n))
    let _ <- Events.emit[Placed](Placed { n: n })
    Effect.pure(n)
  }
}
"#;

const SOURCE_AUDIT: &str = r#"context tp.audit

consumes tp.orders
consumes bynk { Logger, LogLevel }

service OnPlaced from Events(Placed) {
  on event(e: Placed) -> Effect[()] given Logger {
    Logger.log(Info, "audit", Map.empty().insert("n", e.n))
  }
}
"#;

fn tool_exists(name: &str) -> bool {
    which::which(name).is_ok()
}

fn skip(reason: &str) -> bool {
    eprintln!("\n!!! TRACE PROPAGATION (WORKERD) SKIPPED !!!\n{reason}\n");
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

/// One `GET` with `extra` request headers: the status.
fn get(port: u16, path: &str, extra: &[(&str, &str)]) -> Result<u16, String> {
    let addr = format!("127.0.0.1:{port}");
    let mut stream = std::net::TcpStream::connect_timeout(
        &addr.parse().map_err(|e| format!("{e}"))?,
        Duration::from_secs(2),
    )
    .map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .map_err(|e| e.to_string())?;
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    for (name, value) in extra {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| e.to_string())?;
    response
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("no status line: {:?}", response.lines().next()))
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| format!("<could not read {path:?}: {e}>"))
}

/// The trace a `Logger.log` record for request `n` carries in `log`: the
/// 32-hex id after `n: '<n>'`, in the record's `fields`.
fn trace_of(log: &str, n: &str) -> Option<String> {
    let at = log.find(&format!("n: '{n}'"))?;
    let rest = &log[at..];
    let t = rest.find("trace: '")? + "trace: '".len();
    let id = rest.get(t..t + 32)?;
    id.chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        .then(|| id.to_string())
}

/// `trace_of`, retried until `deadline`: the event reaches the subscriber after
/// the response, through a Durable Object.
fn await_trace(log: &Path, n: &str, deadline: Duration) -> Option<String> {
    let start = Instant::now();
    loop {
        if let Some(t) = trace_of(&read_log(log), n) {
            return Some(t);
        }
        if start.elapsed() > deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[test]
fn one_request_leaves_one_trace_in_every_context_it_reaches() {
    if !tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    if !tool_exists("node") && skip("`node` is not on PATH") {
        return;
    }
    let tmp = std::env::temp_dir().join(format!("bynk-trace-propagation-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let src = tmp.join("src/tp");
    fs::create_dir_all(&src).unwrap();
    fs::write(tmp.join("bynk.toml"), MANIFEST).unwrap();
    fs::write(src.join("web.bynk"), SOURCE_WEB).unwrap();
    fs::write(src.join("orders.bynk"), SOURCE_ORDERS).unwrap();
    fs::write(src.join("audit.bynk"), SOURCE_AUDIT).unwrap();

    let options = bynk_driver::try_project_options(&tmp)
        .expect("the project reads")
        .target(BuildTarget::Workers);
    let out = match bynk_driver::project::compile_project(&options) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the trace project:\n{}",
            bynk_driver::render::render_project_errors(&failure.flatten())
        ),
    };
    let out = bynk_driver::strip::strip_project_to_js(out).expect("the project strips to JS");
    bynk_driver::write_output(&out, &tmp).unwrap();

    let toml = read_log(&tmp.join("workers/tp-web/wrangler.toml"));
    assert!(
        toml.contains("\"nodejs_als\""),
        "every Worker carries the flag that gives it a request scope:\n{toml}"
    );

    let ports = wrangler::ports::free_ports::<6>();
    let [
        web_port,
        orders_port,
        audit_port,
        web_inspector,
        orders_inspector,
        audit_inspector,
    ] = ports;
    let logs = ["web", "orders", "audit"].map(|n| tmp.join(format!("{n}-wrangler.log")));
    let spawn = |name: &str, port: u16, inspector: u16, log: &Path| {
        let out_log = fs::File::create(log)?;
        let err_log = out_log.try_clone()?;
        wrangler::Wrangler::spawn(
            wrangler::npx()
                .args(["dev"])
                .args(wrangler::local_edge_vars(&[
                    ("tp.web", "tp.orders"),
                    ("tp.orders", "tp.audit"),
                ]))
                .args([
                    "--port",
                    &port.to_string(),
                    "--inspector-port",
                    &inspector.to_string(),
                ])
                .current_dir(tmp.join(format!("workers/tp-{name}")))
                .stdout(Stdio::from(out_log))
                .stderr(Stdio::from(err_log)),
            &tmp.join("registry"),
        )
    };
    let started = (
        spawn("web", web_port, web_inspector, &logs[0]),
        spawn("orders", orders_port, orders_inspector, &logs[1]),
        spawn("audit", audit_port, audit_inspector, &logs[2]),
    );
    let _wranglers = match started {
        (Ok(a), Ok(b), Ok(c)) => Wranglers(vec![a, b, c]),
        (a, b, c) => {
            drop(Wranglers(
                a.into_iter().chain(b).chain(c).collect::<Vec<_>>(),
            ));
            if skip("wrangler did not start (likely no network to provision it)") {
                return;
            }
            unreachable!()
        }
    };

    // The first request answers once all three Workers are up and the Service
    // Bindings have connected, so it is the readiness probe.
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Ok(200) = get(web_port, "/order/probe", &[]) {
            break;
        }
        if Instant::now() > deadline {
            if skip("the Workers did not answer (likely no network to provision wrangler)") {
                return;
            }
            unreachable!()
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    // An event emitted before the orders Worker's Service Binding to the audit
    // Worker has connected is not retried, so the probe is sent until the audit
    // Worker has logged one.
    let deadline = Instant::now() + Duration::from_secs(120);
    while trace_of(&read_log(&logs[2]), "probe").is_none() {
        if Instant::now() > deadline {
            panic!(
                "the audit Worker never received an event\n{}",
                logs.iter()
                    .map(|l| format!("--- {l:?} ---\n{}", read_log(l)))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        let _ = get(web_port, "/order/probe", &[]);
        std::thread::sleep(Duration::from_millis(1000));
    }

    let all_logs = || {
        logs.iter()
            .map(|l| format!("--- {l:?} ---\n{}", read_log(l)))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let settle = Duration::from_secs(30);

    // One request: the same trace in all three.
    assert_eq!(get(web_port, "/order/a1", &[]), Ok(200));
    let a: Vec<Option<String>> = logs.iter().map(|l| await_trace(l, "a1", settle)).collect();
    let a: Vec<String> = a
        .into_iter()
        .map(|t| t.unwrap_or_else(|| panic!("a context logged no trace for a1\n{}", all_logs())))
        .collect();
    assert!(
        a.iter().all(|t| *t == a[0]),
        "the three contexts logged different traces for one request: {a:?}\n{}",
        all_logs()
    );

    // Another request: another trace.
    assert_eq!(get(web_port, "/order/b2", &[]), Ok(200));
    let b = await_trace(&logs[2], "b2", settle)
        .unwrap_or_else(|| panic!("no trace for b2\n{}", all_logs()));
    assert_ne!(b, a[0], "two requests share a trace");

    // A client's `traceparent` on a public request is not read: the trace is
    // the entry's own, and it is the same in every context.
    let client = "4bf92f3577b34da6a3ce929d0e0e4736";
    let header = format!("00-{client}-00f067aa0ba902b7-01");
    assert_eq!(
        get(web_port, "/order/c3", &[("traceparent", &header)]),
        Ok(200)
    );
    let c: Vec<Option<String>> = logs.iter().map(|l| await_trace(l, "c3", settle)).collect();
    let c: Vec<String> = c
        .into_iter()
        .map(|t| t.unwrap_or_else(|| panic!("a context logged no trace for c3\n{}", all_logs())))
        .collect();
    assert!(c.iter().all(|t| *t == c[0]), "{c:?}\n{}", all_logs());
    assert_ne!(c[0], client, "a public request's traceparent was believed");
}
