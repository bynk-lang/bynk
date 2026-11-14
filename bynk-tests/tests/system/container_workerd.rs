//! #3477 (slice C3): a `Container` against a real `workerd`, with and without
//! Docker.
//!
//! Two smokes of the generated Durable Object class under `wrangler dev`, the
//! way `bynk dev` starts it:
//!
//! - **Containers off** (`containers_off_fault_naming_docker_on_workerd`).
//!   `bynk dev` without Docker starts the Worker with
//!   `--enable-containers=false` and the `BYNK_CONTAINERS_UNAVAILABLE` variable
//!   set. The Worker must still serve, and a call to the container must fault
//!   with a message that names Docker. This needs no Docker, so it runs on any
//!   machine that can run the other workerd smokes.
//! - **A real container** (`a_real_container_serves_send_on_workerd`). Builds a
//!   tiny image from a Dockerfile and drives `Container.send` through the class
//!   and `wrangler dev`: the start, the wait for the port, the proxy, one
//!   instance per name. It needs a running Docker daemon and a network to pull
//!   the base image, so it skips without them, and `BYNK_REQUIRE_DOCKER` turns
//!   that skip into a failure (as `BYNK_REQUIRE_POSTGRES` does for Postgres).
//!   It is a platform contract test, not a language tier. Not run where Docker
//!   is unavailable.
//!
//! The same toolchain skip and `BYNK_REQUIRE_WORKERD` gate as the other workerd
//! smokes; teardown is the shared [`wrangler::Wrangler`]'s process-group stop.

#[path = "../require/mod.rs"]
mod require;
#[path = "../tools/mod.rs"]
mod tools;
#[path = "../wrangler/mod.rs"]
mod wrangler;

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const REQUIRE_WORKERD: &str = "BYNK_REQUIRE_WORKERD";
const REQUIRE_DOCKER: &str = "BYNK_REQUIRE_DOCKER";

const SOURCE: &str = "context box.smoke

consumes bynk
consumes bynk.cloudflare { Container }

capability Echo = Container @build(\"containers/echo\") @port(8080) @sleepAfter(1.minutes)

service api from http {
\ton GET(\"/boot\") () -> Effect[HttpResult[String]] by Visitor {
\t\tOk(\"up\")
\t}

\ton GET(\"/ping/:name\") (name: String) -> Effect[HttpResult[String]] by Visitor given Echo {
\t\tlet req = Request {
\t\t\tmethod: Get,
\t\t\turl: \"http://echo/\",
\t\t\tcontentType: None,
\t\t\tauthorization: None,
\t\t\theaders: Map.empty(),
\t\t\tbody: None,
\t\t}
\t\tlet res <- Echo.send(name, req)
\t\tmatch res {
\t\t\tOk(r) => Ok(r.body)
\t\t\tErr(_) => ServerError(\"container error\")
\t\t}
\t}
}
";

/// The container's server: it answers its start time, so one instance is
/// recognisable across calls.
const SERVER: &str = "const started = String(Date.now());
require(\"http\").createServer((_, res) => res.end(started)).listen(8080);
";

const DOCKERFILE: &str = "FROM --platform=linux/amd64 node:22-alpine
COPY server.js /server.js
CMD [\"node\", \"/server.js\"]
";

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .expect("a free local port")
}

/// A skip that `var` turns into a failure.
fn skip(var: &str, reason: &str) -> bool {
    if require::is_required(var) {
        panic!("{var} is set but {reason}");
    }
    eprintln!("\n!!! CONTAINER (WORKERD) SKIPPED !!!\n{reason}\n");
    true
}

/// `GET path` on `port`, as `(status, body)`, waiting up to `wait` for an
/// answer (a container's first start is slow).
fn get(port: u16, path: &str, wait: Duration) -> Result<(u16, String), String> {
    let addr = format!("127.0.0.1:{port}");
    let mut stream = std::net::TcpStream::connect_timeout(
        &addr.parse().map_err(|e| format!("{e}"))?,
        Duration::from_secs(2),
    )
    .map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(wait))
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
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("unparseable response: {response:?}"))?;
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Ok((status, body))
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| format!("<could not read {path:?}: {e}>"))
}

/// Whether a Docker daemon answers.
fn docker_running() -> bool {
    tools::tool_exists("docker")
        && Command::new("docker")
            .arg("info")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

/// Build the container project, serve it with `wrangler dev` (`extra` after
/// `dev`), and hand the port to `drive` once `/boot` answers. The failures it
/// reports and the wrangler log, or `None` when skipped.
fn run(
    project: &str,
    extra: &[&str],
    drive: &dyn Fn(u16) -> Vec<String>,
) -> Option<(Vec<String>, String)> {
    if !tools::tool_exists("npx") && skip(REQUIRE_WORKERD, "`npx` is not on PATH") {
        return None;
    }
    if !tools::tool_exists("node") && skip(REQUIRE_WORKERD, "`node` is not on PATH") {
        return None;
    }
    let tmp = std::env::temp_dir().join(format!(
        "bynk-container-workerd-{project}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join("src/box")).unwrap();
    fs::create_dir_all(proj.join("containers/echo")).unwrap();
    fs::write(
        proj.join("bynk.toml"),
        format!("[project]\nname = \"{project}\"\nversion = \"0.1.0\"\n\n[unstable]\nallow = [\"bynk.cloudflare.Container\"]\n"),
    )
    .unwrap();
    fs::write(proj.join("src/box/smoke.bynk"), SOURCE).unwrap();
    fs::write(proj.join("containers/echo/server.js"), SERVER).unwrap();
    fs::write(proj.join("containers/echo/Dockerfile"), DOCKERFILE).unwrap();
    // The output goes inside the project: the container's build paths are
    // written from the output directory back to the project.
    let out = proj.join("out");
    let compiled = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(["compile", "--target", "workers", "--emit", "js", "-o"])
        .arg(&out)
        .arg(&proj)
        .output()
        .expect("run bynk");
    assert!(
        compiled.status.success(),
        "the {project} smoke does not compile:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let dir = out.join("workers/box-smoke");
    assert!(
        dir.join("index.js").is_file() && dir.join("wrangler.toml").is_file(),
        "expected out/workers/box-smoke"
    );
    let (port, inspector_port) = (free_port(), free_port());
    let log_path = tmp.join("wrangler.log");
    let out_log = fs::File::create(&log_path).unwrap();
    let err_log = out_log.try_clone().unwrap();
    let spawned = wrangler::Wrangler::spawn(
        tools::base_command("npx")
            .args(["-y", wrangler::SPEC, "dev", "--port", &port.to_string()])
            .args(["--inspector-port", &inspector_port.to_string()])
            .args(extra)
            .current_dir(&dir)
            .stdout(Stdio::from(out_log))
            .stderr(Stdio::from(err_log)),
        &tmp.join("registry"),
    );
    let Ok(server) = spawned else {
        skip(REQUIRE_WORKERD, "could not launch npx");
        return None;
    };
    let deadline = Instant::now() + Duration::from_secs(180);
    while get(port, "/boot", Duration::from_secs(20)).is_err() {
        if Instant::now() > deadline {
            let log = read_log(&log_path);
            drop(server);
            skip(
                REQUIRE_WORKERD,
                &format!(
                    "wrangler dev did not serve within the boot window\n--- wrangler log ---\n{log}"
                ),
            );
            return None;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let failures = drive(port);
    // The fault's detail is logged by the Worker, after the response is sent.
    std::thread::sleep(Duration::from_secs(3));
    let log = read_log(&log_path);
    drop(server);
    let _ = fs::remove_dir_all(&tmp);
    Some((failures, log))
}

#[test]
fn containers_off_fault_naming_docker_on_workerd() {
    // What `bynk dev` does without Docker: the flag that lets wrangler start
    // with no Docker (`ctx.container` is then absent) and the variable that
    // makes the generated class fault naming it. The Worker still serves; a call
    // to the container faults, and the message names Docker.
    let var = format!("{}:docker", bynk_emit::CONTAINERS_UNAVAILABLE_VAR);
    let Some((failures, log)) = run(
        "c-off",
        &["--enable-containers=false", "--var", &var],
        &|port| {
            let mut failures = Vec::new();
            match get(port, "/boot", Duration::from_secs(20)) {
                Ok((200, _)) => {}
                other => failures.push(format!("/boot: {other:?}")),
            }
            match get(port, "/ping/a", Duration::from_secs(30)) {
                Ok((500, _)) => {}
                other => failures.push(format!("/ping/a: expected a fault (500), got {other:?}")),
            }
            failures
        },
    ) else {
        return;
    };
    assert!(
        failures.is_empty(),
        "{}\n--- wrangler log ---\n{log}",
        failures.join("\n")
    );
    assert!(
        log.contains("Docker is not available"),
        "the fault does not name Docker\n--- wrangler log ---\n{log}"
    );
    assert!(
        !log.contains("DOCKER_HOST")
            && !log.to_lowercase().contains("is the docker daemon running"),
        "wrangler tried to reach Docker though containers were off\n--- wrangler log ---\n{log}"
    );
}

#[test]
fn a_real_container_serves_send_on_workerd() {
    if !docker_running() && skip(REQUIRE_DOCKER, "no Docker daemon is running") {
        return;
    }
    let Some((failures, log)) = run("c-real", &[], &|port| {
        let mut failures = Vec::new();
        // The first call starts the container and waits for its port: slow.
        let first = get(port, "/ping/a", Duration::from_secs(300));
        let Ok((200, started)) = &first else {
            return vec![format!(
                "/ping/a: expected the container's answer, got {first:?}"
            )];
        };
        if started.trim().trim_matches('"').parse::<u64>().is_err() {
            failures.push(format!("/ping/a: not a start time: {started:?}"));
        }
        // The same name reaches the same instance.
        match get(port, "/ping/a", Duration::from_secs(60)) {
            Ok((200, again)) if again.trim() == started.trim() => {}
            other => failures.push(format!(
                "/ping/a again: expected {started:?}, got {other:?}"
            )),
        }
        // Another name is another instance, with its own start time.
        match get(port, "/ping/b", Duration::from_secs(300)) {
            Ok((200, other)) if other.trim() != started.trim() => {}
            other => failures.push(format!(
                "/ping/b: expected a different instance, got {other:?}"
            )),
        }
        failures
    }) else {
        return;
    };
    assert!(
        failures.is_empty(),
        "{}\n--- wrangler log ---\n{log}",
        failures.join("\n")
    );
}
