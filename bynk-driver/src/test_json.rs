//! v0.59: the `bynkc test --format json` result model, plus the parser that
//! folds the runner's NDJSON event stream into it.
//!
//! The generated `tests/main.ts` runner emits one JSON event per line when
//! `BYNK_TEST_FORMAT=ndjson` (an **internal** protocol — proposal v0.59,
//! Decision 2); `run_test` captures that stream and renders the single pinned
//! **document** below. The document is built from `#[derive(Serialize)]` structs
//! in **declaration order** — field order *is* the contract (the discipline
//! `bynk/src/report.rs` calls out); we never use `serde_json::json!`, so the
//! `preserve_order` feature some workspace crates enable can't reorder it.
//!
//! There are three terminal states, distinguished by the consumer on the
//! presence/`kind` of `error`:
//! - **normal** — `suites` present, no `error` (may have `failed > 0`);
//! - **compile** — the project never compiled: no `suites`, `error.kind ==
//!   "compile"` carrying the `bynkc` diagnostic lines;
//! - **runtime** — the runner started then died before `run-end`: the observed
//!   `suites` prefix *and* `error.kind == "runtime"` with the captured stderr.

use serde::Serialize;

/// The pinned `bynkc test --format json` document.
#[derive(Debug, PartialEq, Serialize)]
pub struct TestRun {
    pub passed: u32,
    pub failed: u32,
    /// #2138: the cases a remote run skipped (`--skip-unsupported`). Absent
    /// when zero, so every other document's layout is unchanged.
    #[serde(skip_serializing_if = "is_zero")]
    pub skipped: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suites: Option<Vec<Suite>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<TestError>,
    /// #854: the optional coverage block, present only for a `--coverage` run
    /// that produced attributable lines. Last field, so every existing
    /// document's byte layout is unchanged when it is absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<Coverage>,
    /// #2138: where the `system` suites ran — `"remote:<target>"` under
    /// `--against`. Absent for an in-process run. Last field, so every
    /// existing document's layout is unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// The `coverage` block: whole-run totals plus one entry per measured `.bynk`
/// file, keyed by project-relative path. Attributed to `.bynk` source lines, not
/// emitted `.ts`/`.js` (issue #854); generated glue with no source origin is
/// out-of-scope, never counted.
#[derive(Debug, PartialEq, Serialize)]
pub struct Coverage {
    /// Covered executable lines across every measured file.
    pub covered: u32,
    /// Total executable lines across every measured file.
    pub lines: u32,
    /// Whole-run percentage (0–100), rounded.
    pub percent: u32,
    pub files: Vec<FileCoverage>,
}

/// One measured `.bynk` file's line coverage.
#[derive(Debug, PartialEq, Serialize)]
pub struct FileCoverage {
    pub path: String,
    pub covered: u32,
    pub lines: u32,
    pub percent: u32,
    /// Uncovered executable lines, 1-based and ascending.
    pub uncovered: Vec<u32>,
    /// #2104: covered executable lines, 1-based and ascending. With
    /// `uncovered`, every executable line, so an editor can mark both. Named
    /// `covered_lines` because `covered` is already the count. Additive and
    /// last, so a consumer that predates it is unaffected.
    pub covered_lines: Vec<u32>,
}

impl From<&crate::coverage::CoverageReport> for Coverage {
    fn from(r: &crate::coverage::CoverageReport) -> Self {
        Coverage {
            covered: r.total_covered(),
            lines: r.total_lines(),
            percent: r.total_percent(),
            files: r
                .files
                .iter()
                .map(|f| FileCoverage {
                    path: f.path.clone(),
                    covered: f.covered,
                    lines: f.total,
                    percent: crate::coverage::percent(f.covered, f.total),
                    uncovered: f.uncovered.clone(),
                    covered_lines: f.covered_lines.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Suite {
    pub name: String,
    pub kind: String,
    pub cases: Vec<Case>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Case {
    pub name: String,
    /// `"pass"` / `"fail"` from a run, `"skipped"` for a case a remote run
    /// skipped (#2138), or `"discovered"` in a `--no-run`
    /// discovery document (the case was listed, not executed).
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Location {
    pub path: String,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct TestError {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// `bynkc` diagnostic lines (`path:line:col: severity[category]: message`),
    /// for `kind == "compile"`. Empty (and omitted) otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
    /// Captured stderr from a crashed run, for `kind == "runtime"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
}

impl TestRun {
    /// A normal run with no suites (no tests, or `--no-run`).
    pub fn empty() -> Self {
        TestRun {
            passed: 0,
            failed: 0,
            skipped: 0,
            suites: Some(Vec::new()),
            error: None,
            coverage: None,
            transport: None,
        }
    }

    /// Attach a coverage block (`--coverage`, issue #854). A report with no
    /// attributed line is dropped rather than serialising an empty block, so a
    /// coverage run over a project with nothing measurable reads like a normal
    /// run.
    pub fn with_coverage(mut self, report: &crate::coverage::CoverageReport) -> Self {
        if !report.is_empty() {
            self.coverage = Some(Coverage::from(report));
        }
        self
    }

    /// v0.67: a **discovery** document (`--no-run --format json`) — the suites and
    /// cases the compile retained, listed without running. `passed`/`failed` are
    /// 0 and each case carries `outcome: "discovered"` (see [`Case`]), so the
    /// "every case has an outcome" invariant holds and no consumer mistakes a
    /// listed case for a result. Reconciles against a later run document: same
    /// suite `name`/`kind`, same case `name`s.
    pub fn discovered(suites: Vec<Suite>) -> Self {
        TestRun {
            passed: 0,
            failed: 0,
            skipped: 0,
            suites: Some(suites),
            error: None,
            coverage: None,
            transport: None,
        }
    }

    /// The document for a run that could not start or complete outside the
    /// compile step (the runner couldn't be launched, `tsc` rejected the
    /// emitted TS, or the runner died). No suites — use [`ParsedRun::into_document`]
    /// when a partial suite prefix was observed.
    pub fn runtime_error(message: impl Into<String>, stderr: Option<String>) -> Self {
        TestRun {
            passed: 0,
            failed: 0,
            skipped: 0,
            suites: None,
            error: Some(TestError {
                kind: "runtime".to_string(),
                message: Some(message.into()),
                diagnostics: Vec::new(),
                stderr: stderr.filter(|s| !s.trim().is_empty()),
            }),
            coverage: None,
            transport: None,
        }
    }

    /// The document for a project that never compiled.
    pub fn compile_error(diagnostics: Vec<String>) -> Self {
        TestRun {
            passed: 0,
            failed: 0,
            skipped: 0,
            suites: None,
            error: Some(TestError {
                kind: "compile".to_string(),
                message: None,
                diagnostics,
                stderr: None,
            }),
            coverage: None,
            transport: None,
        }
    }

    /// Render to a pretty JSON string (trailing newline). Serde emits struct
    /// fields in declaration order regardless of `preserve_order`.
    /// #2138: fold a remote run into the document — its `transport`, and each
    /// skipped case as an `outcome: "skipped"` case whose `message` is why,
    /// in its suite (a new `integration` suite when none of that name ran).
    pub fn with_remote(mut self, transport: String, skipped: &[(String, String, String)]) -> Self {
        self.transport = Some(transport);
        if skipped.is_empty() {
            return self;
        }
        self.skipped = u32::try_from(skipped.len()).unwrap_or(u32::MAX);
        let suites = self.suites.get_or_insert_with(Vec::new);
        for (suite, case, reason) in skipped {
            let case = Case {
                name: case.clone(),
                outcome: "skipped".to_string(),
                message: Some(reason.clone()),
                location: None,
            };
            match suites.iter_mut().find(|s| &s.name == suite) {
                Some(s) => s.cases.push(case),
                None => suites.push(Suite {
                    name: suite.clone(),
                    kind: "integration".to_string(),
                    cases: vec![case],
                }),
            }
        }
        self
    }

    pub fn render(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("TestRun serialises");
        s.push('\n');
        s
    }
}

/// The outcome of parsing the runner's NDJSON stream: the suites observed, the
/// running tallies, and whether a `run-end` event was seen (a missing `run-end`
/// means the runner died mid-stream — a crashed/incomplete run).
#[derive(Debug, Default, PartialEq)]
pub struct ParsedRun {
    pub passed: u32,
    pub failed: u32,
    pub suites: Vec<Suite>,
    pub complete: bool,
}

impl ParsedRun {
    /// Fold this parsed stream into the final document. `node_ok` is whether the
    /// runner process exited zero; `stderr` is its captured stderr (used only
    /// for a crashed run). A stream with no `run-end` — or a non-zero exit with
    /// no completion — becomes a `runtime` error carrying the observed prefix.
    pub fn into_document(self, stderr: &str) -> TestRun {
        if self.complete {
            TestRun {
                passed: self.passed,
                failed: self.failed,
                skipped: 0,
                suites: Some(self.suites),
                error: None,
                coverage: None,
                transport: None,
            }
        } else {
            let trimmed = stderr.trim();
            TestRun {
                passed: self.passed,
                failed: self.failed,
                skipped: 0,
                suites: Some(self.suites),
                error: Some(TestError {
                    kind: "runtime".to_string(),
                    message: Some("the test runner exited before completing".to_string()),
                    diagnostics: Vec::new(),
                    stderr: (!trimmed.is_empty()).then(|| trimmed.to_string()),
                }),
                coverage: None,
                transport: None,
            }
        }
    }
}

/// Parse the runner's NDJSON stdout into a [`ParsedRun`]. Unparseable or
/// unrecognised lines are skipped (the stream is an internal protocol; a
/// stray line should never abort the whole report).
pub fn parse_ndjson(stdout: &str) -> ParsedRun {
    let mut run = ParsedRun::default();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match value.get("type").and_then(|t| t.as_str()) {
            Some("suite-begin") => {
                run.suites.push(Suite {
                    name: str_field(&value, "name"),
                    kind: str_field(&value, "kind"),
                    cases: Vec::new(),
                });
            }
            Some("case") => {
                let outcome = str_field(&value, "outcome");
                if outcome == "pass" {
                    run.passed += 1;
                } else {
                    run.failed += 1;
                }
                let message = value
                    .get("message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string);
                let location = value
                    .get("location")
                    .and_then(|l| l.as_str())
                    .and_then(parse_location);
                let case = Case {
                    name: str_field(&value, "name"),
                    outcome,
                    message,
                    location,
                };
                if let Some(suite) = run.suites.last_mut() {
                    suite.cases.push(case);
                }
            }
            Some("run-end") => {
                run.complete = true;
            }
            _ => {}
        }
    }
    run
}

fn str_field(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Split a `path:line:col` location string into structured fields. Returns
/// `None` for anything that isn't that shape (e.g. the `"unknown"` fallback a
/// non-assertion throw carries), so such a failure keeps its message but offers
/// no click-through. Splits from the right, so a path containing `:` is safe.
fn parse_location(s: &str) -> Option<Location> {
    let (rest, col) = s.rsplit_once(':')?;
    let (path, line) = rest.rsplit_once(':')?;
    let line: u32 = line.parse().ok()?;
    let col: u32 = col.parse().ok()?;
    if path.is_empty() {
        return None;
    }
    Some(Location {
        path: path.to_string(),
        line,
        col,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #2104: a file's coverage carries both line lists, `covered_lines` last,
    /// so the block's existing field order (the contract) is unchanged.
    #[test]
    fn file_coverage_carries_covered_and_uncovered_lines() {
        let report = crate::coverage::CoverageReport {
            files: vec![crate::coverage::FileCoverage {
                path: "src/demo/m.bynk".into(),
                covered: 2,
                total: 5,
                uncovered: vec![7, 8, 9],
                covered_lines: vec![3, 4],
            }],
        };
        assert_eq!(
            serde_json::to_string(&Coverage::from(&report)).unwrap(),
            r#"{"covered":2,"lines":5,"percent":40,"files":[{"path":"src/demo/m.bynk","covered":2,"lines":5,"percent":40,"uncovered":[7,8,9],"covered_lines":[3,4]}]}"#
        );
    }
}
