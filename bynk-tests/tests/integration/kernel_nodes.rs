//! 1A.28 (#3729): the inline and `Effect` kernel arms (`TestConnection`, `Connection`, the `HttpResult`
//! cookie ops, the request views, `within`, `race`, `detach` and the `EffectResult` combinators) lower
//! to nodes. Each project is emitted in memory and the exact line is pinned; the text-exit counter is
//! read where it is real (the driver links a non-test copy of `bynk-emit`, see `text_exit_counters.rs`),
//! and a fixture whose unrelated constructs add text exits pins its lines only. The builders' own node
//! and text forms, and the mismatch panics, are unit tests in `lower_call/kernel.rs`.
use bynk_emit::emitter::text_counters;

fn fixture_dir(fixture: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("positive")
        .join(fixture)
}

/// `(text exits, every emitted document joined)` for a fixture project, built as the fixture suite builds it.
fn emit(fixture: &str) -> (usize, String) {
    let dir = fixture_dir(fixture);
    text_counters::reset();
    let result = super::e2e::compile_fixture(&dir, super::e2e::fixture_target(&dir));
    let exits = text_counters::exits();
    let out = match result {
        Ok(out) => out,
        Err(failure) => panic!(
            "{fixture} should compile, got:\n{}",
            bynk_driver::render::render_project_errors(&failure.flatten())
        ),
    };
    let text = out
        .artefacts
        .docs
        .values()
        .map(|d| d.text())
        .collect::<Vec<_>>()
        .join("\n");
    (exits, text)
}

/// `(text exits, every document)` for the one-context project `src` on Workers.
fn mem(src: &str) -> (usize, String) {
    text_counters::reset();
    let out = bynk_driver::project::compile_files_in_memory(
        &[("ops/jobs.bynk", src)],
        bynk_check::project_model::BuildTarget::Workers,
        Default::default(),
        true,
    )
    .unwrap_or_else(|e| {
        panic!(
            "should compile:\n{src}\n{}",
            bynk_driver::render::render_project_errors(&e.flatten())
        )
    });
    let text = out
        .artefacts
        .docs
        .values()
        .map(|d| d.text())
        .collect::<Vec<_>>()
        .join("\n");
    (text_counters::exits(), text)
}

/// The lines of `text` equal to `line` once indentation is trimmed.
fn assert_line(text: &str, line: &str) {
    assert!(
        text.lines().any(|l| l.trim_start() == line),
        "missing line {line:?} in\n{text}"
    );
}

fn assert_contains(text: &str, part: &str) {
    assert!(text.contains(part), "missing {part:?} in\n{text}");
}

/// A request view (`headers`), with no other construct that is text, is a call on its receiver with
/// no text exit and no brackets round the identifier.
#[test]
fn a_request_view_is_a_call_without_receiver_brackets() {
    let src = r#"context ops.jobs

service api from http {
	on GET("/h") (headers: RequestHeaders) -> Effect[HttpResult[String]] by v: Visitor {
		let t = headers.get("x-trace")
		Ok("a")
	}
}
"#;
    let (exits, text) = mem(src);
    assert_eq!(exits, 0, "no text exit:\n{text}");
    assert_line(&text, r#"const t = headers.get("x-trace");"#);
}

/// The fixtures' request views, in a handler and under a `.length`, print without the receiver brackets.
#[test]
fn the_request_views_print_without_receiver_brackets() {
    let (_, text) = emit("2341_http_raw_body_headers");
    assert_line(
        &text,
        r#"const __letElse587 = headers.get("X-GitHub-Event");"#,
    );
    let (_, text) = emit("2854_http_query");
    assert_contains(&text, r#"query.get("q")"#);
    assert_contains(&text, "query.params().size");
    assert!(!text.contains("(query)."), "{text}");
}

/// `withCookie` and `clearCookie` are the IIFE over `__r: any`; a cookie argument that is a node
/// (a call) leaves no text exit, one that is still text falls back to the same bytes.
#[test]
fn the_cookie_ops_are_nodes_and_keep_their_layout() {
    let src = r#"context ops.jobs

consumes bynk { Cookie }

fn session(id: String) -> Cookie { Cookie { name: "sid", value: id } }

service api from http {
	on POST("/c") () -> Effect[HttpResult[()]] by v: Visitor {
		NoContent.withCookie(session("abc"))
	}

	on POST("/d") () -> Effect[HttpResult[()]] by v: Visitor {
		NoContent.clearCookie(session(""))
	}
}
"#;
    let (exits, text) = mem(src);
    let (base, _) = mem(&src
        .replace(".withCookie(session(\"abc\"))", "")
        .replace(".clearCookie(session(\"\"))", ""));
    assert_eq!(exits, base, "the cookie ops add no text exit:\n{text}");
    assert_line(
        &text,
        r#"return ((__r: any) => ({ ...__r, cookies: [...(__r.cookies ?? []), (session("abc"))] }))(HttpResult.NoContent);"#,
    );
    assert_line(
        &text,
        r#"return ((__r: any) => ({ ...__r, cookies: [...(__r.cookies ?? []), { ...(session("")), value: "", maxAge: Some(0), expires: Some(0) }] }))(HttpResult.NoContent);"#,
    );
    // A record literal is still text: the same IIFE, byte for byte.
    let (exits, text) = emit("3277_http_cookies_write");
    assert!(exits > 0);
    assert_contains(
        &text,
        r#"cookies: [...(__r.cookies ?? []), { ...({ name: "__Host-sid", value: "","#,
    );
}

/// `cookies()` on an `HttpResult` in a test case: the cast sits in brackets, the receiver does not.
#[test]
fn http_result_cookies_drops_the_receiver_brackets() {
    let (_, text) = emit("3277_http_cookies_write");
    assert_contains(&text, "((r as any).cookies ?? [])");
    assert!(!text.contains("(r) as any"), "{text}");
}

/// `Connection.send` and `close` are calls on the identifier; the held-connection fixture has no
/// other text.
#[test]
fn connection_calls_are_nodes() {
    let (exits, text) = emit("235_held_connection");
    assert_eq!(exits, 0, "no text exit:\n{text}");
    assert_line(
        &text,
        r#"const __r0 = await conn.send({ text: "welcome" });"#,
    );
    assert_line(&text, "const __r1 = await conn.close();");
}

/// A case's `TestConnection`: `sent`, `closed` and `mint`.
#[test]
fn test_connection_ops_are_nodes() {
    let (_, text) = emit("3051_test_websocket_address");
    assert_contains(&text, "__bynkEq([...alice.sent], [{ text: \"welcome\" }])");
    assert_contains(&text, "(alice.closed === false)");
    assert_contains(&text, "tc.mint()");
    assert!(!text.contains("(alice)."), "{text}");
}

/// `race`: a chain of three flattens to one array in source order, and operands that hoist keep that
/// order; `within` takes its deadline and the execution context.
#[test]
fn race_and_within_are_nodes_in_source_order() {
    let src = r#"context ops.jobs

consumes bynk { TimedOut }

capability Prices { fn get(region: String, sku: Int) -> Effect[Int] }
provides Prices = P { fn get(region: String, sku: Int) -> Effect[Int] { sku } }

fn pick(n: Int) -> Result[Int, String] { Ok(n) }

service run {
	on call(sku: Int) -> Effect[Result[Int, String]] given Prices {
		let p <- Prices.get("a", pick(sku)?).race(Prices.get("b", pick(sku + 1)?)).race(Prices.get("c", pick(sku + 2)?))
		let q <- Prices.get("a", sku).within(2.seconds)
		Effect.pure(Ok(p))
	}
}
"#;
    let (exits, text) = mem(src);
    assert_eq!(exits, 0, "no text exit:\n{text}");
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("missing {needle}"))
    };
    assert!(at("const __r0 = pick(sku);") < at("const __r1 = pick(sku + 1);"));
    assert!(at("const __r1 = pick(sku + 1);") < at("const __r2 = pick(sku + 2);"));
    assert_line(
        &text,
        r#"const p = await __bynkRace([deps.Prices.get("a", __r0.value), deps.Prices.get("b", __r1.value), deps.Prices.get("c", __r2.value)], deps.__exec);"#,
    );
    assert_line(
        &text,
        r#"const q = await __bynkWithin(deps.Prices.get("a", sku), 2000, deps.__exec);"#,
    );
    // The helpers' imports are noted as before.
    assert_contains(&text, "import { __bynkWithin, __bynkRace }");
}

/// `detach`: a plain payload names no error type, a `Result` payload names its error type, and the
/// site is the quoted `.bynk` location.
#[test]
fn detach_is_a_node_with_its_site() {
    let src = r#"context ops.jobs

type MailError = | Refused | Bounced(reason: String)

capability Mail { fn send(n: Int) -> Effect[Result[(), MailError]] }
capability Count { fn next(n: Int) -> Effect[Int] }

provides Mail = M { fn send(n: Int) -> Effect[Result[(), MailError]] { Ok(()) } }
provides Count = C { fn next(n: Int) -> Effect[Int] { n } }

service run {
	on call(n: Int) -> Effect[Int] given Mail, Count {
		do Count.next(n).detach()
		do Mail.send(n).detach()
		Effect.pure(n)
	}
}
"#;
    let (exits, text) = mem(src);
    assert_eq!(exits, 0, "no text exit:\n{text}");
    assert_line(
        &text,
        r#"await __bynkDetach(deps.Count.next(n), deps.__exec, "ops/jobs.bynk:13:6");"#,
    );
    assert_line(
        &text,
        r#"await __bynkDetach(deps.Mail.send(n), deps.__exec, "ops/jobs.bynk:14:6", "MailError");"#,
    );
}

/// The four `Effect[Result]` combinators, over an annotated and an inferred callback parameter.
#[test]
fn the_effect_result_combinators_are_nodes() {
    let src = r#"context ops.jobs

fn fetch(id: Int) -> Effect[Result[Int, String]] { Effect.pure(Ok(id)) }
fn label(n: Int) -> String { "user" }
fn next(n: Int) -> Effect[Result[String, String]] { Effect.pure(Ok("ok")) }
fn recover(e: String) -> Effect[Result[Int, String]] { Effect.pure(Ok(0)) }

fn a(id: Int) -> Effect[Result[String, String]] { fetch(id).mapOk((n: Int) => label(n)) }
fn b(id: Int) -> Effect[Result[Int, Int]] { fetch(id).mapErr((e: String) => 500) }
fn c(id: Int) -> Effect[Result[String, String]] { fetch(id).flatMapOk((n: Int) => next(n)) }
fn d(id: Int) -> Effect[Result[Int, String]] { fetch(id).flatMapErr((e: String) => recover(e)) }
fn inferred(id: Int) -> Effect[Result[String, String]] { fetch(id).mapOk((n) => label(n)) }
"#;
    let (exits, text) = mem(src);
    assert_eq!(exits, 0, "no text exit:\n{text}");
    for line in [
        r#"return (async (__e: globalThis.Promise<Result<number, string>>, __f: (x: number) => string) => { const __r = await __e; return __r.tag === "Ok" ? Ok(__f(__r.value)) : __r; })(fetch(id), (n: number) => label(n));"#,
        r#"return (async (__e: globalThis.Promise<Result<number, string>>, __f: (e: string) => number) => { const __r = await __e; return __r.tag === "Err" ? Err(__f(__r.error)) : __r; })(fetch(id), (e: string) => 500);"#,
        r#"return (async (__e: globalThis.Promise<Result<number, string>>, __f: (x: number) => globalThis.Promise<Result<string, string>>) => { const __r = await __e; return __r.tag === "Ok" ? await __f(__r.value) : __r; })(fetch(id), async (n: number) => next(n));"#,
        r#"return (async (__e: globalThis.Promise<Result<number, string>>, __f: (e: string) => globalThis.Promise<Result<number, string>>) => { const __r = await __e; return __r.tag === "Err" ? await __f(__r.error) : __r; })(fetch(id), async (e: string) => recover(e));"#,
        r#"return (async (__e: globalThis.Promise<Result<number, string>>, __f: (x: number) => string) => { const __r = await __e; return __r.tag === "Ok" ? Ok(__f(__r.value)) : __r; })(fetch(id), (n: number) => label(n));"#,
    ] {
        assert_line(&text, line);
    }
}
