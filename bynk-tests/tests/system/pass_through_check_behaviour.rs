//! #3752 (pass-through HTTP, slice 3a): what `bynk check` accepts of the
//! pass-through surface. Slice 3a type-checks an `on ANY(…)` route but builds none
//! (`bynk.http.pass_through_not_emitted`, pinned by its negative fixture), so these
//! cases run `bynk check` itself: the track's §11.2 blog port, a program whose own
//! variants are called `Forward` and `Respond`, and one whose own type is called
//! `InboundRequest`. The refusals are the `3752_*` negative fixtures.

use std::fs;
use std::process::Command;

const ALLOW: &str = "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[unstable]\nallow = [\"bynk.http.rest\", \"bynk.http.ANY\", \"bynk.Upstream\", \"bynk.InboundRequest\", \"bynk.Forwarded\", \"bynk.PassThrough\", \"bynk.UpstreamError\"]\n";

/// `bynk check` on a one-file project `demo` holding `source`: its exit status and output.
fn check(name: &str, source: &str) -> (bool, String) {
    let root =
        std::env::temp_dir().join(format!("bynk-pass-through-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("bynk.toml"), ALLOW).unwrap();
    fs::write(root.join("src/demo.bynk"), source).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("check")
        .arg(&root)
        .output()
        .expect("run bynk");
    let _ = fs::remove_dir_all(&root);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success() && !text.contains("Error:"), text)
}

/// The track's §11.2 port of the blog proxy, with the `consumes` line slice 3a's
/// ruling gives it (the types are `bynk`'s).
#[test]
fn the_blog_port_checks() {
    let (ok, out) = check(
        "port",
        r#"context demo

consumes bynk { Upstream, InboundRequest, PassThrough, UpstreamError }

const BlogOrigin: String = "https://bynk-blog.pages.dev"

capability Pages = Upstream @origin(BlogOrigin) @timeout(30.seconds)

service proxy from http {
	on ANY("/blog/*path") (path: String, request: InboundRequest) -> Effect[PassThrough] by Visitor given Pages {
		if path == "" {
			let q = request.query()
			Respond(PermanentRedirect(if q == "" { "/blog/" } else { "/blog/?\(q)" }))
		} else {
			let got <- Pages.forward(request, path)
			match got {
				Ok(response) => if response.status() == 404 { Respond(NotFound) } else { Forward(response) }
				Err(Timeout) => Respond(GatewayTimeout("the blog did not answer in time"))
				Err(_) => Respond(BadGateway("the blog could not be reached"))
			}
		}
	}
}
"#,
    );
    assert!(ok, "the §11.2 port must check:\n{out}");
}

/// §12: the new `bynk` names are additive. A program written before them, with
/// its own `Forward` and `Respond` variants and bare `Timeout` and `Unreachable`
/// (which `bynk`'s `FetchError` and `CallOutcome` already declare, and now
/// `UpstreamError` too), still compiles, even consuming the whole of `bynk`.
#[test]
fn a_programs_own_forward_and_respond_still_compile() {
    let (ok, out) = check(
        "variants",
        r#"context demo

consumes bynk

type Move = | Forward(steps: Int) | Respond | Hold

fn describe(m: Move) -> String {
	match m {
		Forward(n) => "forward \(n)"
		Respond => "respond"
		Hold => "hold"
	}
}

fn why(e: FetchError) -> String {
	match e {
		Timeout => "timeout"
		_ => "other"
	}
}

fn reached(o: CallOutcome[Int]) -> Bool {
	match o {
		Unreachable(_) => false
		_ => true
	}
}

service api from http {
	on GET("/move") () -> Effect[HttpResult[String]] by Visitor {
		Ok("\(describe(Forward(3))) \(why(Timeout))")
	}
}
"#,
    );
    assert!(
        ok,
        "a program's own `Forward` and `Respond` must still compile:\n{out}"
    );
}

/// D8: a program that imports `PassThrough` beside its own `Forward` names the
/// one it means where both are in scope.
#[test]
fn a_qualified_variant_settles_a_name_both_sums_declare() {
    let (ok, out) = check(
        "qualified",
        r#"context demo

consumes bynk { Upstream, InboundRequest, PassThrough }

type Move = | Forward(steps: Int) | Hold

capability Pages = Upstream @origin("https://upstream.example")

service proxy from http {
	on ANY("/blog/*path") (path: String, request: InboundRequest) -> Effect[PassThrough] by Visitor given Pages {
		let m = Move.Forward(1)
		let got <- Pages.forward(request, path)
		match got {
			Ok(response) => PassThrough.Forward(response)
			Err(_) => PassThrough.Respond(BadGateway("no"))
		}
	}
}
"#,
    );
    assert!(ok, "qualified variants must check:\n{out}");
}

/// D8: where an expected type names one sum, a bare `Forward` or `Respond` is
/// that sum's, though the program declares its own: a return type picks
/// `PassThrough.Forward` in `pass` and `Move.Forward` in `step`, and an
/// annotated `let` picks `PassThrough.Respond` over the program's own `Respond`.
#[test]
fn a_bare_variant_resolves_by_its_expected_type() {
    let (ok, out) = check(
        "expected-type",
        r#"context demo

consumes bynk { Upstream, InboundRequest, Forwarded, PassThrough }

type Move = | Forward(steps: Int) | Hold
type Reply = | Respond(text: String) | Silent

fn step() -> Move {
	Forward(1)
}

fn reply() -> Reply {
	Respond("hi")
}

fn pass(response: Forwarded) -> PassThrough {
	Forward(response)
}

capability Pages = Upstream @origin("https://upstream.example")

service proxy from http {
	on ANY("/blog/*path") (path: String, request: InboundRequest) -> Effect[PassThrough] by Visitor given Pages {
		let got <- Pages.forward(request, path)
		match got {
			Ok(response) => pass(response)
			Err(_) => {
				let answer: PassThrough = Respond(BadGateway("no"))
				answer
			}
		}
	}
}
"#,
    );
    assert!(
        ok,
        "a bare variant must resolve by its expected type:\n{out}"
    );
}

/// §12: `bynk.cloudflare`'s `EmailVerdict` also has a `Forward`, so an email
/// handler that consumes the whole of `bynk` sees two. The handler's
/// `Effect[EmailVerdict]` picks the verdict's, in a tail, a `match` arm and an
/// `if` branch, as it did before `PassThrough` existed.
#[test]
fn an_email_handlers_bare_forward_is_still_the_verdicts() {
    let (ok, out) = check(
        "email-forward",
        r#"context demo

consumes bynk
consumes bynk.cloudflare { InboundEmail, EmailVerdict }

service intake from email("support@q.example") {
	on message(mail: InboundEmail) -> Effect[EmailVerdict] by Sender {
		match mail.subject {
			"f" => Forward("team@q.example")
			_ => Accept
		}
	}
}

service branch from email("branch@q.example") {
	on message(mail: InboundEmail) -> Effect[EmailVerdict] by Sender {
		if mail.subject == "f" {
			Forward("team@q.example")
		} else {
			Accept
		}
	}
}

service tail from email("tail@q.example") {
	on message(mail: InboundEmail) -> Effect[EmailVerdict] by Sender {
		Forward("team@q.example")
	}
}
"#,
    );
    assert!(
        ok,
        "an email handler's bare `Forward` must stay `EmailVerdict.Forward`:\n{out}"
    );
}

/// D11: the boundary rule is the `bynk` type's, found by its declaration, so a
/// program's own `InboundRequest` is an ordinary record.
#[test]
fn a_programs_own_inbound_request_is_not_refused() {
    let (ok, out) = check(
        "own-type",
        r#"context demo

type InboundRequest = { id: String }

service keep {
	on call(r: InboundRequest) -> Effect[String] by Caller {
		Effect.pure(r.id)
	}
}
"#,
    );
    assert!(
        ok,
        "a program's own `InboundRequest` must not be refused:\n{out}"
    );
}
