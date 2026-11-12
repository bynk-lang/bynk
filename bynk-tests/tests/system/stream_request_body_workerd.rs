//! #3380 — a `@raw` handler streams its request body, end to end on a real
//! `workerd`: the platform behaviour the proposal made a condition of building
//! (and ADR "stream-request-body-into-raw-handler" records).
//!
//! One Worker with streamed routes, driven by a dependency-free HTTP/1.1
//! client that sends `Transfer-Encoding: chunked` bodies, so no `Content-Length`
//! is there to fast-reject and the router's counted read is what runs:
//!
//! - `/sum` reads a body of 1 MiB sent in 16 KiB chunks and answers how many bytes
//!   it read;
//! - `/capped` has a 100 000 byte cap; a body sent past it, slowly, is cut off with
//!   the route's `413 PayloadTooLarge`, which a client that is still sending reads
//!   (a fast sender may see its connection reset instead, which is the platform's
//!   and is not asserted);
//! - `/to-r2/…` hands the request stream to `R2.putStream`: a body that fits is stored,
//!   and one the cap cuts off mid-stream faults the write (nothing stored) and
//!   answers `413`;
//! - `/ignore` answers without reading its body, and the Worker serves the next
//!   request normally;
//!
//! A fifth check, a response streamed from the request body while it is still
//! arriving (an echo), is deliberately not here: it works on Miniflare driven
//! directly, but under `wrangler dev`'s proxy it fails ("Network connection
//! lost", and an uncaught "This ReadableStream is currently locked to a reader"
//! from the dev proxy), so this smoke cannot tell the platform from the proxy. The
//! docs tell a handler to read its body before it answers.
//!
//! Same toolchain-skip and `BYNK_REQUIRE_WORKERD` gate as the other workerd tests;
//! teardown goes through [`wrangler::Wrangler`].

use bynk_driver::project::BuildTarget;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[path = "../require/mod.rs"]
mod require;
#[path = "../wrangler/mod.rs"]
mod wrangler;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";
const WRANGLER: &str = wrangler::SPEC;

fn tool_exists(name: &str) -> bool {
    which::which(name).is_ok()
}

/// Route through `cmd /C` on Windows so npm's `npx.cmd` shim resolves
/// (Rust's CreateProcess refuses batch scripts directly — BatBadBut).
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
    eprintln!("\n!!! STREAMED REQUEST BODY (WORKERD) SKIPPED !!!\n{reason}\n");
    if require::is_required(REQUIRE_ENV) {
        panic!("{REQUIRE_ENV} is set but {reason}");
    }
    true
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| format!("<could not read {path:?}: {e}>"))
}

struct Wranglers(Vec<wrangler::Wrangler>);

impl Drop for Wranglers {
    fn drop(&mut self) {
        wrangler::stop_all(&mut self.0);
    }
}

const SOURCE: &str = r#"context up.smoke

consumes bynk.cloudflare { R2 }

capability Archives = R2

service api from http {
  limits { maxBody: 4_000_000 }

  @raw(contentType: "*/*")
  on PUT("/sum") (body: Stream[Bytes]) -> Effect[HttpResult[String]] by v: Visitor {
    let chunks <- body.collect()
    let total = chunks.fold(0, (acc, b) => acc + b.length())
    Ok("\(total)")
  }

  @raw(contentType: "*/*")
  @limit(maxBody: 100_000)
  on PUT("/capped") (body: Stream[Bytes]) -> Effect[HttpResult[String]] by v: Visitor {
    let chunks <- body.collect()
    Ok("read \(chunks.length())")
  }

  @raw(contentType: "*/*")
  on PUT("/ignore") (body: Stream[Bytes]) -> Effect[HttpResult[String]] by v: Visitor {
    Ok("ignored")
  }

  // The request stream goes straight into R2. The declared size is 300 000 and the
  // route's cap is 100 000, so the counted read cuts the upload off mid-stream and the
  // write must fault whole: a 413 for the client, and nothing stored.
  @raw(contentType: "*/*")
  @limit(maxBody: 100_000)
  on PUT("/to-r2/overrun/:key") (key: String, body: Stream[Bytes]) -> Effect[HttpResult[String]] by v: Visitor given Archives {
    let o <- Archives.putStream(key, body, 300_000)
    Ok("stored \(o.size)")
  }

  // The same, with a body that fits.
  @raw(contentType: "*/*")
  @limit(maxBody: 100_000)
  on PUT("/to-r2/fits/:key") (key: String, body: Stream[Bytes]) -> Effect[HttpResult[String]] by v: Visitor given Archives {
    let o <- Archives.putStream(key, body, 50_000)
    Ok("stored \(o.size)")
  }

  on GET("/r2-has/:key") (key: String) -> Effect[HttpResult[String]] by v: Visitor given Archives {
    let found <- Archives.head(key)
    match found {
      Some(_) => Ok("yes")
      None => Ok("no")
    }
  }

  on GET("/ready") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }
}
"#;

/// What one request got back: the status, and the (de-chunked) body.
struct Reply {
    status: u16,
    body: Vec<u8>,
}

/// Parse a whole `Connection: close` HTTP/1.1 response, decoding a chunked body.
fn parse_reply(raw: &[u8]) -> Result<Reply, String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| format!("no header end in {} bytes", raw.len()))?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let status: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("no status in {head:?}"))?;
    let rest = &raw[split + 4..];
    let chunked = head
        .lines()
        .any(|l| l.to_ascii_lowercase().starts_with("transfer-encoding:") && l.contains("chunked"));
    if !chunked {
        return Ok(Reply {
            status,
            body: rest.to_vec(),
        });
    }
    let mut body = Vec::new();
    let mut at = 0;
    loop {
        let line_end = rest[at..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or("truncated chunk size")?;
        let size = usize::from_str_radix(
            String::from_utf8_lossy(&rest[at..at + line_end])
                .split(';')
                .next()
                .unwrap_or("")
                .trim(),
            16,
        )
        .map_err(|e| e.to_string())?;
        at += line_end + 2;
        if size == 0 {
            break;
        }
        body.extend_from_slice(rest.get(at..at + size).ok_or("truncated chunk")?);
        at += size + 2;
    }
    Ok(Reply { status, body })
}

/// One request. `chunks` is the body, sent chunked with `delay` between chunks
/// (none for a `GET`). A reader thread collects the response while the body is
/// still being sent, so a response that comes early is read, and a write that
/// fails because the server has answered and closed is not itself a failure.
fn request(
    port: u16,
    method: &str,
    path: &str,
    chunks: Option<(&[Vec<u8>], Duration)>,
) -> Result<Reply, String> {
    let addr = format!("127.0.0.1:{port}");
    let mut stream = TcpStream::connect_timeout(
        &addr.parse().map_err(|e| format!("{e}"))?,
        Duration::from_secs(2),
    )
    .map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if chunks.is_some() {
        head.push_str("Content-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\n");
    }
    head.push_str("\r\n");
    let mut reader = stream.try_clone().map_err(|e| e.to_string())?;
    let collector = std::thread::spawn(move || {
        let mut all = Vec::new();
        let _ = reader.read_to_end(&mut all);
        all
    });
    let mut write_all = |bytes: &[u8]| stream.write_all(bytes).is_ok();
    if write_all(head.as_bytes())
        && let Some((chunks, delay)) = chunks
    {
        let mut open = true;
        for chunk in chunks {
            let mut framed = format!("{:x}\r\n", chunk.len()).into_bytes();
            framed.extend_from_slice(chunk);
            framed.extend_from_slice(b"\r\n");
            if !write_all(&framed) {
                open = false;
                break;
            }
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
        }
        if open {
            write_all(b"0\r\n\r\n");
        }
    }
    let raw = collector
        .join()
        .map_err(|_| "reader panicked".to_string())?;
    parse_reply(&raw)
}

fn chunks_of(total: usize, each: usize) -> Vec<Vec<u8>> {
    (0..total.div_ceil(each))
        .map(|i| vec![(i % 251) as u8; each.min(total - i * each)])
        .collect()
}

#[test]
fn a_streamed_raw_body_is_counted_unread_and_echoed_on_workerd() {
    if !tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    if !tool_exists("node") && skip("`node` is not on PATH") {
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-stream-body-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj/up");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("smoke.bynk"), SOURCE).unwrap();
    fs::write(
        tmp.join("proj/bynk.toml"),
        "[project]\nname = \"stream-request-body\"\nversion = \"0.1.0\"\n\n[unstable]\nallow = [\"bynk.cloudflare.R2\"]\n",
    )
    .unwrap();

    let out = match bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(tmp.join("proj")).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the streamed-body project to workers:\n{}",
            bynk_driver::render::render_project_errors(&failure.flatten())
        ),
    };
    let out = bynk_driver::strip::strip_project_to_js(out).expect("the project strips to JS");
    bynk_driver::write_output(&out, &tmp).unwrap();

    let dir = tmp.join("workers/up-smoke");
    assert!(
        dir.join("index.js").is_file() && dir.join("wrangler.toml").is_file(),
        "expected workers/up-smoke — context dasherisation changed?"
    );

    // Its own port band, clear of the other workerd tests': one port for the
    // Worker and one for its V8 inspector.
    let port = 9_000 + 2 * (std::process::id() % 500) as u16;
    let inspector_port = port + 1;
    let log_path = tmp.join("wrangler.log");
    let out_log = fs::File::create(&log_path).unwrap();
    let err_log = out_log.try_clone().unwrap();
    let child = wrangler::Wrangler::spawn(
        base_command("npx")
            .args(["-y", WRANGLER, "dev"])
            .args([
                "--port",
                &port.to_string(),
                "--inspector-port",
                &inspector_port.to_string(),
            ])
            .current_dir(&dir)
            .stdout(Stdio::from(out_log))
            .stderr(Stdio::from(err_log)),
        &tmp.join("registry"),
    );
    let child = match child {
        Ok(c) => c,
        Err(_) => {
            if skip("could not launch npx") {
                return;
            }
            unreachable!()
        }
    };
    let wrangler = Wranglers(vec![child]);

    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if request(port, "GET", "/ready", None).is_ok_and(|r| r.status == 200) {
            break;
        }
        if Instant::now() > deadline {
            let log = read_log(&log_path);
            drop(wrangler);
            let _ = fs::remove_dir_all(&tmp);
            if skip(&format!(
                "wrangler dev did not serve within the boot window (likely no network to \
                 provision {WRANGLER})\n{log}"
            )) {
                return;
            }
            unreachable!()
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let log = || {
        std::thread::sleep(Duration::from_millis(1000)); // let wrangler flush the Worker's console
        read_log(&log_path)
    };

    // 1. A 1 MiB body, sent chunked, is read whole by the handler: the count is of
    //    the bytes, not of how the transport split them.
    let mib = chunks_of(1 << 20, 16 * 1024);
    let r = request(port, "PUT", "/sum", Some((&mib, Duration::ZERO))).unwrap_or_else(|e| {
        panic!("/sum: {e}\n{}", log());
    });
    assert_eq!(r.status, 200, "/sum\n{}", log());
    assert_eq!(
        String::from_utf8_lossy(&r.body).trim_matches('"'),
        (1usize << 20).to_string(),
        "the handler read every byte of the streamed body\n{}",
        log()
    );

    // 2. A body past the route's cap, sent slowly and chunked (so there is no
    //    `Content-Length` to refuse it by), is cut off by the counted read and
    //    answered `413 PayloadTooLarge`.
    let over = chunks_of(300_000, 8 * 1024);
    let r = request(
        port,
        "PUT",
        "/capped",
        Some((&over, Duration::from_millis(5))),
    )
    .unwrap_or_else(|e| panic!("/capped: {e}\n{}", log()));
    assert_eq!(r.status, 413, "/capped\n{}", log());
    assert!(
        String::from_utf8_lossy(&r.body).contains("PayloadTooLarge"),
        "the 413 names PayloadTooLarge: {:?}",
        String::from_utf8_lossy(&r.body)
    );
    //    ... and a body within the cap goes through the same route.
    let within = chunks_of(50_000, 8 * 1024);
    let r = request(port, "PUT", "/capped", Some((&within, Duration::ZERO)))
        .unwrap_or_else(|e| panic!("/capped (within): {e}\n{}", log()));
    assert_eq!(r.status, 200, "/capped within the cap\n{}", log());

    // 3. A handler that answers without reading its body leaves the Worker
    //    healthy: the next request is served.
    let some = chunks_of(32 * 1024, 8 * 1024);
    let r = request(port, "PUT", "/ignore", Some((&some, Duration::ZERO)))
        .unwrap_or_else(|e| panic!("/ignore: {e}\n{}", log()));
    assert_eq!(r.status, 200, "/ignore\n{}", log());
    assert!(
        request(port, "GET", "/ready", None).is_ok_and(|r| r.status == 200),
        "the Worker serves a request after one that left its body unread\n{}",
        log()
    );

    // 4. The request stream goes into `R2.putStream`. A body that fits is stored; one
    //    cut off mid-stream by the cap faults the write, which stores nothing, and
    //    the client gets the route's `413`, not a `500`.
    let fits = chunks_of(50_000, 8 * 1024);
    let r = request(port, "PUT", "/to-r2/fits/a", Some((&fits, Duration::ZERO)))
        .unwrap_or_else(|e| panic!("/to-r2/fits: {e}\n{}", log()));
    assert_eq!(r.status, 200, "/to-r2/fits\n{}", log());
    assert!(
        String::from_utf8_lossy(&r.body).contains("stored 50000"),
        "the object is stored whole: {:?}\n{}",
        String::from_utf8_lossy(&r.body),
        log()
    );
    let over = chunks_of(300_000, 8 * 1024);
    let r = request(
        port,
        "PUT",
        "/to-r2/overrun/b",
        Some((&over, Duration::from_millis(5))),
    )
    .unwrap_or_else(|e| panic!("/to-r2/overrun: {e}\n{}", log()));
    assert_eq!(
        r.status,
        413,
        "an overrun through R2.putStream answers 413, not 500\n{}",
        log()
    );
    let has = |key: &str| {
        request(port, "GET", &format!("/r2-has/{key}"), None).map(|r| {
            String::from_utf8_lossy(&r.body)
                .trim_matches('"')
                .to_string()
        })
    };
    assert_eq!(
        has("a").as_deref(),
        Ok("yes"),
        "the fitting upload is stored"
    );
    assert_eq!(
        has("b").as_deref(),
        Ok("no"),
        "the overrun stored nothing\n{}",
        log()
    );

    drop(wrangler);
    let _ = fs::remove_dir_all(&tmp);
}
