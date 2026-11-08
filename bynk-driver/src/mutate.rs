//! `bynk test --mutate` (#3069): check that the suites would notice a small
//! change to the code.
//!
//! The run applies one mutation at a time to a project's own sources, runs the
//! suites that exercise the mutated unit, and reports the mutants no suite
//! failed on, each with its source span. A surviving mutant is a gap in the
//! suites, not a bug in the code.
//!
//! **How a mutant is made.** Sites are found in the lexed source of every
//! non-suite file, inside the declaration items the parser delimits, so each
//! site knows its unit and declaration. The mutant is the file's text with one
//! operator token replaced, handed to an ordinary in-process `bynk test` run as
//! an in-memory source overlay ([`TestArgs::mutant_overlay`]); nothing is
//! written under the project, and the emitted output goes to a scratch
//! directory. A mutant that does not compile is `invalid` and left out of the
//! score, so a mutant is only ever "killed" by a suite that built and failed.
//!
//! **Which suites run.** The suites that target the mutated unit, or a unit
//! that (transitively) `uses` it, or that themselves `uses` it. A mutant no
//! suite reaches is `no suite`: reported, never run, and not scored.
//!
//! The first slice mutates comparison operators (`<`↔`<=`, `>`↔`>=`,
//! `==`↔`!=`) and boolean connectives (`&&`↔`||`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use bynk_syntax::ast::SourceUnit;
use bynk_syntax::lexer::{TokenKind, tokenize_in};
use bynk_syntax::parser::parse_unit;
use bynk_syntax::span::FileId;
use serde_json::{Value, json};

use crate::test_json::TestRun;
use crate::test_runner::{TestArgs, TestFormat, run_test};

/// The seed every run of a `--mutate` session uses unless `--seed` names one:
/// a property test then draws the same inputs for the baseline and every
/// mutant, so a mutant is not killed or spared by luck.
const DEFAULT_SEED: &str = "6d75746174";

/// What `bynk test --mutate` was asked to do.
pub struct MutateArgs {
    pub input: PathBuf,
    pub json: bool,
    pub seed: Option<String>,
    /// The user's `--suite` values: they limit which suites may run at all.
    pub suite: Vec<String>,
    pub only: Option<String>,
    pub max_mutants: Option<usize>,
    pub timeout: Option<Duration>,
    pub all_suites: bool,
    pub fail_under: Option<u32>,
}

/// The replacements a token kind takes, as `(source text, replacement text)`.
fn replacement(kind: &TokenKind) -> Option<(&'static str, &'static str)> {
    Some(match kind {
        TokenKind::Lt => ("<", "<="),
        TokenKind::LtEq => ("<=", "<"),
        TokenKind::Gt => (">", ">="),
        TokenKind::GtEq => (">=", ">"),
        TokenKind::EqEq => ("==", "!="),
        TokenKind::BangEq => ("!=", "=="),
        TokenKind::AmpAmp => ("&&", "||"),
        TokenKind::PipePipe => ("||", "&&"),
        _ => return None,
    })
}

/// One place a mutation applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// The key the file is held under in the project's source map.
    pub key: PathBuf,
    /// The project-relative, forward-slashed path shown to the user.
    pub path: String,
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
    /// The unit (`orders`) and declaration (`refund`) the site is in.
    pub unit: String,
    pub decl: String,
    pub from: &'static str,
    pub to: &'static str,
}

impl Site {
    fn qualified(&self) -> String {
        format!("{}.{}", self.unit, self.decl)
    }

    fn id(&self) -> String {
        format!("{}:{}:{}", self.path, self.line, self.column)
    }
}

/// What the project's files say about how units relate.
#[derive(Debug, Default)]
pub struct Model {
    pub sites: Vec<Site>,
    /// Unit → the units it `uses`.
    uses: BTreeMap<String, BTreeSet<String>>,
    /// Each suite: its target and the units it `uses`.
    suites: Vec<(String, BTreeSet<String>)>,
}

impl Model {
    /// Every unit `unit` depends on, itself included.
    fn closure(&self, units: impl IntoIterator<Item = String>) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut work: Vec<String> = units.into_iter().collect();
        while let Some(u) = work.pop() {
            if !seen.insert(u.clone()) {
                continue;
            }
            if let Some(next) = self.uses.get(&u) {
                work.extend(next.iter().cloned());
            }
        }
        seen
    }

    /// The suite targets (their names as the runner reports them) that reach
    /// `unit`: they target it, or a unit that depends on it, or `use` it.
    pub fn suites_reaching(&self, unit: &str) -> BTreeSet<String> {
        self.suites
            .iter()
            .filter(|(target, uses)| {
                self.closure(std::iter::once(target.clone()).chain(uses.iter().cloned()))
                    .contains(unit)
            })
            .map(|(target, _)| target.clone())
            .collect()
    }
}

/// Lex and parse every file in `sources`, collecting the unit graph and the
/// mutation sites of the non-suite files. A file that does not parse
/// contributes nothing (the baseline run fails first on it).
pub fn build_model(root: &Path, sources: &HashMap<PathBuf, String>) -> Model {
    let mut keys: Vec<&PathBuf> = sources
        .keys()
        .filter(|k| k.extension().is_some_and(|e| e == "bynk"))
        .collect();
    keys.sort();
    let mut model = Model::default();
    for (index, key) in keys.into_iter().enumerate() {
        let text = &sources[key];
        let file = FileId(index as u32);
        let Ok(tokens) = tokenize_in(text, file) else {
            continue;
        };
        let Ok(unit) = parse_unit(&tokens, text) else {
            continue;
        };
        let (name, uses, items) = match &unit {
            SourceUnit::Commons(c) => (c.name.joined(), &c.uses, Some(&c.items)),
            SourceUnit::Context(c) => (c.name.joined(), &c.uses, Some(&c.items)),
            SourceUnit::Suite(s) => {
                let used = s.uses.iter().map(|u| u.target.joined()).collect();
                model.suites.push((s.target.joined(), used));
                continue;
            }
            SourceUnit::Adapter(_) => continue,
        };
        model
            .uses
            .entry(name.clone())
            .or_default()
            .extend(uses.iter().map(|u| u.target.joined()));
        let Some(items) = items else { continue };
        let path = display(root, key);
        let positions = LineIndex::new(text);
        for token in &tokens {
            let Some((from, to)) = replacement(&token.kind) else {
                continue;
            };
            let (start, end) = (token.span.start, token.span.end);
            if text.get(start..end) != Some(from) {
                continue;
            }
            let Some(item) = items
                .iter()
                .find(|i| i.span().start <= start && end <= i.span().end)
            else {
                continue;
            };
            let Some(decl) = item.name() else { continue };
            let (line, column) = positions.at(start);
            let (end_line, end_column) = positions.at(end);
            model.sites.push(Site {
                key: key.clone(),
                path: path.clone(),
                start,
                end,
                line,
                column,
                end_line,
                end_column,
                unit: name.clone(),
                decl: decl.name.clone(),
                from,
                to,
            });
        }
    }
    model
        .sites
        .sort_by(|a, b| (&a.path, a.start).cmp(&(&b.path, b.start)));
    model
}

fn display(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

/// Byte offset → 1-based (line, column in characters).
struct LineIndex<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        LineIndex { text, starts }
    }

    fn at(&self, offset: usize) -> (usize, usize) {
        let line = self.starts.partition_point(|&s| s <= offset) - 1;
        let column = self.text[self.starts[line]..offset].chars().count() + 1;
        (line + 1, column)
    }
}

/// `text` with the site's operator replaced.
pub fn apply(text: &str, site: &Site) -> String {
    let mut out = String::with_capacity(text.len() + 1);
    out.push_str(&text[..site.start]);
    out.push_str(site.to);
    out.push_str(&text[site.end..]);
    out
}

/// A tiny glob: `*` matches within a path segment, `**` across segments.
fn glob_matches(pattern: &str, path: &str) -> bool {
    fn go(p: &[u8], s: &[u8]) -> bool {
        match p.split_first() {
            None => s.is_empty(),
            Some((b'*', rest)) => {
                if let Some(rest) = rest.strip_prefix(b"*") {
                    // `**`: any run of characters, `/` included.
                    let rest = rest.strip_prefix(b"/").unwrap_or(rest);
                    // Only at a segment boundary: `src/**/x.bynk` is not
                    // `src/ax.bynk`.
                    (0..=s.len())
                        .filter(|&i| i == 0 || i == s.len() || s[i - 1] == b'/')
                        .any(|i| go(rest, &s[i..]))
                } else {
                    let limit = s.iter().position(|&c| c == b'/').unwrap_or(s.len());
                    (0..=limit).any(|i| go(rest, &s[i..]))
                }
            }
            Some((c, rest)) => s.split_first().is_some_and(|(d, t)| c == d && go(rest, t)),
        }
    }
    go(pattern.as_bytes(), path.as_bytes())
}

/// Does `entry` (a `bynk.toml` `exclude` item or `--only` value) name `site`?
/// A name is `unit`, or `unit.decl`; anything with a `/` or `*` is a path glob.
fn names(entry: &str, site: &Site) -> bool {
    if entry.contains('/') || entry.contains('*') {
        glob_matches(entry, &site.path)
    } else {
        entry == site.unit || entry == site.qualified()
    }
}

/// How a mutant's run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Killed,
    Timeout,
    Survived,
    Invalid,
    NoSuite,
}

impl Status {
    fn word(self) -> &'static str {
        match self {
            Status::Killed => "killed",
            Status::Timeout => "timeout",
            Status::Survived => "survived",
            Status::Invalid => "invalid",
            Status::NoSuite => "no_suite",
        }
    }
}

/// Classify the document one mutant's run printed.
fn classify(doc: &Value) -> Status {
    if let Some(error) = doc.get("error") {
        // A run that crashes before any case runs (a mutant that throws as the
        // module loads) is `invalid` with the rest: the suites were not what
        // told it from the original.
        return if error.get("kind").and_then(Value::as_str) == Some("timeout") {
            Status::Timeout
        } else {
            Status::Invalid
        };
    }
    match doc.get("failed").and_then(Value::as_u64) {
        Some(0) => Status::Survived,
        Some(_) => Status::Killed,
        None => Status::Invalid,
    }
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fail(program: &str, json: bool, message: String) -> ExitCode {
    if json {
        crate::test_stream::print_run(false, &TestRun::runtime_error(message, None));
    } else {
        eprintln!("{program} test: {message}");
    }
    ExitCode::FAILURE
}

/// Run one in-process `bynk test` and parse the JSON document it prints.
fn run_once(args: TestArgs) -> (bool, String, Option<Value>) {
    let (code, text) = crate::test_stream::capture(|| run_test("bynk", args));
    let doc = serde_json::from_str(&text).ok();
    (code == ExitCode::SUCCESS, text, doc)
}

/// What a failed unmutated run said, for a reader of the rich output.
fn describe_failure(doc: Option<&Value>, raw: &str) -> String {
    let Some(doc) = doc else {
        return raw.trim_end().to_string();
    };
    let mut lines = Vec::new();
    if let Some(error) = doc.get("error") {
        if let Some(message) = error.get("message").and_then(Value::as_str) {
            lines.push(message.to_string());
        }
        for key in ["diagnostics", "stderr"] {
            match error.get(key) {
                Some(Value::Array(items)) => {
                    lines.extend(items.iter().filter_map(Value::as_str).map(str::to_string))
                }
                Some(Value::String(text)) => lines.push(text.trim_end().to_string()),
                _ => {}
            }
        }
    }
    for suite in doc
        .get("suites")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = suite.get("name").and_then(Value::as_str).unwrap_or("?");
        for case in suite
            .get("cases")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|c| c.get("outcome").and_then(Value::as_str) == Some("fail"))
        {
            let case_name = case.get("name").and_then(Value::as_str).unwrap_or("?");
            lines.push(format!("✗ {name}: {case_name}"));
            if let Some(message) = case.get("message").and_then(Value::as_str) {
                lines.push(format!("    {}", message.replace('\n', "\n    ")));
            }
        }
    }
    if lines.is_empty() {
        raw.trim_end().to_string()
    } else {
        lines.join("\n")
    }
}

/// Run `bynk test --mutate`.
pub fn run(program: &str, args: MutateArgs) -> ExitCode {
    let MutateArgs {
        input,
        json,
        seed,
        suite,
        only,
        max_mutants,
        timeout,
        all_suites,
        fail_under,
    } = args;
    if !input.is_dir() {
        return fail(
            program,
            json,
            format!(
                "input `{}` must be a project directory containing `.bynk` files",
                input.display()
            ),
        );
    }
    let sources = match crate::try_project_options(&input) {
        Ok(options) => options.sources.unwrap_or_default(),
        Err(e) => return fail(program, json, e.to_string()),
    };
    let exclude = match crate::project::try_read_mutate_exclude(&input) {
        Ok(list) => list,
        Err(e) => return fail(program, json, e),
    };
    let mut model = build_model(&input, &sources);
    model.sites.retain(|s| !exclude.iter().any(|e| names(e, s)));
    if let Some(only) = &only {
        model.sites.retain(|s| names(only, s));
        if model.sites.is_empty() {
            return fail(
                program,
                json,
                format!(
                    "`--only {only}` names no declaration with anything to mutate (use `unit` or `unit.decl`)"
                ),
            );
        }
    }
    let scratch = Scratch(std::env::temp_dir().join(format!(
        "bynk-mutate-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    )));
    let seed = Some(seed.unwrap_or_else(|| DEFAULT_SEED.to_string()));
    let base_args = |suites: Vec<String>| TestArgs {
        no_run: false,
        format: TestFormat::Json,
        output: Some(scratch.0.join("out")),
        seed: seed.clone(),
        suite: suites,
        ..TestArgs::discovery(input.clone())
    };

    // The unmutated project must pass: otherwise every mutant "fails" and is
    // killed by the baseline's own failure.
    if !json {
        eprintln!("{program} test: running the unmutated suites first");
    }
    let started = Instant::now();
    let (ok, text, doc) = run_once(base_args(suite.clone()));
    let baseline_time = started.elapsed();
    let Some(mut baseline) = doc.clone().filter(|_| ok) else {
        if json {
            print!("{text}");
        } else {
            eprintln!("{}", describe_failure(doc.as_ref(), &text));
        }
        eprintln!(
            "{program} test: the unmutated project does not pass, so there is nothing to mutate — fix it first"
        );
        return ExitCode::FAILURE;
    };
    let suite_names: Vec<String> = baseline
        .get("suites")
        .and_then(Value::as_array)
        .map(|suites| {
            suites
                .iter()
                .filter_map(|s| s.get("name").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let timeout = timeout.unwrap_or_else(|| (baseline_time * 3).max(Duration::from_secs(10)));

    if let Some(max) = max_mutants {
        model.sites.truncate(max);
    }
    let total = model.sites.len();
    let mut results: Vec<(Site, Status, Vec<String>)> = Vec::new();
    for (i, site) in model.sites.iter().enumerate() {
        let selected: Vec<String> = if all_suites {
            suite.clone()
        } else {
            let reaching = model.suites_reaching(&site.unit);
            suite_names
                .iter()
                .filter(|n| reaching.contains(*n))
                .cloned()
                .collect()
        };
        let status = if !all_suites && selected.is_empty() {
            Status::NoSuite
        } else {
            let text = apply(&sources[&site.key], site);
            let (_, _, doc) = run_once(TestArgs {
                mutant_overlay: Some((site.key.clone(), text)),
                run_timeout: Some(timeout),
                ..base_args(selected.clone())
            });
            doc.as_ref().map_or(Status::Invalid, classify)
        };
        if !json {
            eprintln!(
                "[{}/{total}] {}:{}:{} `{}` → `{}`: {}",
                i + 1,
                site.path,
                site.line,
                site.column,
                site.from,
                site.to,
                status.word().replace('_', " ")
            );
        }
        let shown = if all_suites && status != Status::Invalid {
            suite_names.clone()
        } else {
            selected
        };
        results.push((site.clone(), status, shown));
    }

    let count = |s: Status| results.iter().filter(|(_, st, _)| *st == s).count();
    let (killed, timed_out) = (count(Status::Killed), count(Status::Timeout));
    let (survived, invalid, no_suite) = (
        count(Status::Survived),
        count(Status::Invalid),
        count(Status::NoSuite),
    );
    let ran = killed + timed_out + survived;
    let score = crate::coverage::percent((killed + timed_out) as u32, ran as u32);

    if json {
        let mutants: Vec<Value> = results
            .iter()
            .map(|(s, st, suites)| {
                json!({
                    "id": s.id(),
                    "file": s.path,
                    "line": s.line,
                    "column": s.column,
                    "end_line": s.end_line,
                    "end_column": s.end_column,
                    "unit": s.unit,
                    "decl": s.decl,
                    "from": s.from,
                    "to": s.to,
                    "status": st.word(),
                    "suites": suites,
                })
            })
            .collect();
        baseline["mutation"] = json!({
            "total": total,
            "killed": killed,
            "timeout": timed_out,
            "survived": survived,
            "no_suite": no_suite,
            "invalid": invalid,
            "score": score,
            "mutants": mutants,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&baseline).unwrap_or_default()
        );
    } else {
        println!(
            "mutation: {total} mutants, {} killed, {survived} survived, {no_suite} not covered by any suite, {invalid} invalid (score {score}%)",
            killed + timed_out
        );
        for (site, status, _) in &results {
            let label = match status {
                Status::Survived => "SURVIVED",
                Status::NoSuite => "NO SUITE",
                _ => continue,
            };
            println!(
                "  {label:<9} {}:{}:{}  {}  `{}` → `{}`",
                site.path,
                site.line,
                site.column,
                site.qualified(),
                site.from,
                site.to
            );
        }
        if ran == 0 && total > 0 {
            println!("no mutant reached a suite — nothing was scored");
        }
    }
    match fail_under {
        Some(min) if ran == 0 => {
            eprintln!(
                "{program} test: no mutant reached a suite, so --mutate-fail-under {min} cannot be met"
            );
            ExitCode::FAILURE
        }
        Some(min) if score < min => {
            eprintln!("{program} test: mutation score {score}% is below --mutate-fail-under {min}");
            ExitCode::FAILURE
        }
        _ => ExitCode::SUCCESS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(files: &[(&str, &str)]) -> HashMap<PathBuf, String> {
        files
            .iter()
            .map(|(p, t)| (PathBuf::from(format!("/proj/{p}")), t.to_string()))
            .collect()
    }

    const LIMITS: &str =
        "commons limits\n\nfn within(a: Int, b: Int) -> Bool {\n  a < b && b != 0\n}\n";

    #[test]
    fn finds_comparison_and_connective_sites_with_spans() {
        let model = build_model(Path::new("/proj"), &sources(&[("src/limits.bynk", LIMITS)]));
        let found: Vec<(String, &str, &str, usize, usize)> = model
            .sites
            .iter()
            .map(|s| (s.qualified(), s.from, s.to, s.line, s.column))
            .collect();
        assert_eq!(
            found,
            vec![
                ("limits.within".to_string(), "<", "<=", 4, 5),
                ("limits.within".to_string(), "&&", "||", 4, 9),
                ("limits.within".to_string(), "!=", "==", 4, 14),
            ]
        );
        assert_eq!(model.sites[0].path, "src/limits.bynk");
    }

    #[test]
    fn apply_replaces_exactly_the_operator() {
        let model = build_model(Path::new("/proj"), &sources(&[("src/limits.bynk", LIMITS)]));
        let mutant = apply(LIMITS, &model.sites[0]);
        assert_eq!(mutant, LIMITS.replacen("a < b", "a <= b", 1));
    }

    #[test]
    fn operators_in_comments_and_strings_are_not_sites() {
        let text = "commons c\n\n// a < b\nfn f() -> String {\n  \"a == b\"\n}\n";
        let model = build_model(Path::new("/proj"), &sources(&[("src/c.bynk", text)]));
        assert!(model.sites.is_empty(), "{:?}", model.sites);
    }

    #[test]
    fn a_suite_reaches_its_target_and_the_units_that_use_it() {
        let a = "commons a\n\nfn f(x: Int) -> Bool {\n  x > 1\n}\n";
        let b = "commons b\n\nuses a\n\nfn g(x: Int) -> Bool {\n  f(x)\n}\n";
        let c = "commons c\n\nfn h(x: Int) -> Bool {\n  x > 2\n}\n";
        let s = "suite b\n\ncase \"x\" {\n  expect g(2)\n}\n";
        let model = build_model(
            Path::new("/proj"),
            &sources(&[
                ("src/a.bynk", a),
                ("src/b.bynk", b),
                ("src/c.bynk", c),
                ("tests/b.bynk", s),
            ]),
        );
        assert_eq!(
            model.suites_reaching("a"),
            BTreeSet::from(["b".to_string()])
        );
        assert_eq!(
            model.suites_reaching("b"),
            BTreeSet::from(["b".to_string()])
        );
        assert!(model.suites_reaching("c").is_empty());
    }

    #[test]
    fn globs_and_names() {
        assert!(glob_matches("src/generated/**", "src/generated/a/b.bynk"));
        assert!(glob_matches("src/*.bynk", "src/a.bynk"));
        assert!(glob_matches("src/**/x.bynk", "src/a/x.bynk"));
        assert!(glob_matches("src/**/x.bynk", "src/x.bynk"));
        assert!(!glob_matches("src/**/x.bynk", "src/ax.bynk"));
        assert!(glob_matches("**/lonely.bynk", "src/lonely.bynk"));
        assert!(!glob_matches("**/lonely.bynk", "src/notlonely.bynk"));
        assert!(!glob_matches("src/*.bynk", "src/a/b.bynk"));
        let model = build_model(Path::new("/proj"), &sources(&[("src/limits.bynk", LIMITS)]));
        let site = &model.sites[0];
        assert!(names("limits", site));
        assert!(names("limits.within", site));
        assert!(!names("limits.other", site));
        assert!(names("src/**", site));
    }

    #[test]
    fn classifies_run_documents() {
        assert_eq!(
            classify(&json!({"passed": 2, "failed": 0})),
            Status::Survived
        );
        assert_eq!(classify(&json!({"passed": 1, "failed": 1})), Status::Killed);
        assert_eq!(
            classify(&json!({"error": {"kind": "compile"}})),
            Status::Invalid
        );
        assert_eq!(
            classify(&json!({"error": {"kind": "timeout", "message": "timed out after 10s"}})),
            Status::Timeout
        );
    }
}
