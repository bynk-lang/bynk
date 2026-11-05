//! #596: the enumerable `Query` kernel-method registry and the `store Map`
//! `.entries`/`.keys`/`.values` accessor registry must not drift from the
//! checker's dispatch. Every entry either registry lists is driven through the
//! real checker on a `Query`-typed receiver (a `store Map`'s bare-value
//! reading); none may be rejected as `bynk.types.method_not_found` (a phantom
//! kernel method) or `bynk.store.unknown_map_accessor`/
//! `bynk.types.unknown_field` (a phantom accessor). Args are omitted for the
//! methods, so a recognised one surfaces only as an arity/argument error —
//! fine; we assert solely on the "not found" categories.
//!
//! **T1.6′: the method registry's tooth now bites both ways.**
//! `query_kernel_registry_pins_dispatch` catches a `QUERY_METHODS` entry the
//! checker rejects; `query_kernel_registry_has_no_undocumented_methods` (below,
//! mirroring `kernel_registry.rs`'s `list_kernel_registry_has_no_undocumented_methods`)
//! catches the converse — a dispatch arm `check_query_kernel_method` accepts
//! that `QUERY_METHODS` doesn't list. This is exactly how `join`/`joinOn`/
//! `leftJoin`/`groupBy` (ADR 0116/0120) fell behind the registry and the
//! hand-listed `method_not_found` vocabulary string undetected. The accessor
//! registry (`MAP_QUERY_ACCESSORS`) still bites one way only; it does not
//! check the signature *strings* either.
//!
//! `Query` only ever appears through a `store` field (or a value derived from
//! one), which needs a `context`, so the probe is compiled as a one-file
//! project in a temp directory — the same shape `store_op_registry.rs` uses.

use std::fs;

use bynk_check::builtin_names::methods as builtin_methods;
use bynk_check::kernel_methods::{KernelMethod, QUERY_METHODS};
use bynk_check::store_ops::MAP_QUERY_ACCESSORS;
use bynkc::{ProjectFailure, compile_project};

/// Compile a probe agent with `store items: Map[String, Int]` whose handler
/// evaluates each given expression (a `<recv>.<member>` access), and return
/// the diagnostic category codes.
fn probe_codes(tag: &str, exprs: &[String]) -> Vec<String> {
    let mut src = String::from(
        "context probe\n\nagent Probe {\n  key id: String\n  store items: Map[String, Int]\n\n  on call f() -> Effect[()] {\n",
    );
    for e in exprs {
        src += &format!("    let _ = {e}\n");
    }
    src += "    Effect.pure(())\n  }\n}\n";

    let root =
        std::env::temp_dir().join(format!("bynk_query_registry_{}_{tag}", std::process::id()));
    let dir = root.join("src");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("probe.bynk"), &src).unwrap();
    let out = match compile_project(&bynk_testkit::compile_options_single(dir)) {
        Ok(_) => Vec::new(),
        Err(f) => ProjectFailure::flatten(f)
            .iter()
            .map(|e| e.category.to_string())
            .collect(),
    };
    let _ = fs::remove_dir_all(&root);
    out
}

fn method_calls(methods: &[KernelMethod]) -> Vec<String> {
    methods
        .iter()
        .map(|m| format!("items.values.{}()", m.name))
        .collect()
}

#[test]
fn query_kernel_registry_pins_dispatch() {
    let codes = probe_codes("pins", &method_calls(QUERY_METHODS));
    assert!(
        !codes.iter().any(|c| c == "bynk.types.method_not_found"),
        "registry lists method(s) the checker rejects as not-found:\n{}",
        codes.join("\n")
    );
}

/// Negative control: without it, a probe that never reached `Query` dispatch
/// at all (a parse error, an unregistered store field) would report no
/// `method_not_found` and pass vacuously.
#[test]
fn the_probe_actually_reaches_query_dispatch() {
    let codes = probe_codes(
        "control",
        &["items.values.definitelyNotAMethod()".to_string()],
    );
    assert!(
        codes.iter().any(|c| c == "bynk.types.method_not_found"),
        "`items.values.definitelyNotAMethod()` should be an unknown `Query` method, got:\n{}",
        codes.join("\n")
    );
}

#[test]
fn map_query_accessor_registry_pins_dispatch() {
    let exprs: Vec<String> = MAP_QUERY_ACCESSORS
        .iter()
        .map(|a| format!("items.{}", a.name))
        .collect();
    let codes = probe_codes("accessors_pin", &exprs);
    assert!(
        !codes.iter().any(|c| c == "bynk.store.unknown_map_accessor"),
        "registry lists accessor(s) the checker rejects as unknown:\n{}",
        codes.join("\n")
    );
}

/// Negative control for the accessor registry, mirroring the method one above.
#[test]
fn the_probe_actually_reaches_map_accessor_dispatch() {
    let codes = probe_codes(
        "accessors_control",
        &["items.definitelyNotAnAccessor".to_string()],
    );
    assert!(
        codes.iter().any(|c| c == "bynk.store.unknown_map_accessor"),
        "`items.definitelyNotAnAccessor` should be an unknown map accessor, got:\n{}",
        codes.join("\n")
    );
}

/// The match-arm method names inside `check_query_kernel_method`'s real
/// source — the checker's actual `Query` dispatch table. Scraped by regex,
/// the same technique `kernel_registry.rs`'s `list_kernel_dispatch_arm_names`
/// uses for `List`, so this can't itself drift from what the checker accepts.
fn query_kernel_dispatch_arm_names() -> std::collections::BTreeSet<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bynk-check/src/checker/kernels.rs");
    let text = std::fs::read_to_string(&path).expect("read checker/kernels.rs");
    let start = text
        .find("fn check_query_kernel_method")
        .expect("find check_query_kernel_method");
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
    // pattern can end.
    let re = regex::Regex::new(r#""([a-zA-Z]+)"\s*(\||=>|if\b)"#).unwrap();
    let mut names: std::collections::BTreeSet<String> =
        re.captures_iter(body).map(|c| c[1].to_string()).collect();
    // These arms dispatch via `builtin_names::methods` constants (bare
    // identifiers, not string literals), so the textual scan above can't see
    // them — added by their actual constant values (not restated string
    // literals) so a rename of the constant can't drift this list silently.
    for known in [
        builtin_methods::TRAVERSE_ALL,
        builtin_methods::PAR_TRAVERSE_ALL,
        builtin_methods::TRAVERSE_TRY,
        builtin_methods::PAR_TRAVERSE_TRY,
    ] {
        names.insert(known.to_string());
    }
    names
}

/// The direction `query_kernel_registry_pins_dispatch` can't check: a method
/// the checker's `Query` dispatch accepts but `QUERY_METHODS` doesn't list —
/// how `join`/`joinOn`/`leftJoin`/`groupBy` (ADR 0116/0120) fell behind this
/// table undetected before, and how the `method_not_found` message's own
/// hand-listed vocabulary (now generated from `QUERY_METHODS` instead)
/// drifted from both.
#[test]
fn query_kernel_registry_has_no_undocumented_methods() {
    let dispatched = query_kernel_dispatch_arm_names();
    let registered: std::collections::BTreeSet<String> =
        QUERY_METHODS.iter().map(|m| m.name.to_string()).collect();
    let missing: Vec<&String> = dispatched.difference(&registered).collect();
    assert!(
        missing.is_empty(),
        "check_query_kernel_method accepts method(s) QUERY_METHODS doesn't list: {missing:?}"
    );
}

/// Negative control for the scrape itself: without it, `check_query_kernel_method`
/// getting renamed or restructured into a helper the regex can't see would make
/// `query_kernel_dispatch_arm_names` return an empty (or near-empty) set, and
/// `query_kernel_registry_has_no_undocumented_methods` would pass vacuously —
/// the same failure mode the other two negative controls in this file guard
/// against for the compile-based probes.
#[test]
fn the_scrape_actually_finds_dispatch_arms() {
    let dispatched = query_kernel_dispatch_arm_names();
    assert!(
        dispatched.contains("collect") && dispatched.len() >= QUERY_METHODS.len(),
        "query_kernel_dispatch_arm_names scraped too few names ({}) to be reading \
         check_query_kernel_method's real dispatch: {dispatched:?}",
        dispatched.len()
    );
}
