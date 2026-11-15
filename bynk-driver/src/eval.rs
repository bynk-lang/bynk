//! `bynk eval` (#3585): evaluate a snippet in a project's context and report
//! the value it ends in.
//!
//! The snippet becomes the body of one `case` in a synthetic `suite` that
//! targets a unit of the project, exactly as a doc example marked `bynk test`
//! does ([`crate::doc_examples`]), so it sees what a suite for that unit sees:
//! the unit's declarations, its own `consumes`, and the `stub`s it declares. It
//! is built and run by [`crate::test_runner::run_test`] under the test runtime,
//! so no real capability is ever reached (a capability the snippet calls needs a
//! `stub`). The case's last expression is not discarded, as an ordinary case's
//! is: it is encoded as the wire shape `Json.encode` gives it and reported
//! beside its type (`lower_test_case_body_reporting` in `bynk-emit`).
//!
//! **Stateless.** Each evaluation is independent. A client that wants an
//! earlier cell's `let`s in scope sends them again ahead of the snippet.
//!
//! **Header.** A suite's `consumes` and `stub` clauses belong to the suite, not
//! to a case, so the snippet's leading `consumes` and `stub` lines (with the
//! blank and `//` comment lines among them) are the suite's, and the rest is
//! the case's body. A snippet uses what it needs from another unit by
//! `consumes`ing it, as a suite file does.
//!
//! **Line-preserving.** The synthetic suite has the snippet's lines, each on
//! the line it has in the snippet; only the first line is longer, by the
//! generated `suite … {` opening.
//!
//! This is an explicit, local run of code the user wrote in their own workspace,
//! at the trust level of `bynk test`. It does not overturn ADR 0638, which kept
//! the refined-type explorer off the build-and-run path because a hover is an
//! implicit request.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use bynk_syntax::ast::SourceUnit;
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::parse_unit_with_recovery;
use serde_json::Value;

use crate::test_runner::{TestArgs, TestFormat, run_test};

/// The name of the synthetic case. `bynk-driver` lowers a case of this name
/// with its tail reported, and the run selects it by this name.
pub const EVAL_CASE: &str = "bynk eval";

/// How long an evaluation may run before it is killed, when the caller sets no
/// limit of its own.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// What to evaluate, and where.
#[derive(Debug, Clone)]
pub struct EvalRequest {
    /// The project root.
    pub input: PathBuf,
    /// The dotted name of the unit (a `commons` or a `context`) whose scope the
    /// snippet is evaluated in.
    pub unit: String,
    /// The snippet: optional leading `consumes`/`stub` lines, then statements
    /// ending in the expression whose value is wanted.
    pub snippet: String,
    /// Unsaved buffers, layered over the project's files (ADR 0626): an
    /// absolute path to the file's text.
    pub buffers: HashMap<PathBuf, String>,
    /// The project's own suites to build in, by target unit: the suites whose
    /// `stub`s the snippet wants. Every other suite of the project is left out
    /// of the build, so an unrelated suite that does not compile cannot block an
    /// evaluation; a suite named here is built, and its errors are reported.
    pub suites: Vec<String>,
    /// The time limit. The evaluation is killed, and reported as timed out,
    /// after this long.
    pub timeout: Duration,
}

/// One diagnostic of a snippet that does not check.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct EvalDiagnostic {
    /// The file it is in, as the project names it; absent for the snippet
    /// itself, whose `line` and `col` are then the snippet's own (1-based).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    pub line: u32,
    pub col: u32,
    /// `"error"`, `"warning"` or `"hint"`.
    pub severity: String,
    /// The diagnostic's code, `bynk.types.let_annotation_mismatch`.
    pub code: String,
    pub message: String,
}

/// The result document `bynk eval` prints: `{"value", "type", "diagnostics"}`,
/// and an `error` when the snippet ran to no value.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct EvalReport {
    /// The wire shape of the snippet's last expression; `null` when there is
    /// none (and when the expression is the unit value, which `type` tells).
    pub value: Option<Value>,
    /// That expression's type, or `null` with no value.
    #[serde(rename = "type")]
    pub ty: Option<String>,
    /// Why the snippet did not check, one entry each; empty otherwise.
    pub diagnostics: Vec<EvalDiagnostic>,
    /// Absent when there is a value or `diagnostics` says why not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<EvalError>,
}

/// A snippet that checked but ran to no value.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct EvalError {
    /// `"runtime"` (a refusal, a failed `expect`, an unstubbed capability),
    /// `"timeout"`, or `"setup"` (no such unit, no runner).
    pub kind: &'static str,
    pub message: String,
}

impl EvalReport {
    /// Whether the snippet produced a value.
    pub fn succeeded(&self) -> bool {
        self.ty.is_some()
    }
}

impl From<EvalOutcome> for EvalReport {
    fn from(outcome: EvalOutcome) -> Self {
        let none = |diagnostics, error| EvalReport {
            value: None,
            ty: None,
            diagnostics,
            error,
        };
        let error = |kind, message| none(Vec::new(), Some(EvalError { kind, message }));
        match outcome {
            EvalOutcome::Value { value, ty } => EvalReport {
                value: Some(value),
                ty: Some(ty),
                diagnostics: Vec::new(),
                error: None,
            },
            EvalOutcome::Diagnostics(diagnostics) => none(diagnostics, None),
            EvalOutcome::Failed(message) => error("runtime", message),
            EvalOutcome::TimedOut(message) => error("timeout", message),
            EvalOutcome::Error(message) => error("setup", message),
        }
    }
}

/// Where the snippet sits in the file it was built into, to put a diagnostic
/// back at the snippet's own line and column.
struct Placement {
    /// The file's name.
    file: String,
    /// The lines of the file ahead of the snippet's first.
    line_offset: u32,
    /// The characters generated ahead of the snippet on its first line.
    prefix: usize,
}

/// The `--format short` line `line` as a diagnostic. A line with no location
/// (`error[code]: message`) has line and column 0.
fn parse_diagnostic(line: &str, at: &Placement) -> Option<EvalDiagnostic> {
    let (location, rest) = [": error[", ": warning[", ": hint["]
        .iter()
        .find_map(|marker| {
            line.find(marker)
                .map(|at| (Some(&line[..at]), &line[at + 2..]))
        })
        .or_else(|| {
            ["error[", "warning[", "hint["]
                .iter()
                .any(|m| line.starts_with(m))
                .then_some((None, line))
        })?;
    let (severity, rest) = rest.split_once('[')?;
    let (code, message) = rest.split_once("]: ")?;
    let (mut file, mut line_no, mut col) = (None, 0u32, 0u32);
    if let Some(location) = location {
        let mut parts = location.rsplitn(3, ':');
        col = parts.next()?.parse().ok()?;
        line_no = parts.next()?.parse().ok()?;
        let path = parts.next()?;
        let in_file = Path::new(path).file_name().and_then(|n| n.to_str()) == Some(&at.file);
        if in_file && line_no > at.line_offset {
            // The snippet's own line; the first carries any generated opening.
            line_no -= at.line_offset;
            if line_no == 1 {
                col = col.saturating_sub(at.prefix as u32).max(1);
            }
        } else {
            file = Some(path.to_string());
        }
    }
    Some(EvalDiagnostic {
        file,
        line: line_no,
        col,
        severity: severity.to_string(),
        code: code.to_string(),
        message: message.to_string(),
    })
}

/// What an evaluation produced.
#[derive(Debug, Clone, PartialEq)]
pub enum EvalOutcome {
    /// The snippet ran. `value` is the wire shape of its last expression and
    /// `ty` that expression's type.
    Value { value: Value, ty: String },
    /// The project or the snippet did not compile: the diagnostics.
    Diagnostics(Vec<EvalDiagnostic>),
    /// The snippet compiled and ran, but a failure stopped it (a refused
    /// refined-type construction, an `expect` that did not hold, a capability
    /// with no stub).
    Failed(String),
    /// The snippet ran past its time limit and was killed.
    TimedOut(String),
    /// The evaluation could not be set up or run (no such unit, no runner).
    Error(String),
}

/// The unit `text` declares, when it parses without an error. A warning the
/// parser reports (a lint) does not stop a unit being found.
fn parse_clean(text: &str) -> Option<SourceUnit> {
    let tokens = tokenize(text).ok()?;
    let (unit, errors) = parse_unit_with_recovery(&tokens, text);
    errors
        .iter()
        .all(|e| bynk_syntax::Severity::for_error(e) != bynk_syntax::Severity::Error)
        .then_some(unit)
        .flatten()
}

/// The key, in `sources`, of the file that declares the unit `unit`.
fn unit_file(sources: &HashMap<PathBuf, String>, unit: &str) -> Option<PathBuf> {
    let mut keys: Vec<&PathBuf> = sources.keys().collect();
    keys.sort();
    keys.into_iter()
        .find(|key| match parse_clean(&sources[*key]) {
            Some(SourceUnit::Commons(c)) => c.name.joined() == unit,
            Some(SourceUnit::Context(c)) => c.name.joined() == unit,
            _ => false,
        })
        .cloned()
}

/// What stands in for a suite file left out of the build: a suite of one case
/// that is never selected. (A suite with no case emits a module that does not
/// type-check.)
fn left_out(target: &str) -> String {
    format!(
        "// left out of this `bynk eval` build\nsuite {target} {{ case \"left out\" {{ expect 1 == 1 }} }}\n"
    )
}

/// A stand-in for every suite of `sources` that is not among `keep` (by target
/// unit), to lay over them: a snippet needs only its target unit's own
/// declarations, and the project's suites are built only when asked for.
fn left_out_suites(
    sources: &HashMap<PathBuf, String>,
    keep: &[String],
    unit: &str,
) -> HashMap<PathBuf, String> {
    let mut out = HashMap::new();
    for (key, text) in sources {
        let target = match parse_clean(text) {
            Some(SourceUnit::Suite(suite)) => Some(suite.target.joined()),
            Some(_) => None,
            // A suite that does not parse is still left out, as the target's.
            None => text
                .lines()
                .any(|l| l.starts_with("suite "))
                .then(|| unit.to_string()),
        };
        if let Some(target) = target.filter(|t| !keep.contains(t)) {
            out.insert(key.clone(), left_out(&target));
        }
    }
    out
}

/// How many leading lines of `snippet` are the suite's own: `consumes` and
/// `stub` clauses (each running to its closing bracket if it opens one), and
/// the blank and `//` lines among them. Zero when the snippet opens with
/// anything else.
fn header_lines(snippet: &str) -> usize {
    let mut end = 0;
    let mut depth: i32 = 0;
    for (i, line) in snippet.lines().enumerate() {
        let trimmed = line.trim();
        if depth > 0 {
            // The continuation of a clause with an open bracket.
        } else if trimmed.starts_with("consumes ") || trimmed.starts_with("stub ") {
            end = i + 1;
        } else if !(trimmed.is_empty() || trimmed.starts_with("//")) {
            break;
        }
        if depth > 0 || trimmed.starts_with("consumes ") || trimmed.starts_with("stub ") {
            depth += bracket_delta(trimmed);
            if depth <= 0 {
                depth = 0;
                end = i + 1;
            }
        }
    }
    end
}

/// How many more brackets `line` opens than it closes, leaving out those in a
/// string literal and in a `//` comment.
fn bracket_delta(line: &str) -> i32 {
    let mut delta = 0;
    let mut in_string = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if in_string => {
                chars.next();
            }
            '"' => in_string = !in_string,
            '/' if !in_string && chars.peek() == Some(&'/') => break,
            '{' | '(' | '[' if !in_string => delta += 1,
            '}' | ')' | ']' if !in_string => delta -= 1,
            _ => {}
        }
    }
    delta
}

/// The snippet's lines, with the case opened on the last header line (or the
/// first, with no header) and no closer, and how many characters that opening
/// adds to the first line.
fn case_lines(snippet: &str) -> (Vec<String>, usize) {
    let header = header_lines(snippet);
    let mut lines: Vec<String> = snippet.lines().map(str::to_string).collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    let open = format!("case \"{EVAL_CASE}\" {{");
    if header == 0 {
        lines[0] = format!("{open} {}", lines[0]);
        (lines, open.len() + 1)
    } else {
        lines[header - 1].push_str(&format!(" {open}"));
        (lines, 0)
    }
}

/// The synthetic suite for `snippet`, targeting `unit`: the snippet's own
/// lines, with the suite and case openings added to the first and last header
/// lines and the closers after the last line. Also how many characters the
/// generated opening adds to the first line.
pub fn suite_text(unit: &str, snippet: &str) -> (String, usize) {
    let (mut lines, case_prefix) = case_lines(snippet);
    let suite_prefix = format!("suite {unit} {{ ");
    lines[0] = format!("{suite_prefix}{}", lines[0]);
    let mut text = lines.join("\n");
    text.push_str("\n} }\n");
    (text, suite_prefix.len() + case_prefix)
}

/// `existing`, a suite file, with the snippet's case added at its end (before
/// its closing brace, in brace form, whose offset is `close`): the snippet then
/// sees the suite's own `consumes`, `stub`s, `fn`s and fixtures. Also the lines
/// ahead of the snippet's first and the characters generated on it.
fn merged_suite_text(existing: &str, close: Option<usize>, snippet: &str) -> (String, u32, usize) {
    let (lines, prefix) = case_lines(snippet);
    let head = &existing[..close.unwrap_or(existing.len())];
    let mut text = format!("{head}\n{}\n", lines.join("\n"));
    let offset = text[..head.len() + 1].matches('\n').count() as u32;
    text.push_str("}\n");
    if let Some(close) = close {
        text.push_str(&existing[close..]);
    }
    (text, offset, prefix)
}

/// The suite file of `sources` for the target `unit` that a snippet is merged
/// into when the caller names that suite: its key and the offset of its closing
/// brace. An error says why the unit's suites cannot host a snippet.
fn host_suite(
    sources: &HashMap<PathBuf, String>,
    unit: &str,
) -> Result<Option<(PathBuf, Option<usize>)>, String> {
    let mut keys: Vec<&PathBuf> = sources.keys().collect();
    keys.sort();
    for key in keys {
        let text = &sources[key];
        let Some(SourceUnit::Suite(suite)) = parse_clean(text) else {
            continue;
        };
        if suite.target.joined() != unit {
            continue;
        }
        if suite
            .tier
            .is_some_and(|t| t != bynk_syntax::ast::TestTier::Unit)
        {
            return Err(format!(
                "the suite for `{unit}` is not a unit suite, and a snippet is evaluated at the unit tier"
            ));
        }
        let close = match suite.form {
            bynk_syntax::ast::CommonsForm::Fragment => None,
            bynk_syntax::ast::CommonsForm::Brace => match text[..suite.span.end].rfind('}') {
                Some(at) => Some(at),
                None => continue,
            },
        };
        return Ok(Some((key.clone(), close)));
    }
    Ok(None)
}

/// Run `request`: the snippet's value, or why there is none.
pub fn evaluate(request: &EvalRequest) -> EvalOutcome {
    // Absolute, so the synthetic file's key is inside the root an overlay is
    // judged against, however the project was named.
    let input = request
        .input
        .canonicalize()
        .unwrap_or_else(|_| request.input.clone());
    let options = match crate::try_project_options(&input) {
        Ok(o) => o,
        Err(e) => return EvalOutcome::Error(e.to_string()),
    };
    // The project's sources, with the buffers laid over them, to find the unit.
    // A buffer is matched to a project file by canonical path, as the overlay
    // is, however either was spelled.
    let buffers: HashMap<PathBuf, String> = request
        .buffers
        .iter()
        .map(|(path, text)| {
            (
                path.canonicalize().unwrap_or_else(|_| path.clone()),
                text.clone(),
            )
        })
        .collect();
    let mut sources = options.sources.clone().unwrap_or_default();
    sources.retain(|k, _| !buffers.contains_key(&k.canonicalize().unwrap_or_else(|_| k.clone())));
    sources.extend(buffers.clone());
    let Some(key) = unit_file(&sources, &request.unit) else {
        return EvalOutcome::Error(format!(
            "no `commons` or `context` named `{}` in the project",
            request.unit
        ));
    };
    // The suite sits beside the unit's file. A file the project really has is
    // never overwritten.
    let stem = key.file_stem().unwrap_or_default().to_string_lossy();
    let mut synthetic = key.with_file_name(format!("{stem}.eval.bynk"));
    let mut n = 0;
    while sources.contains_key(&synthetic) {
        n += 1;
        synthetic = key.with_file_name(format!("{stem}.eval{n}.bynk"));
    }
    let mut overlay = buffers;
    overlay.extend(left_out_suites(&sources, &request.suites, &request.unit));
    // A snippet that wants a suite's stubs is built into that suite; any other
    // gets a suite of its own, beside the unit's file.
    let hosted = if request.suites.contains(&request.unit) {
        match host_suite(&sources, &request.unit) {
            Ok(host) => host,
            Err(message) => return EvalOutcome::Error(message),
        }
    } else {
        None
    };
    let placement = match hosted {
        Some((key, close)) => {
            let (text, line_offset, prefix) =
                merged_suite_text(&sources[&key], close, &request.snippet);
            let file = key
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            overlay.insert(key, text);
            Placement {
                file,
                line_offset,
                prefix,
            }
        }
        None => {
            let (text, prefix) = suite_text(&request.unit, &request.snippet);
            let file = synthetic
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            overlay.insert(synthetic, text);
            Placement {
                file,
                line_offset: 0,
                prefix,
            }
        }
    };
    let args = TestArgs {
        no_run: false,
        format: TestFormat::Json,
        suite: vec![request.unit.clone()],
        case: vec![EVAL_CASE.to_string()],
        run_timeout: Some(request.timeout),
        overlay: Some(overlay),
        ..TestArgs::discovery(input)
    };
    let (code, text) = crate::test_stream::capture(|| run_test("bynk", args));
    interpret(code, &text, &placement)
}

/// What the `bynk test --format json` document `text` says about the snippet.
fn interpret(code: ExitCode, text: &str, at: &Placement) -> EvalOutcome {
    let Ok(doc) = serde_json::from_str::<Value>(text) else {
        return EvalOutcome::Error(format!("the test run said: {}", text.trim()));
    };
    let lines = |key: &str| -> Vec<String> {
        doc["error"][key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l.as_str().map(str::to_string))
            .collect()
    };
    let message = doc["error"]["message"].as_str().map(str::to_string);
    match doc["error"]["kind"].as_str() {
        Some("compile") => {
            return EvalOutcome::Diagnostics(
                lines("diagnostics")
                    .iter()
                    .filter_map(|l| parse_diagnostic(l, at))
                    .collect(),
            );
        }
        Some("timeout") => {
            return EvalOutcome::TimedOut(message.unwrap_or_else(|| "timed out".to_string()));
        }
        Some(_) => {
            let mut parts: Vec<String> = message.into_iter().collect();
            parts.extend(doc["error"]["stderr"].as_str().map(str::to_string));
            return EvalOutcome::Error(parts.join("\n"));
        }
        None => {}
    }
    if let Some(eval) = doc.get("eval") {
        let ty = eval["type"].as_str().unwrap_or_default().to_string();
        return match eval.get("value") {
            Some(value) => EvalOutcome::Value {
                value: value.clone(),
                ty,
            },
            None => EvalOutcome::Failed(format!(
                "the snippet ran, but its value, of type `{ty}`, has no wire shape to report"
            )),
        };
    }
    let failure = doc["suites"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| s["cases"].as_array().into_iter().flatten())
        .find(|c| c["outcome"] == "fail")
        .map(|c| c["message"].as_str().unwrap_or("the snippet failed"));
    match failure {
        Some(message) => EvalOutcome::Failed(message.to_string()),
        None if code == ExitCode::SUCCESS => {
            EvalOutcome::Error("the snippet ran but reported no value".to_string())
        }
        None => EvalOutcome::Error(format!("the test run failed: {}", text.trim())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snippet_without_a_header_is_all_case_body() {
        assert_eq!(header_lines("let x = 1\nx + 1"), 0);
        assert_eq!(
            suite_text("demo.calc", "double(21)"),
            (
                "suite demo.calc { case \"bynk eval\" { double(21)\n} }\n".to_string(),
                "suite demo.calc { case \"bynk eval\" { ".len()
            )
        );
    }

    #[test]
    fn leading_consumes_and_stub_lines_are_the_suites() {
        let snippet =
            "consumes shop.order { Placed }\n// a stub\nstub Clock.now() returns 0\nlet x = 1\nx";
        assert_eq!(header_lines(snippet), 3);
        assert_eq!(
            suite_text("demo.calc", snippet).0,
            "suite demo.calc { consumes shop.order { Placed }\n// a stub\nstub Clock.now() returns 0 case \"bynk eval\" {\nlet x = 1\nx\n} }\n"
        );
    }

    #[test]
    fn a_clause_with_an_open_bracket_runs_to_its_close() {
        let snippet = "consumes shop.order {\n  Placed,\n  Shipped,\n}\nlet n = 1\nn";
        assert_eq!(header_lines(snippet), 4);
    }

    #[test]
    fn a_clause_after_a_statement_is_the_bodys() {
        // It is a parse error there, and the compiler says so at its line.
        assert_eq!(header_lines("let x = 1\nconsumes a.b\nx"), 0);
    }

    const UNIT: &str = "\
context demo

consumes bynk { Fetch, Request, Method, Response }

type Percent = Int where InRange(0, 100)

fn double(n: Int) -> Int {
  n * 2
}

fn spin(n: Int) -> Int {
  spin(n + 1)
}

service relay {
  on call(url: String) -> Effect[Int] given Fetch {
    let res <- Fetch.send(Request {
      method: Get,
      url: url,
      contentType: None,
      authorization: None,
      headers: Map.empty(),
      body: None,
    })
    match res {
      Ok(r) => Effect.pure(r.status)
      Err(_) => Effect.pure(-1)
    }
  }
}
";

    const FETCH: &str = "\
consumes bynk { Fetch, Request, Method, Response }
let res <- Fetch.send(Request { method: Get, url: \"https://example.com\", contentType: None, authorization: None, headers: Map.empty(), body: None })
";

    /// A scratch project, removed when dropped.
    struct Project(PathBuf);

    impl std::ops::Deref for Project {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Project {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn project(tag: &str) -> Project {
        project_with(tag, UNIT)
    }

    fn project_with(tag: &str, unit: &str) -> Project {
        let root = std::env::temp_dir().join(format!("bynk_eval_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("bynk.toml"), "[project]\nname = \"p\"\n").unwrap();
        std::fs::write(root.join("src/demo.bynk"), unit).unwrap();
        Project(root.canonicalize().unwrap())
    }

    fn eval_with(root: &Path, snippet: &str, timeout: Duration) -> EvalOutcome {
        evaluate(&EvalRequest {
            input: root.to_path_buf(),
            unit: "demo".to_string(),
            snippet: snippet.to_string(),
            buffers: HashMap::new(),
            suites: Vec::new(),
            timeout,
        })
    }

    fn eval(root: &Path, snippet: &str) -> EvalOutcome {
        eval_with(root, snippet, Duration::from_secs(120))
    }

    fn value(outcome: EvalOutcome) -> (Value, String) {
        match outcome {
            EvalOutcome::Value { value, ty } => (value, ty),
            other => panic!("expected a value, got {other:?}"),
        }
    }

    /// One project, several evaluations: each is a real build and run.
    #[test]
    fn evaluates_in_a_units_scope() {
        let root = project("scope");

        let (v, ty) = value(eval(&root, "double(21)"));
        assert_eq!((v, ty.as_str()), (serde_json::json!(42), "Int"));

        // Earlier cells arrive as a preamble of `let`s.
        let (v, _) = value(eval(&root, "let x = 4\ndouble(x)"));
        assert_eq!(v, serde_json::json!(8));

        let (v, ty) = value(eval(&root, "[1, 2, 3]"));
        assert_eq!(
            (v, ty.as_str()),
            (serde_json::json!([1, 2, 3]), "List[Int]")
        );

        // A refined-type construction that succeeds, and one that fails.
        let (v, ty) = value(eval(&root, "Percent.of(50)"));
        assert_eq!(v, serde_json::json!({"kind": "Ok", "value": 50}));
        assert_eq!(ty, "Result[Percent, ValidationError]");
        let (v, _) = value(eval(&root, "Percent.of(500)"));
        assert_eq!(v["kind"], "Err");
        assert_eq!(v["error"]["message"], "must be in range [0, 100]");
    }

    #[test]
    fn a_capability_needs_a_stub() {
        let root = project("stub");
        // Unstubbed, the test runtime refuses: nothing reaches the network.
        match eval(&root, &format!("{FETCH}res")) {
            EvalOutcome::Failed(message) => {
                assert!(message.contains("`Fetch.send` is not stubbed"), "{message}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        // Stubbed in the preamble, it answers.
        let stubbed = format!(
            "stub Fetch.send(_) returns Ok(Response {{ status: 204, headers: Map.empty(), body: \"\" }})\n{FETCH}"
        );
        let (v, _) = value(eval(&root, &format!("{stubbed}res")));
        assert_eq!(v["kind"], "Ok");
        assert_eq!(v["value"]["status"], 204);
    }

    #[test]
    fn a_snippet_that_does_not_check_reports_diagnostics() {
        let root = project("diag");
        match eval(&root, "let x: Int = \"oops\"\nx") {
            EvalOutcome::Diagnostics(found) => {
                // In the snippet, on its line, at the column in the snippet.
                let d = found
                    .iter()
                    .find(|d| d.code == "bynk.types.let_annotation_mismatch")
                    .unwrap_or_else(|| panic!("{found:?}"));
                assert_eq!((d.file.as_deref(), d.line, d.col), (None, 1, 14));
                assert_eq!(d.severity, "error");
            }
            other => panic!("expected diagnostics, got {other:?}"),
        }
    }

    #[test]
    fn a_snippet_that_does_not_end_is_killed() {
        let root = project("timeout");
        let outcome = eval_with(&root, "spin(0)", Duration::from_secs(5));
        assert!(matches!(outcome, EvalOutcome::TimedOut(_)), "{outcome:?}");
    }

    #[test]
    fn an_unsaved_buffer_is_what_is_evaluated() {
        let root = project("buffer");
        let file = root.join("src/demo.bynk");
        let edited = UNIT.replace("n * 2", "n * 3");
        let outcome = evaluate(&EvalRequest {
            input: root.to_path_buf(),
            unit: "demo".to_string(),
            snippet: "double(2)".to_string(),
            buffers: HashMap::from([(file, edited)]),
            suites: Vec::new(),
            timeout: Duration::from_secs(120),
        });
        assert_eq!(value(outcome).0, serde_json::json!(6));
    }

    #[test]
    fn an_unknown_unit_is_an_error() {
        let root = project("unknown");
        let outcome = evaluate(&EvalRequest {
            input: root.to_path_buf(),
            unit: "nope".to_string(),
            snippet: "1".to_string(),
            buffers: HashMap::new(),
            suites: Vec::new(),
            timeout: Duration::from_secs(5),
        });
        assert!(matches!(outcome, EvalOutcome::Error(_)), "{outcome:?}");
    }

    #[test]
    fn a_short_line_is_a_diagnostic() {
        let at = |line_offset| Placement {
            file: "demo.eval.bynk".to_string(),
            line_offset,
            prefix: 10,
        };
        let d = parse_diagnostic(
            "/p/src/demo.eval.bynk:2:5: error[bynk.resolve.unknown_name]: unknown name `x`",
            &at(0),
        )
        .unwrap();
        assert_eq!((d.file, d.line, d.col), (None, 2, 5));
        let d = parse_diagnostic("src/demo.bynk:7:1: warning[a.b]: odd: really", &at(0)).unwrap();
        assert_eq!(d.file.as_deref(), Some("src/demo.bynk"));
        assert_eq!(d.message, "odd: really");
        let d = parse_diagnostic("error[a.b]: no place", &at(0)).unwrap();
        assert_eq!((d.line, d.col), (0, 0));
        // Built into a suite, the lines ahead of the snippet are the suite's.
        let line = "/p/tests/demo.bynk:9:3: error[a.b]: here";
        assert_eq!(
            parse_diagnostic(
                line,
                &Placement {
                    file: "demo.bynk".to_string(),
                    line_offset: 8,
                    prefix: 0
                }
            )
            .unwrap()
            .line,
            1
        );
        assert!(
            parse_diagnostic(
                line,
                &Placement {
                    file: "demo.bynk".to_string(),
                    line_offset: 12,
                    prefix: 0
                }
            )
            .unwrap()
            .file
            .is_some()
        );
    }

    #[test]
    fn brackets_in_strings_and_comments_do_not_extend_a_clause() {
        let snippet = "stub Fetch.send(_) returns Ok(Response { status: 200, headers: Map.empty(), body: \"(\" })\nconsumes a.b // (old)\nlet x = 1\nx";
        assert_eq!(header_lines(snippet), 2);
    }

    #[test]
    fn a_value_with_no_wire_shape_says_so() {
        let root = project("shapeless");
        match eval(&root, "Effect.pure(1)") {
            EvalOutcome::Failed(message) => {
                assert!(message.contains("no wire shape"), "{message}");
            }
            other => panic!("expected a report of no wire shape, got {other:?}"),
        }
    }

    const STUBBING_SUITE: &str = "\
suite demo {
  consumes bynk { Fetch, Request, Method, Response }
  stub Fetch.send(_) returns Ok(Response { status: 204, headers: Map.empty(), body: \"\" })

  case \"x\" {
    expect double(1) == 2
  }
}
";

    fn with_suite(root: &Path, text: &str) {
        std::fs::create_dir_all(root.join("tests")).unwrap();
        std::fs::write(root.join("tests/demo.bynk"), text).unwrap();
    }

    #[test]
    fn a_suite_that_does_not_compile_does_not_block_an_evaluation() {
        let root = project("broken_suite");
        with_suite(
            &root,
            "suite demo {\n  case \"broken\" {\n    let x: Int = \"no\"\n    expect x == 1\n  }\n}\n",
        );
        // Another unit's suite, left out as well, still builds a module that
        // type-checks beside the evaluation's.
        std::fs::write(
            root.join("src/other.bynk"),
            "commons other\n\nfn one() -> Int {\n  1\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("tests/other.bynk"),
            "suite other {\n  case \"one\" {\n    expect one() == 1\n  }\n}\n",
        )
        .unwrap();
        assert_eq!(value(eval(&root, "double(2)")).0, serde_json::json!(4));
    }

    #[test]
    fn a_named_suite_is_built_in_and_its_stubs_apply() {
        let root = project("named_suite");
        with_suite(&root, STUBBING_SUITE);
        let send = FETCH.to_string() + "res";
        // Left out, the suite's stub is not there to answer.
        assert!(matches!(eval(&root, &send), EvalOutcome::Failed(_)));
        // Named, it is, and the snippet needs no stub of its own.
        let request = |suites: Vec<String>| EvalRequest {
            input: root.to_path_buf(),
            unit: "demo".to_string(),
            snippet: FETCH.to_string() + "res",
            buffers: HashMap::new(),
            suites,
            timeout: Duration::from_secs(120),
        };
        let (v, _) = value(evaluate(&request(vec!["demo".to_string()])));
        assert_eq!(v["value"]["status"], 204);
        // And a named suite that does not compile is reported.
        with_suite(
            &root,
            "suite demo {\n  case \"x\" {\n    let n: Int = \"no\"\n  }\n}\n",
        );
        match evaluate(&request(vec!["demo".to_string()])) {
            EvalOutcome::Diagnostics(d) => {
                assert!(d.iter().any(|d| d.file.is_some()), "{d:?}");
            }
            other => panic!("expected the suite's diagnostics, got {other:?}"),
        }
    }

    #[test]
    fn a_unit_the_parser_only_warns_about_is_found() {
        // Three nested `match`es draw a lint from the parser, not an error.
        let unit = "\
context demo

fn sum(a: Option[Int], b: Option[Int], c: Option[Int]) -> Int {
  match a {
    Some(x) => match b {
      Some(y) => match c {
        Some(z) => x + y + z
        None => 0
      }
      None => 0
    }
    None => 0
  }
}
";
        let root = project_with("lint", unit);
        let (v, _) = value(eval(&root, "sum(Some(1), Some(2), Some(3))"));
        assert_eq!(v, serde_json::json!(6));
    }
}
