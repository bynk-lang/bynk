//! Compiler diagnostics.
//!
//! Every error has a category (a dotted namespace string like
//! `bynk.parse.expected_token`), a primary span, a primary message, and
//! optionally some secondary labels and notes. Rendering goes through
//! [`ariadne`] for source-pointing colour output.

use ariadne::{Color, Config, IndexType, Label, Report, ReportKind};

use crate::span::Span;

/// A compile error.
#[derive(Debug, Clone)]
pub struct CompileError {
    pub category: &'static str,
    pub span: Span,
    pub message: String,
    pub labels: Vec<(Span, String)>,
    pub notes: Vec<String>,
    /// v0.26 (ADR 0054): machine-applicable fixes, authored at the diagnosis
    /// site — the only place the exact spans and replacement are known.
    /// Consumed by the LSP (`codeAction`) and, later, a CLI `--fix`.
    pub suggestions: Vec<Suggestion>,
}

/// Which file one of a [`CompileError`]'s secondary labels lies in, for
/// [`CompileError::report_in`]. A label's span is a byte offset into its own
/// file's text, which need not be the file the error itself is reported in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelHome<'a> {
    /// The file the error is reported in.
    Primary,
    /// Another file: the name to show it under and its text.
    File { filename: &'a str, source: &'a str },
    /// A file with no text to show — the label is rendered as a note.
    Unavailable,
}

/// A structured fix for the error it is attached to (v0.26, ADR 0054).
///
/// `edits` are span → replacement: an empty replacement deletes the span; an
/// empty span inserts at its position. Spans are offsets into the same source
/// text as the error's own span.
#[derive(Debug, Clone)]
pub struct Suggestion {
    /// Human-facing action title, e.g. "remove `Clock` from the `given` clause".
    pub message: String,
    pub edits: Vec<(Span, String)>,
    pub applicability: Applicability,
}

/// Whether a [`Suggestion`] can be applied without review (mirrors rustc;
/// gates a future CLI `--fix` and the LSP's one-click apply).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applicability {
    /// The fix is exactly right — safe to apply mechanically.
    MachineApplicable,
    /// The fix contains placeholder text a human must complete; never
    /// auto-applied.
    HasPlaceholders,
    /// The fix is a complete edit but changes meaning the author must choose
    /// (`derives Ord` makes declaration order significant): offered as a quick
    /// fix, never taken by a fix-all or `--fix`.
    MaybeIncorrect,
}

/// Severity classification for a [`CompileError`]. Mirrors LSP severity levels
/// so the LSP server can map diagnostics to the protocol without reinterpreting
/// error categories. Lives in the syntax leaf beside `CompileError` (it
/// classifies one): shared by the IDE diagnose path (`bynk-ide`) and the
/// `short`/`json` renderers, without either depending on the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    /// Classify a [`CompileError`] by its category's registered severity
    /// (`crate::diagnostics::REGISTRY`) — the registry entry built via `warn`
    /// is the single source of truth for which codes are non-failing
    /// warnings (ADR 0117); everything else defaults to `Error`, including a
    /// category that isn't registered at all (which `tests/diagnostics_registry.rs`
    /// asserts cannot happen for a code actually emitted in source).
    pub fn for_error(err: &CompileError) -> Severity {
        crate::diagnostics::lookup(err.category)
            .map(|d| d.severity)
            .unwrap_or(Severity::Error)
    }
}

/// Split diagnostics into `(errors, warnings)` by severity (ADR 0117). The build
/// fails iff the `errors` half is non-empty; the `warnings` half surfaces but
/// does not gate compilation. Relative order within each half is preserved.
pub fn partition_by_severity(
    diagnostics: Vec<CompileError>,
) -> (Vec<CompileError>, Vec<CompileError>) {
    diagnostics
        .into_iter()
        .partition(|d| Severity::for_error(d) == Severity::Error)
}

impl CompileError {
    pub fn new(category: &'static str, span: Span, message: impl Into<String>) -> Self {
        Self {
            category,
            span,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            suggestions: Vec::new(),
        }
    }

    /// Shift every span in this diagnostic — the primary span, secondary
    /// labels, and suggestion edits — right by `delta` bytes. Used to rebase a
    /// diagnostic produced against a substring (e.g. an interpolation hole
    /// re-lexed on its own) into the full source, so the location is correct
    /// and every span stays a valid char boundary. (#716.)
    pub fn offset_spans(mut self, delta: usize) -> Self {
        self.span = self.span.offset(delta);
        for (span, _) in &mut self.labels {
            *span = span.offset(delta);
        }
        for suggestion in &mut self.suggestions {
            for (span, _) in &mut suggestion.edits {
                *span = span.offset(delta);
            }
        }
        self
    }

    pub fn with_label(mut self, span: Span, label: impl Into<String>) -> Self {
        self.labels.push((span, label.into()));
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Attach a machine-applicable fix (v0.26). Mirrors [`Self::with_note`];
    /// the suggestion is authored where the diagnostic is raised.
    pub fn with_suggestion(
        mut self,
        message: impl Into<String>,
        edits: Vec<(Span, String)>,
        applicability: Applicability,
    ) -> Self {
        self.suggestions.push(Suggestion {
            message: message.into(),
            edits,
            applicability,
        });
        self
    }

    /// Build an [`ariadne::Report`] for this error, rendered against `source`
    /// (labelled `filename`). Colour is on (for the CLI and human-facing test
    /// output). Every secondary label is taken to lie in `source` too; see
    /// [`Self::report_in`] for one whose labels may lie in other files.
    ///
    /// A **secondary** label whose span does not sit cleanly within `source`
    /// belongs to *another* file — a cross-file "declared here" pointing at a
    /// `uses`-imported callee, or (#696) at a sibling file in a multi-file unit.
    /// Rendering it here would underline unrelated text, and a byte span that
    /// lands mid-codepoint would panic ariadne's byte→char mapping (#716). Such a
    /// label is demoted to a note so the information survives without the
    /// misplacement or the panic. The demotion test is deliberately conservative:
    /// out-of-bounds **or** not on a char boundary of `source`. It cannot catch a
    /// cross-file span that happens to be in-bounds and boundary-aligned — a
    /// caller that knows each label's file passes it to [`Self::report_in`].
    pub fn report_for<'a>(
        &'a self,
        filename: &'a str,
        source: &str,
    ) -> Report<'a, (&'a str, std::ops::Range<usize>)> {
        self.report_with_config(filename, source, &[], Config::default())
    }

    /// [`Self::report_for`] with colour disabled, for transcripts committed to
    /// the repo — no ANSI escape codes, so the output is byte-stable across
    /// machines.
    pub fn report_plain_for<'a>(
        &'a self,
        filename: &'a str,
        source: &str,
    ) -> Report<'a, (&'a str, std::ops::Range<usize>)> {
        self.report_with_config(filename, source, &[], Config::default().with_color(false))
    }

    /// [`Self::report_for`] for an error whose secondary labels may lie in other
    /// files: `homes[i]` says which file `self.labels[i]` lies in (a label past
    /// the end of `homes` is [`LabelHome::Primary`]). A label in another file is
    /// drawn against that file's text, under its own filename, so the report's
    /// cache must hold every [`LabelHome::File`] source as well as `source`.
    /// `color` picks [`Self::report_for`]'s or [`Self::report_plain_for`]'s
    /// output.
    pub fn report_in<'a>(
        &'a self,
        filename: &'a str,
        source: &str,
        homes: &[LabelHome<'a>],
        color: bool,
    ) -> Report<'a, (&'a str, std::ops::Range<usize>)> {
        self.report_with_config(filename, source, homes, Config::default().with_color(color))
    }

    /// True when `span` sits cleanly inside `source` — in-bounds and on char
    /// boundaries at both ends — so ariadne can underline it without misplacing
    /// the caret or panicking on a byte offset that splits a codepoint (#716).
    fn label_fits(span: &Span, source: &str) -> bool {
        span.end <= source.len()
            && source.is_char_boundary(span.start)
            && source.is_char_boundary(span.end)
    }

    /// `span`'s byte range in `source` as the char range ariadne indexes by.
    /// An offset inside a codepoint counts from that codepoint's start; one
    /// past the end stays past it by the same amount, so ariadne still sees an
    /// out-of-bounds span as out of bounds.
    fn char_range(source: &str, span: &Span) -> std::ops::Range<usize> {
        let at = |byte: usize| {
            if byte > source.len() {
                source.chars().count() + (byte - source.len())
            } else {
                source[..source.floor_char_boundary(byte)].chars().count()
            }
        };
        at(span.start)..at(span.end)
    }

    fn report_with_config<'a>(
        &'a self,
        filename: &'a str,
        source: &str,
        homes: &[LabelHome<'a>],
        config: Config,
    ) -> Report<'a, (&'a str, std::ops::Range<usize>)> {
        let primary_span = (filename, Self::char_range(source, &self.span));
        // ADR 0117: a warning-severity diagnostic must render as a warning, not
        // an error — `report_with_config` is the only `ReportKind` in the
        // workspace, so this is the one place that decides it.
        let kind = match Severity::for_error(self) {
            Severity::Error => ReportKind::Error,
            Severity::Warning => ReportKind::Warning,
        };
        // Spans are byte offsets into the UTF-8 source, but ariadne is handed
        // char offsets ([`Self::char_range`]), each converted against the text
        // its span lies in. Its `IndexType::Byte` mode is wrong for a label in
        // another file: ariadne 0.6 heads that file's snippet by looking the
        // label's *char* offset up as a byte offset, so the header names the
        // wrong line and column after any non-ASCII text — and panics when
        // that offset splits a codepoint (#1956: `bynk.bynk` is full of `—`).
        let mut builder = Report::build(kind, primary_span.clone())
            .with_config(config.with_index_type(IndexType::Char))
            .with_code(self.category)
            .with_message(&self.message)
            .with_label(
                Label::new(primary_span)
                    .with_message(&self.message)
                    .with_color(Color::Red),
            );

        for (i, (span, label)) in self.labels.iter().enumerate() {
            let (label_file, label_source) = match homes.get(i).unwrap_or(&LabelHome::Primary) {
                LabelHome::Primary => (filename, source),
                LabelHome::File { filename, source } => (*filename, *source),
                LabelHome::Unavailable => {
                    builder = builder.with_note(label);
                    continue;
                }
            };
            if !Self::label_fits(span, label_source) {
                // The label's span does not fit its file's source — demote to a
                // note rather than underlining unrelated text (or panicking on a
                // mid-codepoint offset).
                builder = builder.with_note(label);
                continue;
            }
            builder = builder.with_label(
                Label::new((label_file, Self::char_range(label_source, span)))
                    .with_message(label)
                    .with_color(Color::Yellow),
            );
        }

        for note in &self.notes {
            builder = builder.with_note(note);
        }

        // Finding #49 (ADR 0054): a structured suggestion previously reached
        // only the LSP's code-action surface; the CLI never rendered it at
        // all. Shown as a `help:`-prefixed note — mirrors rustc's own
        // note/help distinction — rather than a new ariadne builder call, so
        // it composes with an arbitrary number of suggestions.
        for suggestion in &self.suggestions {
            builder = builder.with_note(format!("help: {}", suggestion.message));
        }

        builder.finish()
    }
}

#[cfg(test)]
mod warning_channel_tests {
    use super::*;
    use crate::span::Span;

    #[test]
    fn partition_splits_by_severity() {
        let warn = CompileError::new("bynk.given.unused_capability", Span::default(), "unused");
        let err = CompileError::new("bynk.types.argument_mismatch", Span::default(), "bad");
        let (errors, warnings) = partition_by_severity(vec![warn, err]);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].category, "bynk.types.argument_mismatch");
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].category, "bynk.given.unused_capability");
    }

    /// `report_with_config` (the only `ReportKind` in the workspace) hardcoded
    /// `ReportKind::Error`, so a warning-severity diagnostic printed "Error:"
    /// and there was no way for a renderer built on `report_for` to tell them
    /// apart. It must pick the `ReportKind` from `Severity::for_error`.
    #[test]
    fn report_for_renders_warning_severity_as_a_warning_not_an_error() {
        let source = "commons w\n\nfn f() -> Int { 1 }\n";
        let warn = CompileError::new("bynk.given.unused_capability", Span::default(), "unused");
        let rendered = {
            let mut out = Vec::new();
            let mut cache = ("w.bynk", ariadne::Source::from(source));
            warn.report_plain_for("w.bynk", source)
                .write(&mut cache, &mut out)
                .unwrap();
            String::from_utf8(out).unwrap()
        };
        assert!(
            rendered.contains("Warning:"),
            "expected a `Warning:` report for a warning-severity category, got:\n{rendered}"
        );
        assert!(
            !rendered.contains("Error:"),
            "a warning-severity category must not render as `Error:`, got:\n{rendered}"
        );

        let err = CompileError::new("bynk.types.argument_mismatch", Span::default(), "mismatch");
        let rendered_err = {
            let mut out = Vec::new();
            let mut cache = ("w.bynk", ariadne::Source::from(source));
            err.report_plain_for("w.bynk", source)
                .write(&mut cache, &mut out)
                .unwrap();
            String::from_utf8(out).unwrap()
        };
        assert!(
            rendered_err.contains("Error:"),
            "an error-severity category must still render as `Error:`, got:\n{rendered_err}"
        );
    }
}
