//! The `Duration` primitive (v0.86, ADR 0112): literals type as `Duration`,
//! the operator surface (D3/D4) accepts the blessed forms and rejects the rest,
//! and the kernel/static conversions resolve.
//!
//! Each case is a single-file `commons` (which the single-file `compile` API
//! accepts), so no `context` wrapper is needed.

/// Compile `body` as the sole expression of a `commons` function with return
/// type `ret`, returning the diagnostic category codes (empty on success).
fn codes(ret: &str, body: &str) -> Vec<String> {
    let src = format!("commons demo\n\nfn f() -> {ret} {{\n  {body}\n}}\n");
    match bynkc::compile(&src, "demo.bynk") {
        Ok(_) => Vec::new(),
        Err(e) => e.iter().map(|d| d.category.to_string()).collect(),
    }
}

// -- literals + the blessed operator surface (D3/D4) --

#[test]
fn duration_literal_and_arithmetic_type_cleanly() {
    assert_eq!(
        codes("Duration", "5.minutes + 30.seconds"),
        Vec::<String>::new()
    );
    assert_eq!(codes("Duration", "2.minutes * 3"), Vec::<String>::new());
    assert_eq!(codes("Duration", "3 * 1.hours"), Vec::<String>::new());
    assert_eq!(
        codes("Duration", "1.hours - 10.minutes"),
        Vec::<String>::new()
    );
}

#[test]
fn every_unit_is_accepted() {
    assert_eq!(
        codes(
            "Duration",
            "100.milliseconds + 1.seconds + 1.minutes + 1.hours + 1.days"
        ),
        Vec::<String>::new()
    );
}

#[test]
fn duration_comparison_types_bool() {
    assert_eq!(codes("Bool", "1.hours > 30.minutes"), Vec::<String>::new());
    assert_eq!(
        codes("Bool", "5.minutes == 5.minutes"),
        Vec::<String>::new()
    );
}

#[test]
fn int_plus_duration_is_rejected() {
    // v0.90 (ADR 0114 D5): ADR 0112 D4's `Int`↔`Duration` clock-math coercion is
    // withdrawn — timestamp math goes through `Instant`, so `Int + Duration` is
    // now a `no_numeric_coercion` error.
    assert_eq!(
        codes("Int", "1000 + 5.minutes"),
        vec!["bynk.types.no_numeric_coercion".to_string()]
    );
    assert_eq!(
        codes("Int", "1000 - 5.minutes"),
        vec!["bynk.types.no_numeric_coercion".to_string()]
    );
}

#[test]
fn instant_clock_math_types_cleanly() {
    // ADR 0114 D3: advancing/retreating an instant by a span; the span between.
    assert_eq!(
        codes("Instant", "Instant.fromEpochMillis(1000) + 5.minutes"),
        Vec::<String>::new()
    );
    assert_eq!(
        codes("Instant", "Instant.fromEpochMillis(1000) - 5.minutes"),
        Vec::<String>::new()
    );
    assert_eq!(
        codes(
            "Duration",
            "Instant.fromEpochMillis(2000) - Instant.fromEpochMillis(1000)"
        ),
        Vec::<String>::new()
    );
}

// -- the conversions (D5) --

#[test]
fn to_millis_and_static_constructor_resolve() {
    assert_eq!(codes("Int", "5.minutes.toMillis()"), Vec::<String>::new());
    assert_eq!(
        codes("Duration", "Duration.millis(1000)"),
        Vec::<String>::new()
    );
}

// -- rejected forms --

#[test]
fn duration_plus_int_is_rejected() {
    // `Duration + Int` is NOT the sanctioned mix (only `Int + Duration` is).
    let cs = codes("Duration", "5.minutes + 3");
    assert!(
        cs.contains(&"bynk.types.no_numeric_coercion".to_string()),
        "{cs:?}"
    );
}

#[test]
fn duration_times_duration_is_rejected() {
    let cs = codes("Duration", "5.minutes * 2.minutes");
    assert!(
        cs.contains(&"bynk.types.no_numeric_coercion".to_string()),
        "{cs:?}"
    );
}

#[test]
fn duration_compared_with_int_is_rejected() {
    let cs = codes("Bool", "5.minutes > 3");
    assert!(
        cs.contains(&"bynk.types.type_mismatch".to_string()),
        "{cs:?}"
    );
}

#[test]
fn unknown_unit_is_a_plain_field_access_error() {
    // `1.fortnights` is not a unit, so it stays a field access on `Int`.
    let cs = codes("Duration", "1.fortnights");
    assert!(
        !cs.is_empty(),
        "an unknown unit must not type as a Duration"
    );
    assert!(
        !cs.contains(&"bynk.duration.literal_overflow".to_string()),
        "{cs:?}"
    );
}

#[test]
fn duration_literal_overflow_is_rejected() {
    // `days` factor is 86_400_000; a huge magnitude overflows i64 millis.
    let cs = codes("Duration", "9999999999999.days");
    assert!(
        cs.contains(&"bynk.duration.literal_overflow".to_string()),
        "{cs:?}"
    );
}

#[test]
fn unknown_duration_method_is_rejected() {
    let cs = codes("Int", "5.minutes.toFortnights()");
    assert!(
        cs.contains(&"bynk.types.method_not_found".to_string()),
        "{cs:?}"
    );
}

#[test]
fn unknown_duration_static_is_rejected() {
    let cs = codes("Duration", "Duration.fortnights(3)");
    assert!(
        cs.contains(&"bynk.resolve.unknown_static_member".to_string()),
        "{cs:?}"
    );
}
