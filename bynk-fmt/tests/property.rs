//! Property tests for the formatter's round-trip invariants over *generated*
//! sources (#517).
//!
//! The corpus suites already pin `fmt(fmt(x)) == fmt(x)` and re-parseability
//! for every committed fixture — but the corpus tests what the corpus
//! contains. These properties assert the same invariants over a
//! grammar-directed generator, so shapes nobody has committed yet (deep
//! nesting, odd literal spellings, comments in every legal slot) are
//! exercised on every CI run.
//!
//! Invariants, for any generated source `s` the formatter accepts:
//!  1. Idempotency: `fmt(fmt(s)) == fmt(s)`.
//!  2. Re-parse: `fmt(s)` tokenises and parses.
//!  3. Comment preservation: item- and statement-level `--` comments in `s`
//!     survive into `fmt(s)` (expression-interior comments are documented
//!     lossiness — the generator deliberately never puts them there).

use proptest::prelude::*;

use bynk_fmt::{FormatOptions, format_source};
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::{parse_unit_with_warnings, parse_units};

/// A lowercase identifier that is not a keyword or contextual word the
/// grammar treats specially.
fn ident() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,5}".prop_filter("keyword-free", |s| {
        // A denylist is enough: colliding with a *non*-listed keyword makes
        // the parse fail and the case is skipped, costing coverage — while a
        // listed word changes the parse silently. Keep the contextual set.
        !matches!(
            s.as_str(),
            "on" | "suite"
                | "case"
                | "key"
                | "old"
                | "new"
                | "result"
                | "called"
                | "never"
                | "before"
                | "once"
                | "times"
                | "with"
        )
    })
}

fn int_lit() -> impl Strategy<Value = String> {
    prop_oneof![
        (0i64..=9_999).prop_map(|n| n.to_string()),
        // Digit separators are legal in every numeric position (#511).
        (1i64..=9).prop_map(|n| format!("{n}_000")),
        (1i64..=99).prop_map(|n| format!("-{n}")),
    ]
}

fn str_lit() -> impl Strategy<Value = String> {
    "[ a-zA-Z0-9_.!-]{0,12}".prop_map(|s| format!("\"{s}\""))
}

fn base_ty() -> impl Strategy<Value = &'static str> {
    prop_oneof![Just("Int"), Just("String"), Just("Bool"), Just("Float"),]
}

/// An expression, recursively composed from the value grammar. Kept to the
/// pure subset every body position accepts.
fn expr() -> impl Strategy<Value = String> {
    let leaf = prop_oneof![
        int_lit(),
        str_lit(),
        Just("true".to_string()),
        Just("false".to_string()),
        ident(),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone()).prop_map(|(a, b)| format!("{a} + {b}")),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| format!("{a} == {b}")),
            inner.clone().prop_map(|a| format!("({a})")),
            (ident(), proptest::collection::vec(inner.clone(), 0..3))
                .prop_map(|(f, args)| format!("{f}({})", args.join(", "))),
            proptest::collection::vec(inner.clone(), 0..3)
                .prop_map(|xs| format!("[{}]", xs.join(", "))),
            (inner.clone(), inner.clone(), inner)
                .prop_map(|(c, t, e)| format!("if ({c}) {{ {t} }} else {{ {e} }}")),
        ]
    })
}

/// An optional `--` comment line — only ever emitted at item/statement level,
/// where preservation is promised.
fn comment() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        "[ a-zA-Z0-9_.!?-]{0,20}".prop_map(|s| format!("// {s}\n")),
    ]
}

/// A top-level item: a type declaration or a function.
fn item() -> impl Strategy<Value = String> {
    let ty = prop_oneof![
        (comment(), ident(), base_ty()).prop_map(|(c, n, b)| { format!("{c}type T_{n} = {b}\n") }),
        (comment(), ident(), int_lit(), int_lit()).prop_map(|(c, n, lo, hi)| {
            // InRange bounds must not be inverted; order them.
            let (a, b): (i64, i64) = (
                lo.replace('_', "").parse().unwrap(),
                hi.replace('_', "").parse().unwrap(),
            );
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            format!("{c}type R_{n} = Int where InRange({lo}, {hi})\n")
        }),
        (comment(), ident(), ident(), base_ty(), base_ty()).prop_map(|(c, n, f, t1, t2)| {
            format!("{c}type P_{n} = {{ {f}_a: {t1}, {f}_b: {t2} }}\n")
        }),
    ];
    let fun = (
        comment(),
        ident(),
        ident(),
        base_ty(),
        comment(),
        expr(),
        expr(),
    )
        .prop_map(|(c, name, p, ty, body_comment, bound, second)| {
            // A block is `let` statements plus exactly one tail expression —
            // there are no bare expression statements — and the tail must be
            // the fn's return type, so generated exprs bind to locals and a
            // literal `0` closes the body.
            format!(
                "{c}fn f_{name}({p}: {ty}) -> Int {{\n  {body_comment}let x_{name} = {bound}\n  let y_{name} = {second}\n  0\n}}\n"
            )
        });
    prop_oneof![ty, fun]
}

fn commons() -> impl Strategy<Value = String> {
    (ident(), proptest::collection::vec(item(), 1..5), comment()).prop_map(
        |(name, items, trailing)| format!("commons gen_{name}\n\n{}{trailing}", items.join("\n")),
    )
}

/// #1664: an optional `---` doc block before an item: none, attached (directly
/// above the item), or orphaned (a blank line between, so the parser attaches
/// it to nothing).
fn doc() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        "[a-z]{1,12}".prop_map(|s| format!("---\ndoc {s}\n---\n")),
        "[a-z]{1,12}".prop_map(|s| format!("---\ndoc {s}\n---\n\n")),
    ]
}

/// A commons whose items may carry doc blocks, attached or orphaned, plus an
/// optional doc block at the end of the file (always an orphan).
fn commons_with_docs() -> impl Strategy<Value = String> {
    (
        ident(),
        proptest::collection::vec((doc(), item()), 1..5),
        "[a-z]{0,12}",
    )
        .prop_map(|(name, items, tail)| {
            let body: Vec<String> = items
                .into_iter()
                .map(|(doc, item)| format!("{doc}{item}"))
                .collect();
            let tail = if tail.is_empty() {
                String::new()
            } else {
                format!("\n---\ndoc {tail}\n---\n")
            };
            format!("commons gen_{name}\n\n{}{tail}", body.join("\n"))
        })
}

/// The number of `bynk.parse.orphan_doc_block` warnings a parse of `source`
/// reports.
fn orphan_warnings(source: &str) -> usize {
    let tokens = tokenize(source).expect("tokenises");
    let (_, warnings) = parse_unit_with_warnings(&tokens, source).expect("parses");
    warnings
        .iter()
        .filter(|w| w.category == "bynk.parse.orphan_doc_block")
        .count()
}

fn doc_lines(source: &str) -> Vec<String> {
    source
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("doc "))
        .map(str::to_string)
        .collect()
}

fn comment_lines(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("//"))
        .map(|body| body.trim().to_string())
        .filter(|body| !body.is_empty())
        .collect()
}

/// Meta-guard: the properties above quantify over "whatever the formatter
/// accepts", which silently degrades to vacuous if the generator drifts into
/// producing mostly-rejected sources. Keep the acceptance rate honest.
#[test]
fn generator_output_is_mostly_accepted() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::deterministic();
    let opts = FormatOptions::default();
    let total = 200;
    let mut accepted = 0;
    for _ in 0..total {
        let src = commons().new_tree(&mut runner).unwrap().current();
        if format_source(&src, &opts).is_ok() {
            accepted += 1;
        }
    }
    if accepted < total {
        // Diagnostic: show the first few rejects.
        let mut runner2 = TestRunner::deterministic();
        let mut shown = 0;
        for _ in 0..total {
            let src = commons().new_tree(&mut runner2).unwrap().current();
            if let Err(e) = format_source(&src, &opts) {
                eprintln!(
                    "--- reject ---\n{src}\n--- errors: {:?}",
                    e.errors
                        .iter()
                        .map(|x| (x.category, x.message.clone()))
                        .collect::<Vec<_>>()
                );
                shown += 1;
                if shown >= 3 {
                    break;
                }
            }
        }
    }
    assert!(
        accepted * 10 >= total * 9,
        "only {accepted}/{total} generated sources were accepted — the \
         round-trip properties are going vacuous"
    );
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        // Failures persist a seed under proptest-regressions/ — commit it so
        // the exact case replays in CI forever.
        ..ProptestConfig::default()
    })]

    #[test]
    fn formatting_is_idempotent_and_reparseable(src in commons()) {
        let opts = FormatOptions::default();
        // Not every generated source is semantically meaningful; the property
        // quantifies over whatever the formatter accepts.
        let Ok(once) = format_source(&src, &opts) else {
            return Ok(());
        };
        // 2. The formatted output re-parses.
        let tokens = tokenize(&once).expect("formatted output tokenises");
        parse_units(&tokens, &once).expect("formatted output parses");
        // 1. Idempotency.
        let twice = format_source(&once, &opts).expect("formatted output reformats");
        prop_assert_eq!(&twice, &once, "fmt(fmt(s)) != fmt(s)");
    }

    /// #1664, #1756: the formatter keeps every `---` doc block, an orphan
    /// included, and never changes which blocks are orphans: formatting must
    /// not attach an orphan to the declaration below it, or detach an attached
    /// block.
    #[test]
    fn doc_blocks_are_always_kept(src in commons_with_docs()) {
        let opts = FormatOptions::default();
        // Not every generated source parses; the property quantifies over the
        // rest, and of those the formatter may refuse none for a doc block.
        let formatted = match format_source(&src, &opts) {
            Ok(formatted) => formatted,
            Err(e) => {
                let refused_a_doc = e.errors.iter().any(|x| {
                    x.category == "bynk.fmt.comment_loss" && (x.message.contains(" doc ") || x.message.contains(" docs "))
                });
                prop_assert!(!refused_a_doc, "refused a doc block\n--- input ---\n{}", src);
                return Ok(());
            }
        };
        let after = doc_lines(&formatted);
        for d in doc_lines(&src) {
            prop_assert!(
                after.contains(&d),
                "doc block `{}` was dropped by the formatter\n--- input ---\n{}\n--- output ---\n{}",
                d, src, formatted
            );
        }
        prop_assert_eq!(
            orphan_warnings(&formatted),
            orphan_warnings(&src),
            "formatting changed which doc blocks are orphans\n--- input ---\n{}\n--- output ---\n{}",
            src,
            formatted
        );
    }

    #[test]
    fn item_level_comments_survive_formatting(src in commons()) {
        let opts = FormatOptions::default();
        let Ok(formatted) = format_source(&src, &opts) else {
            return Ok(());
        };
        let before = comment_lines(&src);
        let after = comment_lines(&formatted);
        for c in &before {
            prop_assert!(
                after.iter().any(|a| a.contains(c.as_str())),
                "comment `{}` was dropped by the formatter\n--- input ---\n{}\n--- output ---\n{}",
                c, src, formatted
            );
        }
    }
}
