//! #2878: `bynk token` against a running Worker under `bynk dev` (local
//! `wrangler dev`/workerd; nothing reaches Cloudflare).
//!
//! Serves the `tests/fixtures/token` project with the secret passed the way the
//! guide documents (`bynk dev -- --var NAME:value`), mints tokens with the real
//! `bynk token` from the same secret in the environment, and calls the
//! `Reader` actor's `/me` route: a plain token is `200`, `--expired` and
//! `--not-before 10m` are `401`, as are a token signed with another secret and a
//! request with no token.
//!
//! Skipped when `node`, or both `wrangler` and `npx`, are unavailable; a
//! non-empty `BYNK_REQUIRE_WORKERD` turns the skip into a failure, as for the
//! other workerd smokes. Unix-only, like them: `bynk dev` runs in its own
//! process group, which the guard stops on drop.

#![cfg(unix)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const SECRET: &str = "dev-only-secret";

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

impl Drop for Group {
    fn drop(&mut self) {
        let pid = libc::pid_t::try_from(self.0.id()).expect("a pid fits pid_t");
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

/// One `GET` to `localhost:<port>`, returning the raw reply. Each address
/// `localhost` resolves to is tried (wrangler may listen on `::1` only).
fn get(port: u16, path: &str, bearer: Option<&str>) -> Option<String> {
    use std::net::ToSocketAddrs;
    let mut s = ("localhost", port)
        .to_socket_addrs()
        .ok()?
        .find_map(|addr| {
            std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(5)).ok()
        })?;
    s.set_read_timeout(Some(Duration::from_secs(30))).ok()?;
    let auth = bearer
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\n{auth}Connection: close\r\n\r\n"
    )
    .ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    (!out.is_empty()).then_some(out)
}

/// [`get`] until it answers: the readiness file means the Worker answered a
/// probe, but a slow runner can still drop the next connection.
fn get_within(port: u16, path: &str, bearer: Option<&str>) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(reply) = get(port, path, bearer) {
            return reply;
        }
        assert!(Instant::now() < deadline, "no answer from {path} in 30s");
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Mint with the real `bynk token`, signing with `SECRET` from the environment.
fn mint(bynk: &Path, project: &Path, extra: &[&str]) -> String {
    mint_with(bynk, project, SECRET, extra)
}

fn mint_with(bynk: &Path, project: &Path, secret: &str, extra: &[&str]) -> String {
    let out = Command::new(bynk)
        .args(["token", "Reader", "--sub", "me"])
        .args(extra)
        .arg("--input")
        .arg(project)
        .env("READER_SECRET", secret)
        .output()
        .expect("run bynk token");
    assert!(
        out.status.success(),
        "bynk token {extra:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn status(reply: &str) -> &str {
    reply.lines().next().unwrap_or("")
}

#[test]
fn a_minted_token_is_accepted_and_expired_or_not_yet_valid_ones_are_refused() {
    if (!have("node") || !(have("wrangler") || have("npx")))
        && skip("`node`, and `wrangler` or `npx`, are not available")
    {
        return;
    }
    let bynk = PathBuf::from(env!("CARGO_BIN_EXE_bynk"));
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/token");
    // `bynk dev` writes its output beside the project, so serve a copy.
    let dir = std::env::temp_dir().join(format!("bynk-token-dev-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let proj = dir.join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::copy(fixture.join("bynk.toml"), proj.join("bynk.toml")).unwrap();
    for entry in std::fs::read_dir(fixture.join("src")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), proj.join("src").join(entry.file_name())).unwrap();
    }
    let ready = dir.join("ready.json");
    let (port, inspector) = free_ports();

    let mut cmd = Command::new(&bynk);
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut dev = Group(
        cmd.arg("dev")
            .arg(&proj)
            .arg("--base-port")
            .arg(port.to_string())
            .arg("--inspect-port")
            .arg(inspector.to_string())
            .arg("--ready-json")
            .arg(&ready)
            .arg("--")
            .arg("--var")
            .arg(format!("READER_SECRET:{SECRET}"))
            .env_remove("READER_SECRET")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn bynk dev"),
    );

    let deadline = Instant::now() + Duration::from_secs(180);
    while !ready.exists() {
        if let Ok(Some(st)) = dev.0.try_wait() {
            panic!("bynk dev exited before it was ready: {st}");
        }
        assert!(Instant::now() < deadline, "no readiness file within 180s");
        std::thread::sleep(Duration::from_millis(250));
    }

    let good = mint(&bynk, &proj, &[]);
    let reply = get_within(port, "/me", Some(&good));
    assert!(status(&reply).contains(" 200"), "valid token: {reply}");

    let expired = mint(&bynk, &proj, &["--expired"]);
    let reply = get_within(port, "/me", Some(&expired));
    assert!(status(&reply).contains(" 401"), "--expired: {reply}");

    let early = mint(&bynk, &proj, &["--not-before", "10m"]);
    let reply = get_within(port, "/me", Some(&early));
    assert!(status(&reply).contains(" 401"), "--not-before 10m: {reply}");

    // Signed with another secret: refused, so the 200 above is the Worker
    // verifying the signature, not just checking `exp`/`nbf`.
    let forged = mint_with(&bynk, &proj, "wrong-secret", &[]);
    let reply = get_within(port, "/me", Some(&forged));
    assert!(status(&reply).contains(" 401"), "wrong secret: {reply}");

    let reply = get_within(port, "/me", None);
    assert!(status(&reply).contains(" 401"), "no token: {reply}");

    drop(dev);
    let _ = std::fs::remove_dir_all(&dir);
}
