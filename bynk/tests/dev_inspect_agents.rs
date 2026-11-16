//! #3581 (the dev agent-inspection track), slice 0b: `bynk dev inspect` lists a
//! running session's agent instances and reads their stored state, end to end on
//! real `wrangler dev` (local mode; nothing reaches Cloudflare). Skipped when
//! `node`, or both `wrangler` and `npx`, are missing; a non-empty
//! `BYNK_REQUIRE_WORKERD` turns the skip into a failure.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[path = "support/dev_child.rs"]
mod dev_child;
#[path = "../../bynk-tests/tests/support/ports.rs"]
mod ports;

use dev_child::Group;

fn skip(reason: &str) -> bool {
    eprintln!("skipping: {reason}");
    if std::env::var("BYNK_REQUIRE_WORKERD").is_ok_and(|v| !v.is_empty()) {
        panic!("BYNK_REQUIRE_WORKERD is set but {reason}");
    }
    true
}

fn unavailable() -> bool {
    (!dev_child::have("node") || !(dev_child::have("wrangler") || dev_child::have("npx")))
        && skip("`node`, and `wrangler` or `npx`, are not available")
}

const SOURCE: &str = r#"context demo.shop

agent Cart {
  key id: String
  store lines: Map[String, Int]
  store total: Cell[Int] = 0

  on call add(sku: String, n: Int) -> Effect[Int] {
    let _ <- lines.put(sku, n)
    let _ <- total.update((t) => t + n)
    Effect.pure(total)
  }
}

// Refuses the commit that would take `taken` past 3.
agent Gate {
  key id: String
  store taken: Cell[Int] = 0

  invariant under_limit: taken < 3

  on call take() -> Effect[Int] {
    let _ <- taken.update((n) => n + 1)
    Effect.pure(taken)
  }
}

service api from http {
  on GET("/gate/:id") (id: String) -> Effect[HttpResult[String]] by v: Visitor {
    let n <- Gate(id).take()
    Ok("\(n)")
  }

  on GET("/add/:id/:sku") (id: String, sku: String) -> Effect[HttpResult[String]] by v: Visitor {
    let t <- Cart(id).add(sku, 2)
    Ok("\(t)")
  }
}
"#;

fn project(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("bynk-inspect-agents-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src").join("demo")).unwrap();
    std::fs::write(dir.join("bynk.toml"), "[project]\nname = \"inspect\"\n").unwrap();
    std::fs::write(dir.join("src").join("demo").join("shop.bynk"), SOURCE).unwrap();
    dir
}

fn bynk(proj: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bynk"))
        .current_dir(proj)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run bynk")
}

fn get(port: u16, path: &str) -> String {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    use std::io::{Read, Write};
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn without_a_session_there_is_nothing_to_inspect() {
    let proj = project("none");
    let out = bynk(&proj, &["dev", "inspect", "agents"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no running `bynk dev` session"), "{err}");
    let _ = std::fs::remove_dir_all(proj);
}

#[test]
fn a_running_session_lists_its_instances_and_reads_their_state() {
    if unavailable() {
        return;
    }
    let proj = project("live");
    let port = ports::free_port();
    let ready = proj.join("ready.json");
    let log = proj.join("bynk-dev.log");
    let (out, err) = dev_child::log_to(&log);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynk"));
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut dev = Group(
        cmd.arg("dev")
            .arg(&proj)
            .arg("--base-port")
            .arg(port.to_string())
            .arg("--ready-json")
            .arg(&ready)
            .stdout(out)
            .stderr(err)
            .spawn()
            .expect("spawn bynk dev"),
    );
    let deadline = Instant::now() + Duration::from_secs(180);
    while !ready.exists() {
        if let Ok(Some(status)) = dev.0.try_wait() {
            panic!("{}", dev_child::exited_early(status, &log));
        }
        assert!(Instant::now() < deadline, "no readiness file within 180s");
        std::thread::sleep(Duration::from_millis(250));
    }

    // Two instances, one of them addressed twice.
    for path in ["/add/u1/apple", "/add/u1/pear", "/add/u2/fig"] {
        let reply = get(port, path);
        assert!(reply.contains("200 OK"), "{path}: {reply}");
    }

    // The registry write is fire-and-forget: wait for both instances to be listed.
    let mut listed = serde_json::Value::Null;
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        let out = bynk(&proj, &["dev", "inspect", "agents", "--class", "Cart"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        listed = serde_json::from_slice(&out.stdout).expect("agents prints JSON");
        if listed["agents"].as_array().is_some_and(|a| a.len() == 2) {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let keys: Vec<&str> = listed["agents"]
        .as_array()
        .expect("agents")
        .iter()
        .map(|a| a["key"].as_str().expect("a string key"))
        .collect();
    assert_eq!(keys.len(), 2, "{listed}");
    assert!(keys.contains(&"u1") && keys.contains(&"u2"), "{keys:?}");
    assert_eq!(listed["agents"][0]["context"], "demo.shop");

    // One instance's state: the cell, and the collection paged.
    let out = bynk(
        &proj,
        &["dev", "inspect", "agent", "Cart", "u1", "--limit", "1"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let state: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(state["readable"], true, "{state}");
    assert_eq!(state["cells"]["total"], 4, "{state}");
    assert_eq!(state["collections"]["lines"]["size"], 2, "{state}");
    assert_eq!(
        state["collections"]["lines"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let next = state["collections"]["lines"]["next"]
        .as_str()
        .expect("a next cursor");
    let out = bynk(
        &proj,
        &[
            "dev", "inspect", "agent", "Cart", "u1", "--field", "lines", "--after", next,
        ],
    );
    let second: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        second["collections"]["lines"]["entries"][0]["key"], "pear",
        "{second}"
    );

    // An instance that was never addressed is reported as such.
    let out = bynk(&proj, &["dev", "inspect", "agent", "Cart", "nobody"]);
    assert!(!out.status.success());

    // The route refuses anything not signed.
    let unsigned = {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        use std::io::{Read, Write};
        write!(
            stream,
            "POST /_bynk/inspect/agents HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut s = String::new();
        stream.read_to_string(&mut s).unwrap();
        s
    };
    assert!(unsigned.starts_with("HTTP/1.1 401"), "{unsigned}");

    drop(dev);
    assert!(
        !proj.join(".bynk").join("dev").join("session.json").exists(),
        "the session file is removed when the session stops"
    );
    let _ = std::fs::remove_dir_all(proj);
}

/// The body of a 200 reply, trimmed; the status line otherwise.
fn gate_body(reply: &str) -> String {
    match reply.split_once("\r\n\r\n") {
        Some((head, body)) if head.contains(" 200 ") => body.trim().to_string(),
        _ => reply.lines().next().unwrap_or_default().to_string(),
    }
}

#[test]
fn a_session_records_each_committed_transition_and_not_a_refused_one() {
    if unavailable() {
        return;
    }
    let proj = project("history");
    let port = ports::free_port();
    let ready = proj.join("ready.json");
    let log = proj.join("bynk-dev.log");
    let (out, err) = dev_child::log_to(&log);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynk"));
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut dev = Group(
        cmd.arg("dev")
            .arg(&proj)
            .args(["--base-port", &port.to_string()])
            .arg("--ready-json")
            .arg(&ready)
            .stdout(out)
            .stderr(err)
            .spawn()
            .expect("spawn bynk dev"),
    );
    let deadline = Instant::now() + Duration::from_secs(180);
    while !ready.exists() {
        if let Ok(Some(status)) = dev.0.try_wait() {
            panic!("{}", dev_child::exited_early(status, &log));
        }
        assert!(Instant::now() < deadline, "no readiness file within 180s");
        std::thread::sleep(Duration::from_millis(250));
    }

    // Two commits go through; the third would take `taken` to 3 and is refused.
    for expected in ["\"1\"", "\"2\""] {
        assert_eq!(gate_body(&get(port, "/gate/g1")), expected);
    }
    let refused = get(port, "/gate/g1");
    assert!(
        !refused.contains(" 200 "),
        "the third take is refused: {refused}"
    );

    // The registry write is fire-and-forget: wait for the instance to be listed.
    let until = Instant::now() + Duration::from_secs(20);
    let state = loop {
        let out = bynk(&proj, &["dev", "inspect", "agent", "Gate", "g1"]);
        if out.status.success() {
            break serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap();
        }
        assert!(
            Instant::now() < until,
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        std::thread::sleep(Duration::from_millis(250));
    };
    assert_eq!(
        state["cells"]["taken"], 2,
        "the refused commit did not persist: {state}"
    );
    let history = state["history"].as_array().expect("history");
    assert_eq!(
        history.len(),
        2,
        "two commits, no entry for the refused one: {state}"
    );
    assert_eq!(history[0]["handler"], "take");
    assert!(
        history[0]["old"].is_null(),
        "the first commit replaced nothing"
    );
    assert_eq!(history[0]["new"]["cells"]["taken"], 1);
    assert_eq!(history[1]["old"]["cells"]["taken"], 1);
    assert_eq!(history[1]["new"]["cells"]["taken"], 2);
    assert!(history[1]["at"].as_u64().unwrap() >= history[0]["at"].as_u64().unwrap());

    drop(dev);
    let _ = std::fs::remove_dir_all(proj);
}
