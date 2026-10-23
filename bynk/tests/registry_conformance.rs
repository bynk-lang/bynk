//! The registry conformance suite (first-party registry track R0, #2311), over
//! the real binaries: `bynk registry serve` on a loopback port must pass every
//! check, and a proxy in front of it that breaks one rule must fail exactly
//! that check. No test touches the network.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bynk"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("registry-conformance")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn bynk(root: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(bin());
    cmd.args(args)
        .current_dir(root)
        .env("BYNK_HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env_remove("BYNK_REGISTRY_CONFORMANCE_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A registry at `root/reg` with a token for `acme` and a narrow one for
/// `globex`: `(full, narrow)`.
fn registry(root: &Path) -> (String, String) {
    let (code, _, err) = bynk(
        root,
        &[
            "registry",
            "init",
            "reg",
            "--id",
            "acme",
            "--key-out",
            "acme.key",
        ],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    let mint = |org: &str| {
        let (code, out, err) = bynk(
            root,
            &["registry", "token", "tokens.toml", "--org", org],
            &[],
        );
        assert_eq!(code, 0, "{err}");
        out.trim().to_string()
    };
    (mint("acme"), mint("globex"))
}

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn serve(root: &Path) -> Server {
    let mut child = Command::new(bin())
        .args([
            "registry",
            "serve",
            "reg",
            "--key",
            "acme.key",
            "--tokens",
            "tokens.toml",
            "--listen",
            "127.0.0.1:0",
        ])
        .current_dir(root)
        .env("BYNK_HOME", root.join("home"))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    let mut port = None;
    for line in lines.by_ref() {
        let line = line.unwrap();
        if let Some(rest) = line.split(" on http://127.0.0.1:").nth(1) {
            port = rest.split_whitespace().next().and_then(|p| p.parse().ok());
            break;
        }
    }
    std::thread::spawn(move || for _ in lines {});
    Server {
        child,
        port: port.expect("the server reported its address"),
    }
}

/// `bynk registry conformance --format json` against `port`: `(code, json)`.
fn conformance(
    root: &Path,
    port: u16,
    full: &str,
    narrow: &str,
    extra: &[&str],
) -> (i32, String, String) {
    let index = format!("http://localhost:{port}");
    let mut args = vec![
        "registry",
        "conformance",
        index.as_str(),
        "--org",
        "acme",
        "--narrow-token-env",
        "NARROW",
        "--format",
        "json",
    ];
    args.extend(extra);
    bynk(
        root,
        &args,
        &[
            ("BYNK_REGISTRY_CONFORMANCE_TOKEN", full),
            ("NARROW", narrow),
        ],
    )
}

/// The outcome the JSON report gives check `id`.
fn outcome(json: &str, id: &str) -> String {
    let at = json
        .find(&format!("\"id\": \"{id}\""))
        .unwrap_or_else(|| panic!("no check `{id}` in {json}"));
    let start = json[..at].rfind('{').unwrap();
    let end = at + json[at..].find('}').unwrap();
    let object = &json[start..end];
    let o = object.find("\"outcome\": \"").unwrap() + "\"outcome\": \"".len();
    object[o..o + object[o..].find('"').unwrap()].to_string()
}

#[test]
fn the_reference_server_passes_every_check() {
    let root = scratch("reference");
    let (full, narrow) = registry(&root);
    let server = serve(&root);
    let port = proxy(server.port, Fault::None);
    let (code, json, err) = conformance(&root, port, &full, &narrow, &[]);
    assert_eq!(code, 0, "{json}{err}");
    for id in [
        "config.shape",
        "config.chain",
        "config.delegations",
        "publish.created",
        "publish.read-back",
        "publish.signer",
        "publish.retry-same-tree",
        "publish.conflict",
        "refuse.other-package",
        "refuse.other-version",
        "refuse.junk",
        "refuse.traversal",
        "refuse.path-dependency",
        "refuse.clean",
        "auth.missing",
        "auth.unknown",
        "auth.scope",
        "limits.too-large",
        "consume.resolve",
        "yank.yank",
        "yank.unyank",
        "yank.missing",
        "errors.shape",
    ] {
        assert_eq!(outcome(&json, id), "pass", "{id}: {json}");
    }
    assert!(json.contains("\"failed\": 0"), "{json}");
    assert!(
        !json.contains(&full) && !err.contains(&full),
        "no token in output"
    );

    // The throwaway package is named, and left yanked.
    let published = json
        .split("\"published\": \"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .expect("a published package")
        .to_string();
    assert!(published.starts_with("acme/conformance_"), "{published}");
    assert!(err.contains(&published), "{err}");
    let index = std::fs::read_to_string(
        root.join("reg")
            .join(published.replace('/', std::path::MAIN_SEPARATOR_STR)),
    )
    .unwrap();
    assert_eq!(
        index.lines().count(),
        1,
        "one line, nothing refused left one: {index}"
    );
    assert!(index.contains("\"yanked\":true"), "{index}");

    // `--read-only` reads it back and publishes nothing.
    let before = std::fs::read_dir(root.join("reg/acme")).unwrap().count();
    let index_url = format!("http://localhost:{port}");
    let (code, out, err) = bynk(
        &root,
        &[
            "registry",
            "conformance",
            &index_url,
            "--read-only",
            "--package",
            &published,
            "--format",
            "json",
        ],
        &[],
    );
    assert_eq!(code, 0, "{out}{err}");
    for id in [
        "config.shape",
        "config.chain",
        "config.delegations",
        "read.lines",
        "read.archive",
        "read.revoked",
    ] {
        assert_eq!(outcome(&out, id), "pass", "{id}: {out}");
    }
    assert!(out.contains("\"published\": null"), "{out}");
    assert_eq!(
        std::fs::read_dir(root.join("reg/acme")).unwrap().count(),
        before
    );
}

#[test]
fn a_check_is_skipped_or_refused_without_what_it_needs() {
    let root = scratch("needs");
    let (full, _) = registry(&root);
    let server = serve(&root);
    let index = format!("http://localhost:{}", proxy(server.port, Fault::None));
    // No narrow token: `auth.scope` is skipped, and the run still passes.
    let (code, json, err) = bynk(
        &root,
        &[
            "registry",
            "conformance",
            &index,
            "--org",
            "acme",
            "--format",
            "json",
        ],
        &[("BYNK_REGISTRY_CONFORMANCE_TOKEN", &full)],
    );
    assert_eq!(code, 0, "{json}{err}");
    assert_eq!(outcome(&json, "auth.scope"), "skip", "{json}");
    // No token at all: the run says how to supply one, and fails.
    let (code, json, err) = bynk(
        &root,
        &[
            "registry",
            "conformance",
            &index,
            "--org",
            "acme",
            "--format",
            "json",
        ],
        &[],
    );
    assert_eq!(code, 1, "{json}{err}");
    assert_eq!(outcome(&json, "publish.token"), "fail", "{json}");
    assert!(json.contains("BYNK_REGISTRY_CONFORMANCE_TOKEN"), "{json}");
    assert!(err.contains("error[bynk.registry.nonconformant]"), "{err}");
    // Every check the run couldn't reach is still reported, skipped, so the
    // set of ids is stable.
    for id in [
        "publish.created",
        "auth.scope",
        "errors.shape",
        "consume.resolve",
    ] {
        assert_eq!(outcome(&json, id), "skip", "{id}: {json}");
    }

    // A narrow-token variable that was named but isn't set fails `auth.scope`
    // rather than skipping it.
    let (code, json, err) = bynk(
        &root,
        &[
            "registry",
            "conformance",
            &index,
            "--org",
            "acme",
            "--narrow-token-env",
            "NO_SUCH_TOKEN_VAR",
            "--format",
            "json",
        ],
        &[("BYNK_REGISTRY_CONFORMANCE_TOKEN", &full)],
    );
    assert_eq!(code, 1, "{json}{err}");
    assert_eq!(outcome(&json, "auth.scope"), "fail", "{json}");
    assert!(json.contains("NO_SUCH_TOKEN_VAR"), "{json}");
}

/// How the proxy breaks the protocol.
#[derive(Clone, Copy)]
enum Fault {
    /// Nothing: the bytes are forwarded as they are, in both directions, with
    /// no HTTP parsing. The tests that drive the server itself still go
    /// through a proxy, for its listener on `[::1]` (see `proxy`), but they
    /// must see the server's own framing (keep-alive, chunked bodies, its
    /// `Content-Length`), which `relay` would rewrite.
    None,
    /// A publish's `200` (a same-tree retry) becomes `409`.
    RetryConflicts,
    /// Every refusal loses its JSON body.
    PlainRefusals,
    /// A `PUT` of `0.0.1` is answered `201` without reaching the server.
    LyingCreated,
    /// Every index line read back has its signature corrupted.
    BadSignature,
    /// A `PUT`'s archive is altered on the way, so the server signs another
    /// tree.
    SwapsTree,
}

/// An HTTP proxy to `upstream` that applies `fault`, one thread per address.
///
/// It listens on BOTH loopback addresses, on one port. `http://` is allowed
/// only for `localhost`, and Windows resolves `localhost` to `::1` first: with
/// a listener on `127.0.0.1` alone, every connection first had `[::1]`
/// refused, and Windows retries a refused connect for about 2 s before the
/// client falls back. Each conformance run opens dozens of connections, so
/// `a_server_breaking_a_rule_fails_that_check` took over 4 minutes on the
/// Windows CI leg against about 5 s on Linux. Where the host has no IPv6
/// loopback, the proxy listens on `127.0.0.1` alone, which is all `localhost`
/// can mean there.
fn proxy(upstream: u16, fault: Fault) -> u16 {
    let (port, listeners) = loopback_listeners();
    for listener in listeners {
        std::thread::spawn(move || {
            for client in listener.incoming().flatten() {
                if matches!(fault, Fault::None) {
                    std::thread::spawn(move || forward(client, upstream));
                    continue;
                }
                if let Ok(handle) = client.try_clone() {
                    let _ = relay(handle, upstream, fault);
                }
                linger(client);
            }
        });
    }
    port
}

/// Copy bytes between `client` and `upstream` both ways until each side
/// closes, so the server's framing reaches the client untouched.
fn forward(client: TcpStream, upstream: u16) {
    let Ok(server) = TcpStream::connect(("127.0.0.1", upstream)) else {
        return;
    };
    let (Ok(mut client_r), Ok(mut server_w)) = (client.try_clone(), server.try_clone()) else {
        return;
    };
    let up = std::thread::spawn(move || {
        let _ = std::io::copy(&mut client_r, &mut server_w);
        let _ = server_w.shutdown(std::net::Shutdown::Write);
    });
    let (mut server_r, mut client_w) = (server, client);
    let _ = std::io::copy(&mut server_r, &mut client_w);
    let _ = client_w.shutdown(std::net::Shutdown::Write);
    let _ = up.join();
}

/// Close `client` only once it has finished sending. `read_message` reads a
/// request body only when it has a `Content-Length`, so a body framed another
/// way (a bodiless POST such as yank's, sent chunked) can be left unread.
/// Closing a socket with unread input sends a reset rather than a FIN, and a
/// reset can discard the response before the client reads it: the client
/// then fails with "Peer disconnected", about one run in two for a test that
/// asserts every check. So stop writing, drain what is left until the client
/// hangs up, and only then close.
fn linger(mut client: TcpStream) {
    let _ = client.shutdown(std::net::Shutdown::Write);
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let _ = std::io::copy(&mut client, &mut std::io::sink());
}

/// Listeners on `127.0.0.1` and, when the host has one, `[::1]`, sharing a
/// port. The `127.0.0.1` port is chosen by the OS; if it is taken on `[::1]`,
/// try another.
fn loopback_listeners() -> (u16, Vec<TcpListener>) {
    for _ in 0..20 {
        let v4 = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = v4.local_addr().unwrap().port();
        match TcpListener::bind(("::1", port)) {
            Ok(v6) => return (port, vec![v4, v6]),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            // No IPv6 loopback: `localhost` cannot resolve to `::1` either.
            Err(_) => return (port, vec![v4]),
        }
    }
    panic!("no port free on both 127.0.0.1 and [::1] after 20 tries");
}

/// One HTTP message: its first line, headers and body.
struct Message {
    start: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn read_message(stream: &mut impl Read, to_eof: bool) -> std::io::Result<Message> {
    let mut reader = BufReader::new(stream);
    let mut start = String::new();
    reader.read_line(&mut start)?;
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h)?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let length = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok());
    let mut body = Vec::new();
    match length {
        Some(n) => {
            body.resize(n, 0);
            reader.read_exact(&mut body)?;
        }
        None if to_eof => {
            reader.read_to_end(&mut body)?;
        }
        None => {}
    }
    Ok(Message {
        start: start.trim_end().to_string(),
        headers,
        body,
    })
}

fn write_message(stream: &mut impl Write, m: &Message) -> std::io::Result<()> {
    write!(stream, "{}\r\n", m.start)?;
    for (k, v) in &m.headers {
        if k.eq_ignore_ascii_case("content-length")
            || k.eq_ignore_ascii_case("connection")
            || k.eq_ignore_ascii_case("transfer-encoding")
        {
            continue;
        }
        write!(stream, "{k}: {v}\r\n")?;
    }
    write!(
        stream,
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        m.body.len()
    )?;
    stream.write_all(&m.body)?;
    stream.flush()
}

fn relay(mut client: TcpStream, upstream: u16, fault: Fault) -> std::io::Result<()> {
    let mut request = read_message(&mut client, false)?;
    let mut parts = request.start.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    if matches!(fault, Fault::LyingCreated) && method == "PUT" && path.ends_with("/0.0.1") {
        let name = path
            .trim_start_matches("/api/")
            .rsplit_once('/')
            .map(|(n, _)| n.to_string())
            .unwrap_or_default();
        let line = format!(
            "{{\"checksum\":\"sha256-AAAA\",\"deps\":[],\"name\":\"{name}\",\"npm\":[],\"sig\":\"AAAA\",\"toolchain\":null,\"version\":\"0.0.1\",\"yanked\":false}}"
        );
        let reply = Message {
            start: "HTTP/1.1 201 Created".into(),
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: line.into_bytes(),
        };
        return write_message(&mut client, &reply);
    }
    if matches!(fault, Fault::SwapsTree) && method == "PUT" {
        let at = request.body.windows(4).position(|w| w == b"  1\n");
        if let Some(at) = at {
            request.body[at + 2] = b'9';
        }
    }
    let mut up = TcpStream::connect(("127.0.0.1", upstream))?;
    write_message(&mut up, &request)?;
    let mut response = read_message(&mut up, true)?;
    let status: u16 = response
        .start
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    match fault {
        Fault::RetryConflicts if method == "PUT" && status == 200 => {
            response.start = "HTTP/1.1 409 Conflict".into();
            response.body = br#"{"error":"version_exists","message":"exists"}"#.to_vec();
        }
        Fault::PlainRefusals if status >= 400 => {
            response
                .headers
                .retain(|(k, _)| !k.eq_ignore_ascii_case("content-type"));
            response
                .headers
                .push(("Content-Type".into(), "text/plain".into()));
            response.body = b"refused".to_vec();
        }
        Fault::BadSignature
            if method == "GET" && path.contains("/conformance_") && status == 200 =>
        {
            let text =
                String::from_utf8_lossy(&response.body).replace("\"sig\":\"", "\"sig\":\"AAAA");
            response.body = text.into_bytes();
        }
        _ => {}
    }
    write_message(&mut client, &response)
}

#[test]
fn a_server_breaking_a_rule_fails_that_check() {
    let root = scratch("faults");
    let (full, narrow) = registry(&root);
    let server = serve(&root);
    for (fault, id) in [
        (Fault::RetryConflicts, "publish.retry-same-tree"),
        (Fault::PlainRefusals, "errors.shape"),
        (Fault::LyingCreated, "publish.read-back"),
        (Fault::BadSignature, "publish.read-back"),
        (Fault::SwapsTree, "publish.read-back"),
    ] {
        let port = proxy(server.port, fault);
        let (code, json, err) = conformance(&root, port, &full, &narrow, &[]);
        assert_eq!(code, 1, "{id}: {json}{err}");
        assert_eq!(outcome(&json, id), "fail", "{id}: {json}");
        assert_eq!(outcome(&json, "config.shape"), "pass", "{id}: {json}");
        assert!(
            err.contains(&format!(
                "error[bynk.registry.nonconformant]: `{id}` failed"
            )),
            "{id}: {err}"
        );
    }
}
