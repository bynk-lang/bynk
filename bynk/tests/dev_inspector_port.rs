//! #3237: `bynk dev`'s inspector port for a single-context project.
//!
//! `wrangler dev` always starts an inspector, on 9229 unless told otherwise, so
//! two sessions starting together both claimed 9229 and one died with `Address
//! already in use`; and an explicit `--inspect-port` was ignored for one
//! context. These run real `wrangler dev` sessions (local mode; nothing reaches
//! Cloudflare), skipped when `node`, or both `wrangler` and `npx`, are missing;
//! a non-empty `BYNK_REQUIRE_WORKERD` turns the skip into a failure.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[path = "support/dev_child.rs"]
mod dev_child;

use dev_child::Group;

fn skip(reason: &str) -> bool {
    eprintln!("skipping: {reason}");
    if std::env::var("BYNK_REQUIRE_WORKERD").is_ok_and(|v| !v.is_empty()) {
        panic!("BYNK_REQUIRE_WORKERD is set but {reason}");
    }
    true
}

/// Whether the tools `bynk dev` needs are missing (and so the test skips).
fn unavailable() -> bool {
    (!dev_child::have("node") || !(dev_child::have("wrangler") || dev_child::have("npx")))
        && skip("`node`, and `wrangler` or `npx`, are not available")
}

/// A free localhost port, as the OS reports it right now.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind an ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

/// A one-context project under a fresh temp dir; returns the project root.
fn project(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bynk-inspector-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("bynk.toml"), "[project]\nname = \"inspector\"\n").unwrap();
    std::fs::write(
        dir.join("src").join("svc.bynk"),
        "context svc\n\nservice api from http {\n\ton GET(\"/hello\") () -> Effect[HttpResult[String]] by Visitor {\n\t\tOk(\"hi\")\n\t}\n}\n",
    )
    .unwrap();
    dir
}

/// One running `bynk dev` on `proj` with `extra` flags: its group guard, its
/// readiness file and its output log.
struct Session {
    dev: Group,
    ready: PathBuf,
    log: PathBuf,
}

fn start(proj: &Path, extra: &[String]) -> Session {
    let ready = proj.join("ready.json");
    let log = proj.join("bynk-dev.log");
    let (out, err) = dev_child::log_to(&log);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynk"));
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let dev = Group(
        cmd.arg("dev")
            .arg(proj)
            .arg("--base-port")
            .arg(free_port().to_string())
            .arg("--ready-json")
            .arg(&ready)
            .args(extra)
            .stdout(out)
            .stderr(err)
            .spawn()
            .expect("spawn bynk dev"),
    );
    Session { dev, ready, log }
}

/// Wait for every session's readiness file, failing with the session's own
/// output if one exits first.
fn wait_ready(sessions: &mut [Session]) {
    let deadline = Instant::now() + Duration::from_secs(180);
    while sessions.iter().any(|s| !s.ready.exists()) {
        for s in sessions.iter_mut() {
            if !s.ready.exists()
                && let Ok(Some(status)) = s.dev.0.try_wait()
            {
                panic!("{}", dev_child::exited_early(status, &s.log));
            }
        }
        assert!(Instant::now() < deadline, "no readiness file within 180s");
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn listening(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_secs(5),
    )
    .is_ok()
}

#[test]
fn sessions_started_together_each_get_their_own_inspector() {
    if unavailable() {
        return;
    }
    // Neither session is given an inspector flag: each used to take 9229.
    let (a, b, c) = (project("a"), project("b"), project("c"));
    let mut sessions = [start(&a, &[]), start(&b, &[]), start(&c, &[])];
    wait_ready(&mut sessions);
    drop(sessions);
    for dir in [a, b, c] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn an_explicit_inspect_port_reaches_wrangler_for_a_single_context() {
    if unavailable() {
        return;
    }
    let port = free_port();
    let proj = project("explicit");
    let mut sessions = [start(&proj, &["--inspect-port".into(), port.to_string()])];
    wait_ready(&mut sessions);
    assert!(
        listening(port),
        "nothing is listening on the --inspect-port {port}\n{}",
        std::fs::read_to_string(&sessions[0].log).unwrap_or_default()
    );
    drop(sessions);
    let _ = std::fs::remove_dir_all(proj);
}

#[test]
fn a_taken_inspector_port_is_reported_by_bynk_not_wrangler() {
    if unavailable() {
        return;
    }
    let held = std::net::TcpListener::bind("127.0.0.1:0").expect("hold a port");
    let port = held.local_addr().unwrap().port();
    let proj = project("taken");
    let out = Command::new(env!("CARGO_BIN_EXE_bynk"))
        .arg("dev")
        .arg(&proj)
        .arg("--inspect-port")
        .arg(port.to_string())
        .stdin(Stdio::null())
        .output()
        .expect("run bynk dev");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "bynk dev should refuse\n{stderr}");
    assert!(
        stderr.contains(&format!("inspector port {port} is already in use")),
        "expected bynk's own message, got:\n{stderr}"
    );
    drop(held);
    let _ = std::fs::remove_dir_all(proj);
}
