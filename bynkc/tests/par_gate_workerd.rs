//! DECISION H of the `par` block (concurrent effects, slice 0, #2640): a join in
//! an agent handler adds no interleaving that sequential awaits do not already
//! allow, on a real `workerd`.
//!
//! The track's Q4 assumes that a Durable Object's input gate treats `par { … }`
//! like two sequential `<-`s, apart from removing the synchronous gap between
//! the two lines. That is a claim about the platform, so it is checked here.
//!
//! One agent handler reads a `store` field, makes two subrequests to a server
//! this test runs and holds, then writes the field back. It is written twice:
//! `seqWork` awaits the subrequests one after the other, `parWork` joins them
//! in a `par` block. Two requests to the same agent are driven through each:
//!
//! - **overlap:** the second request is sent while the first is waiting on its
//!   subrequests, then every held subrequest is released;
//! - **serial:** the second is sent only after the first has finished.
//!
//! Each run's outcome is what both requests read, and the field's final value.
//! The claim holds when the `par` outcome of every schedule is one the
//! sequential version produces for some schedule. The test also prints the
//! order the subrequests reached the server in, which differs between the two
//! forms by construction (`par` issues both of a request's subrequests at once)
//! and is not part of the claim.
//!
//! Skips loudly without the toolchain; a non-empty `BYNK_REQUIRE_WORKERD`
//! (Linux CI sets `1`) turns the skip into a failure. The `wrangler dev` is
//! stopped, with its `workerd`, when the guard drops (#1686).

use bynkc::BuildTarget;
use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod require;
mod wrangler;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";
const WRANGLER: &str = wrangler::SPEC;

fn source(held_port: u16) -> String {
    let req = |step: &str| {
        format!(
            "Request {{ method: Get, url: \"http://127.0.0.1:{held_port}/\\(tag)/{step}\", contentType: None, authorization: None, headers: Map.empty(), body: None }}"
        )
    };
    format!(
        r#"context gate

consumes bynk {{ Fetch }}

agent Room {{
  key id: String

  store hits: Cell[Int] = 0

  on call seqWork(tag: String) -> Effect[Int] given Fetch {{
    let seen = hits
    let a <- Fetch.send({a})
    let b <- Fetch.send({b})
    hits := seen + 1
    Effect.pure(seen)
  }}

  on call parWork(tag: String) -> Effect[Int] given Fetch {{
    let seen = hits
    par {{
      let a <- Fetch.send({a})
      let b <- Fetch.send({b})
    }}
    hits := seen + 1
    Effect.pure(seen)
  }}

  on call peek() -> Effect[Int] {{
    Effect.pure(hits)
  }}
}}

service api from http {{
  on GET("/seq/:room/:tag") (room: String, tag: String) -> Effect[HttpResult[String]] by v: Visitor {{
    let n <- Room(room).seqWork(tag)
    Ok("\(n)")
  }}

  on GET("/par/:room/:tag") (room: String, tag: String) -> Effect[HttpResult[String]] by v: Visitor {{
    let n <- Room(room).parWork(tag)
    Ok("\(n)")
  }}

  on GET("/peek/:room") (room: String) -> Effect[HttpResult[String]] by v: Visitor {{
    let n <- Room(room).peek()
    Ok("\(n)")
  }}
}}
"#,
        a = req("a"),
        b = req("b"),
    )
}

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
    eprintln!("\n!!! PAR GATE (WORKERD) SKIPPED !!!\n{reason}\n");
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

/// The server the agent's subrequests go to. It answers none of them until
/// told to, and records the order they arrive in as `<tag>/<step>`.
struct Held {
    arrived: Arc<Mutex<Vec<String>>>,
    waiting: Arc<Mutex<Vec<TcpStream>>>,
}

impl Held {
    fn start(listener: TcpListener) -> Held {
        let arrived = Arc::new(Mutex::new(Vec::new()));
        let waiting = Arc::new(Mutex::new(Vec::new()));
        let (a, w) = (arrived.clone(), waiting.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let (a, w) = (a.clone(), w.clone());
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(conn.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        return;
                    }
                    // `GET /<tag>/<step> HTTP/1.1`
                    let path = line.split_whitespace().nth(1).unwrap_or("/");
                    a.lock()
                        .unwrap()
                        .push(path.trim_start_matches('/').to_string());
                    w.lock().unwrap().push(conn);
                });
            }
        });
        Held { arrived, waiting }
    }

    fn arrivals(&self) -> Vec<String> {
        self.arrived.lock().unwrap().clone()
    }

    /// Wait until `label` has arrived. `false` on timeout.
    fn wait_for(&self, label: &str, within: Duration) -> bool {
        let end = Instant::now() + within;
        while Instant::now() < end {
            if self.arrivals().iter().any(|l| l == label) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// Answer every subrequest now waiting.
    fn release_all(&self) {
        for mut conn in self.waiting.lock().unwrap().drain(..) {
            let _ = conn
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok");
        }
    }
}

/// What a schedule produced: what each request read, and the final value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Outcome {
    first: String,
    second: String,
    last: String,
}

/// Drive two requests at one agent through `mode` (`seq` or `par`). With
/// `overlap`, the second is sent while the first waits on its subrequests;
/// otherwise only after the first is done. Returns the outcome and the order
/// the subrequests arrived in.
fn run(port: u16, held: &Held, mode: &str, overlap: bool, room: &str) -> (Outcome, Vec<String>) {
    let start_len = held.arrivals().len();
    let spawn_request = |tag: &str| {
        let path = format!("/{mode}/{room}/{tag}");
        std::thread::spawn(move || get(port, &path))
    };
    let first = spawn_request("r1");
    assert!(
        held.wait_for("r1/a", Duration::from_secs(30)),
        "the first request's subrequest never arrived"
    );
    let second = if overlap {
        let second = spawn_request("r2");
        // Whether the second request starts while the first waits is part of
        // what is being compared, so a miss here is an outcome, not a failure.
        let _ = held.wait_for("r2/a", Duration::from_secs(3));
        // Release until both are done.
        let end = Instant::now() + Duration::from_secs(30);
        while !(first.is_finished() && second.is_finished()) && Instant::now() < end {
            held.release_all();
            std::thread::sleep(Duration::from_millis(50));
        }
        second
    } else {
        let end = Instant::now() + Duration::from_secs(30);
        while !first.is_finished() && Instant::now() < end {
            held.release_all();
            std::thread::sleep(Duration::from_millis(50));
        }
        let second = spawn_request("r2");
        assert!(
            held.wait_for("r2/a", Duration::from_secs(30)),
            "the second request's subrequest never arrived"
        );
        while !second.is_finished() && Instant::now() < end {
            held.release_all();
            std::thread::sleep(Duration::from_millis(50));
        }
        second
    };
    let body = |r: Result<Result<(u16, String), String>, _>| match r {
        Ok(Ok((200, b))) => b,
        other => format!("<{other:?}>"),
    };
    let first = body(first.join());
    let second = body(second.join());
    let last = match get(port, &format!("/peek/{room}")) {
        Ok((200, b)) => b,
        other => format!("<{other:?}>"),
    };
    let order = held.arrivals()[start_len..].to_vec();
    (
        Outcome {
            first,
            second,
            last,
        },
        order,
    )
}

#[test]
fn a_par_join_in_an_agent_handler_adds_no_interleaving() {
    if !tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    if !tool_exists("node") && skip("`node` is not on PATH") {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let held_port = listener.local_addr().unwrap().port();

    let tmp = std::env::temp_dir().join(format!("bynk-par-gate-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("gate.bynk"), source(held_port)).unwrap();
    let out = match bynkc::compile_project(
        &bynk_testkit::compile_options_single(&proj).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the par gate project to workers:\n{}",
            bynkc::render_project_errors(&failure.flatten())
        ),
    };
    let out = bynkc::strip_project_to_js(out).expect("the par gate project strips to JS");
    bynkc::write_output(&out, &tmp).unwrap();
    let dir = tmp.join("workers/gate");
    assert!(
        dir.join("index.js").is_file() && dir.join("wrangler.toml").is_file(),
        "expected {dir:?}"
    );

    // A port band clear of the other workerd tests' (20_000 and up; see the notes in
    // `callee_faulted_workerd.rs`).
    let base = 10_000 + 2 * (std::process::id() % 4_000) as u16;
    let (port, inspector) = (base, base + 1);
    let log_path = tmp.join("wrangler.log");
    let spawned = (|| -> std::io::Result<wrangler::Wrangler> {
        let out_log = fs::File::create(&log_path)?;
        let err_log = out_log.try_clone()?;
        wrangler::Wrangler::spawn(
            base_command("npx")
                .args(["-y", WRANGLER, "dev"])
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

    // `peek` of a fresh room answers once the Worker is up.
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Ok((200, body)) = get(port, "/peek/boot") {
            assert_eq!(body, "\"0\"", "a fresh agent has not been hit");
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

    let held = Held::start(listener);
    let mut seq_outcomes = BTreeSet::new();
    let mut report = String::new();
    let record = |report: &mut String,
                  mode: &str,
                  overlap: bool,
                  outcome: &Outcome,
                  order: &[String]| {
        report.push_str(&format!(
            "{mode:>3} {:>7}: first read {}, second read {}, final {}; subrequests reached the server as {order:?}\n",
            if overlap { "overlap" } else { "serial" },
            outcome.first,
            outcome.second,
            outcome.last,
        ));
    };
    for (overlap, room) in [(true, "seq-overlap"), (false, "seq-serial")] {
        let (outcome, order) = run(port, &held, "seq", overlap, room);
        record(&mut report, "seq", overlap, &outcome, &order);
        seq_outcomes.insert(outcome);
    }
    // Whether the second request starts while the first waits is the platform's
    // call, and could differ between two runs. So a `par` outcome the sequential
    // runs so far did not produce earns the sequential form more runs, before it
    // counts as a finding.
    const ATTEMPTS: usize = 3;
    for (overlap, room) in [(false, "par-serial"), (true, "par-overlap")] {
        for attempt in 1..=ATTEMPTS {
            let (outcome, order) = run(port, &held, "par", overlap, &format!("{room}-{attempt}"));
            record(&mut report, "par", overlap, &outcome, &order);
            if seq_outcomes.contains(&outcome) {
                break;
            }
            assert!(
                attempt < ATTEMPTS,
                "a `par` join produced an outcome the sequential form cannot: {outcome:?} \
                 (sequential forms produced {seq_outcomes:?})\n{report}\n--- worker log ---\n{}",
                log()
            );
            let (more, order) = run(
                port,
                &held,
                "seq",
                overlap,
                &format!("seq-{room}-{attempt}"),
            );
            record(&mut report, "seq", overlap, &more, &order);
            seq_outcomes.insert(more);
        }
    }
    eprintln!("par gate (workerd):\n{report}");
    drop(guard);
    let _ = fs::remove_dir_all(&tmp);
}
