//! #4483: a record field renamed with `replaces` inside a stored value, on a real `workerd`.
//!
//! One scratch project, built twice as a deploy sees it: version 1 declares
//! `type Prefs = { width: Int }`, stored in a `Cell` and in a `Map` value; version 2 declares
//! `size: Int replaces width`. Version 2 builds with no `migrate from` step (the lock's path
//! comparison reads the rename), and the Durable Object storage version 1 wrote, kept on the same
//! persisted state across the restart, reads back through the new name. The lock's side is also
//! pinned without workerd in `field_rename_lock.rs`.
//!
//! The same toolchain skip and `BYNK_REQUIRE_WORKERD` gate as the other workerd tests; see
//! `idempotency_workerd.rs`.

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

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";

/// `Prefs` with `field`, in a `Cell` and a `Map` value, behind a plain HTTP service.
fn source(field: &str, value: &str) -> String {
    format!(
        "context shop\n\ntype Prefs = {{ {field} }}\n\nagent Cart {{\n\tkey id: String\n\n\tstore prefs: Cell[Prefs] = Prefs {{ {value} }}\n\tstore named: Map[String, Prefs]\n\n\ton call seed() -> Effect[()] {{\n\t\tprefs := Prefs {{ {value} }}\n\t\tnamed.put(\"a\", Prefs {{ {value} }})\n\t}}\n\n\ton call cell() -> Effect[Prefs] {{\n\t\tprefs\n\t}}\n\n\ton call mapped() -> Effect[Option[Prefs]] {{\n\t\tnamed.get(\"a\")\n\t}}\n}}\n\nservice api from http {{\n\ton GET(\"/seed/:id\") (id: String) -> Effect[HttpResult[String]] by Visitor {{\n\t\tlet _ <- Cart(id).seed()\n\t\tOk(\"seeded\")\n\t}}\n\n\ton GET(\"/cell/:id\") (id: String) -> Effect[HttpResult[Prefs]] by Visitor {{\n\t\tlet p <- Cart(id).cell()\n\t\tOk(p)\n\t}}\n\n\ton GET(\"/mapped/:id\") (id: String) -> Effect[HttpResult[Option[Prefs]]] by Visitor {{\n\t\tlet p <- Cart(id).mapped()\n\t\tOk(p)\n\t}}\n}}\n"
    )
}

fn skip(reason: &str) -> bool {
    if require::is_required(REQUIRE_ENV) {
        panic!("{REQUIRE_ENV} is set but {reason}");
    }
    eprintln!("\n!!! FIELD RENAME (WORKERD) SKIPPED !!!\n{reason}\n");
    true
}

/// `GET path` on `port`, as `(status, body)`.
fn get(port: u16, path: &str) -> Result<(u16, String), String> {
    let addr = format!("127.0.0.1:{port}");
    let mut stream = std::net::TcpStream::connect_timeout(
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
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("unparseable response: {response:?}"))?;
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.trim().to_string())
        .unwrap_or_default();
    Ok((status, body))
}

/// `bynk compile --target workers`, which also reads and writes the project's `bynk.schema.lock`.
fn compile(proj: &Path, out: &Path) -> Result<(), String> {
    let run = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(["compile", "--target", "workers", "--emit", "js", "-o"])
        .arg(out)
        .arg(proj)
        .output()
        .expect("run bynk");
    if run.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{}{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        ))
    }
}

/// `wrangler dev` in `dir` on the state kept at `state`, once it serves; `None` (after a skip) when
/// it never does.
fn serve(dir: &Path, tmp: &Path, state: &Path, log: &str) -> Option<(wrangler::Wrangler, u16)> {
    let [port, inspector] = wrangler::ports::free_ports();
    let log_path = tmp.join(log);
    let out_log = fs::File::create(&log_path).unwrap();
    let err_log = out_log.try_clone().unwrap();
    let spawned = wrangler::Wrangler::spawn(
        wrangler::npx()
            .args([
                "dev",
                "--port",
                &port.to_string(),
                "--inspector-port",
                &inspector.to_string(),
                "--persist-to",
            ])
            .arg(state)
            .current_dir(dir)
            .stdout(Stdio::from(out_log))
            .stderr(Stdio::from(err_log)),
        &tmp.join("registry"),
    );
    let Ok(server) = spawned else {
        skip("could not launch npx");
        return None;
    };
    let deadline = Instant::now() + Duration::from_secs(180);
    while get(port, "/cell/boot-probe").is_err() {
        if Instant::now() > deadline {
            let said = fs::read_to_string(&log_path).unwrap_or_default();
            drop(server);
            skip(&format!(
                "wrangler dev did not serve within the boot window (likely no network \
                 to provision {})\n--- wrangler log ---\n{said}",
                wrangler::SPEC
            ));
            return None;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Some((server, port))
}

#[test]
fn a_record_field_renamed_in_a_stored_value_reads_back_on_workerd() {
    if !tools::tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    let tmp =
        std::env::temp_dir().join(format!("bynk-field-rename-workerd-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join("src/shop")).unwrap();
    fs::write(
        proj.join("bynk.toml"),
        "[project]\nname = \"rename-smoke\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let state = tmp.join("state");
    let mut failures = Vec::new();

    // Version 1, as deployed: the value lives under `width`.
    fs::write(
        proj.join("src/shop/cart.bynk"),
        source("width: Int", "width: 7"),
    )
    .unwrap();
    if let Err(said) = compile(&proj, &tmp.join("out1")) {
        panic!("version 1 does not compile:\n{said}");
    }
    {
        let dir = tmp.join("out1/workers/shop");
        let Some((server, port)) = serve(&dir, &tmp, &state, "first.log") else {
            let _ = fs::remove_dir_all(&tmp);
            return;
        };
        match get(port, "/seed/ada") {
            Ok((200, _)) => {}
            other => failures.push(format!("seeding version 1: {other:?}")),
        }
        match get(port, "/cell/ada") {
            Ok((200, body)) if body.contains("\"width\"") && body.contains('7') => {}
            other => failures.push(format!("version 1 reads its own value: {other:?}")),
        }
        drop(server);
    }

    // Version 2: renamed with the clause, and built with no `migrate from`.
    fs::write(
        proj.join("src/shop/cart.bynk"),
        source("size: Int replaces width", "size: 0"),
    )
    .unwrap();
    if let Err(said) = compile(&proj, &tmp.join("out2")) {
        panic!("a rename inside a stored record needs no migration, but the build says:\n{said}");
    }
    {
        let dir = tmp.join("out2/workers/shop");
        let Some((server, port)) = serve(&dir, &tmp, &state, "second.log") else {
            let _ = fs::remove_dir_all(&tmp);
            return;
        };
        // What version 1 stored, read through the new name, in the `Cell` and the `Map` value.
        for path in ["/cell/ada", "/mapped/ada"] {
            match get(port, path) {
                Ok((200, body)) if body.contains("\"size\"") && body.contains('7') => {
                    if body.contains("\"width\"") {
                        failures.push(format!("{path} still shows the old name: {body}"));
                    }
                }
                other => failures.push(format!(
                    "after the rename, {path} should read size 7: {other:?}"
                )),
            }
        }
        drop(server);
    }
    let first = fs::read_to_string(tmp.join("first.log")).unwrap_or_default();
    let second = fs::read_to_string(tmp.join("second.log")).unwrap_or_default();
    let _ = fs::remove_dir_all(&tmp);
    assert!(
        failures.is_empty(),
        "a renamed record field inside a stored value on workerd:\n{}\n--- first wrangler log ---\n{first}\n--- second wrangler log ---\n{second}",
        failures.join("\n")
    );
}
