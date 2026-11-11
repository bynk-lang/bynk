//! The `bynk explain` example gate (#3163).
//!
//! An [`Explain::example`] is a short before-and-after snippet, written to be
//! read: it elides bodies with `…` and shows statements without the unit around
//! them, so it cannot be compiled as it stands. Each explained code therefore
//! has a pair of whole programs under `tests/fixtures/explain/<code>/`:
//!
//! - `bad.bynk` must fail to compile with that code, and with no other;
//! - `good.bynk` must compile.
//!
//! The snippet is tied to the pair line by line. A line marked `✗` must appear
//! in `bad.bynk`, a line marked `✓` in `good.bynk`, and an unmarked line in
//! either, compared without the trailing comment and the indentation; a line
//! that elides with `…` is compared up to the `…`. The compiler must also report
//! the code on a line marked `✗`. So the line an explanation calls wrong is the
//! one the compiler refuses, for the reason given, the line it calls right is
//! in a program it accepts, and a syntax change that breaks either fails here.
//!
//! A program that starts `commons …` is compiled as a single file, and one that
//! starts `context …` as a one-file project, as `doc_examples.rs` does.
//!
//! [`Explain::example`]: bynk_syntax::diagnostics::Explain::example

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use bynk_syntax::diagnostics::EXPLANATIONS;

/// Explained codes whose example is not Bynk source, with the reason. Their
/// examples show a lockfile or a command line, which no compile can check.
const NOT_BYNK_SOURCE: &[(&str, &str)] = &[
    (
        "bynk.registry.withdrawn",
        "the example is the rendered diagnostic and a `bynk update` command",
    ),
    (
        "bynk.requires.unapproved",
        "the example is a `bynk.lock` table before and after `bynk approve`",
    ),
];

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/explain")
}

/// Compile one program, returning each error's code and the 1-indexed line it
/// is reported on (empty on success).
fn errors_of(source: &str, tag: &str) -> Vec<(String, usize)> {
    let located = |errors: &[bynk_syntax::error::CompileError]| {
        errors
            .iter()
            .map(|e| {
                let (line, _) = bynk_syntax::span::line_col(source, e.span.start);
                (e.category.to_string(), line)
            })
            .collect()
    };
    let header = source
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("//"))
        .unwrap_or("");
    if let Some(name) = header.strip_prefix("context ") {
        let name = name.trim_end_matches('{').trim();
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("explain-{tag}"));
        let _ = fs::remove_dir_all(&root);
        let file = root.join(name.split('.').collect::<PathBuf>().with_extension("bynk"));
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, source).unwrap();
        let result = bynk_driver::project::compile_project(&bynk_testkit::compile_options_single(
            root.clone(),
        ))
        .map_err(bynk_driver::project::ProjectFailure::flatten);
        let _ = fs::remove_dir_all(&root);
        match result {
            Ok(_) => Vec::new(),
            Err(errors) => located(&errors),
        }
    } else {
        match bynk_emit::compile(source, tag) {
            Ok(_) => Vec::new(),
            Err(errors) => located(&errors),
        }
    }
}

/// A line's code, without its trailing `//` comment and its indentation.
///
/// This reads text, not tokens. It assumes no string literal in an example
/// holds a `//` after whitespace, or a `✗` or `✓`: none does today, and one
/// that did would fail the gate, not pass it, because its line would be cut
/// short or marked and then not found.
fn code_of(line: &str) -> &str {
    // A `//` inside a string literal (a URL, say) is not a comment: take the
    // last `//` that is preceded by whitespace or starts the line.
    let cut = line
        .match_indices("//")
        .filter(|(i, _)| *i == 0 || line[..*i].ends_with(char::is_whitespace))
        .map(|(i, _)| i)
        .last();
    match cut {
        Some(i) => line[..i].trim(),
        None => line.trim(),
    }
}

/// The code lines of an example, each with its mark: `Some(true)` for a line
/// whose comment carries `✗`, `Some(false)` for `✓`, `None` for neither.
fn example_lines(example: &str) -> Vec<(&str, Option<bool>)> {
    example
        .lines()
        .map(|line| {
            let mark = match (line.contains('✗'), line.contains('✓')) {
                (true, false) => Some(true),
                (false, true) => Some(false),
                _ => None,
            };
            (code_of(line), mark)
        })
        .filter(|(code, _)| !code.is_empty())
        .collect()
}

/// The 1-indexed lines of `program` that are the example line `code`. A `code`
/// that elides with `…` matches a line that starts with what precedes the `…`;
/// with nothing before the `…` there is nothing to compare, so it matches none.
fn lines_matching(program: &str, code: &str) -> Vec<usize> {
    let (want, elided) = match code.split_once('…') {
        Some((before, _)) => (before.trim(), true),
        None => (code, false),
    };
    if want.is_empty() {
        return Vec::new();
    }
    program
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            let have = code_of(line);
            if elided {
                have.starts_with(want)
            } else {
                have == want
            }
        })
        .map(|(i, _)| i + 1)
        .collect()
}

fn has_line(program: &str, code: &str) -> bool {
    !lines_matching(program, code).is_empty()
}

#[test]
fn every_explanation_example_is_backed_by_a_compiled_pair() {
    let exempt: BTreeSet<&str> = NOT_BYNK_SOURCE.iter().map(|(code, _)| *code).collect();
    let mut failures: Vec<String> = Vec::new();
    for e in EXPLANATIONS {
        if exempt.contains(e.code) {
            continue;
        }
        let dir = fixtures_root().join(e.code);
        let read = |name: &str| fs::read_to_string(dir.join(name)).ok();
        let (Some(bad), Some(good)) = (read("bad.bynk"), read("good.bynk")) else {
            failures.push(format!(
                "{}: no `bad.bynk` and `good.bynk` under {}",
                e.code,
                dir.display()
            ));
            continue;
        };

        let bad_errors = errors_of(&bad, &format!("{}-bad", e.code));
        if bad_errors.is_empty() || bad_errors.iter().any(|(c, _)| c != e.code) {
            failures.push(format!(
                "{}: bad.bynk must fail with this code alone, but its errors are {bad_errors:?}",
                e.code
            ));
        }
        let good_errors = errors_of(&good, &format!("{}-good", e.code));
        if !good_errors.is_empty() {
            failures.push(format!(
                "{}: good.bynk must compile, but its errors are {good_errors:?}",
                e.code
            ));
        }

        let lines = example_lines(e.example);
        for want_bad in [true, false] {
            if !lines.iter().any(|(_, mark)| *mark == Some(want_bad)) {
                failures.push(format!(
                    "{}: the example marks no line `{}`",
                    e.code,
                    if want_bad { '✗' } else { '✓' }
                ));
            }
        }
        // The compiler must report the code on a line the example marks `✗`:
        // the marked line is then the cause, not merely a line of a program
        // that fails somewhere else.
        let marked_bad: BTreeSet<usize> = lines
            .iter()
            .filter(|(_, mark)| *mark == Some(true))
            .flat_map(|(code, _)| lines_matching(&bad, code))
            .collect();
        if !bad_errors.is_empty() && !bad_errors.iter().any(|(_, l)| marked_bad.contains(l)) {
            failures.push(format!(
                "{}: bad.bynk reports its errors at {bad_errors:?}, on no line the example \
                 marks `✗` (lines {marked_bad:?} of bad.bynk)",
                e.code
            ));
        }
        for (code, mark) in lines {
            if code
                .split_once('…')
                .is_some_and(|(before, _)| before.trim().is_empty())
            {
                failures.push(format!(
                    "{}: the example's line `{code}` has nothing before its `…` to compare",
                    e.code
                ));
                continue;
            }
            let (found, place) = match mark {
                Some(true) => (has_line(&bad, code), "bad.bynk"),
                Some(false) => (has_line(&good, code), "good.bynk"),
                None => (
                    has_line(&bad, code) || has_line(&good, code),
                    "bad.bynk or good.bynk",
                ),
            };
            if !found {
                failures.push(format!(
                    "{}: the example's line `{code}` is not in {place}",
                    e.code
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "`bynk explain` examples are out of step with their compiled pairs \
         (bynk-tests/tests/fixtures/explain/):\n  {}",
        failures.join("\n  ")
    );
}

/// The exemption list and the fixture directory name only explained codes, so
/// neither outlives the explanation it belongs to.
#[test]
fn explain_fixtures_and_exemptions_name_explained_codes() {
    let explained: BTreeSet<&str> = EXPLANATIONS.iter().map(|e| e.code).collect();
    for (code, reason) in NOT_BYNK_SOURCE {
        assert!(
            explained.contains(code),
            "NOT_BYNK_SOURCE lists `{code}`, which has no explanation"
        );
        assert!(!reason.is_empty(), "`{code}` is exempt without a reason");
        assert!(
            !fixtures_root().join(code).exists(),
            "`{code}` is exempt and has a fixture pair: drop one"
        );
    }
    for entry in fs::read_dir(fixtures_root()).expect("the explain fixtures exist") {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            explained.contains(name.as_str()),
            "tests/fixtures/explain/{name} matches no explained code"
        );
    }
}

#[test]
fn marked_lines_are_compared_without_comment_or_indent() {
    assert_eq!(
        code_of("    count := count + 1   // ✗ reads"),
        "count := count + 1"
    );
    assert_eq!(
        code_of("let u = \"https://x\"  // ✓"),
        "let u = \"https://x\""
    );
    assert_eq!(
        example_lines("type T = Int\n\nf(1)  // ✗ no\nf(2)  // ✓ yes"),
        vec![
            ("type T = Int", None),
            ("f(1)", Some(true)),
            ("f(2)", Some(false))
        ]
    );
    assert!(has_line(
        "fn f(u: User) -> Text {\n  u.name\n}",
        "fn f(u: User) -> Text { … }"
    ));
    assert!(!has_line(
        "fn f(u: Usr) -> Text {",
        "fn f(u: User) -> Text { … }"
    ));
    assert!(has_line("  x.name  // why", "x.name"));
    assert!(!has_line("x.names", "x.name"));
    // Nothing before the `…`: no line to hold the program to.
    assert!(!has_line("fn f() {\n  1\n}", "…"));
    assert_eq!(lines_matching("a\n  b // c\nb", "b"), vec![2, 3]);
}
