//! #3443: `~>` is removed. It is still lexed, so a program that uses it gets
//! `bynk.parse.removed_send_arrow`, which names the replacement, rather than a
//! generic parse error. There is no fix: `~>` was last deprecated with a
//! rewrite in v0.603.x, and the removal does not carry that rewrite forward.

fn categories(text: &str) -> Vec<&'static str> {
    bynk_ide::diagnose(text)
        .iter()
        .map(|d| d.error.category)
        .collect()
}

#[test]
fn a_send_statement_is_refused_with_the_removal_code() {
    let src =
        "commons m\n\nfn run(ping: Effect[()]) -> Effect[()] {\n  ~> ping\n  Effect.pure(())\n}\n";
    let found = categories(src);
    assert!(
        found.contains(&"bynk.parse.removed_send_arrow"),
        "{found:?}"
    );
    assert!(
        !found.iter().any(|c| c.starts_with("bynk.send.")),
        "{found:?}"
    );
}

#[test]
fn the_refusal_names_the_replacement_and_offers_no_fix() {
    let src =
        "commons m\n\nfn run(ping: Effect[()]) -> Effect[()] {\n  ~> ping\n  Effect.pure(())\n}\n";
    let diags = bynk_ide::diagnose(src);
    let err = &diags
        .iter()
        .find(|d| d.error.category == "bynk.parse.removed_send_arrow")
        .expect("the removal error")
        .error;
    assert!(err.message.contains("do e.detach()"), "{}", err.message);
    assert!(err.suggestions.is_empty(), "{:?}", err.suggestions);
}

#[test]
fn the_replacement_is_clean() {
    let src = "commons m\n\nfn run(ping: Effect[()]) -> Effect[()] {\n  do ping.detach()\n  Effect.pure(())\n}\n";
    assert!(categories(src).is_empty(), "{:?}", categories(src));
}
