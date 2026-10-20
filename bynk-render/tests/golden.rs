//! Ariadne-rendered golden tests (#520).
//!
//! `bynk-render` previously had no direct tests; every diagnostic-rendering
//! regression (misplaced underlines, dropped labels) surfaced only through
//! humans. Each case renders a representative diagnostic with the colourless
//! plain renderer (byte-stable across machines) and compares against a
//! committed transcript.
//!
//! Regenerate after an intentional rendering change:
//!
//!     BYNK_BLESS=1 cargo test -p bynk-render --test golden

use std::fs;
use std::path::PathBuf;

use bynk_render::render_errors_plain;
use bynk_syntax::CompileError;
use bynk_syntax::span::Span;

struct Case {
    name: &'static str,
    source: &'static str,
    errors: Vec<CompileError>,
}

/// The byte span of `needle` in `source`.
fn find(source: &str, needle: &str) -> Span {
    let start = source
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not in source"));
    Span::new(start, start + needle.len())
}

fn cases() -> Vec<Case> {
    let regex_src = "commons demo {\n  type Bad = String where Matches(\"[unclosed\")\n}\n";
    let node_src = "commons demo {\n  type Node = { value: Int, next: Node }\n}\n";
    let non_ascii_src = "commons demo {\n  // naïve café\n  type T = Int where Positive\n  fn f() -> Int { nope }\n}\n";
    vec![
        Case {
            name: "message_and_note",
            source: regex_src,
            errors: vec![
                CompileError::new(
                    "bynk.types.invalid_regex",
                    find(regex_src, "Matches(\"[unclosed\")"),
                    "invalid regular expression in `Matches(\"[unclosed\")`",
                )
                .with_note("regex parse error (JS `RegExp` semantics): unbalanced bracket"),
            ],
        },
        Case {
            name: "secondary_label",
            source: node_src,
            errors: vec![
                CompileError::new(
                    "bynk.resolve.recursive_record_field",
                    find(node_src, "next: Node"),
                    "record `Node` cannot directly contain a field of its own type",
                )
                .with_label(find(node_src, "Node"), "type declared here")
                .with_note("wrap the recursive reference in `Option[...]` to break the cycle"),
            ],
        },
        Case {
            // The byte-vs-char regression class (#512): non-ASCII text before
            // the span must not shift the underline.
            name: "non_ascii_line",
            source: non_ascii_src,
            errors: vec![CompileError::new(
                "bynk.resolve.unknown_name",
                find(non_ascii_src, "nope"),
                "unknown name `nope`",
            )],
        },
    ]
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

#[test]
fn rendered_diagnostics_match_goldens() {
    let bless = std::env::var_os("BYNK_BLESS").is_some();
    let mut failures = Vec::new();
    for case in cases() {
        let rendered = render_errors_plain(&case.errors, case.source, "probe.bynk");
        let path = golden_dir().join(format!("{}.txt", case.name));
        if bless {
            fs::write(&path, &rendered).unwrap();
            continue;
        }
        let want = fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "could not read {} ({e}); regenerate with \
                 BYNK_BLESS=1 cargo test -p bynk-render --test golden",
                path.display()
            )
        });
        if rendered != want {
            failures.push(format!(
                "=== {} ===\n--- expected ---\n{want}\n--- actual ---\n{rendered}",
                case.name
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "rendered output drifted (regenerate with BYNK_BLESS=1 if intended):\n{}",
        failures.join("\n")
    );
}
