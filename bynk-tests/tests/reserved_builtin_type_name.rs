//! #637: the compiler-known built-in type names (`List`, `Map`, `Query`, …) are
//! reserved in *type* position — a `type` declaration may not reuse one — but
//! remain ordinary identifiers in *value* position. This pins both halves of the
//! contract the keyword reference now documents; the type-position rejection is
//! also covered end-to-end by negative fixture `395_reserved_builtin_type_redeclare`.

/// A built-in type name in value position — a record field, a `let` binding, and
/// a reference to that binding — compiles clean. If this regresses, the docs'
/// claim that these names are usable as value identifiers is false.
#[test]
fn builtin_type_names_are_free_in_value_position() {
    let source = "\
commons demo {
  type R = { Map: Int, List: Int }
  fn f() -> Int {
    let Query = 5
    let List = 7
    Query + List
  }
}
";
    let result = bynkc::compile(source, "value_position.bynk");
    assert!(
        result.is_ok(),
        "built-in type names must be usable as value-position identifiers, got: {:#?}",
        result.err()
    );
}

/// The same name in type position is rejected at the declaration, with the
/// dedicated diagnostic rather than an incoherent downstream error.
#[test]
fn builtin_type_name_is_reserved_in_type_position() {
    let source = "commons demo {\n  type Query = String\n}\n";
    let errors = bynkc::compile(source, "type_position.bynk")
        .expect_err("redeclaring a built-in type name must fail");
    assert!(
        errors
            .iter()
            .any(|e| e.category == "bynk.resolve.reserved_builtin_type"),
        "expected `bynk.resolve.reserved_builtin_type`, got: {:#?}",
        errors
    );
}
