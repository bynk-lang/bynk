//! #3927 (typed HTML, slice 1): a template fills and renders on a real `workerd`.
//!
//! The positive fixture `3927_template_contexts` is built for the Workers target and served with
//! `wrangler dev`. A request whose path parameter is markup gets back the page with it escaped as
//! element text (C1) and in `<title>` (C2), and the fixture's attribute value escaped in both
//! quote styles (C3), with the footer `Html` inserted as is. The unit tier of the same template is
//! the fixture's own suite (`behaviour_fixtures.rs`).
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
use std::time::Duration;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let path = entry.path();
        let dest = to.join(entry.file_name());
        if path.is_dir() {
            if entry.file_name() != "expected" {
                copy_dir(&path, &dest);
            }
        } else {
            fs::copy(&path, &dest).unwrap();
        }
    }
}

fn skip(reason: &str) -> bool {
    if require::is_required(REQUIRE_ENV) {
        panic!("{REQUIRE_ENV} is set but {reason}");
    }
    eprintln!("\n!!! TEMPLATE (WORKERD) SKIPPED !!!\n{reason}\n");
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
    if let Err(why) =
        wrangler::wait_until_ready(&log_path, port, &|| get(port, "/boot-probe").is_ok())
    {
        let said = fs::read_to_string(&log_path).unwrap_or_default();
        drop(server);
        skip(&format!(
            "{why} (likely no network to provision {})\n--- wrangler log ---\n{said}",
            wrangler::SPEC
        ));
        return None;
    }
    Some((server, port))
}

#[test]
fn a_template_fills_with_its_escapes_on_workerd() {
    if !tools::tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    let tmp = std::env::temp_dir().join(format!("bynk-template-workerd-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive/3927_template_contexts"),
        &proj,
    );
    if let Err(said) = compile(&proj, &tmp.join("out")) {
        panic!("the template fixture does not compile for workers:\n{said}");
    }
    let dir = tmp.join("out/workers/t-web");
    let Some((server, port)) = serve(&dir, &tmp, &tmp.join("state"), "wrangler.log") else {
        let _ = fs::remove_dir_all(&tmp);
        return;
    };
    let mut failures = Vec::new();
    match get(port, "/%3Cscript%3Ealert(1)%3C%2Fscript%3E") {
        Ok((200, body)) => {
            for want in [
                "<title>&lt;script&gt;alert(1)&lt;/script&gt;</title>",
                "<h1>&lt;script&gt;alert(1)&lt;/script&gt;</h1>",
                "alt=\"&quot;&#39; onmouseover=x &#39;&quot;\"",
                "title='&quot;&#39; onmouseover=x &#39;&quot;'",
                "<textarea name=\"note\">&lt;/textarea&gt;&lt;script&gt;x&lt;/script&gt;</textarea>",
                "<footer><p>Made with <b>care</b></p></footer>",
            ] {
                if !body.contains(want) {
                    failures.push(format!("the page lacks `{want}`:\n{body}"));
                }
            }
            if body.contains("<script>alert(1)") {
                failures.push(format!("the value reached the page unescaped:\n{body}"));
            }
        }
        other => failures.push(format!("GET the page: {other:?}")),
    }
    drop(server);
    let log = fs::read_to_string(tmp.join("wrangler.log")).unwrap_or_default();
    let _ = fs::remove_dir_all(&tmp);
    assert!(
        failures.is_empty(),
        "a template on workerd:\n{}\n--- wrangler log ---\n{log}",
        failures.join("\n")
    );
}
