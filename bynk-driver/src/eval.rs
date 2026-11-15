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

/// The `--format short` line `line` as a diagnostic. A line with no location
/// (`error[code]: message`) has line and column 0.
fn parse_diagnostic(line: &str, synthetic: &str, prefix: usize) -> Option<EvalDiagnostic> {
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
        if Path::new(path).file_name().and_then(|n| n.to_str()) == Some(synthetic) {
            // The snippet's own line; the first carries the generated opening.
            if line_no == 1 {
                col = col.saturating_sub(prefix as u32).max(1);
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

/// The key, in `sources`, of the file that declares the unit `unit`.
fn unit_file(sources: &HashMap<PathBuf, String>, unit: &str) -> Option<PathBuf> {
    let mut keys: Vec<&PathBuf> = sources.keys().collect();
    keys.sort();
    keys.into_iter()
        .find(|key| {
            let source = &sources[*key];
            let Ok(tokens) = tokenize(source) else {
                return false;
            };
            let (parsed, errors) = parse_unit_with_recovery(&tokens, source);
            errors.is_empty()
                && match parsed {
                    Some(SourceUnit::Commons(c)) => c.name.joined() == unit,
                    Some(SourceUnit::Context(c)) => c.name.joined() == unit,
                    _ => false,
                }
        })
        .cloned()
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
            for c in trimmed.chars() {
                match c {
                    '{' | '(' | '[' => depth += 1,
                    '}' | ')' | ']' => depth -= 1,
                    _ => {}
                }
            }
            if depth <= 0 {
                depth = 0;
                end = i + 1;
            }
        }
    }
    end
}

/// The synthetic suite for `snippet`, targeting `unit`: the snippet's own
/// lines, with the suite and case openings added to the first and last header
/// lines and the closers after the last line. Also how many characters the
/// generated opening adds to the first line.
pub fn suite_text(unit: &str, snippet: &str) -> (String, usize) {
    let header = header_lines(snippet);
    let mut lines: Vec<String> = snippet.lines().map(str::to_string).collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    let open = format!("case \"{EVAL_CASE}\" {{");
    let prefix = if header == 0 {
        format!("suite {unit} {{ {open} ")
    } else {
        lines[header - 1].push_str(&format!(" {open}"));
        format!("suite {unit} {{ ")
    };
    lines[0] = format!("{prefix}{}", lines[0]);
    let mut text = lines.join("\n");
    text.push_str("\n} }\n");
    (text, prefix.len())
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
    let mut sources = options.sources.clone().unwrap_or_default();
    sources.extend(request.buffers.clone());
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
    let mut overlay = request.buffers.clone();
    let (text, prefix) = suite_text(&request.unit, &request.snippet);
    let synthetic_name = synthetic
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    overlay.insert(synthetic, text);
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
    interpret(code, &text, &synthetic_name, prefix)
}

/// What the `bynk test --format json` document `text` says about the snippet.
fn interpret(code: ExitCode, text: &str, synthetic: &str, prefix: usize) -> EvalOutcome {
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
                    .filter_map(|l| parse_diagnostic(l, synthetic, prefix))
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
        return EvalOutcome::Value {
            value: eval["value"].clone(),
            ty: eval["type"].as_str().unwrap_or_default().to_string(),
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

    fn project(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("bynk_eval_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("bynk.toml"), "[project]\nname = \"p\"\n").unwrap();
        std::fs::write(root.join("src/demo.bynk"), UNIT).unwrap();
        root.canonicalize().unwrap()
    }

    fn eval_with(root: &Path, snippet: &str, timeout: Duration) -> EvalOutcome {
        evaluate(&EvalRequest {
            input: root.to_path_buf(),
            unit: "demo".to_string(),
            snippet: snippet.to_string(),
            buffers: HashMap::new(),
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
            "{}stub Fetch.send(_) returns Ok(Response {{ status: 204, headers: Map.empty(), body: \"\" }})\n{}",
            "", FETCH
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
            input: root.clone(),
            unit: "demo".to_string(),
            snippet: "double(2)".to_string(),
            buffers: HashMap::from([(file, edited)]),
            timeout: Duration::from_secs(120),
        });
        assert_eq!(value(outcome).0, serde_json::json!(6));
    }

    #[test]
    fn an_unknown_unit_is_an_error() {
        let root = project("unknown");
        let outcome = evaluate(&EvalRequest {
            input: root,
            unit: "nope".to_string(),
            snippet: "1".to_string(),
            buffers: HashMap::new(),
            timeout: Duration::from_secs(5),
        });
        assert!(matches!(outcome, EvalOutcome::Error(_)), "{outcome:?}");
    }

    #[test]
    fn a_short_line_is_a_diagnostic() {
        let d = parse_diagnostic(
            "/p/src/demo.eval.bynk:2:5: error[bynk.resolve.unknown_name]: unknown name `x`",
            "demo.eval.bynk",
            10,
        )
        .unwrap();
        assert_eq!((d.file, d.line, d.col), (None, 2, 5));
        let d = parse_diagnostic(
            "src/demo.bynk:7:1: warning[a.b]: odd: really",
            "demo.eval.bynk",
            10,
        )
        .unwrap();
        assert_eq!(d.file.as_deref(), Some("src/demo.bynk"));
        assert_eq!(d.message, "odd: really");
        let d = parse_diagnostic("error[a.b]: no place", "demo.eval.bynk", 10).unwrap();
        assert_eq!((d.line, d.col), (0, 0));
    }
}
