//! #4123: `bynk check --format json` and `--format ndjson`, the pinned
//! machine-readable diagnostics surface (version 1).
//!
//! `json` is one document, `{ "version", "diagnostics", "summary" }`; `ndjson`
//! is one diagnostic per line and a final summary line, for a consumer that
//! streams. Both go to **stdout**; stderr keeps only what is not a diagnostic
//! (a manifest that cannot be read, an input that does not exist), and the exit
//! status is the one `rich` and `short` give.
//!
//! Like [`crate::test_json`], every shape is a `#[derive(Serialize)]` struct
//! written in **declaration order**: field order *is* the contract, and nothing
//! here uses `serde_json::json!`, so a `preserve_order` feature enabled
//! elsewhere in a build cannot reorder it.
//!
//! ## Positions
//!
//! A [`Pos`] is 1-based `line` and `column`, plus the 0-based `byteOffset` into
//! the file. The column counts UTF-16 code units (the unit the Language Server
//! Protocol and JavaScript string indices use, so an astral character is two),
//! not bytes and not characters; `byteOffset` is the exact byte position for a
//! consumer that slices the file's bytes.
//!
//! ## Which diagnostics
//!
//! Everything `check` found, hints included (`severity` says which): a terminal
//! hides hints, a machine surface does not (#2838). Diagnostics come in file
//! order (by the path as shown) then position order; one with no file, a
//! project-level problem, sorts last and carries `null` for `file`, `start` and
//! `end`.

use bynk_syntax::error::{Applicability, CompileError, Severity};
use bynk_syntax::span::{LineIndex, Span};
use serde::Serialize;

/// The schema version of both documents. A change that removes or reshapes a
/// field bumps it; an added field does not.
pub const VERSION: u32 = 1;

/// A position in a file: see the module docs for the units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pos {
    pub line: u32,
    pub column: u32,
    #[serde(rename = "byteOffset")]
    pub byte_offset: u32,
}

/// One file's text and line table, which turns a byte span into [`Pos`]es.
pub struct Home<'a> {
    file: &'a str,
    source: &'a str,
    index: LineIndex,
}

impl<'a> Home<'a> {
    /// `file` is the path as shown to the user; `source` is its text.
    pub fn new(file: &'a str, source: &'a str) -> Self {
        Self {
            file,
            source,
            index: LineIndex::new(source),
        }
    }

    fn pos(&self, offset: usize) -> Pos {
        let offset = offset.min(self.source.len());
        let (line, column) = self.index.utf16_line_col(self.source, offset);
        Pos {
            line: line + 1,
            column: column + 1,
            byte_offset: offset as u32,
        }
    }

    fn range(&self, span: Span) -> (Pos, Pos) {
        (self.pos(span.start), self.pos(span.end))
    }
}

/// A secondary label: a span of the code with a short message. `file` is absent
/// (with the positions) for a label into a file whose text is not available. A
/// label into a built-in unit names it by its bare identity path (`bynk.bynk`),
/// which is not a file on disk.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Label {
    pub file: Option<String>,
    pub start: Option<Pos>,
    pub end: Option<Pos>,
    pub message: String,
}

/// One replacement of a [`Fix`]: the span `start..end` of `file` becomes
/// `replacement` (empty deletes it; an empty span inserts).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Edit {
    pub file: String,
    pub start: Pos,
    pub end: Pos,
    pub replacement: String,
}

/// A suggested change. `applicability` is the suggestion's own: `"machine"` for
/// a fix that is exactly right in itself, or `"suggested"` for one a person must
/// read first, because it carries placeholder text or changes what the program
/// means. It is not a promise that `check --fix` applies the fix: `--fix` also
/// skips a hint unless its code is named, and leaves alone a diagnostic that
/// offers more than one candidate fix, where two alternatives would conflict. A
/// consumer that applies fixes should apply one per diagnostic.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Fix {
    pub message: String,
    pub applicability: &'static str,
    pub edits: Vec<Edit>,
}

/// One diagnostic.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Diagnostic {
    /// The stable diagnostic code, e.g. `bynk.types.type_mismatch`.
    pub code: String,
    /// `"error"`, `"warning"` or `"hint"`.
    pub severity: &'static str,
    pub file: Option<String>,
    pub start: Option<Pos>,
    pub end: Option<Pos>,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
    pub fixes: Vec<Fix>,
}

/// How many diagnostics of each severity a run found. `hints` is counted so the
/// three sum to the length of `diagnostics`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub errors: u32,
    pub warnings: u32,
    pub hints: u32,
}

/// The `--format json` document.
#[derive(Debug, PartialEq, Serialize)]
pub struct Document {
    pub version: u32,
    pub diagnostics: Vec<Diagnostic>,
    pub summary: Summary,
}

/// One `--format ndjson` diagnostic line: the diagnostic with `type` and
/// `version` ahead of it, so a line is meaningful without its neighbours.
#[derive(Serialize)]
struct DiagnosticLine<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    version: u32,
    #[serde(flatten)]
    diagnostic: &'a Diagnostic,
}

/// The `--format ndjson` summary line, always last.
#[derive(Serialize)]
struct SummaryLine {
    #[serde(rename = "type")]
    kind: &'static str,
    version: u32,
    #[serde(flatten)]
    summary: Summary,
}

fn severity_word(err: &CompileError) -> &'static str {
    match Severity::for_error(err) {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Hint => "hint",
    }
}

fn applicability_word(a: Applicability) -> &'static str {
    match a {
        Applicability::MachineApplicable => "machine",
        Applicability::HasPlaceholders | Applicability::MaybeIncorrect => "suggested",
    }
}

/// `err` as a [`Diagnostic`]. `primary` is the file it is reported in (`None`
/// for a project-level problem with no file); `labels` is the home of each
/// secondary label, index for index (`None` where its file's text is not
/// available). A suggestion's edits are offsets into the primary file, so a
/// diagnostic with no primary file carries no fixes.
pub fn diagnostic(
    err: &CompileError,
    primary: Option<&Home<'_>>,
    labels: &[Option<&Home<'_>>],
) -> Diagnostic {
    let (start, end) = match primary {
        Some(home) => {
            let (s, e) = home.range(err.span);
            (Some(s), Some(e))
        }
        None => (None, None),
    };
    let labels = err
        .labels
        .iter()
        .enumerate()
        .map(|(i, (span, message))| {
            let home = labels.get(i).copied().flatten();
            let range = home.map(|h| h.range(*span));
            Label {
                file: home.map(|h| h.file.to_string()),
                start: range.as_ref().map(|(s, _)| s.clone()),
                end: range.map(|(_, e)| e),
                message: message.clone(),
            }
        })
        .collect();
    let fixes = match primary {
        Some(home) => err
            .suggestions
            .iter()
            .map(|s| Fix {
                message: s.message.clone(),
                applicability: applicability_word(s.applicability),
                edits: s
                    .edits
                    .iter()
                    .map(|(span, replacement)| {
                        let (start, end) = home.range(*span);
                        Edit {
                            file: home.file.to_string(),
                            start,
                            end,
                            replacement: replacement.clone(),
                        }
                    })
                    .collect(),
            })
            .collect(),
        None => Vec::new(),
    };
    Diagnostic {
        code: err.category.to_string(),
        severity: severity_word(err),
        file: primary.map(|h| h.file.to_string()),
        start,
        end,
        message: err.message.clone(),
        labels,
        notes: err.notes.clone(),
        fixes,
    }
}

/// What a machine-format run gathers while it checks: the diagnostics, and
/// whether every input was actually checked. A run that could not start on an
/// input (a manifest that does not read, a file that does not exist) says why on
/// stderr and writes no document, so a consumer never mistakes "nothing was
/// checked" for "nothing was found".
#[derive(Debug, Default)]
pub struct Collector {
    pub diagnostics: Vec<Diagnostic>,
    /// Set when an input could not be checked at all.
    pub incomplete: bool,
}

/// The diagnostics of one single-file check: every one is in `filename`, and
/// so is every label.
pub fn collect_source(
    errors: &[CompileError],
    filename: &str,
    source: &str,
    out: &mut Vec<Diagnostic>,
) {
    let home = Home::new(filename, source);
    for err in errors {
        let labels = vec![Some(&home); err.labels.len()];
        out.push(diagnostic(err, Some(&home), &labels));
    }
}

/// Put `diagnostics` in file order, then position order. A diagnostic with no
/// file sorts last. The sort is stable, so the compiler's own order breaks a
/// tie at one position.
pub fn sorted(mut diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    diagnostics.sort_by(|a, b| {
        let key = |d: &Diagnostic| {
            (
                d.file.is_none(),
                d.file.clone(),
                d.start.as_ref().map(|p| p.byte_offset),
            )
        };
        key(a).cmp(&key(b))
    });
    diagnostics
}

/// The severity counts of `diagnostics`.
pub fn summarise(diagnostics: &[Diagnostic]) -> Summary {
    let count = |word: &str| diagnostics.iter().filter(|d| d.severity == word).count() as u32;
    Summary {
        errors: count("error"),
        warnings: count("warning"),
        hints: count("hint"),
    }
}

/// The `--format json` document for `diagnostics`, with a trailing newline.
pub fn render_json(diagnostics: Vec<Diagnostic>) -> String {
    let diagnostics = sorted(diagnostics);
    let summary = summarise(&diagnostics);
    let document = Document {
        version: VERSION,
        diagnostics,
        summary,
    };
    let mut text = serde_json::to_string_pretty(&document).expect("a Document serialises");
    text.push('\n');
    text
}

/// The `--format ndjson` stream for `diagnostics`: one line each, then the
/// summary line.
pub fn render_ndjson(diagnostics: Vec<Diagnostic>) -> String {
    let diagnostics = sorted(diagnostics);
    let mut text = String::new();
    for diagnostic in &diagnostics {
        let line = DiagnosticLine {
            kind: "diagnostic",
            version: VERSION,
            diagnostic,
        };
        text.push_str(&serde_json::to_string(&line).expect("a diagnostic serialises"));
        text.push('\n');
    }
    let summary = SummaryLine {
        kind: "summary",
        version: VERSION,
        summary: summarise(&diagnostics),
    };
    text.push_str(&serde_json::to_string(&summary).expect("a summary serialises"));
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(start: usize, end: usize, message: &str) -> CompileError {
        CompileError::new("bynk.types.type_mismatch", Span::new(start, end), message)
    }

    #[test]
    fn a_column_counts_utf16_units_and_the_byte_offset_counts_bytes() {
        // "é" is 2 bytes and 1 unit; "🦀" is 4 bytes and 2 units.
        let source = "é🦀x\nlast";
        let home = Home::new("a.bynk", source);
        let x = source.find('x').unwrap();
        let pos = home.pos(x);
        assert_eq!((pos.line, pos.column, pos.byte_offset), (1, 4, 6));
        let last = home.pos(source.find("last").unwrap());
        assert_eq!((last.line, last.column, last.byte_offset), (2, 1, 8));
    }

    #[test]
    fn diagnostics_sort_by_file_then_position_and_a_fileless_one_is_last() {
        let source = "aaaa bbbb";
        let b = Home::new("b.bynk", source);
        let a = Home::new("a.bynk", source);
        let mut out = vec![
            diagnostic(&err(5, 9, "second in b"), Some(&b), &[]),
            diagnostic(&err(0, 1, "no file"), None, &[]),
            diagnostic(&err(0, 4, "first in b"), Some(&b), &[]),
            diagnostic(&err(0, 1, "in a"), Some(&a), &[]),
        ];
        out = sorted(out);
        let order: Vec<&str> = out.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(order, ["in a", "first in b", "second in b", "no file"]);
    }

    #[test]
    fn the_summary_counts_each_severity() {
        let mut out = Vec::new();
        collect_source(&[err(0, 1, "m")], "a.bynk", "x", &mut out);
        assert_eq!(
            summarise(&out),
            Summary {
                errors: 1,
                warnings: 0,
                hints: 0
            }
        );
    }

    #[test]
    fn a_line_of_ndjson_names_its_type_and_version_first() {
        let source = "x";
        let mut out = Vec::new();
        collect_source(&[err(0, 1, "m")], "a.bynk", source, &mut out);
        let text = render_ndjson(out);
        let mut lines = text.lines();
        let first = lines.next().unwrap();
        assert!(
            first.starts_with(
                r#"{"type":"diagnostic","version":1,"code":"bynk.types.type_mismatch""#
            ),
            "{first}"
        );
        assert_eq!(
            lines.next().unwrap(),
            r#"{"type":"summary","version":1,"errors":1,"warnings":0,"hints":0}"#
        );
        assert!(lines.next().is_none());
    }
}
