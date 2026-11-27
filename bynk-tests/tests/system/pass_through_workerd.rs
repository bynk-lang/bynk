//! #3752 (pass-through HTTP, slice 3): an `on ANY(…)` route on workerd, forwarding
//! to a loopback upstream (`design/tracks/pass-through-http.md` §10.2). It shows
//! what only a real Worker can: the upstream's status, headers and streamed body
//! pass through, nothing is synthesised on a path the route owns, an ordinary
//! route keeps the paths it owns, and the §8 SSRF requests never reach a host or
//! path the program didn't name. Each check names the decision or §8 row it holds.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What the upstream received, one line per request: `METHOD target host=…`.
type Seen = Arc<Mutex<Vec<String>>>;

fn header_of(head: &str, name: &str) -> Option<String> {
    head.lines().skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case(name)
            .then(|| v.trim().to_string())
    })
}

/// The body after `head`, read by its `content-length` or as chunks.
fn read_body(stream: &mut std::net::TcpStream, head: &str, mut rest: Vec<u8>) -> Vec<u8> {
    let mut buf = [0u8; 4096];
    if let Some(len) = header_of(head, "content-length").and_then(|l| l.parse::<usize>().ok()) {
        while rest.len() < len {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => rest.extend_from_slice(&buf[..n]),
            }
        }
        rest.truncate(len);
        return rest;
    }
    if header_of(head, "transfer-encoding").is_some_and(|t| t.eq_ignore_ascii_case("chunked")) {
        while !rest.windows(5).any(|w| w == b"0\r\n\r\n") {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => rest.extend_from_slice(&buf[..n]),
            }
        }
        return dechunk(&rest);
    }
    rest
}

fn dechunk(mut raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(i) = raw.windows(2).position(|w| w == b"\r\n") {
        let size =
            usize::from_str_radix(String::from_utf8_lossy(&raw[..i]).trim(), 16).unwrap_or(0);
        if size == 0 {
            break;
        }
        let start = i + 2;
        out.extend_from_slice(&raw[start..(start + size).min(raw.len())]);
        raw = &raw[(start + size + 2).min(raw.len())..];
    }
    out
}

/// One request off `stream`: its head and body.
fn read_request(stream: &mut std::net::TcpStream) -> Option<(String, Vec<u8>)> {
    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    let split = loop {
        if let Some(i) = got.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return None,
            Ok(n) => got.extend_from_slice(&buf[..n]),
        }
    };
    let head = String::from_utf8_lossy(&got[..split]).to_string();
    let body = read_body(stream, &head, got[split..].to_vec());
    Some((head, body))
}

/// The loopback upstream (§10.2). `/status/<n>` answers that status with a strong
/// `ETag`, an `x-up` header, a `Set-Cookie` and its own HSTS; `/echo` answers what
/// it was sent; `/hang` never answers; anything else is `404`.
fn upstream(seen: Seen) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the upstream");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let seen = seen.clone();
            std::thread::spawn(move || {
                let mut stream = stream;
                let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                let Some((head, body)) = read_request(&mut stream) else {
                    return;
                };
                let mut first = head.lines().next().unwrap_or("").split_whitespace();
                let (method, target) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
                let host = header_of(&head, "host").unwrap_or_default();
                seen.lock()
                    .unwrap()
                    .push(format!("{method} {target} host={host}"));
                let path = target.split('?').next().unwrap_or("");
                let has = |n: &str| {
                    if header_of(&head, n).is_some() {
                        "yes"
                    } else {
                        "no"
                    }
                };
                let (status, headers, text): (&str, String, String) = match path {
                    "/status/200" => (
                        "200 OK",
                        "content-type: text/plain\r\netag: \"u1\"\r\nx-up: 1\r\nset-cookie: up=1; Domain=example.org\r\nstrict-transport-security: max-age=1; includeSubDomains\r\n".into(),
                        "hello".into(),
                    ),
                    "/status/304" => ("304 Not Modified", "etag: \"u1\"\r\n".into(), String::new()),
                    "/status/404" => ("404 Not Found", "content-type: text/html\r\n".into(), "<p>missing</p>".into()),
                    // D16: an off-host redirect, passed through and never followed.
                    "/status/302off" => (
                        "302 Found",
                        "location: https://elsewhere.example/landing\r\n".into(),
                        String::new(),
                    ),
                    "/status/308" => ("308 Permanent Redirect", "location: /posts/x/\r\n".into(), String::new()),
                    "/echo" => (
                        "200 OK",
                        "content-type: text/plain\r\n".into(),
                        format!(
                            "{method} {target} body={} cookie={} auth={}",
                            String::from_utf8_lossy(&body),
                            has("cookie"),
                            has("authorization")
                        ),
                    ),
                    "/hang" => {
                        std::thread::sleep(Duration::from_secs(30));
                        return;
                    }
                    _ => ("404 Not Found", String::new(), "no such page".into()),
                };
                let body = if method == "HEAD" || status.starts_with("304") {
                    ""
                } else {
                    text.as_str()
                };
                let reply = format!(
                    "HTTP/1.1 {status}\r\n{headers}content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    if method == "HEAD" {
                        text.len()
                    } else {
                        body.len()
                    }
                );
                let _ = stream.write_all(reply.as_bytes());
            });
        }
    });
    port
}

/// A response as the test reads it.
struct Answer {
    status: u16,
    head: String,
    body: String,
}

impl Answer {
    fn header(&self, name: &str) -> Option<String> {
        header_of(&self.head, name)
    }
}

/// `method path` on `port` with `headers` and `body`, as an [`Answer`].
fn send(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Result<Answer, String> {
    let addr = format!("127.0.0.1:{port}");
    let mut stream = std::net::TcpStream::connect_timeout(
        &addr.parse().map_err(|e| format!("{e}"))?,
        Duration::from_secs(2),
    )
    .map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .map_err(|e| e.to_string())?;
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    for (k, v) in headers {
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    if !body.is_empty() {
        request.push_str(&format!("content-length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| format!("no response head: {:?}", String::from_utf8_lossy(&raw)))?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let rest = raw[split + 4..].to_vec();
    let body = if header_of(&head, "transfer-encoding")
        .is_some_and(|t| t.eq_ignore_ascii_case("chunked"))
    {
        dechunk(&rest)
    } else {
        rest
    };
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("unparseable response: {head:?}"))?;
    Ok(Answer {
        status,
        head,
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

const MANIFEST: &str = "[project]\nname = \"p-smoke\"\nversion = \"0.1.0\"\n\n[unstable]\nallow = [\"bynk.http.rest\", \"bynk.http.ANY\", \"bynk.Upstream\", \"bynk.InboundRequest\", \"bynk.Forwarded\", \"bynk.PassThrough\", \"bynk.UpstreamError\"]\n";

fn source(upstream: u16) -> String {
    format!(
        "context p.smoke

consumes bynk {{ Upstream, InboundRequest, PassThrough, UpstreamError }}

capability Pages = Upstream @origin(\"http://127.0.0.1:{upstream}\") @timeout(2.seconds)

service proxy from http {{
\ton GET(\"/ready\") () -> Effect[HttpResult[String]] by Visitor {{
\t\tOk(\"ready\")
\t}}

\ton GET(\"/blog\") () -> Effect[HttpResult[String]] by Visitor {{
\t\tPermanentRedirect(\"/blog/\")
\t}}

\ton ANY(\"/blog/*path\") (path: String, request: InboundRequest) -> Effect[PassThrough] by Visitor given Pages {{
\t\tlet got <- Pages.forward(request, if path == \"\" {{ \"/\" }} else {{ path }})
\t\tmatch got {{
\t\t\tOk(response) => Forward(response)
\t\t\tErr(Timeout) => Respond(GatewayTimeout(\"the upstream timed out\"))
\t\t\tErr(InvalidPath) => Respond(BadRequest(\"not a path the upstream serves\"))
\t\t\tErr(_) => Respond(BadGateway(\"the upstream is unreachable\"))
\t\t}}
\t}}
}}
"
    )
}

/// One check: what it holds, and what was wrong, if anything.
fn check(
    failures: &mut Vec<String>,
    what: &str,
    answer: Result<Answer, String>,
    ok: &dyn Fn(&Answer) -> bool,
) {
    match answer {
        Ok(a) if ok(&a) => {}
        Ok(a) => failures.push(format!("{what}: got {} {:?}\n{}", a.status, a.body, a.head)),
        Err(e) => failures.push(format!("{what}: {e}")),
    }
}

#[test]
fn a_pass_through_route_forwards_to_a_loopback_upstream_on_workerd() {
    let seen: Seen = Arc::default();
    let up = upstream(seen.clone());
    let src = source(up);
    let Some((failures, log)) = super::binding_workerd::run_smoke_with(
        "p-smoke",
        "p",
        &src,
        &[("bynk.toml", MANIFEST)],
        &|_| None,
        "/ready",
        &[],
        &|port| {
            let mut f = Vec::new();
            let none: &[(&str, &str)] = &[];
            check(
                &mut f,
                "D9: status, headers and body pass through (200)",
                send(port, "GET", "/blog/status/200", none, ""),
                &|a| {
                    a.status == 200
                        && a.body == "hello"
                        && a.header("etag").as_deref() == Some("\"u1\"")
                        && a.header("x-up").as_deref() == Some("1")
                        && a.header("x-content-type-options").as_deref() == Some("nosniff")
                },
            );
            check(
                &mut f,
                "§8 credential leakage from the upstream: Set-Cookie stripped; D9 the upstream's HSTS never reaches the proxy",
                send(port, "GET", "/blog/status/200", none, ""),
                &|a| {
                    a.header("set-cookie").is_none()
                        && a.header("strict-transport-security").is_none()
                },
            );
            check(
                &mut f,
                "D9: no router 304 or weak ETag is synthesised on an ANY path",
                send(
                    port,
                    "GET",
                    "/blog/status/200",
                    &[("if-none-match", "\"u1\"")],
                    "",
                ),
                &|a| a.status == 200 && a.header("etag").as_deref() == Some("\"u1\""),
            );
            check(
                &mut f,
                "D9: an upstream 304 passes",
                send(port, "GET", "/blog/status/304", none, ""),
                &|a| a.status == 304,
            );
            check(
                &mut f,
                "D9: an upstream 404 page is the page",
                send(port, "GET", "/blog/status/404", none, ""),
                &|a| {
                    a.status == 404
                        && a.body == "<p>missing</p>"
                        && a.header("content-security-policy").is_some()
                },
            );
            check(
                &mut f,
                "D9: an upstream 308 passes",
                send(port, "GET", "/blog/status/308", none, ""),
                &|a| a.status == 308 && a.header("location").as_deref() == Some("/posts/x/"),
            );
            check(
                &mut f,
                "D9: HEAD is forwarded as HEAD",
                send(port, "HEAD", "/blog/status/200", none, ""),
                &|a| a.status == 200 && a.body.is_empty(),
            );
            check(
                &mut f,
                "D9: OPTIONS is forwarded, not answered with Allow",
                send(port, "OPTIONS", "/blog/echo", none, ""),
                &|a| {
                    a.status == 200
                        && a.body.starts_with("OPTIONS /echo")
                        && a.header("allow").is_none()
                },
            );
            check(
                &mut f,
                "D13, D15: the method, raw query and body pass; no inbound header (§8 credential leakage to the upstream)",
                send(
                    port,
                    "POST",
                    "/blog/echo?q=%2F1",
                    &[("cookie", "s=1"), ("authorization", "Bearer t")],
                    "payload",
                ),
                &|a| {
                    a.status == 200 && a.body == "POST /echo?q=%2F1 body=payload cookie=no auth=no"
                },
            );
            check(
                &mut f,
                "§8 TRACE reflecting credentials; method confusion: a method outside the seven is 501",
                send(port, "PROPFIND", "/blog/echo", none, ""),
                &|a| a.status == 501,
            );
            check(
                &mut f,
                "§8 WebSocket upgrade tunnelling: an Upgrade is 501",
                send(
                    port,
                    "GET",
                    "/blog/echo",
                    &[("upgrade", "websocket"), ("connection", "upgrade")],
                    "",
                ),
                &|a| a.status == 501,
            );
            check(
                &mut f,
                "D5: an ordinary route keeps its path for every method (405 with Allow)",
                send(port, "POST", "/blog", none, ""),
                &|a| a.status == 405 && a.header("allow").is_some_and(|v| v.contains("GET")),
            );
            check(
                &mut f,
                "D5: an ordinary route at equal shape wins",
                send(port, "GET", "/blog", none, ""),
                &|a| a.status == 308 && a.header("location").as_deref() == Some("/blog/"),
            );
            check(
                &mut f,
                "§8 SSRF into the upstream's own private paths via `..`: refused, nothing sent",
                send(port, "GET", "/blog/..%2F..%2Fadmin", none, ""),
                &|a| a.status == 400,
            );
            check(
                &mut f,
                "§8 SSRF into the upstream's own private paths via `..`: a mixed-case `%2E%2e` segment never reaches it",
                send(port, "GET", "/blog/%2E%2e/admin", none, ""),
                &|a| a.status == 400 || a.status == 404,
            );
            check(
                &mut f,
                "§8 SSRF into the upstream's own private paths via `..`: a double-encoded `%252e%252e%252fadmin` is refused, nothing sent",
                send(port, "GET", "/blog/%252e%252e%252fadmin", none, ""),
                &|a| a.status == 400,
            );
            check(
                &mut f,
                "§8 SSRF into the upstream's own private paths via `..`: a triple-encoded `%25252e` is refused, nothing sent",
                send(port, "GET", "/blog/%25252e%25252e/admin", none, ""),
                &|a| a.status == 400,
            );
            check(
                &mut f,
                "§8 request smuggling (header and log injection through a decoding upstream): `%0d%0aX-Injected:%201` is refused, nothing sent",
                send(port, "GET", "/blog/%0d%0aX-Injected:%201", none, ""),
                &|a| a.status == 400,
            );
            check(
                &mut f,
                "§8 open redirect via the upstream (D16): an off-host Location passes unchanged and is never followed",
                send(port, "GET", "/blog/status/302off", none, ""),
                &|a| {
                    a.status == 302
                        && a.header("location").as_deref()
                            == Some("https://elsewhere.example/landing")
                },
            );
            check(
                &mut f,
                "§8 SSRF (protocol-relative): a path never changes the host",
                send(port, "GET", "/blog//other.example/x", none, ""),
                &|a| a.status == 404,
            );
            check(
                &mut f,
                "§8 oversized or slow upstream responses: @timeout bounds the head (504)",
                send(port, "GET", "/blog/hang", none, ""),
                &|a| a.status == 504,
            );
            let seen = seen.lock().unwrap().clone();
            let host = format!("host=127.0.0.1:{up}");
            if seen.iter().any(|s| !s.ends_with(&host)) {
                f.push(format!(
                    "§8 SSRF: every request reaches the declared origin, got {seen:?}"
                ));
            }
            if seen.iter().any(|s| {
                s.contains("admin")
                    || s.contains("Injected")
                    || s.starts_with("PROPFIND")
                    || s.contains("upgrade")
            }) {
                f.push(format!("a refused request reached the upstream: {seen:?}"));
            }
            if !seen.iter().any(|s| s.starts_with("GET //other.example/x ")) {
                f.push(format!(
                    "§8 SSRF: `//other.example/x` is a path on the upstream, got {seen:?}"
                ));
            }
            if !seen.iter().any(|s| s.starts_with("HEAD /status/200 ")) {
                f.push(format!(
                    "D9: HEAD reaches the upstream as HEAD, got {seen:?}"
                ));
            }
            f
        },
    ) else {
        return;
    };
    assert!(
        failures.is_empty(),
        "a pass-through route on workerd disagrees with D5–D15:\n{}\n--- wrangler log ---\n{log}",
        failures.join("\n")
    );
}
