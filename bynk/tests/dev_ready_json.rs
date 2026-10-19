//! #2138 (end-to-end testing track §3.5, DECISION F): `bynk dev --ready-json`
//! against a real local `wrangler dev` (workerd, local mode; nothing reaches
//! Cloudflare). Starts `bynk dev` on a one-context project, waits for the
//! readiness file, checks it names the context's URL and that the URL answers,
//! then stops `bynk dev` with `SIGTERM` to its own pid and checks the file is
//! gone.
//!
//! Skipped when `node`, or both `wrangler` and `npx`, are unavailable; a non-empty
//! `BYNK_REQUIRE_WORKERD` turns the skip into a failure, as for the other
//! workerd smokes. `bynk dev` runs in its own process group, and the guard
//! stops that whole group on drop, so a failing assertion never strands a
//! `workerd`; nothing else on the machine is signalled.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn skip(reason: &str) -> bool {
    eprintln!("skipping: {reason}");
    if std::env::var("BYNK_REQUIRE_WORKERD").is_ok_and(|v| !v.is_empty()) {
        panic!("BYNK_REQUIRE_WORKERD is set but {reason}");
    }
    true
}

/// Two distinct localhost ports, free when asked.
fn free_ports() -> (u16, u16) {
    let bind = || std::net::TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
    let (a, b) = (bind(), bind());
    let port = |l: &std::net::TcpListener| l.local_addr().expect("local addr").port();
    (port(&a), port(&b))
}

/// Stops `bynk dev`'s whole process group on drop: `SIGTERM`, then `SIGKILL`
/// for anything still there. The group is the one this test created.
struct Group(Child);

impl Group {
    fn pid(&self) -> libc::pid_t {
        libc::pid_t::try_from(self.0.id()).expect("a pid fits pid_t")
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        let pid = self.pid();
        // SAFETY: `kill` takes plain integers; a negative pid names the
        // process group this test created for `bynk dev`.
        unsafe { libc::kill(-pid, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            let _ = self.0.try_wait();
            // SAFETY: as above; signal 0 only checks the group still exists.
            if unsafe { libc::kill(-pid, 0) } != 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // SAFETY: as above.
        unsafe { libc::kill(-pid, libc::SIGKILL) };
        let _ = self.0.wait();
    }
}

/// One `GET` to the URL the readiness file names, `http://localhost:<port>`.
/// `localhost` may resolve to `::1` and `127.0.0.1`, and wrangler listens on
/// whichever its platform picks (`::1` on macOS), so each address is tried, as
/// `bynk dev`'s own readiness probe does. A fixed `127.0.0.1` failed there.
fn get(port: u16, path: &str) -> Option<String> {
    use std::net::ToSocketAddrs;
    let mut s = ("localhost", port)
        .to_socket_addrs()
        .ok()?
        .find_map(|addr| {
            std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(5)).ok()
        })?;
    s.set_read_timeout(Some(Duration::from_secs(30))).ok()?;
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    (!out.is_empty()).then_some(out)
}

/// [`get`] until it answers or `within` passes: the readiness file means the
/// Worker answered a probe, but a slow CI runner can still drop the next
/// connection while workerd settles.
fn get_within(port: u16, path: &str, within: Duration) -> Option<String> {
    let deadline = Instant::now() + within;
    loop {
        if let Some(reply) = get(port, path) {
            return Some(reply);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[test]
fn ready_json_names_each_context_once_it_answers_and_is_removed_on_stop() {
    // `bynk dev` resolves wrangler itself: a local or global install, or `npx`
    // (as CI does).
    if (!have("node") || !(have("wrangler") || have("npx")))
        && skip("`node`, and `wrangler` or `npx`, are not available")
    {
        return;
    }
    let dir = std::env::temp_dir().join(format!("bynk-ready-json-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("bynk.toml"), "[project]\nname = \"ready\"\n").unwrap();
    std::fs::write(
        dir.join("src").join("svc.bynk"),
        "context svc\n\nservice api from http {\n\ton GET(\"/hello\") () -> Effect[HttpResult[String]] by Visitor {\n\t\tOk(\"hi\")\n\t}\n}\n",
    )
    .unwrap();
    let ready: PathBuf = dir.join("ready.json");
    let (port, inspector) = free_ports();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynk"));
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut dev = Group(
        cmd.arg("dev")
            .arg(&dir)
            .arg("--base-port")
            .arg(port.to_string())
            .arg("--inspect-port")
            .arg(inspector.to_string())
            .arg("--ready-json")
            .arg(&ready)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn bynk dev"),
    );

    let deadline = Instant::now() + Duration::from_secs(180);
    while !ready.exists() {
        if let Ok(Some(status)) = dev.0.try_wait() {
            panic!("bynk dev exited before it was ready: {status}");
        }
        assert!(Instant::now() < deadline, "no readiness file within 180s");
        std::thread::sleep(Duration::from_millis(250));
    }
    let urls: BTreeMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(&ready).unwrap()).expect("JSON object");
    assert_eq!(
        urls,
        BTreeMap::from([("svc".to_string(), format!("http://localhost:{port}"))])
    );
    let reply = get_within(port, "/hello", Duration::from_secs(30))
        .expect("the ready context answers on the URL the file names");
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");

    // Stop `bynk dev` itself (its pid, not the group), so its own shutdown
    // path is what removes the file.
    // SAFETY: `kill` takes plain integers; the pid is the child spawned above.
    unsafe { libc::kill(dev.pid(), libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if dev.0.try_wait().ok().flatten().is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "bynk dev did not stop within 30s"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!ready.exists(), "the readiness file is removed on stop");
    drop(dev);
    let _ = std::fs::remove_dir_all(&dir);
}
