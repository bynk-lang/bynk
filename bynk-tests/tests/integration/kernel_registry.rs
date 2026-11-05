//! v0.30.2 (ADR 0063): the enumerable kernel-method registry must not drift
//! from the checker's dispatch. Every method the registry lists is driven
//! through the real checker on a receiver of the right type; none may be
//! rejected as `method_not_found` (a phantom registry entry). Args are
//! omitted, so a recognised method surfaces only as an arity error — which is
//! fine; we assert solely on `method_not_found`.
//!
//! `QUERY_METHODS` is well-formedness-checked here but not dispatch-pinned:
//! a `Query`-typed receiver only ever arises through a `store` field, which
//! needs a `context` the single-file `diagnose` this file's probe uses can't
//! accept — its dispatch pin lives in `query_kernel_registry.rs` instead,
//! compiled as a one-file project (the `store_op_registry.rs` shape).

use bynk_check::kernel_methods::{
    EFFECT_RESULT_METHODS, FLOAT_METHODS, INT_METHODS, KernelMethod, LIST_METHODS, MAP_METHODS,
    OPTION_METHODS, QUERY_METHODS, RESULT_METHODS, STRING_METHODS,
};

/// `(let binding, receiver expr, methods)` — a receiver of each kernel type.
fn cases() -> Vec<(&'static str, &'static [KernelMethod])> {
    vec![
        ("let i = 1", INT_METHODS),
        ("let fl = 1.0", FLOAT_METHODS),
        ("let st = \"x\"", STRING_METHODS),
        ("let li = [1]", LIST_METHODS),
        ("let op: Option[Int] = Some(1)", OPTION_METHODS),
        ("let re: Result[Int, String] = Ok(1)", RESULT_METHODS),
        (
            "let er: Effect[Result[Int, String]] = Effect.pure(Ok(1))",
            EFFECT_RESULT_METHODS,
        ),
        ("let mp: Map[String, Int] = Map.empty()", MAP_METHODS),
    ]
}

fn binding_name(decl: &str) -> &str {
    // `let <name>...` — the identifier after `let`.
    decl["let ".len()..].split([' ', ':']).next().unwrap()
}

#[test]
fn kernel_registry_pins_dispatch() {
    let cases = cases();
    let mut body = String::from("commons probe.registry\n  fn probe() -> Int {\n");
    for (decl, _) in &cases {
        body += &format!("    {decl}\n");
    }
    for (decl, methods) in &cases {
        let recv = binding_name(decl);
        for meth in *methods {
            // No args — a recognised method gives an arity error, never
            // `method_not_found`.
            body += &format!("    let _ = {recv}.{}()\n", meth.name);
        }
    }
    body += "    0\n  }\n";

    let diags = bynk_ide::diagnose(&body);
    let phantom: Vec<_> = diags
        .iter()
        .filter(|d| d.error.category == "bynk.types.method_not_found")
        .map(|d| d.error.message.clone())
        .collect();
    assert!(
        phantom.is_empty(),
        "registry lists method(s) the checker rejects as not-found:\n{}",
        phantom.join("\n")
    );
}

/// The match-arm method names inside `check_list_kernel_method`'s real
/// source — the checker's actual `List` dispatch table. Scraped by regex
/// (the same technique `diagnostics_registry.rs` uses to keep the diagnostic
/// registry honest against emit sites) rather than duplicated by hand, so
/// this can't itself drift from what the checker accepts.
fn list_kernel_dispatch_arm_names() -> std::collections::BTreeSet<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bynk-check/src/checker/kernels.rs");
    let text = std::fs::read_to_string(&path).expect("read checker/kernels.rs");
    let start = text
        .find("fn check_list_kernel_method")
        .expect("find check_list_kernel_method");
    let after = &text[start..];
    let open = after.find('{').expect("find opening brace");
    let mut depth = 0i32;
    let mut end = None;
    for (i, c) in after[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &after[..end.expect("find matching closing brace") + 1];

    // A quoted method name immediately followed by `|` (another alternative
    // in the same arm), `=>`, or an `if` guard — the three ways a match-arm
    // pattern can end. Nested matches/guards elsewhere in the body use no
    // quoted string in that position, so this doesn't pick up false matches.
    let re = regex::Regex::new(r#""([a-zA-Z]+)"\s*(\||=>|if\b)"#).unwrap();
    let mut names: std::collections::BTreeSet<String> =
        re.captures_iter(body).map(|c| c[1].to_string()).collect();
    // These arms dispatch via `builtin_names::methods` constants (bare
    // identifiers, not string literals), so the textual scan above can't see
    // them — added by their known constant values instead.
    for known in [
        "foldEff",
        "forEach",
        "parTraverse",
        "traverseAll",
        "parTraverseAll",
        "traverseTry",
        "parTraverseTry",
    ] {
        names.insert(known.to_string());
    }
    names
}

/// The direction `kernel_registry_pins_dispatch` can't check: a method the
/// checker's `List` dispatch accepts but `LIST_METHODS` doesn't list — how
/// `join`/`joinOn`/`leftJoin`/`groupBy` (ADR 0116/0120) fell behind
/// `QUERY_METHODS`' copy of the same vocabulary undetected. `upTo` is the one
/// deliberate exception: a History-only accessor gated on the element being a
/// synthesised `__History_*_Step` record, not a general `List` method.
#[test]
fn list_kernel_registry_has_no_undocumented_methods() {
    let dispatched = list_kernel_dispatch_arm_names();
    let registered: std::collections::BTreeSet<String> =
        LIST_METHODS.iter().map(|m| m.name.to_string()).collect();
    let missing: Vec<&String> = dispatched
        .difference(&registered)
        .filter(|n| n.as_str() != "upTo")
        .collect();
    assert!(
        missing.is_empty(),
        "check_list_kernel_method accepts method(s) LIST_METHODS doesn't list: {missing:?}"
    );
}

#[test]
fn registries_are_well_formed() {
    for methods in [
        INT_METHODS,
        FLOAT_METHODS,
        STRING_METHODS,
        LIST_METHODS,
        MAP_METHODS,
        OPTION_METHODS,
        RESULT_METHODS,
        EFFECT_RESULT_METHODS,
        QUERY_METHODS,
    ] {
        assert!(!methods.is_empty());
        for meth in methods {
            assert!(!meth.name.is_empty());
            // The signature begins with the method name (a `name(...) -> T`).
            assert!(
                meth.signature.starts_with(meth.name),
                "signature {:?} should lead with {:?}",
                meth.signature,
                meth.name
            );
        }
    }
}
