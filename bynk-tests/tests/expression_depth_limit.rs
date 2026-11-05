//! Regression coverage for the front end's expression-depth bound (#714).
//!
//! Several expression builders assemble a tree *iteratively* rather than by
//! recursing `parse_expr`, so they never trip the recursion guard that catches
//! nested source (#713): the associative operator chains (`+`, `*`, `&&`, `||`),
//! and the postfix receiver spine (`a.b.c…`, `f()?.g()…`). A flat `1 + 1 + … + 1`
//! or a long `x.a.a.a…` produces an arbitrarily deep tree that overflows every
//! recursive consumer downstream (the checker's `type_of`, the formatter, the
//! emitter, and the AST's own recursive `Drop`). A `bynkc check` on a 20 000-term
//! chain used to abort with `fatal runtime error: stack overflow` (exit 134).
//!
//! The fix counts every such fold against the same nesting budget the parser
//! already uses for recursive descent (reported as `bynk.parse.nesting_too_deep`
//! at `MAX_NESTING_DEPTH = 64`), so the tree is never built past the bound.
//! Most tests here run on an ordinary (small, ~2 MiB) test thread: an unbounded
//! builder would overflow this thread, so a passing run is itself the evidence
//! that the tree stays bounded. The one exception is `chain_composes_with_nesting`,
//! which needs deep parentheses — whose ladder-per-level descent overflows a
//! small stack before the logical bound — so it runs on a generous stack to
//! exercise the (stack-independent) depth accounting.

/// A single-file commons whose `f` returns an `Int` from a `+` chain of `terms`
/// ones.
fn chain_source(terms: usize) -> String {
    let chain = vec!["1"; terms].join(" + ");
    format!("commons demo.deep\nfn f() -> Int {{\n  {chain}\n}}\n")
}

fn is_too_deep(errors: &[bynk_syntax::CompileError]) -> bool {
    errors
        .iter()
        .any(|e| e.category == "bynk.parse.nesting_too_deep")
}

#[test]
fn chain_within_the_limit_compiles() {
    // 64 folds sit exactly on the bound; a chain at or below it is a perfectly
    // valid program and must still compile.
    assert!(
        bynkc::compile(&chain_source(64), "within.bynk").is_ok(),
        "a chain at the depth bound should compile",
    );
}

#[test]
fn chain_one_past_the_limit_is_rejected() {
    // The trip point: one more term than `chain_within_the_limit_compiles`.
    let errors = bynkc::compile(&chain_source(65), "edge.bynk")
        .expect_err("a chain one past the bound must be rejected");
    assert!(is_too_deep(&errors), "got: {:?}", categories(&errors));
}

#[test]
fn pathological_chain_is_rejected_not_crashed() {
    // The exact reproduction from #714: a chain far past the bound. The process
    // must survive (no stack overflow) and report the diagnostic.
    let errors = bynkc::compile(&chain_source(20_000), "deep.bynk")
        .expect_err("an over-deep chain must be rejected");
    assert!(is_too_deep(&errors), "got: {:?}", categories(&errors));
}

#[test]
fn postfix_receiver_spine_is_bounded() {
    // A long field chain builds a left-nested receiver spine iteratively, the
    // same crash class as a `+` chain — it must be bounded too.
    let src = format!(
        "commons demo.deep\nfn f() -> Int {{\n  x{}\n}}\n",
        ".a".repeat(5_000)
    );
    let errors =
        bynkc::compile(&src, "postfix.bynk").expect_err("a long field spine must be rejected");
    assert!(is_too_deep(&errors), "got: {:?}", categories(&errors));
}

#[test]
fn unary_run_is_bounded() {
    // `!!!…x` self-recurses in `parse_unary`; a long run must diagnose rather
    // than overflow the parser's own stack.
    let src = format!(
        "commons demo.deep\nfn f() -> Bool {{\n  {}x\n}}\n",
        "!".repeat(5_000)
    );
    let errors = bynkc::compile(&src, "unary.bynk").expect_err("a long unary run must be rejected");
    assert!(is_too_deep(&errors), "got: {:?}", categories(&errors));
}

#[test]
fn chain_composes_with_nesting() {
    // The bound is a single shared budget, so a chain composes with the ambient
    // nesting depth: a 40-term chain and 30 levels of parens are each fine
    // alone, but nesting the chain 30 parens deep exceeds the budget along that
    // root-to-leaf path and is rejected. This locks in the shared-budget
    // composition against regression.
    //
    // Runs on a generous stack: unlike a chain fold, each paren re-descends the
    // whole precedence ladder, so 30 parens overflow a default (~2 MiB) test
    // thread before the logical bound is even reached. This test exercises the
    // parser's depth *accounting*, which is stack-independent.
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let chain = vec!["1"; 40].join(" + ");
            let alone = format!("commons demo.deep\nfn f() -> Int {{\n  {chain}\n}}\n");
            let parens_alone = format!(
                "commons demo.deep\nfn f() -> Int {{\n  {}1{}\n}}\n",
                "(".repeat(30),
                ")".repeat(30)
            );
            let nested = format!(
                "commons demo.deep\nfn f() -> Int {{\n  {}{chain}{}\n}}\n",
                "(".repeat(30),
                ")".repeat(30)
            );
            assert!(
                bynkc::compile(&alone, "a.bynk").is_ok(),
                "the chain alone is valid"
            );
            assert!(
                bynkc::compile(&parens_alone, "p.bynk").is_ok(),
                "the nesting alone is valid"
            );
            let errors = bynkc::compile(&nested, "n.bynk")
                .expect_err("chain + nesting together exceed the budget");
            assert!(is_too_deep(&errors), "got: {:?}", categories(&errors));
        })
        .unwrap()
        .join()
        .unwrap();
}

fn categories(errors: &[bynk_syntax::CompileError]) -> Vec<&str> {
    errors.iter().map(|e| e.category).collect()
}
