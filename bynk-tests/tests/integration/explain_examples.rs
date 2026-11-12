//! The `bynk explain` example gate (#3163).
//!
//! An [`Explain::example`] is a short before-and-after snippet, written to be
//! read: it elides bodies with `…` and shows statements without the unit around
//! them, so it cannot be compiled as it stands. Each explained code therefore
//! has a *bad* and a *good* whole program under
//! `tests/fixtures/explain/<code>/`: the bad one must fail to compile with that
//! code, and with no other; the good one must compile. A fixture takes one of
//! two forms, never a mixture:
//!
//! - **file form**, for a rule one file can show: `bad.bynk` and `good.bynk`.
//!   A program that starts `commons …` is compiled as a single file, and one
//!   that starts `context …` as a one-file project, as `doc_examples.rs` does.
//! - **directory form**, for a rule that needs a second unit or a manifest:
//!   `bad/` and `good/`, each a project tree laid out as the project-mode
//!   negative fixtures are (`src/…`, with an optional `bynk.toml`) and compiled
//!   as they are (`e2e.rs`'s `compile_fixture`).
//!
//! The snippet is tied to the pair line by line. A line marked `✗` must appear
//! in the bad program, a line marked `✓` in the good one, and an unmarked line
//! in either, compared without the trailing comment and the indentation; a line
//! that elides with `…` is compared up to the `…`. In the directory form a line
//! may be in any file of its project. The compiler must also report the code on
//! a line marked `✗`, in the file it attributes the error to. So the line an
//! explanation calls wrong is the one the compiler refuses, for the reason
//! given, the line it calls right is in a program it accepts, and a syntax
//! change that breaks either fails here.
//!
//! **File labels.** An example that spans files says which one each snippet is
//! in with a comment-only line holding just a path, such as `// shop.bynk` or
//! `// shop/till.bynk`: the file's path under the project's `src/`, or
//! `// bynk.toml` for the manifest. The label applies until the next one, and
//! the gate holds each line under it to that file: a line marked `✗` must be in
//! the labelled file of the bad project, a `✓` line in the labelled file of the
//! good one, and an unmarked line in either. A line before the first label may
//! be in any file. A label must name a file that exists in the bad or the good
//! project, and a label in the file form is refused, since it has no files to
//! name.
//!
//! **Layout.** The line comparison ignores indentation, so the example is also
//! checked as `bynk explain` prints it, that is, the string after Rust has
//! decoded it. Within a run of non-blank lines, a line inside a `{ … }` block
//! must be indented past the line that opens the block, by any width, and the
//! closing `}` must be back at that line's indentation. A blank line ends the
//! run, so an example may leave a block open. One shape is not a nesting: a
//! block opener marked `✗` directly followed, at the same indentation, by a
//! block opener marked `✓` is the line and its replacement. This catches a
//! body printed flush left, which happens when its indentation is written as
//! source whitespace after a `\` line continuation: the continuation swallows
//! it, so leading spaces are written `\x20`. For the same reason a source line
//! holding only `\` prints no blank line, and is refused anywhere in
//! `diagnostics.rs`; a blank line is `\n\`. The layout needs no fixture, so it
//! is reported even for a code whose fixture fails to load.
//!
//! [`Explain::example`]: bynk_syntax::diagnostics::Explain::example

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use bynk_syntax::diagnostics::EXPLANATIONS;

/// Explained codes that are **warnings**, which this gate cannot back: it compiles `bad/`
/// and requires the code among its *errors*. Each is held by a positive fixture
/// with an `expected_warnings.txt` instead (`fixtures/positive/`), named here.
const WARNING_ONLY: &[(&str, &str)] = &[
    (
        "bynk.send.deprecated",
        "a warning; `positive/221_async_send_waituntil` holds it with `expected_warnings.txt`",
    ),
    (
        "bynk.actor.signed_cookie_ttl_long",
        "a warning; `positive/3386_signed_cookie_ttl_long` holds it with `expected_warnings.txt`",
    ),
    (
        "bynk.actor.signed_cookie_unprefixed",
        "a warning; `positive/3386_signed_cookie_unprefixed` holds it with `expected_warnings.txt`",
    ),
    (
        "bynk.http.cookie_cross_site_without_credentials",
        "a warning; `positive/3329_cookie_cross_site_without_credentials` holds it with `expected_warnings.txt`, and `positive/3329_cookie_cross_site_credentials` holds the credentialed good case",
    ),
    (
        "bynk.http.cookie_untyped_has_field",
        "a warning; `positive/3446_cookie_untyped_has_field` holds it with `expected_warnings.txt`",
    ),
    (
        "bynk.http.cookie_domain_widens",
        "a warning; `positive/3329_cookie_domain_widens` holds it with `expected_warnings.txt`",
    ),
    (
        "bynk.postgres.sql_invalid",
        "a warning (the parser is not Postgres' own); `positive/3321_postgres_sql_invalid` and `_in_transaction` hold it with `expected_warnings.txt`",
    ),
    (
        "bynk.mysql.sql_invalid",
        "a warning (the parser is not MySQL's own); `positive/3322_mysql_sql_invalid` holds it with `expected_warnings.txt`",
    ),
];

/// Explained codes whose example is not Bynk source, with the reason. Their
/// examples show a lockfile, a manifest entry or a command line, which no compile can check.
const NOT_BYNK_SOURCE: &[(&str, &str)] = &[
    (
        "bynk.package.alias_shadows_unit",
        "the example is a `[dependencies]` entry in `bynk.toml`, and the diagnostic is attributed to the manifest, which has no line to check",
    ),
    (
        "bynk.package.dependency_not_found",
        "the example is a `[dependencies]` entry in `bynk.toml`, and the diagnostic is attributed to the manifest, which has no line to check",
    ),
    (
        "bynk.package.name_mismatch",
        "the example is a `[dependencies]` entry in `bynk.toml`, and the diagnostic is attributed to the manifest, which has no line to check",
    ),
    (
        "bynk.project.export_context",
        "the example is a `[package] exports` entry in `bynk.toml`, and the diagnostic is attributed to the manifest, which has no line to check",
    ),
    (
        "bynk.package.duplicate_source",
        "the example is a `[dependencies]` entry in `bynk.toml`, and the diagnostic is attributed to the manifest, which has no line to check",
    ),
    (
        "bynk.dependency.path_outside_root",
        "the example is a `[dependencies]` entry in a dependency's `bynk.toml`, and the diagnostic is attributed to the manifest, which has no line to check",
    ),
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

/// One compile error: its code, the 1-indexed line it is reported on, and the
/// text of the file that line is in (so the line can be matched to the example).
/// `text` is `None` when the error names no source file, or one the build did
/// not snapshot, so there is no line to read.
struct Reported {
    code: String,
    line: usize,
    text: Option<String>,
}

/// One side of a fixture, the bad program or the good one: its source files as
/// `(path under the side, text)`, what compiling them reports (nothing on
/// success), and how to name it in a failure message.
struct Side {
    name: String,
    files: Vec<(String, String)>,
    errors: Vec<Reported>,
}

/// Compile one single-file program.
fn errors_of(source: &str, tag: &str) -> Vec<Reported> {
    let located = |errors: &[bynk_syntax::error::CompileError]| {
        errors
            .iter()
            .map(|e| Reported {
                code: e.category.to_string(),
                line: bynk_syntax::span::line_col(source, e.span.start).0,
                text: Some(source.to_string()),
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

/// Compile a project tree as the project-mode negative fixtures are, reading
/// each error's line in the file it is attributed to. The tree is only read.
fn project_errors(root: &Path) -> Vec<Reported> {
    let target = bynk_driver::project::BuildTarget::Bundle;
    let Err(failure) = super::e2e::compile_fixture(root, target) else {
        return Vec::new();
    };
    failure
        .errors
        .iter()
        .map(|attributed| {
            let text = attributed
                .source_path
                .as_deref()
                .and_then(|path| failure.snapshots.iter().find(|(p, _)| p == path))
                .map(|(_, text)| text.clone());
            let start = attributed.error.span.start;
            Reported {
                code: attributed.error.category.to_string(),
                line: text
                    .as_deref()
                    .map_or(0, |t| bynk_syntax::span::line_col(t, start).0),
                text,
            }
        })
        .collect()
}

/// The `.bynk` files and `bynk.toml` under `root`, as `(path under root, text)`
/// in path order. The example's lines are held to these.
fn source_files(root: &Path) -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        for entry in fs::read_dir(dir).unwrap().map(Result::unwrap) {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if path.extension().is_some_and(|x| x == "bynk") || path.ends_with("bynk.toml") {
                let rel = path.strip_prefix(root).unwrap();
                let rel = rel.to_string_lossy().replace('\\', "/");
                out.push((rel, fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// Which form a fixture directory takes, or why it is malformed.
fn fixture_form(dir: &Path) -> Result<bool, String> {
    let file = |n: &str| dir.join(n).is_file();
    let tree = |n: &str| dir.join(n).is_dir();
    match (
        file("bad.bynk"),
        file("good.bynk"),
        tree("bad"),
        tree("good"),
    ) {
        (true, true, false, false) => Ok(false),
        (false, false, true, true) => Ok(true),
        (false, false, false, false) => {
            Err("has no fixture: add `bad.bynk` and `good.bynk`, or `bad/` and `good/`".to_string())
        }
        (b, g, false, false) if b != g => {
            Err("has only one of `bad.bynk` and `good.bynk`: it needs both".to_string())
        }
        (false, false, b, g) if b != g => {
            Err("has only one of `bad/` and `good/`: it needs both".to_string())
        }
        _ => Err(
            "mixes the file form and the directory form: use `bad.bynk` and \
                  `good.bynk`, or `bad/` and `good/`, not both"
                .to_string(),
        ),
    }
}

/// Load and compile one side of a fixture: `which` is `bad` or `good`.
fn load_side(dir: &Path, project: bool, which: &str, tag: &str) -> Result<Side, String> {
    if !project {
        let name = format!("{which}.bynk");
        let source = fs::read_to_string(dir.join(&name)).map_err(|e| format!("{name}: {e}"))?;
        let errors = errors_of(&source, &format!("{tag}-{which}"));
        return Ok(Side {
            name,
            files: vec![(format!("{which}.bynk"), source)],
            errors,
        });
    }
    let root = dir.join(which);
    if !root.join("src").is_dir() {
        return Err(format!("`{which}/` has no `src/` directory"));
    }
    Ok(Side {
        name: format!("{which}/"),
        files: source_files(&root),
        errors: project_errors(&root),
    })
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

/// A line's mark: `Some(true)` if it carries `✗`, `Some(false)` for `✓`, and
/// `None` for neither or both.
fn mark_of(line: &str) -> Option<bool> {
    match (line.contains('✗'), line.contains('✓')) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// The code lines of an example, each with its mark (see [`mark_of`]).
fn example_lines(example: &str) -> Vec<(&str, Option<bool>)> {
    example
        .lines()
        .map(|line| (code_of(line), mark_of(line)))
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

/// Whether any file of `side` holds the example line `code`.
fn side_has(side: &Side, code: &str) -> bool {
    side.files.iter().any(|(_, text)| has_line(text, code))
}

/// Whether `side` holds the example line `code` in the file `label` names, or
/// in any file when the line has no label.
fn side_has_in(side: &Side, label: Option<&str>, code: &str) -> bool {
    let Some(label) = label else {
        return side_has(side, code);
    };
    let want = labelled_path(label);
    (side.files.iter()).any(|(path, text)| *path == want && has_line(text, code))
}

/// The file label in force at each code line of `example`, in step with
/// [`example_lines`]: `None` before the first label.
fn line_labels(example: &str) -> Vec<Option<&str>> {
    let mut current = None;
    let mut out = Vec::new();
    for line in example.lines() {
        if let Some(label) = file_label(line) {
            current = Some(label);
        } else if !code_of(line).is_empty() {
            out.push(current);
        }
    }
    out
}

/// A comment-only line that holds just a path is a file label (see the module
/// doc): the path of a `.bynk` file under `src/`, or `bynk.toml`.
fn file_label(line: &str) -> Option<&str> {
    let label = line.trim().strip_prefix("//")?.trim();
    let is_path =
        !label.contains(char::is_whitespace) && (label.ends_with(".bynk") || label == "bynk.toml");
    is_path.then_some(label)
}

/// Where a labelled file lives under a project side.
fn labelled_path(label: &str) -> String {
    if label == "bynk.toml" {
        label.to_string()
    } else {
        format!("src/{label}")
    }
}

/// Failures of an example's file labels: one that names no file of the bad or
/// good project, or any label in a single-file fixture.
fn label_failures(example: &str, project: bool, bad: &Side, good: &Side) -> Vec<String> {
    let mut out = Vec::new();
    for label in example.lines().filter_map(file_label) {
        let want = labelled_path(label);
        let exists = |side: &Side| side.files.iter().any(|(path, _)| *path == want);
        if !project {
            out.push(format!(
                "the example labels a file `{label}`, but a single-file fixture has no files to label"
            ));
        } else if !exists(bad) && !exists(good) {
            out.push(format!(
                "the example labels a file `{label}`, but neither `bad/` nor `good/` has `{want}`"
            ));
        }
    }
    out
}

/// The failures of the compile side of a fixture: the bad program fails with
/// the code alone, and the good one compiles.
fn compile_failures(code: &str, bad: &Side, good: &Side) -> Vec<String> {
    let mut out = Vec::new();
    if bad.errors.is_empty() || bad.errors.iter().any(|e| e.code != code) {
        let got: Vec<(&str, usize)> = bad.errors.iter().map(|e| (&*e.code, e.line)).collect();
        out.push(format!(
            "{}: {} must fail with this code alone, but its errors are {got:?}",
            code, bad.name
        ));
    }
    if !good.errors.is_empty() {
        let got: Vec<(&str, usize)> = good.errors.iter().map(|e| (&*e.code, e.line)).collect();
        out.push(format!(
            "{}: {} must compile, but its errors are {got:?}",
            code, good.name
        ));
    }
    out
}

/// The failures of the example's marks: some `✗` and some `✓` line, and the
/// bad program's error reported, in its own file, on a line marked `✗`, so the
/// marked line is the cause and not merely a line of a program that fails
/// somewhere else.
fn mark_failures(code: &str, lines: &[(&str, Option<bool>)], bad: &Side) -> Vec<String> {
    let mut out = Vec::new();
    for want_bad in [true, false] {
        if !lines.iter().any(|(_, mark)| *mark == Some(want_bad)) {
            let mark = if want_bad { '✗' } else { '✓' };
            out.push(format!("{code}: the example marks no line `{mark}`"));
        }
    }
    let marked = |e: &Reported| {
        let text = e.text.as_deref().unwrap_or_default();
        lines
            .iter()
            .filter(|(_, mark)| *mark == Some(true))
            .any(|(want, _)| lines_matching(text, want).contains(&e.line))
    };
    if let Some(e) = bad.errors.iter().find(|e| e.text.is_none()) {
        // Nothing to read a line from: say so, rather than blame the marks.
        out.push(format!(
            "{code}: {} reports `{}` with no source file to read its line from (no path, or \
             one the build did not snapshot), so the gate cannot tell where it is",
            bad.name, e.code
        ));
    } else if !bad.errors.is_empty() && !bad.errors.iter().any(marked) {
        let at: Vec<(&str, usize)> = bad.errors.iter().map(|e| (&*e.code, e.line)).collect();
        out.push(format!(
            "{code}: {} reports its errors at {at:?}, on no line the example marks `✗`",
            bad.name
        ));
    }
    out
}

/// The failures of the example's lines: each one is in the program its mark
/// names, in the file its label names if it has one, and none has nothing
/// before its `…` to compare.
fn line_failures(
    code: &str,
    lines: &[(&str, Option<bool>)],
    labels: &[Option<&str>],
    bad: &Side,
    good: &Side,
) -> Vec<String> {
    let mut out = Vec::new();
    for ((line, mark), label) in lines.iter().zip(labels) {
        if line
            .split_once('…')
            .is_some_and(|(before, _)| before.trim().is_empty())
        {
            out.push(format!(
                "{code}: the example's line `{line}` has nothing before its `…` to compare"
            ));
            continue;
        }
        let in_side = |side: &Side| side_has_in(side, *label, line);
        let (found, place) = match mark {
            Some(true) => (in_side(bad), bad.name.clone()),
            Some(false) => (in_side(good), good.name.clone()),
            None => (
                in_side(bad) || in_side(good),
                format!("{} or {}", bad.name, good.name),
            ),
        };
        if !found {
            let at = label.map_or(String::new(), |l| format!(" `{}` of", labelled_path(l)));
            out.push(format!(
                "{code}: the example's line `{line}` is not in{at} {place}"
            ));
        }
    }
    out
}

/// The width of a line's indentation, in columns.
fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Whether `line` is the `✓` form of the `✗` block opener `opener`: the two
/// stand side by side at one indentation and both open a block, so the second
/// replaces the first instead of sitting inside it.
fn replaces_opener(opener: &str, line: &str) -> bool {
    indent_of(opener) == indent_of(line)
        && mark_of(opener) == Some(true)
        && mark_of(line) == Some(false)
        && code_of(line).ends_with('{')
}

/// The layout failures of an example as `bynk explain` prints it (see the
/// module doc): within a run of non-blank lines, a line inside a `{ … }` block
/// that is not indented past the line that opens the block, or a closing `}`
/// that is not back at that line's column.
///
/// Like [`code_of`], this reads text, not tokens: a block opens on a line whose
/// code ends in `{` and closes on one whose code starts with `}`.
fn layout_failures(example: &str) -> Vec<String> {
    let mut out = Vec::new();
    // The lines that open the blocks still open in the current run.
    let mut open: Vec<&str> = Vec::new();
    for line in example.lines() {
        if line.trim().is_empty() {
            open.clear();
            continue;
        }
        let (indent, code) = (indent_of(line), code_of(line));
        let opener = open.last().copied();
        if code.starts_with('}') {
            open.pop();
            if let Some(opener) = opener.filter(|o| indent_of(o) != indent) {
                out.push(format!(
                    "the example's line `{}` closes the block `{}` opens, but is not at its \
                     indentation",
                    line.trim(),
                    code_of(opener)
                ));
            }
        } else if let Some(opener) = opener.filter(|o| indent_of(o) >= indent) {
            if replaces_opener(opener, line) {
                open.pop();
            } else {
                out.push(format!(
                    "the example's line `{}` is inside the block `{}` opens, but is not \
                     indented past it: write the indentation as `\\x20`, since a `\\` line \
                     continuation swallows the spaces after it",
                    line.trim(),
                    code_of(opener)
                ));
            }
        }
        if code.ends_with('{') {
            open.push(line);
        }
    }
    out
}

/// Every failure of one explained code's fixture.
fn fixture_failures(code: &str, example: &str) -> Vec<String> {
    // The layout needs no fixture, so it is reported even when one fails to load.
    let mut out: Vec<String> = layout_failures(example)
        .iter()
        .map(|why| format!("{code}: {why}"))
        .collect();
    let dir = fixtures_root().join(code);
    let project = match fixture_form(&dir) {
        Ok(project) => project,
        Err(why) => {
            out.push(format!("{code}: {} {why}", dir.display()));
            return out;
        }
    };
    let load = |which| load_side(&dir, project, which, code);
    let (bad, good) = match (load("bad"), load("good")) {
        (Ok(bad), Ok(good)) => (bad, good),
        (Err(why), _) | (_, Err(why)) => {
            out.push(format!("{code}: {why}"));
            return out;
        }
    };
    let lines = example_lines(example);
    out.extend(compile_failures(code, &bad, &good));
    out.extend(mark_failures(code, &lines, &bad));
    out.extend(line_failures(
        code,
        &lines,
        &line_labels(example),
        &bad,
        &good,
    ));
    for why in label_failures(example, project, &bad, &good) {
        out.push(format!("{code}: {why}"));
    }
    out
}

#[test]
fn every_explanation_example_is_backed_by_a_compiled_pair() {
    let exempt: BTreeSet<&str> = NOT_BYNK_SOURCE
        .iter()
        .chain(WARNING_ONLY)
        .map(|(code, _)| *code)
        .collect();
    let failures: Vec<String> = EXPLANATIONS
        .iter()
        .filter(|e| !exempt.contains(e.code))
        .flat_map(|e| fixture_failures(e.code, e.example))
        .collect();
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
    for (code, reason) in NOT_BYNK_SOURCE.iter().chain(WARNING_ONLY) {
        assert!(
            explained.contains(code),
            "an exemption lists `{code}`, which has no explanation"
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

#[test]
fn a_block_body_is_indented_past_the_line_that_opens_it() {
    // Two spaces or four per level, nested blocks, and a blank line all pass.
    assert!(layout_failures("fn f() -> Int {\n  1\n}").is_empty());
    let nested = "service s {\n    on call() -> Int {   // ✗ why\n        1\n    }\n}";
    assert!(layout_failures(nested).is_empty());
    assert!(layout_failures("fn f() -> Int { … }\nf(1)  // ✗\nf(2)  // ✓").is_empty());
    // The regression (#3163): the body's indentation was written as source
    // whitespace after a `\` continuation, so it prints flush left.
    let flush = layout_failures("capability Clock {\nfn now() -> Int\n}");
    assert_eq!(flush.len(), 1, "{flush:?}");
    assert!(flush[0].contains("`fn now() -> Int`"), "{flush:?}");
    assert!(flush[0].contains("`capability Clock {`"), "{flush:?}");
    // Every line of the block is held, not only the first.
    assert_eq!(layout_failures("fn f() {\n    a\nb\n}").len(), 1);
    assert_eq!(layout_failures("a {\n  b {\n  c\n  }\n}").len(), 1);
    // A comment does not hide the `{`, and a comment-only line is held too.
    assert_eq!(layout_failures("fn f() {   // ✗ no\n1\n}").len(), 1);
    assert_eq!(layout_failures("fn f() {\n// why\n    1\n}").len(), 1);
    // The closer sits at the opener's column.
    assert_eq!(layout_failures("fn f() {\n    1\n    }").len(), 1);
    assert!(layout_failures("if a {\n  1\n} else {\n  2\n}").is_empty());
    assert_eq!(layout_failures("if a {\n  1\n} else {\n2\n}").len(), 1);
    // A blank line ends the run: an example may leave a block open.
    assert!(layout_failures("service s {\n    on call() {   // ✗\n\nf()").is_empty());
}

#[test]
fn the_layout_is_reported_when_the_fixture_fails_to_load() {
    let out = fixture_failures("bynk.no.such_code", "fn f() {\n1\n}");
    assert_eq!(out.len(), 2, "{out:?}");
    assert!(out[0].contains("not indented past it"), "{out:?}");
    assert!(out[1].contains("has no fixture"), "{out:?}");
}

#[test]
fn a_marked_opener_may_stand_beside_its_replacement() {
    // The `✗` header and the `✓` one, at one indentation: not a nesting.
    let pair = "service s {\n    on a(t: T) {   // ✗ no\n    on a(t: U) {   // ✓ yes";
    assert!(layout_failures(pair).is_empty());
    // What follows the pair is inside the `✓` block.
    assert_eq!(layout_failures(&format!("{pair}\n    f()")).len(), 1);
    assert!(layout_failures(&format!("{pair}\n        f()\n    }}\n}}")).is_empty());
    // Only that shape is let through: a flush-left body is not a replacement,
    // whatever it is marked, and nor is an unmarked or reversed pair.
    assert_eq!(layout_failures("fn f() {   // ✗\n1   // ✓").len(), 1);
    assert_eq!(layout_failures("on a() {\non b() {   // ✓").len(), 1);
    assert_eq!(layout_failures("on a() {   // ✓\non b() {   // ✗").len(), 1);
    assert_eq!(
        layout_failures("  on a() {   // ✗\non b() {   // ✓").len(),
        1
    );
}

/// A source line holding only `\` is a second continuation, swallowed with the
/// first: it prints no blank line, though it reads as one. That holds in any
/// string literal, so the whole of `diagnostics.rs` is scanned, not only
/// `EXPLANATIONS`. A separator is written `\n\` on its own line.
#[test]
fn no_diagnostics_string_separates_with_a_lone_continuation() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bynk-syntax/src/diagnostics.rs");
    let source = fs::read_to_string(&path).expect("the diagnostics source is readable");
    let lone: Vec<usize> = source
        .lines()
        .enumerate()
        .filter(|(_, line)| line.trim() == "\\")
        .map(|(i, _)| i + 1)
        .collect();
    assert!(
        lone.is_empty(),
        "bynk-syntax/src/diagnostics.rs has a line holding only `\\` at {lone:?}: it prints \
         nothing, so write `\\n\\` for a blank line"
    );
}

fn side(name: &str, files: &[(&str, &str)], errors: &[(&str, usize, &str)]) -> Side {
    Side {
        name: name.to_string(),
        files: files
            .iter()
            .map(|(p, t)| (p.to_string(), t.to_string()))
            .collect(),
        errors: errors
            .iter()
            .map(|(code, line, text)| Reported {
                code: code.to_string(),
                line: *line,
                text: Some(text.to_string()),
            })
            .collect(),
    }
}

#[test]
fn a_file_label_is_a_comment_holding_only_a_path() {
    assert_eq!(file_label("// shop.bynk"), Some("shop.bynk"));
    assert_eq!(file_label("   // shop/till.bynk  "), Some("shop/till.bynk"));
    assert_eq!(file_label("// bynk.toml"), Some("bynk.toml"));
    // Prose, a marked line, and code are not labels.
    assert_eq!(file_label("// see shop.bynk"), None);
    assert_eq!(file_label("uses pricing  // shop.bynk"), None);
    assert_eq!(file_label("// ✗ shop.bynk"), None);
    assert_eq!(file_label("// a note"), None);
    // A label is not an example code line, so the line gate ignores it.
    assert!(
        example_lines("// shop.bynk\nuses pricing")
            .iter()
            .all(|(c, _)| *c == "uses pricing")
    );
}

#[test]
fn labels_must_name_a_file_of_the_project() {
    let bad = side("bad/", &[("src/shop.bynk", "")], &[]);
    let good = side("good/", &[("src/pricing.bynk", ""), ("bynk.toml", "")], &[]);
    let ok = "// shop.bynk\nx\n// pricing.bynk\ny\n// bynk.toml\nz";
    assert!(label_failures(ok, true, &bad, &good).is_empty());
    let missing = label_failures("// orders.bynk\nx", true, &bad, &good);
    assert_eq!(missing.len(), 1);
    assert!(missing[0].contains("`src/orders.bynk`"), "{missing:?}");
    // The label is the path under `src/`, so a bare file name misses a nested one.
    let nested = side("bad/", &[("src/shop/till.bynk", "")], &[]);
    assert!(label_failures("// till.bynk", true, &nested, &good).len() == 1);
    assert!(label_failures("// shop/till.bynk", true, &nested, &good).is_empty());
    // A single-file fixture has nothing to label.
    assert_eq!(
        label_failures("// bad.bynk\nx", false, &bad, &good).len(),
        1
    );
}

#[test]
fn the_error_line_is_read_in_the_file_it_is_reported_in() {
    let shop = "uses pricing\nfn cost() -> Int {\n  lineTotal(1)\n}";
    let pricing = "private fn lineTotal(i: Int) -> Int { i }\nfn other() -> Int { 1 }";
    let example = [("lineTotal(1)", Some(true)), ("x", Some(false))];
    let reported = |line, text| side("bad/", &[], &[("c", line, text)]);
    // Line 3 of `shop` is the marked line.
    assert!(mark_failures("c", &example, &reported(3, shop)).is_empty());
    // The same line number in the other file is not: the marked text is not there.
    let wrong = mark_failures("c", &example, &reported(3, pricing));
    assert_eq!(wrong.len(), 1);
    assert!(
        wrong[0].contains("on no line the example marks"),
        "{wrong:?}"
    );
    // Nor is another line of the right file.
    assert_eq!(mark_failures("c", &example, &reported(1, shop)).len(), 1);
}

#[test]
fn example_lines_are_found_in_any_file_of_a_project() {
    let bad = side(
        "bad/",
        &[
            ("src/a.bynk", "type A = Int"),
            ("src/b.bynk", "uses a  // c"),
        ],
        &[],
    );
    assert!(side_has(&bad, "uses a"));
    assert!(side_has(&bad, "type A = Int"));
    assert!(!side_has(&bad, "uses b"));
    let good = side("good/", &[("src/a.bynk", "uses b")], &[]);
    let lines = [
        ("uses a", Some(true)),
        ("uses b", Some(false)),
        ("type A = Int", None),
    ];
    let unlabelled = [None, None, None];
    assert!(line_failures("c", &lines, &unlabelled, &bad, &good).is_empty());
    // A `✓` line only in the bad project is not found.
    let swapped = [("uses a", Some(false))];
    assert_eq!(line_failures("c", &swapped, &[None], &bad, &good).len(), 1);
}

#[test]
fn a_fixture_is_one_form_or_the_other() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("explain-form-test");
    let _ = fs::remove_dir_all(&root);
    let make = |name: &str, entries: &[&str]| {
        let dir = root.join(name);
        for entry in entries {
            match entry.strip_suffix('/') {
                Some(d) => fs::create_dir_all(dir.join(d)).unwrap(),
                None => {
                    fs::create_dir_all(&dir).unwrap();
                    fs::write(dir.join(entry), "").unwrap();
                }
            }
        }
        fs::create_dir_all(&dir).unwrap();
        dir
    };
    let form = |name: &str, entries: &[&str]| fixture_form(&make(name, entries));
    assert_eq!(form("files", &["bad.bynk", "good.bynk"]), Ok(false));
    assert_eq!(form("dirs", &["bad/", "good/"]), Ok(true));
    let empty = form("empty", &[]).unwrap_err();
    assert!(empty.contains("has no fixture"), "{empty}");
    let half_file = form("half-file", &["bad.bynk"]).unwrap_err();
    assert!(half_file.contains("only one of `bad.bynk`"), "{half_file}");
    let half_dir = form("half-dir", &["good/"]).unwrap_err();
    assert!(half_dir.contains("only one of `bad/`"), "{half_dir}");
    for (name, entries) in [
        ("mixed-a", &["bad.bynk", "good/"][..]),
        ("mixed-b", &["bad/", "good.bynk"][..]),
        ("both", &["bad.bynk", "good.bynk", "bad/", "good/"][..]),
    ] {
        let why = form(name, entries).unwrap_err();
        assert!(why.contains("mixes the file form"), "{name}: {why}");
    }
    let _ = fs::remove_dir_all(&root);
}

/// The failures of one example against a bad and a good project.
fn placed(example: &str, bad: &Side, good: &Side) -> Vec<String> {
    let (lines, labels) = (example_lines(example), line_labels(example));
    line_failures("c", &lines, &labels, bad, good)
}

#[test]
fn a_labelled_line_must_be_in_the_file_its_label_names() {
    let bad = side(
        "bad/",
        &[("src/a.bynk", "type A = Int"), ("src/b.bynk", "uses a")],
        &[],
    );
    let good = side(
        "good/",
        &[("src/a.bynk", "uses a"), ("src/b.bynk", "type B = Int")],
        &[],
    );
    let ok = "type A = Int\n// b.bynk\nuses a  // ✗\n// a.bynk\nuses a  // ✓";
    assert_eq!(line_labels(ok), vec![None, Some("b.bynk"), Some("a.bynk")]);
    assert!(placed(ok, &bad, &good).is_empty());
    // `type A = Int` is in the bad project, but in `a.bynk`, not the labelled `b.bynk`.
    let wrong_file = placed("// b.bynk\ntype A = Int  // ✗", &bad, &good);
    assert_eq!(wrong_file.len(), 1, "{wrong_file:?}");
    assert!(
        wrong_file[0].contains("`src/b.bynk` of bad/"),
        "{wrong_file:?}"
    );
    // `uses a` is in `b.bynk` of the bad project, not of the good one.
    let wrong_side = placed("// b.bynk\nuses a  // ✓", &bad, &good);
    assert_eq!(wrong_side.len(), 1, "{wrong_side:?}");
    // Unmarked: either project's labelled file will do.
    assert!(placed("// b.bynk\ntype B = Int", &bad, &good).is_empty());
    assert_eq!(placed("// b.bynk\ntype A = Int", &bad, &good).len(), 1);
    // A label on a file neither project has finds no line at all.
    assert_eq!(placed("// c.bynk\nuses a", &bad, &good).len(), 1);
}

#[test]
fn an_error_with_no_source_file_is_reported_as_such() {
    let mut bad = side("bad/", &[], &[("c", 1, "x")]);
    bad.errors[0].text = None;
    let lines = [("x", Some(true)), ("y", Some(false))];
    let out = mark_failures("c", &lines, &bad);
    assert_eq!(out.len(), 1, "{out:?}");
    assert!(out[0].contains("no source file to read"), "{out:?}");
    assert!(!out[0].contains("marks `✗`"), "{out:?}");
}
