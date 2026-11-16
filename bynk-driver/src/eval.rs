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
use std::path::PathBuf;
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

/// What an evaluation produced.
#[derive(Debug, Clone, PartialEq)]
pub enum EvalOutcome {
    /// The snippet ran. `value` is the wire shape of its last expression and
    /// `ty` that expression's type.
    Value { value: Value, ty: String },
    /// The project or the snippet did not compile: the diagnostics, one line
    /// each.
    Diagnostics(Vec<String>),
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

/// The synthetic suite for `snippet`, targeting `unit`: the snippet's own
/// lines, with the suite and case openings added to the first and last header
/// lines and the closers after the last line.
pub fn suite_text(unit: &str, snippet: &str) -> String {
    let header = header_lines(snippet);
    let mut lines: Vec<String> = snippet.lines().map(str::to_string).collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    let open = format!("case \"{EVAL_CASE}\" {{");
    if header == 0 {
        lines[0] = format!("suite {unit} {{ {open} {}", lines[0]);
    } else {
        lines[0] = format!("suite {unit} {{ {}", lines[0]);
        lines[header - 1].push_str(&format!(" {open}"));
    }
    let mut text = lines.join("\n");
    text.push_str("\n} }\n");
    text
}

/// Run `request`: the snippet's value, or why there is none.
pub fn evaluate(request: &EvalRequest) -> EvalOutcome {
    let options = match crate::try_project_options(&request.input) {
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
    // The overlay judges a new file against the project's canonical root, so the
    // suite is keyed by the canonical form of its folder (which exists).
    let synthetic = match (
        synthetic.parent().map(std::path::Path::canonicalize),
        synthetic.file_name(),
    ) {
        (Some(Ok(dir)), Some(name)) => dir.join(name),
        _ => synthetic,
    };
    let mut overlay = buffers;
    overlay.insert(synthetic, suite_text(&request.unit, &request.snippet));
    let args = TestArgs {
        no_run: false,
        format: TestFormat::Json,
        suite: vec![request.unit.clone()],
        case: vec![EVAL_CASE.to_string()],
        run_timeout: Some(request.timeout),
        overlay: Some(overlay),
        ..TestArgs::discovery(request.input.clone())
    };
    let (code, text) = crate::test_stream::capture(|| run_test("bynk", args));
    interpret(code, &text)
}

/// What the `bynk test --format json` document `text` says about the snippet.
fn interpret(code: ExitCode, text: &str) -> EvalOutcome {
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
        Some("compile") => return EvalOutcome::Diagnostics(lines("diagnostics")),
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
    use std::path::Path;

    #[test]
    fn a_snippet_without_a_header_is_all_case_body() {
        assert_eq!(header_lines("let x = 1\nx + 1"), 0);
        assert_eq!(
            suite_text("demo.calc", "double(21)"),
            "suite demo.calc { case \"bynk eval\" { double(21)\n} }\n"
        );
    }

    #[test]
    fn leading_consumes_and_stub_lines_are_the_suites() {
        let snippet =
            "consumes shop.order { Placed }\n// a stub\nstub Clock.now() returns 0\nlet x = 1\nx";
        assert_eq!(header_lines(snippet), 3);
        assert_eq!(
            suite_text("demo.calc", snippet),
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
        let root = std::env::temp_dir().join(format!("bynk_eval_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("bynk.toml"), "[project]\nname = \"p\"\n").unwrap();
        std::fs::write(root.join("src/demo.bynk"), UNIT).unwrap();
        // Not `canonicalize`, which gives a `\\?\` path on Windows that `tsc` refuses.
        Project(std::path::absolute(&root).unwrap())
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
            EvalOutcome::Diagnostics(lines) => {
                assert!(
                    lines
                        .iter()
                        .any(|l| l.contains("bynk.types.let_annotation_mismatch")),
                    "{lines:?}"
                );
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
            timeout: Duration::from_secs(5),
        });
        assert!(matches!(outcome, EvalOutcome::Error(_)), "{outcome:?}");
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
}
