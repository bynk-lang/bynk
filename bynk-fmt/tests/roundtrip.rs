//! Round-trip + idempotency tests against the bynkc positive fixture corpus.
//!
//! For every single-file fixture (those with `input.bynk`), we:
//!  1. Format the source. It must succeed.
//!  2. Format the result again. It must equal the first format.
//!  3. Parse the formatted result. It must parse without errors (semantic
//!     preservation: formatter does not introduce syntax errors).
//!
//! Project-shaped fixtures (those with `src/`) are walked recursively and
//! each `.bynk` file is tested the same way.

use std::fs;
use std::path::{Path, PathBuf};

use bynk_fmt::{FormatOptions, format_source};
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::parse_units;

fn fixture_dirs() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("bynkc/tests/fixtures/positive");
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(&root) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn collect_bynk_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(rd) = fs::read_dir(&dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().and_then(|e| e.to_str()) == Some("bynk") {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

fn check_file(path: &Path, opts: &FormatOptions) -> Result<(), String> {
    let source = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let once = format_source(&source, opts).map_err(|e| {
        format!(
            "first format of {}: {} errors",
            path.display(),
            e.errors.len()
        )
    })?;
    let twice = format_source(&once, opts).map_err(|e| {
        format!(
            "second format of {}: {} errors",
            path.display(),
            e.errors.len()
        )
    })?;
    if once != twice {
        return Err(format!(
            "formatter not idempotent on {}:\n--- once ---\n{once}\n--- twice ---\n{twice}",
            path.display()
        ));
    }
    // Round-trip parse.
    let tokens = tokenize(&once).map_err(|e| {
        format!(
            "re-tokenise formatted output {}: {}",
            path.display(),
            e.message
        )
    })?;
    parse_units(&tokens, &once).map_err(|errs| {
        format!(
            "re-parse formatted output {}: {} errors; first: {}",
            path.display(),
            errs.len(),
            errs.first().map(|e| e.message.as_str()).unwrap_or("?")
        )
    })?;
    Ok(())
}

/// A fixture's `.bynk` files, in either layout: a lone `input.bynk`, or a
/// `src/` tree.
fn fixture_files(dir: &Path) -> Vec<PathBuf> {
    let input = dir.join("input.bynk");
    if input.exists() {
        vec![input]
    } else {
        collect_bynk_files(&dir.join("src"))
    }
}

#[test]
fn round_trip_positive_corpus() {
    let opts = FormatOptions::default();
    let mut failures = Vec::new();
    for dir in fixture_dirs() {
        for f in fixture_files(&dir) {
            if let Err(e) = check_file(&f, &opts) {
                failures.push(e);
            }
        }
    }
    if !failures.is_empty() {
        panic!(
            "formatter round-trip failures ({}):\n{}",
            failures.len(),
            failures.join("\n\n")
        );
    }
}

/// v1.1 — comment-preservation round-trip. For each canonical fixture
/// shape, inject line comments at several positions, format twice, and
/// verify all of the original comment bodies survive both passes.
#[test]
fn round_trip_preserves_injected_comments() {
    let opts = FormatOptions::default();
    let cases: &[(&str, &[&str])] = &[
        (
            "commons x.y {\n\
             // intro\n\
             type T = Int where NonNegative  // inline\n\
             // between\n\
             fn add(a: Int, b: Int) -> Int {\n\
             let s = a + b\n\
             // before tail\n\
             s\n\
             }\n\
             // afterword\n\
             }\n",
            &[
                " intro",
                " inline",
                " between",
                " before tail",
                " afterword",
            ],
        ),
        (
            "commons x.y\n\n\
             // first decl\n\
             type T = Int where Positive\n\
             // last\n",
            &[" first decl", " last"],
        ),
    ];
    for (i, (src, expected_bodies)) in cases.iter().enumerate() {
        let once = format_source(src, &opts).expect("first format must succeed");
        for body in *expected_bodies {
            assert!(
                once.contains(&format!("//{body}")),
                "case {i}: comment `//{body}` missing from first format:\n{once}"
            );
        }
        let twice = format_source(&once, &opts).expect("second format must succeed");
        assert_eq!(once, twice, "case {i}: formatter not idempotent");
        // Re-parses cleanly.
        let tokens = tokenize(&twice).expect("re-tokenise");
        parse_units(&tokens, &twice).expect("re-parse");
    }
}

/// v0.15: the formatter must preserve `exports capability { … }` and dotted
/// cross-context `given` references verbatim. Idempotency alone would not
/// catch a clause being silently dropped (a regression class seen before),
/// so assert the round-tripped source still carries them.
#[test]
fn round_trip_preserves_cross_context_capability_syntax() {
    let opts = FormatOptions::default();
    let src = "context ops.metrics\n\n\
        consumes platform.time\n\n\
        exports capability { Stamp }\n\n\
        capability Stamp {\n\
        \x20 fn make() -> Effect[Int]\n\
        }\n\n\
        provides Stamp = ClockStamp given platform.time.Clock {\n\
        \x20 fn make() -> Effect[Int] {\n\
        \x20   let t <- platform.time.Clock.now()\n\
        \x20   t\n\
        \x20 }\n\
        }\n\n\
        service report {\n\
        \x20 on call() -> Effect[Int] given Stamp {\n\
        \x20   let s <- Stamp.make()\n\
        \x20   s\n\
        \x20 }\n\
        }\n";
    let out = format_source(src, &opts).expect("format must succeed");
    for needle in [
        "exports capability { Stamp }",
        "provides Stamp = ClockStamp given platform.time.Clock {",
        "given Stamp {",
    ] {
        assert!(out.contains(needle), "formatter dropped `{needle}`:\n{out}");
    }
    let twice = format_source(&out, &opts).expect("second format must succeed");
    assert_eq!(out, twice, "formatter not idempotent");
}

/// v0.43: string-interpolation holes (`\(expr)`) must survive formatting —
/// the round-trip corpus test proves idempotence and re-parseability, but not
/// that the holes are *preserved* (a formatter that dropped them to plain text
/// would still pass both). This pins that the `\(…)` form re-emits verbatim.
#[test]
fn round_trip_preserves_string_interpolation() {
    let opts = FormatOptions::default();
    let src = "commons demo {\n\
        \x20 fn greet(name: String, n: Int) -> String {\n\
        \x20   \"Hi, \\(name)! You are #\\(add(n, 1)).\"\n\
        \x20 }\n\
        }\n";
    let out = format_source(src, &opts).expect("format must succeed");
    assert!(
        out.contains("\"Hi, \\(name)! You are #\\(add(n, 1)).\""),
        "formatter dropped or mangled the interpolation:\n{out}"
    );
    let twice = format_source(&out, &opts).expect("second format must succeed");
    assert_eq!(out, twice, "formatter not idempotent");
}

/// v0.18: braced capability selection (`consumes U { Cap, … }`) must survive
/// formatting — in contexts and in adapter bodies. The v0.17 formatter
/// silently dropped the braces (a semantic-changing format the idempotency
/// check alone cannot catch).
#[test]
fn round_trip_preserves_braced_consumes() {
    let opts = FormatOptions::default();
    let src = "context shop.orders {\n\
        \x20 consumes bynk { Clock, Logger }\n\n\
        \x20 type Order = { sku: String, placedAt: Int }\n\
        }\n";
    let out = format_source(src, &opts).expect("format must succeed");
    assert!(
        out.contains("consumes bynk { Clock, Logger }"),
        "formatter dropped the braced selection:\n{out}"
    );
    let twice = format_source(&out, &opts).expect("second format must succeed");
    assert_eq!(out, twice, "formatter not idempotent");

    let adapter_src = "adapter tokens {\n\
        \x20 binding \"./tokens.binding.ts\" requires { \"jose\": \"^5\" }\n\
        \x20 consumes bynk { Secrets }\n\n\
        \x20 exports capability { Jwt }\n\n\
        \x20 capability Jwt {\n\
        \x20   fn sign(sub: String) -> Effect[String]\n\
        \x20 }\n\n\
        \x20 provides Jwt = JoseJwt given Secrets\n\
        }\n";
    let out = format_source(adapter_src, &opts).expect("format must succeed");
    for needle in [
        "consumes bynk { Secrets }",
        "provides Jwt = JoseJwt given Secrets",
    ] {
        assert!(out.contains(needle), "formatter dropped `{needle}`:\n{out}");
    }
    let twice = format_source(&out, &opts).expect("second format must succeed");
    assert_eq!(out, twice, "formatter not idempotent");
}
