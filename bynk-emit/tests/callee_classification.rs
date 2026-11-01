//! Moved here from `bynk-check/tests/` at #1541: a cross-crate agreement
//! check between `bynk-check`'s classification and `bynk-emit`'s emission
//! belongs in the downstream crate (`bynk-emit` already depends on
//! `bynk-check`), not behind a `bynk-check`-on-`bynk-emit` dev-dependency
//! that existed only for `differential_analysis.rs`'s now-deleted
//! comparison — this file was `bynk-check`'s only other reason to keep it.
//!
//! P6.0 (#1139), the "Done when" differential check: for a representative
//! call shape per [`Callee`] variant reachable from a plain `fn` body,
//! assert that the classification `bynk-check`'s own dispatch
//! (`checker::calls`) records agrees with what `bynk-emit`'s existing,
//! **untouched** `lower_method_call`/`lower_call` independently classifies
//! the same call site as — proven by the shape of the TypeScript
//! `bynk-emit` actually emits for it, not by re-deriving the classification
//! a second time in this test.
//!
//! Each case parses+resolves+checks the fixture once (via `bynk-check`
//! directly, to read `TypedCommons::callees`) and separately compiles the
//! same source through `bynk_emit::compile` (a second, independent parse —
//! `bynk-emit`'s real, production entry point, unmodified by this slice).
//! Category-by-category agreement is asserted by pattern-matching the
//! recorded `Callee` and independently pattern-matching a substring of the
//! emitted TS that only that call shape could have produced.
//!
//! Scope note: `Callee::Capability`/`CrossCap`/`Cross`/`Agent`/`AgentInit`/
//! `TestService` (6 of the enum's 15 variants) all dispatch only inside a
//! handler/test body, which needs the full context/service/capability
//! project pipeline (`context_checks::check_context_declarations`) to check
//! at all — `bynk-check`'s single-file `resolver::resolve`/`checker::check`
//! entry points this file otherwise uses never reach them, and the
//! project-level entry point that does (`analyse_project`) does not thread
//! a `callees` sink (only `expr_types`/`requirements`/etc.) — out of scope
//! for this slice (P6.0 populates the sink; wiring every analysis entry
//! point to expose it is a later concern, same footing as the LSP query the
//! doc comment on `Callee` already defers). The remaining 9 variants —
//! `Fn`, `Ctor`, `Refine`, `Unsafe`, `Static`, `Method`, `Kernel`,
//! `Intrinsic`, `Value` — are exactly the ones dispatched from an ordinary
//! `fn` body (by `check_call` directly, or by `check_method_call` via its
//! own bare-ident receiver branches — `check_static_call`'s capability arm
//! is the only one of the three top-level dispatchers this file's fixtures
//! never reach), so they're covered directly below.

use bynk_check::checker::{self, Callee};
use bynk_check::resolver;
use bynk_syntax::{lexer, parser};

/// Check `source` with `bynk-check` directly (bypassing `bynk-emit`) and
/// return every `Callee` its checker recorded for this unit.
fn callees_for(source: &str) -> Vec<Callee> {
    let tokens = lexer::tokenize(source).expect("lex");
    let (commons, _warnings) = parser::parse_with_warnings(&tokens, source).expect("parse");
    let resolved = resolver::resolve(commons).expect("resolve");
    let typed = checker::check(resolved).expect("check");
    typed.callees.into_values().collect()
}

/// `bynk-emit`'s real, unmodified single-file compile — the independent
/// second implementation this test differentials against.
fn emit(source: &str) -> String {
    bynk_emit::compile(source, "test.bynk").expect("compile")
}

#[test]
fn free_function_call_is_classified_fn_and_emits_a_plain_call() {
    let src = r#"
commons demo {
  fn double(n: Int) -> Int {
    n * 2
  }

  fn use_it(n: Int) -> Int {
    double(n)
  }
}
"#;
    let callees = callees_for(src);
    assert!(
        callees
            .iter()
            .any(|c| matches!(c, Callee::Fn(f, _) if f.name.display() == "double")),
        "expected a Callee::Fn(\"double\"), got {callees:?}"
    );
    let ts = emit(src);
    assert!(
        ts.contains("double(n)"),
        "expected a plain function-call emission, got:\n{ts}"
    );
}

#[test]
fn refined_of_constructor_is_classified_refine_and_emits_the_of_helper() {
    let src = r#"
commons demo {
  type Reps = Int where InRange(1, 100)

  fn make(n: Int) -> Result[Reps, ValidationError] {
    Reps.of(n)
  }
}
"#;
    let callees = callees_for(src);
    assert!(
        callees
            .iter()
            .any(|c| matches!(c, Callee::Refine(t) if t.name.name == "Reps")),
        "expected a Callee::Refine(\"Reps\"), got {callees:?}"
    );
    let ts = emit(src);
    assert!(
        ts.contains("Reps.of(n)") && ts.contains("of(value: number)"),
        "expected the refined type's own `.of` runtime-constructor emission shape, got:\n{ts}"
    );
}

#[test]
fn opaque_unsafe_constructor_is_classified_unsafe() {
    // `Refine`/`Unsafe` are the pair most likely to be confused by a later
    // consumer — `.of`/`.unsafe` on the *same* opaque decl both carry an
    // `Arc<TypeDecl>` and differ only by which variant recorded them — so
    // this pins `.unsafe` down against the emitter independently of the
    // `.of` case above.
    let src = r#"
commons demo {
  type Token = opaque String

  fn make(s: String) -> Token {
    Token.unsafe(s)
  }
}
"#;
    let callees = callees_for(src);
    assert!(
        callees
            .iter()
            .any(|c| matches!(c, Callee::Unsafe(t) if t.name.name == "Token")),
        "expected a Callee::Unsafe(\"Token\"), got {callees:?}"
    );
    let ts = emit(src);
    assert!(
        ts.contains("unsafe(value: string): Token") && ts.contains("return Token.unsafe(s)"),
        "expected the opaque `.unsafe` constructor's own emission shape — a plain \
         (non-`Result`-wrapped) brand cast, unlike `.of`'s validating constructor — got:\n{ts}"
    );
}

#[test]
fn bare_sum_variant_construction_is_classified_ctor() {
    let src = r#"
commons demo {
  type Shape =
    | Circle(radius: Int)
    | Square(side: Int)

  fn make(n: Int) -> Shape {
    Circle(n)
  }
}
"#;
    let callees = callees_for(src);
    assert!(
        callees.iter().any(
            |c| matches!(c, Callee::Ctor { sum, tag } if sum.name.name == "Shape" && tag == "Circle")
        ),
        "expected a Callee::Ctor{{ sum: Shape, tag: Circle }}, got {callees:?}"
    );
    let ts = emit(src);
    assert!(
        ts.contains("Circle") && ts.contains("radius"),
        "expected the variant constructor's own emission shape, got:\n{ts}"
    );
}

#[test]
fn qualified_variant_construction_is_also_classified_ctor() {
    let src = r#"
commons demo {
  type Shape =
    | Circle(radius: Int)
    | Square(side: Int)

  fn make(n: Int) -> Shape {
    Shape.Circle(n)
  }
}
"#;
    let callees = callees_for(src);
    assert!(
        callees.iter().any(
            |c| matches!(c, Callee::Ctor { sum, tag } if sum.name.name == "Shape" && tag == "Circle")
        ),
        "expected a Callee::Ctor{{ sum: Shape, tag: Circle }} for the qualified form too, got {callees:?}"
    );
    let ts = emit(src);
    assert!(
        ts.contains("Circle") && ts.contains("radius"),
        "expected the variant constructor's own emission shape, got:\n{ts}"
    );
}

#[test]
fn user_static_method_is_classified_static() {
    let src = r#"
commons demo {
  type Point = { x: Int, y: Int }

  fn Point.origin() -> Point {
    Point { x: 0, y: 0 }
  }

  fn use_it() -> Point {
    Point.origin()
  }
}
"#;
    let callees = callees_for(src);
    assert!(
        callees
            .iter()
            .any(|c| matches!(c, Callee::Static(f) if f.name.display().ends_with("origin"))),
        "expected a Callee::Static(\"Point.origin\"), got {callees:?}"
    );
    let ts = emit(src);
    assert!(
        ts.contains("origin("),
        "expected the static method's own emission shape, got:\n{ts}"
    );
}

#[test]
fn user_instance_method_is_classified_method() {
    let src = r#"
commons demo {
  type Point = { x: Int, y: Int }

  fn Point.shiftX(self, dx: Int) -> Point {
    Point { x: self.x + dx, y: self.y }
  }

  fn use_it(p: Point) -> Point {
    p.shiftX(1)
  }
}
"#;
    let callees = callees_for(src);
    assert!(
        callees
            .iter()
            .any(|c| matches!(c, Callee::Method(f, _) if f.name.display().ends_with("shiftX"))),
        "expected a Callee::Method(\"Point.shiftX\"), got {callees:?}"
    );
    let ts = emit(src);
    assert!(
        ts.contains("shiftX("),
        "expected the instance method's own emission shape, got:\n{ts}"
    );
}

#[test]
fn list_kernel_method_is_classified_kernel() {
    let src = r#"
commons demo {
  fn double_all(xs: List[Int]) -> List[Int] {
    xs.map((x) => x * 2)
  }
}
"#;
    let callees = callees_for(src);
    assert!(
        callees
            .iter()
            .any(|c| matches!(c, Callee::Kernel { op, .. } if op == "map")),
        "expected a Callee::Kernel{{ op: \"map\" }}, got {callees:?}"
    );
    let ts = emit(src);
    assert!(
        ts.contains(".map("),
        "expected the List kernel method's own emission shape (a native `.map`), got:\n{ts}"
    );
}

#[test]
fn duration_millis_static_is_classified_intrinsic() {
    // Not differential against `emit`, unlike the other cases: `Duration` has
    // no runtime representation distinct from a bare `Int` (a compile-time-
    // only brand, ADR 0112), so `Duration.millis(n)` erases to the identity
    // conversion — the emitted TS is just `return n;`, with no "Duration" or
    // "millis" substring left to assert on. `bynk-emit`'s own
    // `check_duration_static`/lowering agreement is exercised by the
    // existing fixture suite (e.g. `bynkc/tests/fixtures/positive`'s
    // Duration cases); this test only pins the checker's own classification.
    let src = r#"
commons demo {
  fn make(n: Int) -> Duration {
    Duration.millis(n)
  }
}
"#;
    let callees = callees_for(src);
    assert!(
        callees.iter().any(
            |c| matches!(c, Callee::Intrinsic { ns, op } if *ns == "Duration" && op == "millis")
        ),
        "expected a Callee::Intrinsic{{ ns: \"Duration\", op: \"millis\" }}, got {callees:?}"
    );
}

#[test]
fn function_typed_local_application_is_classified_value() {
    let src = r#"
commons demo {
  fn apply(f: (Int) -> Int, x: Int) -> Int {
    f(x)
  }
}
"#;
    let callees = callees_for(src);
    assert!(
        callees
            .iter()
            .any(|c| matches!(c, Callee::Value(name) if name == "f")),
        "expected a Callee::Value(\"f\"), got {callees:?}"
    );
    let ts = emit(src);
    assert!(
        ts.contains("f(x)"),
        "expected a plain application emission for the function-typed local, got:\n{ts}"
    );
}
