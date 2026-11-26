//! #3926: a route counted by `key: identity` is verified by a compose `verify` step before the
//! limiter is asked, so the step must hold every check the cookie seam makes before an identity
//! is trusted: the cross-site refusal of a state-changing request, the cookie's signature and
//! lifetime, and, for a `sessions = true` actor, the session-live check. The system suite proves
//! the refusals that a principal can drive (a cross-site request, a forged and an expired cookie);
//! a revoked session cannot be driven through `by`, since the driver mints a live session per
//! call, so this reads the blessed emission and pins that the `verify` method carries the check,
//! in the order the wrapper always ran it, ahead of the `Ok` that releases the caller to the limiter.

use std::path::Path;

fn compose_of(fixture: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive")
        .join(fixture)
        .join("expected/workers");
    let worker = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{}: {e}", root.display()))
        .next()
        .expect("one worker")
        .unwrap()
        .path();
    std::fs::read_to_string(worker.join("compose.ts")).expect("compose.ts")
}

/// The text of the compose method `name`: from its signature to the next sibling method.
fn method<'a>(compose: &'a str, name: &str) -> &'a str {
    let start = compose
        .find(&format!("async {name}("))
        .unwrap_or_else(|| panic!("no `{name}` in the compose surface"));
    let rest = &compose[start..];
    let end = rest[1..].find("\n    async ").map_or(rest.len(), |i| i + 1);
    &rest[..end]
}

fn in_order(text: &str, needles: &[&str]) {
    let mut from = 0;
    for n in needles {
        let at = text[from..]
            .find(n)
            .unwrap_or_else(|| panic!("`{n}` missing, or out of order, in:\n{text}"));
        from += at + n.len();
    }
}

#[test]
fn a_signed_cookie_verify_step_holds_the_whole_seam_before_it_releases_the_caller() {
    let compose = compose_of("3926_rate_limit_identity_session");
    let verify = method(&compose, "http_POST_rename__verify");
    in_order(
        verify,
        &[
            "__csrfAllowed(",        // the cross-site refusal of a state-changing request
            "__sessionCookieOf(",    // the cookie
            "__verifySignedCookie(", // its signature, key, audience and lifetime
            ".isLive(",              // the session-live check of a `sessions = true` actor
            "tag: \"Ok\"",           // only then is the caller released to the limiter
        ],
    );
    // A refusal is an `Err` carrying the result the wrapper would have returned.
    assert!(
        verify.contains("tag: \"Err\", error: HttpResult.Forbidden"),
        "{verify}"
    );
    assert!(
        verify.contains("tag: \"Err\", error: HttpResult.Unauthorized"),
        "{verify}"
    );
}

#[test]
fn the_run_step_never_verifies_again() {
    let compose = compose_of("3926_rate_limit_identity_session");
    let run = method(&compose, "http_POST_rename");
    for forbidden in ["__verifySignedCookie", "__csrfAllowed", ".isLive("] {
        assert!(
            !run.contains(forbidden),
            "`{forbidden}` in the run step:\n{run}"
        );
    }
}

#[test]
fn a_bearer_verify_step_verifies_before_it_releases_the_caller() {
    let compose = compose_of("3926_rate_limit_identity_bearer");
    let verify = method(&compose, "http_POST_notes__verify");
    in_order(verify, &["__verifyBearerJwtHs256(", "tag: \"Ok\""]);
    let run = method(&compose, "http_POST_notes");
    assert!(!run.contains("__verifyBearerJwtHs256"), "{run}");
}
