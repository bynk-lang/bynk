//! #3430 — the `SignedCookie` log-in, reuse and log-out loop, end to end on a real
//! `workerd`, with real `fetch`-style requests and no test-language involvement.
//!
//! The `system` tier's driver mints a valid cookie for `by User("ada")` and probes the seam
//! with `Nobody`, `Forged` and `Expired`, so the issue path (`User.issue`) and the verify
//! path are each covered, but a `system` case cannot present the cookie `/login` actually
//! set (#3386 Q7 chose no cookie jar, and the credential cookie is refused as a hand-set
//! header). So this test drives the Worker from outside the Bynk test language, and carries
//! the cookie from one response to the next request exactly as a browser would:
//!
//! 1. `/login` sets the `__Host-session` cookie with its attributes: `HttpOnly`, `Secure`,
//!    `Path=/`, no `Domain`, and a `Max-Age` equal to the actor's `ttl`;
//! 2. that exact cookie, sent back to `/me`, authenticates as the same identity;
//! 3. `/logout` sets the clearing cookie, under the same name and attributes;
//! 4. the cleared cookie, and no cookie at all, are refused with `401`;
//! 5. the cookie from step 1 is refused by a Worker holding a different key (a rotation with
//!    no `previous`), which is the same key and `kid` the issuer signed with being the only
//!    thing that made step 2 pass.
//!
//! Same toolchain-skip and `BYNK_REQUIRE_WORKERD` gate as the other workerd tests; teardown
//! goes through [`wrangler::Wrangler`].

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

/// Two distinct 32-byte keys, base64url: the Worker's key, and the one it is rotated to.
const KEY_A: &str = bynk_driver::dev_run::TEST_SESSION_KEY;
const KEY_B: &str = "b3RoZXItc2Vzc2lvbi1rZXktMDEyMzQ1Njc4OTAxMjM";

const SOURCE: &str = r#"context shop.api

consumes bynk { Cookie, Clock }

type UserId = String where NonEmpty && MaxLength(64)

type Login = { user: String }

actor User { auth = SignedCookie(secret = "SESSION_KEY", ttl = 12.hours), identity = UserId }

service api from http {
  on POST("/login") (body: Login) -> Effect[HttpResult[String]] by v: Visitor given Clock {
    let Ok(id) = UserId.of(body.user) else {
      BadRequest("bad user")
    }
    let c <- User.issue(id)
    Created("welcome").withCookie(c)
  }

  on GET("/me") () -> Effect[HttpResult[String]] by u: User {
    Ok(u.identity)
  }

  on POST("/logout") () -> Effect[HttpResult[()]] by u: User {
    NoContent.withCookie(User.expire())
  }

  on GET("/ready") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("up")
  }
}
"#;

/// What one request got back.
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    /// Every `Set-Cookie` header's value, in order.
    fn set_cookies(&self) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
            .map(|(_, v)| v.as_str())
            .collect()
    }
}

/// Parse a whole `Connection: close` HTTP/1.1 response, decoding a chunked body.
fn parse_reply(raw: &[u8]) -> Result<Reply, String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| format!("no header end in {} bytes", raw.len()))?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut lines = head.lines();
    let status: u16 = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("no status in {head:?}"))?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let rest = &raw[split + 4..];
    let chunked = headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("transfer-encoding") && v.contains("chunked"));
    let mut body = Vec::new();
    if chunked {
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
    } else {
        body.extend_from_slice(rest);
    }
    Ok(Reply {
        status,
        headers,
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

/// One request. `cookie` is the `Cookie` header, as a browser would send it; `body` is a
/// JSON body. A browser's own page sends `Sec-Fetch-Site: same-origin`, which the seam's
/// CSRF check needs on a state-changing request.
fn request(
    port: u16,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: Option<&str>,
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
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nSec-Fetch-Site: same-origin\r\n"
    );
    if let Some(cookie) = cookie {
        req.push_str(&format!("Cookie: {cookie}\r\n"));
    }
    match body {
        Some(b) => req.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        )),
        None => req.push_str("\r\n"),
    }
    stream
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    let read = stream.read_to_end(&mut raw).err().map(|e| e.to_string());
    parse_reply(&raw).map_err(|e| match read {
        Some(read) => format!("{e} (reading the response failed: {read})"),
        None => e,
    })
}

/// A port nothing is listening on: bind port 0, read what the OS chose, release it.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("the OS hands out a free port")
        .port()
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
    eprintln!("\n!!! SIGNED COOKIE LOOP (WORKERD) SKIPPED !!!\n{reason}\n");
    if require::is_required(REQUIRE_ENV) {
        panic!("{REQUIRE_ENV} is set but {reason}");
    }
    true
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| format!("<could not read {path:?}: {e}>"))
}

/// Removes the test's scratch directory when it goes out of scope, panic or early return
/// included. Declared before the [`Wranglers`], so the Workers stop first.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Wranglers(Vec<wrangler::Wrangler>);

impl Drop for Wranglers {
    fn drop(&mut self) {
        wrangler::stop_all(&mut self.0);
    }
}

/// Start `wrangler dev` in `dir` with `SESSION_KEY` set to `key`. `Err` is the reason it
/// could not be launched.
fn spawn_worker(
    dir: &Path,
    tmp: &Path,
    name: &str,
    key: &str,
) -> Result<(wrangler::Wrangler, u16, std::path::PathBuf), String> {
    let (port, inspector_port) = (free_port(), free_port());
    let log_path = tmp.join(format!("wrangler-{name}.log"));
    let out_log = fs::File::create(&log_path).map_err(|e| e.to_string())?;
    let err_log = out_log.try_clone().map_err(|e| e.to_string())?;
    let child = wrangler::Wrangler::spawn(
        base_command("npx")
            .args(["-y", WRANGLER, "dev"])
            .args([
                "--port",
                &port.to_string(),
                "--inspector-port",
                &inspector_port.to_string(),
                "--persist-to",
                &tmp.join(format!("state-{name}")).to_string_lossy(),
                "--var",
                &format!("SESSION_KEY:{key}"),
            ])
            .current_dir(dir)
            .stdout(Stdio::from(out_log))
            .stderr(Stdio::from(err_log)),
        &tmp.join(format!("registry-{name}")),
    )
    .map_err(|e| format!("could not launch npx: {e}"))?;
    Ok((child, port, log_path))
}

/// Wait for the Worker on `port` to answer `/ready`; `Err` carries the log.
fn wait_ready(port: u16, log_path: &Path) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if request(port, "GET", "/ready", None, None).is_ok_and(|r| r.status == 200) {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(read_log(log_path));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// The `name=value` pair at the head of a `Set-Cookie`, as a browser would send it back.
fn cookie_pair(set_cookie: &str) -> &str {
    set_cookie.split(';').next().unwrap_or("").trim()
}

/// A `Set-Cookie`'s attributes after the name=value pair, lower-cased.
fn attributes(set_cookie: &str) -> Vec<String> {
    set_cookie
        .split(';')
        .skip(1)
        .map(|a| a.trim().to_ascii_lowercase())
        .collect()
}

#[test]
fn the_cookie_login_sets_authenticates_until_logout_on_workerd() {
    if !tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    if !tool_exists("node") && skip("`node` is not on PATH") {
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-signed-cookie-loop-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let _scratch = Scratch(tmp.clone());
    let proj = tmp.join("proj/shop");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("api.bynk"), SOURCE).unwrap();
    fs::write(
        tmp.join("proj/bynk.toml"),
        "[project]\nname = \"signed-cookie-loop\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let out = match bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(tmp.join("proj")).target(BuildTarget::Workers),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the signed-cookie project to workers:\n{}",
            bynk_driver::render::render_project_errors(&failure.flatten())
        ),
    };
    let out = bynk_driver::strip::strip_project_to_js(out).expect("the project strips to JS");
    bynk_driver::write_output(&out, &tmp).unwrap();

    let dir = tmp.join("workers/shop-api");
    assert!(
        dir.join("index.js").is_file() && dir.join("wrangler.toml").is_file(),
        "expected workers/shop-api — context dasherisation changed?"
    );

    // Two Workers from the same build: one with the key that signs, and one holding a
    // different key (a rotation that did not keep the old one as `previous`).
    let mut wranglers = Wranglers(Vec::new());
    let mut boot = |name: &str, key: &str| match spawn_worker(&dir, &tmp, name, key) {
        Ok((child, port, log)) => {
            wranglers.0.push(child);
            Some((port, log))
        }
        Err(reason) => {
            if skip(&reason) {
                return None;
            }
            unreachable!()
        }
    };
    let Some((port, log_path)) = boot("a", KEY_A) else {
        return;
    };
    let Some((rotated_port, rotated_log)) = boot("b", KEY_B) else {
        return;
    };
    for (p, l) in [(port, &log_path), (rotated_port, &rotated_log)] {
        if let Err(log) = wait_ready(p, l) {
            drop(wranglers);
            if skip(&format!(
                "wrangler dev did not serve within the boot window (likely no network to \
                 provision {WRANGLER})\n{log}"
            )) {
                return;
            }
            unreachable!()
        }
    }
    let log = || {
        std::thread::sleep(Duration::from_millis(1000)); // let wrangler flush the Worker's console
        read_log(&log_path)
    };

    // 1. Log in: the Worker sets the session cookie with the safe attributes.
    let r = request(port, "POST", "/login", None, Some(r#"{"user":"ada"}"#))
        .unwrap_or_else(|e| panic!("/login: {e}\n{}", log()));
    assert_eq!(r.status, 201, "/login\n{}", log());
    let cookies = r.set_cookies();
    assert_eq!(cookies.len(), 1, "one Set-Cookie: {:?}", r.headers);
    let issued = cookies[0];
    assert!(
        issued.starts_with("__Host-session="),
        "the cookie is the `__Host-` session cookie: {issued}"
    );
    let value = issued
        .split(';')
        .next()
        .and_then(|p| p.split_once('='))
        .map(|(_, v)| v)
        .unwrap_or("");
    assert!(
        !value.is_empty(),
        "the cookie carries a credential: {issued}"
    );
    let attrs = attributes(issued);
    for want in ["httponly", "secure", "path=/", "max-age=43200"] {
        assert!(attrs.iter().any(|a| a == want), "`{want}` in {issued}");
    }
    assert!(
        !attrs.iter().any(|a| a.starts_with("domain")),
        "a `__Host-` cookie has no Domain: {issued}"
    );
    let session = cookie_pair(issued).to_string();

    // 2. Reuse: the cookie, sent back as the browser would, authenticates as `ada`.
    let r = request(port, "GET", "/me", Some(&session), None)
        .unwrap_or_else(|e| panic!("/me: {e}\n{}", log()));
    assert_eq!(r.status, 200, "/me with the issued cookie\n{}", log());
    assert_eq!(
        r.body.trim_matches('"'),
        "ada",
        "the issued cookie authenticates as the identity it was issued for"
    );

    // 3. Log out: the Worker clears the cookie under the same name and attributes.
    let r = request(port, "POST", "/logout", Some(&session), None)
        .unwrap_or_else(|e| panic!("/logout: {e}\n{}", log()));
    assert_eq!(r.status, 204, "/logout\n{}", log());
    let cookies = r.set_cookies();
    assert_eq!(cookies.len(), 1, "one clearing Set-Cookie: {:?}", r.headers);
    let cleared = cookies[0];
    assert!(
        cleared.starts_with("__Host-session=;"),
        "an empty value under the same name: {cleared}"
    );
    let attrs = attributes(cleared);
    for want in ["httponly", "secure", "path=/", "max-age=0"] {
        assert!(attrs.iter().any(|a| a == want), "`{want}` in {cleared}");
    }

    // 4. The cleared cookie, and no cookie, are refused. (This is the browser dropping its
    //    cookie, not server-side revocation: a stateless signed cookie, `session` above, still
    //    verifies until its `ttl` runs out, which is why the `ttl` is required and kept short.)
    let r = request(port, "GET", "/me", Some(cookie_pair(cleared)), None)
        .unwrap_or_else(|e| panic!("/me (cleared): {e}\n{}", log()));
    assert_eq!(r.status, 401, "the cleared cookie is refused\n{}", log());
    let r = request(port, "GET", "/me", None, None)
        .unwrap_or_else(|e| panic!("/me (none): {e}\n{}", log()));
    assert_eq!(r.status, 401, "no cookie is refused\n{}", log());

    // 5. A Worker holding a different key refuses the cookie the first one issued, so what
    //    made step 2 pass is the key (and `kid`) the issuer signed with. First a positive
    //    control: that Worker does authenticate a cookie it issued itself, so its refusal below
    //    is the key mismatch and not a Worker that refuses everything.
    let rotated_log_text = || read_log(&rotated_log);
    let r = request(
        rotated_port,
        "POST",
        "/login",
        None,
        Some(r#"{"user":"ada"}"#),
    )
    .unwrap_or_else(|e| panic!("/login (rotated): {e}\n{}", rotated_log_text()));
    assert_eq!(r.status, 201, "/login (rotated)\n{}", rotated_log_text());
    let own = cookie_pair(r.set_cookies().first().copied().unwrap_or("")).to_string();
    assert!(
        own.starts_with("__Host-session="),
        "the rotated Worker issues its own cookie: {:?}",
        r.headers
    );
    assert_ne!(own, session, "a different key signs a different credential");
    let r = request(rotated_port, "GET", "/me", Some(&own), None)
        .unwrap_or_else(|e| panic!("/me (rotated, own cookie): {e}\n{}", rotated_log_text()));
    assert_eq!(
        (r.status, r.body.trim_matches('"')),
        (200, "ada"),
        "the rotated Worker authenticates a cookie it issued\n{}",
        rotated_log_text()
    );
    let r = request(rotated_port, "GET", "/me", Some(&session), None)
        .unwrap_or_else(|e| panic!("/me (rotated): {e}\n{}", read_log(&rotated_log)));
    assert_eq!(
        r.status,
        401,
        "a cookie signed under another key is refused\n{}",
        read_log(&rotated_log)
    );

    drop(wranglers);
}
