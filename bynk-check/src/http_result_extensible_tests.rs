//! #4536 (D23 of the pass-through HTTP track): the built-in `HttpResult` is
//! extensible, like the first-party `FetchError` and `Method`: a `match` that
//! covers it, at any depth, needs a wildcard arm.

use bynk_syntax::ast::{HTTP_VARIANTS, HttpVariantPayload};

/// The codes a one-file project's analysis reports.
fn codes(source: &str) -> Vec<&'static str> {
    crate::testkit::analyse(&[("app/probe.bynk", source)])
        .analysis
        .errors
        .iter()
        .map(|a| a.error.category)
        .collect()
}

/// One arm per variant `HttpResult` has today, each pattern wrapped by `wrap`.
fn every_variant(wrap: &str) -> String {
    HTTP_VARIANTS
        .iter()
        .map(|v| {
            let arity = match v.payload {
                HttpVariantPayload::None => 0,
                HttpVariantPayload::Value
                | HttpVariantPayload::Message
                | HttpVariantPayload::Location
                | HttpVariantPayload::Streamed => 1,
                HttpVariantPayload::Raw | HttpVariantPayload::RawStreamed => 2,
                HttpVariantPayload::RawTagged
                | HttpVariantPayload::Partial
                | HttpVariantPayload::RawError => 3,
            };
            let pat = if arity == 0 {
                v.name.to_string()
            } else {
                format!("{}({})", v.name, vec!["_"; arity].join(", "))
            };
            format!("\t\t{} => \"x\"\n", wrap.replace("P", &pat))
        })
        .collect()
}

/// A commons function matching a value of type `ty` with `arms`.
fn matching(ty: &str, arms: &str) -> String {
    format!("commons app.probe\n\nfn label(r: {ty}) -> String {{\n\tmatch r {{\n{arms}\t}}\n}}\n")
}

const NEEDS_WILDCARD: &str = "bynk.types.extensible_match_needs_wildcard";

#[test]
fn every_current_variant_without_a_wildcard_is_refused() {
    assert_eq!(
        codes(&matching("HttpResult[String]", &every_variant("P"))),
        [NEEDS_WILDCARD]
    );
}

#[test]
fn a_wildcard_or_binding_satisfies_it_and_is_reachable() {
    for last in ["\t\t_ => \"later\"\n", "\t\tother => \"later\"\n"] {
        let arms = every_variant("P") + last;
        assert_eq!(
            codes(&matching("HttpResult[String]", &arms)),
            Vec::<&str>::new(),
            "{last:?} covers the future variant and is not `bynk.types.unreachable_arm`"
        );
    }
}

#[test]
fn a_nested_http_result_needs_its_wildcard_too() {
    let in_result = every_variant("Ok(P)") + "\t\tErr(_) => \"e\"\n";
    assert_eq!(
        codes(&matching("Result[HttpResult[String], String]", &in_result)),
        [NEEDS_WILDCARD]
    );
    assert_eq!(
        codes(&matching(
            "Result[HttpResult[String], String]",
            &(in_result.clone() + "\t\tOk(_) => \"later\"\n")
        )),
        Vec::<&str>::new()
    );
    let in_option = every_variant("Some(P)") + "\t\tNone => \"n\"\n";
    assert_eq!(
        codes(&matching("Option[HttpResult[String]]", &in_option)),
        [NEEDS_WILDCARD]
    );
    assert_eq!(
        codes(&matching(
            "Option[HttpResult[String]]",
            &(in_option + "\t\tSome(_) => \"later\"\n")
        )),
        Vec::<&str>::new()
    );
}

/// The diagnostic names `HttpResult` and offers the wildcard as its fill.
#[test]
fn the_diagnostic_names_http_result_and_offers_the_wildcard() {
    let a = crate::testkit::analyse(&[(
        "app/probe.bynk",
        &matching("HttpResult[String]", &every_variant("P")),
    )]);
    let err = &a
        .analysis
        .errors
        .iter()
        .find(|e| e.error.category == NEEDS_WILDCARD)
        .expect("the wildcard request")
        .error;
    assert!(
        err.message.contains("`HttpResult` may gain variants"),
        "{}",
        err.message
    );
    let fill = err.suggestions.first().expect("a fill");
    assert!(
        fill.edits
            .iter()
            .any(|(_, t)| t.trim_start().starts_with("_ => ")),
        "{fill:?}"
    );
}

/// A partial `match` keeps its ordinary diagnostic: only the missing
/// variants, plus the wildcard request, are reported.
#[test]
fn a_partial_match_still_names_its_missing_variants() {
    let c = codes(&matching(
        "HttpResult[String]",
        "\t\tOk(_) => \"ok\"\n\t\tNotFound => \"nf\"\n",
    ));
    assert!(c.contains(&"bynk.types.non_exhaustive_match"), "{c:?}");
    assert!(c.contains(&NEEDS_WILDCARD), "{c:?}");
}

/// `HttpResult` is a reserved name: a user type cannot take it, so no user
/// type is caught by the rule.
#[test]
fn no_user_type_can_be_named_http_result() {
    let c = codes("commons app.probe\n\ntype HttpResult = enum { A, B }\n");
    assert_eq!(
        c,
        ["bynk.resolve.reserved_builtin_type"],
        "a user `HttpResult` is refused as a reserved name"
    );
}
