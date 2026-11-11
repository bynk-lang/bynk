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
//! that elides with `…` is compared up to the `…`. So the line an explanation
//! calls wrong is in a program the compiler refuses for the reason given, the
//! line it calls right is in one it accepts, and a syntax change that breaks
//! either fails here.
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

/// Compile one program, returning the codes of its errors (empty on success).
fn error_codes(source: &str, tag: &str) -> Vec<String> {
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
            Err(errors) => errors.iter().map(|e| e.category.to_string()).collect(),
        }
    } else {
        match bynk_emit::compile(source, tag) {
            Ok(_) => Vec::new(),
            Err(errors) => errors.iter().map(|e| e.category.to_string()).collect(),
        }
    }
}

/// A line's code, without its trailing `//` comment and its indentation.
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

/// Whether `program` has the marked line `code`. A `code` that elides with `…`
/// matches a line that starts with what precedes the `…`.
fn has_line(program: &str, code: &str) -> bool {
    let (want, elided) = match code.split_once('…') {
        Some((before, _)) => (before.trim(), true),
        None => (code, false),
    };
    program.lines().map(code_of).any(|have| {
        if elided {
            have.starts_with(want)
        } else {
            have == want
        }
    })
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

        let bad_codes = error_codes(&bad, &format!("{}-bad", e.code));
        if bad_codes.is_empty() || bad_codes.iter().any(|c| c != e.code) {
            failures.push(format!(
                "{}: bad.bynk must fail with this code alone, but its errors are {bad_codes:?}",
                e.code
            ));
        }
        let good_codes = error_codes(&good, &format!("{}-good", e.code));
        if !good_codes.is_empty() {
            failures.push(format!(
                "{}: good.bynk must compile, but its errors are {good_codes:?}",
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
        for (code, mark) in lines {
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
}
