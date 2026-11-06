//! The stable-toolchain corner of the fuzzing setup (#516).
//!
//! The real fuzzers live in `fuzz/` (nightly + libFuzzer, run on a schedule);
//! this test runs the same invariants over deterministic mutations of the
//! fixture corpus on every ordinary `cargo test`, so the class of bug the
//! fuzzers hunt (panics, hangs, mis-aligned spans on adversarial input)
//! keeps a fast in-tree tripwire.
//!
//! Mutations use a fixed-seed LCG — the run is byte-for-byte reproducible,
//! and a failure message names the source fixture, the mutation, and the
//! mutant text.

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

/// Deterministic LCG (Numerical Recipes constants) — no dependency, stable
/// across platforms and releases.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn corpus() -> Vec<(PathBuf, String)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("bynk")
                && let Ok(text) = fs::read_to_string(&p)
            {
                out.push((p, text));
            }
        }
    }
    out.sort();
    assert!(
        out.len() > 100,
        "corpus discovery looks broken: {}",
        out.len()
    );
    out
}

/// One deterministic mutation of `source`. The menu favours the shapes that
/// have historically broken parsers: truncation (unclosed constructs), byte
/// deletion/duplication, delimiter swaps, and non-ASCII injection.
fn mutate(source: &str, rng: &mut Lcg) -> String {
    let mut s = source.to_string();
    match rng.below(6) {
        // Truncate at an arbitrary char boundary.
        0 => {
            let cut = rng.below(s.len() + 1);
            let cut = (0..=cut)
                .rev()
                .find(|&i| s.is_char_boundary(i))
                .unwrap_or(0);
            s.truncate(cut);
        }
        // Delete one char.
        1 => {
            if let Some((at, c)) = s.char_indices().nth(rng.below(s.chars().count().max(1))) {
                s.replace_range(at..at + c.len_utf8(), "");
            }
        }
        // Duplicate a random slice.
        2 => {
            let start = (0..=rng.below(s.len() + 1))
                .rev()
                .find(|&i| s.is_char_boundary(i))
                .unwrap_or(0);
            let end = (start..=s.len().min(start + 16))
                .rev()
                .find(|&i| s.is_char_boundary(i))
                .unwrap_or(start);
            let slice = s[start..end].to_string();
            s.insert_str(start, &slice);
        }
        // Swap a delimiter for a mismatched one.
        3 => {
            let from = ['{', '}', '(', ')', '[', ']', '"'][rng.below(7)];
            let to = ['}', '{', ')', '(', ']', '[', '\''][rng.below(7)];
            if let Some(at) = s.find(from) {
                s.replace_range(at..at + 1, &to.to_string());
            }
        }
        // Inject non-ASCII at a char boundary.
        4 => {
            let at = (0..=rng.below(s.len() + 1))
                .rev()
                .find(|&i| s.is_char_boundary(i))
                .unwrap_or(0);
            s.insert_str(at, ["é", "→", "🦀", "\u{202e}"][rng.below(4)]);
        }
        // Splice a keyword into the middle.
        _ => {
            let at = (0..=rng.below(s.len() + 1))
                .rev()
                .find(|&i| s.is_char_boundary(i))
                .unwrap_or(0);
            s.insert_str(
                at,
                [" match ", " on ", " := ", " where ", " commons "][rng.below(5)],
            );
        }
    }
    s
}

fn check_spans(source: &str, span: bynk_syntax::span::Span) {
    assert!(
        span.start <= span.end && span.end <= source.len(),
        "span out of bounds: {span:?} in {} bytes",
        source.len()
    );
    assert!(
        source.is_char_boundary(span.start) && source.is_char_boundary(span.end),
        "span splits a codepoint: {span:?}"
    );
}

/// The parse-target invariants: no panics, spans in-bounds and char-aligned,
/// through both the bailing CLI path and the recovering LSP path.
fn parse_invariants(source: &str) {
    match bynk_syntax::lexer::tokenize(source) {
        Err(e) => check_spans(source, e.span),
        Ok(tokens) => {
            if let Err(errors) = bynk_syntax::parser::parse_units(&tokens, source) {
                for e in errors {
                    check_spans(source, e.span);
                }
            }
            let (_, errors) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, source);
            for e in errors {
                check_spans(source, e.span);
            }
        }
    }
}

#[test]
fn mutated_corpus_upholds_parse_invariants() {
    let mut rng = Lcg(0xB1_4C);
    for (path, text) in corpus() {
        for round in 0..6 {
            let mutant = mutate(&text, &mut rng);
            let result = catch_unwind(AssertUnwindSafe(|| parse_invariants(&mutant)));
            if let Err(payload) = result {
                let msg = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "non-string panic".into());
                panic!(
                    "parse invariants broken on mutant {round} of {}:\n{msg}\n--- mutant ---\n{mutant}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn mutated_corpus_never_panics_the_compiler() {
    let mut rng = Lcg(0xC0_11_EC);
    for (i, (path, text)) in corpus().into_iter().enumerate() {
        // The full pipeline is ~100× slower than a parse; sample the corpus
        // so this stays a smoke test, not a build-time tax.
        if i % 10 != 0 {
            continue;
        }
        let mutant = mutate(&text, &mut rng);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = bynkc::compile(&mutant, "fuzz-smoke.bynk");
        }));
        if result.is_err() {
            panic!(
                "the compiler panicked on a mutant of {}:\n--- mutant ---\n{mutant}",
                path.display()
            );
        }
    }
}
