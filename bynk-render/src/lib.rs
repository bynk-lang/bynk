//! Bynk's shared diagnostic-rendering layer.
//!
//! The presentation layer over [`bynk_syntax::CompileError`]: ariadne human
//! output and the `short`/`json`-feeding line forms. Every renderer takes
//! `&[CompileError]` + `source` + `filename` — it is agnostic about *where* the
//! errors came from. Both CLI front-ends adopt it so they render identically
//! (ADR 0100).
//!
//! **Invariant (ADR 0100):** this crate depends on `bynk-syntax` **only** (plus
//! `ariadne`). It must never see `AttributedError`/`ProjectFailure` (which live
//! in `bynk-emit`): the `AttributedError → CompileError` flattening stays *above*
//! render, in the front-end, so there is no `render → emit` cycle. A function
//! here taking a `ProjectFailure` would not even compile — the dependency isn't
//! present, by design.
//!
//! Extracted from `bynkc` as slice 6 of the crate-decomposition track.

use std::path::Path;

use ariadne::Source;
use bynk_syntax::error::Severity;
use bynk_syntax::{CompileError, span};

/// Re-exported for [`render_error_in`]'s callers, which say with it which file
/// each of an error's labels lies in.
pub use bynk_syntax::error::LabelHome;

/// Render a list of compile errors to a string (for tests) using the given
/// filename as the diagnostic source label.
pub fn render_errors(errors: &[CompileError], source: &str, filename: &str) -> String {
    String::from_utf8_lossy(&render_all(errors, source, filename, true)).into_owned()
}

/// Render a list of compile errors to a string with colour disabled and the
/// given filename as the source label. Unlike [`render_errors`], the output
/// contains no ANSI escape codes, so it is byte-stable — suitable for the
/// committed diagnostic transcripts under `site/src/diagnostics/`.
pub fn render_errors_plain(errors: &[CompileError], source: &str, filename: &str) -> String {
    String::from_utf8_lossy(&render_all(errors, source, filename, false)).into_owned()
}

/// Render to stderr, used by the CLI. Coloured only when [`stderr_color`]
/// allows it.
pub fn print_errors(errors: &[CompileError], source: &str, filename: &str) {
    use std::io::Write;
    let out = render_all(errors, source, filename, stderr_color());
    let _ = std::io::stderr().lock().write_all(&out);
}

/// The ariadne reports for `errors`. A report whose spans touch a line
/// longer than [`MAX_LINE`] renders its own copy of the source, cut by
/// [`cropped`]; the rest share one view of `source`.
fn render_all(errors: &[CompileError], source: &str, filename: &str, color: bool) -> Vec<u8> {
    fn write<'a>(
        err: &'a CompileError,
        filename: &'a str,
        cache: &mut (&'a str, Source<&str>),
        color: bool,
        out: &mut Vec<u8>,
    ) {
        let source = cache.1.text();
        let report = if color {
            err.report_for(filename, source)
        } else {
            err.report_plain_for(filename, source)
        };
        report
            .write(&mut *cache, out)
            .expect("write to Vec<u8> cannot fail");
    }
    let long = long_lines(source);
    let mut cache = (filename, Source::from(source));
    let mut index = None;
    let mut out = Vec::new();
    for err in errors {
        let touches_long = std::iter::once(&err.span)
            .chain(err.labels.iter().map(|(s, _)| s))
            .filter(|s| fits(s, source))
            .any(|s| {
                long.iter()
                    .any(|&(start, end)| s.start <= end && s.end >= start)
            });
        if !touches_long {
            write(err, filename, &mut cache, color, &mut out);
            continue;
        }
        let (shown, shown_err) = cropped(err, source);
        let mut one = Vec::new();
        write(
            &shown_err,
            filename,
            &mut (filename, Source::from(&*shown)),
            color,
            &mut one,
        );
        // ariadne heads the report with the primary span's `line:col` in the
        // text it was given; on a line cut at its start that column is the cut
        // one. Put back the column in the file, as the short form reports it.
        let cut = span::line_col(&shown, shown_err.span.start);
        let real = index
            .get_or_insert_with(|| span::LineIndex::new(source))
            .line_col(source, err.span.start);
        if cut != real {
            one = fix_header(
                &one,
                &format!("{filename}:{}:{}", cut.0, cut.1),
                &format!("{filename}:{}:{}", real.0, real.1),
            );
        }
        out.extend(one);
    }
    out
}

/// Render one error whose secondary labels may lie in other files, coloured
/// as [`render_errors`] is: `homes[i]` says which file `err.labels[i]` lies in
/// (see [`CompileError::report_in`]). A label in another file is drawn in a
/// snippet of that file, under its own name, rather than against `source`.
pub fn render_error_in(
    err: &CompileError,
    source: &str,
    filename: &str,
    homes: &[LabelHome<'_>],
) -> String {
    String::from_utf8_lossy(&render_in(err, source, filename, homes, true)).into_owned()
}

/// [`render_error_in`] with colour disabled, byte-stable like
/// [`render_errors_plain`].
pub fn render_error_in_plain(
    err: &CompileError,
    source: &str,
    filename: &str,
    homes: &[LabelHome<'_>],
) -> String {
    String::from_utf8_lossy(&render_in(err, source, filename, homes, false)).into_owned()
}

/// [`render_error_in`] to stderr, coloured as [`print_errors`] is.
pub fn print_error_in(err: &CompileError, source: &str, filename: &str, homes: &[LabelHome<'_>]) {
    use std::io::Write;
    let out = render_in(err, source, filename, homes, stderr_color());
    let _ = std::io::stderr().lock().write_all(&out);
}

/// The report behind [`render_error_in`]. With every label in `source` it is
/// [`render_all`]'s. Otherwise the labels in `source` are cut to fit its long
/// lines as [`render_all`] cuts them (#1666), and the others are added after:
/// another file is never cut, so a label on one of its long lines becomes a
/// note instead.
fn render_in(
    err: &CompileError,
    source: &str,
    filename: &str,
    homes: &[LabelHome<'_>],
    color: bool,
) -> Vec<u8> {
    let home = |i: usize| homes.get(i).copied().unwrap_or(LabelHome::Primary);
    if (0..err.labels.len()).all(|i| home(i) == LabelHome::Primary) {
        return render_all(std::slice::from_ref(err), source, filename, color);
    }
    let mut local = err.clone();
    local.labels.clear();
    let mut elsewhere = Vec::new();
    for (i, label) in err.labels.iter().enumerate() {
        match home(i) {
            LabelHome::Primary => local.labels.push(label.clone()),
            other => elsewhere.push((label.clone(), other)),
        }
    }
    let touches_long = |s: &span::Span, text: &str| {
        fits(s, text)
            && long_lines(text)
                .iter()
                .any(|&(start, end)| s.start <= end && s.end >= start)
    };
    let cut = std::iter::once(&local.span)
        .chain(local.labels.iter().map(|(s, _)| s))
        .any(|s| touches_long(s, source));
    let (shown, mut shown_err) = if cut {
        cropped(&local, source)
    } else {
        (source.to_string(), local)
    };
    let mut shown_homes = vec![LabelHome::Primary; shown_err.labels.len()];
    for ((span, label), home) in elsewhere {
        let home = match home {
            LabelHome::File { source: text, .. } if touches_long(&span, text) => {
                LabelHome::Unavailable
            }
            other => other,
        };
        shown_err.labels.push((span, label));
        shown_homes.push(home);
    }
    let mut cache = Files(vec![(filename, Source::from(shown.as_str()))]);
    for home in &shown_homes {
        if let LabelHome::File { filename, source } = *home
            && !cache.0.iter().any(|(f, _)| *f == filename)
        {
            cache.0.push((filename, Source::from(source)));
        }
    }
    let mut out = Vec::new();
    shown_err
        .report_in(filename, &shown, &shown_homes, color)
        .write(&mut cache, &mut out)
        .expect("write to Vec<u8> cannot fail");
    if cut {
        // As in `render_all`: put back the primary span's column in the file.
        let at_cut = span::line_col(&shown, shown_err.span.start);
        let real = span::line_col(source, err.span.start);
        if at_cut != real {
            out = fix_header(
                &out,
                &format!("{filename}:{}:{}", at_cut.0, at_cut.1),
                &format!("{filename}:{}:{}", real.0, real.1),
            );
        }
    }
    out
}

/// A report's sources by filename — the primary file's and each other file a
/// label lies in. ariadne's own multi-source cache wants `'static` ids.
struct Files<'a>(Vec<(&'a str, Source<&'a str>)>);

impl<'a> ariadne::Cache<&'a str> for Files<'a> {
    type Storage = &'a str;

    fn fetch(&mut self, id: &&'a str) -> Result<&Source<&'a str>, impl std::fmt::Debug> {
        self.0
            .iter()
            .find(|(name, _)| name == id)
            .map(|(_, source)| source)
            .ok_or_else(|| format!("no source for `{id}`"))
    }

    fn display<'b>(&self, id: &'b &'a str) -> Option<impl std::fmt::Display + 'b> {
        Some(*id)
    }
}

/// The `[start, end)` byte ranges (line endings excluded) of the lines of
/// `source` longer than [`MAX_LINE`].
fn long_lines(source: &str) -> Vec<(usize, usize)> {
    let mut long = Vec::new();
    let mut start = 0;
    for raw in source.split_inclusive('\n') {
        let len = raw.trim_end_matches(['\n', '\r']).len();
        if len > MAX_LINE {
            long.push((start, start + len));
        }
        start += raw.len();
    }
    long
}

/// `report` with `cut` replaced by `real` on its header line, the first
/// carrying ariadne's `╭`: only there, so a message that happens to quote
/// `cut` is left alone.
fn fix_header(report: &[u8], cut: &str, real: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(report);
    let Some(corner) = text.find('╭') else {
        return report.to_vec();
    };
    let start = text[..corner].rfind('\n').map_or(0, |i| i + 1);
    let end = text[corner..].find('\n').map_or(text.len(), |i| corner + i);
    let header = text[start..end].replacen(cut, real, 1);
    format!("{}{header}{}", &text[..start], &text[end..]).into_bytes()
}

/// #1666: a line longer than this (in bytes) is cut to a window when a report
/// renders it. Rendering cost and output size grow with the line, and a
/// generated or minified line can be a megabyte.
pub const MAX_LINE: usize = 400;

/// Bytes kept either side of the labels on a cut line.
const CONTEXT: usize = 80;

/// One source line as a report sees it after [`cropped`]: where the original
/// line ends, the window `[win_start, win_end)` of it that is kept, and
/// where the kept text begins in the cropped source (after any `…`).
struct LineWindow {
    end: usize,
    win_start: usize,
    win_end: usize,
    new_text_start: usize,
}

/// #1666: `source` and `err` as a report should render them, every line
/// longer than [`MAX_LINE`] cut to a window and marked `…` where text was
/// dropped, and the error's spans (the primary one and each label's) moved
/// into the cut text. A window holds the line's labels with [`CONTEXT`] bytes
/// either side, a label longer than [`MAX_LINE`] keeping only its head; when
/// the labels are too far apart for that, it holds the primary span's
/// neighbourhood, and a label left outside becomes a note. A long line without
/// labels keeps its first [`MAX_LINE`] bytes. Cuts land on char boundaries.
fn cropped(err: &CompileError, source: &str) -> (String, CompileError) {
    let spans: Vec<(usize, usize)> = std::iter::once(&err.span)
        .chain(err.labels.iter().map(|(s, _)| s))
        .filter(|s| fits(s, source))
        .map(|s| (s.start, s.end))
        .collect();
    let mut out = String::with_capacity(source.len().min(64 * 1024));
    let mut lines: Vec<LineWindow> = Vec::new();
    let mut start = 0;
    for raw in source.split_inclusive('\n') {
        let line_len = raw.trim_end_matches(['\n', '\r']).len();
        let end = start + line_len;
        let (win_start, win_end) = if line_len <= MAX_LINE {
            (start, end)
        } else {
            let mut lo = usize::MAX;
            let mut hi = 0;
            for &(s, e) in &spans {
                if e < start || s > end {
                    continue;
                }
                let s = s.max(start);
                let e = e.min(end).min(s + MAX_LINE);
                lo = lo.min(s);
                hi = hi.max(e);
            }
            let (lo, hi) = if lo == usize::MAX {
                (start, start + MAX_LINE)
            } else {
                (
                    lo.saturating_sub(CONTEXT).max(start),
                    (hi + CONTEXT).min(end),
                )
            };
            // Labels far apart on one line could still leave a long window;
            // then keep the primary span's neighbourhood.
            let (lo, hi) = if hi - lo > MAX_LINE + 2 * CONTEXT {
                let s = err.span.start.clamp(start, end);
                (
                    s.saturating_sub(CONTEXT).max(start),
                    (s + MAX_LINE).min(end),
                )
            } else {
                (lo, hi)
            };
            (floor_char(source, lo), ceil_char(source, hi))
        };
        if win_start > start {
            out.push('…');
        }
        let new_text_start = out.len();
        out.push_str(&source[win_start..win_end]);
        if win_end < end {
            out.push('…');
        }
        out.push_str(&raw[line_len..]);
        lines.push(LineWindow {
            end,
            win_start,
            win_end,
            new_text_start,
        });
        start += raw.len();
    }
    let line_of = |offset: usize| lines.get(lines.partition_point(|l| l.end < offset));
    let map = |offset: usize| -> usize {
        let Some(l) = line_of(offset) else {
            return out.len();
        };
        // An offset cut from the line lands on the nearer edge of its window.
        l.new_text_start + offset.clamp(l.win_start, l.win_end) - l.win_start
    };
    let mut err = err.clone();
    err.span.start = map(err.span.start);
    err.span.end = map(err.span.end).max(err.span.start);
    for (span, _) in &mut err.labels {
        // Outside means no byte of the label is kept: it starts at or past a
        // cut end, or ends at or before a cut start (an empty label, before).
        let outside = line_of(span.start).is_some_and(|l| {
            (span.start >= l.win_end && l.win_end < l.end)
                || if span.start == span.end {
                    span.start < l.win_start
                } else {
                    span.end <= l.win_start
                }
        });
        if !fits(span, source) || outside {
            // Another file's label, or one cut from its line: keep it out of
            // range of the cut text too, so the report makes it a note.
            span.start = usize::MAX;
            span.end = usize::MAX;
            continue;
        }
        span.start = map(span.start);
        span.end = map(span.end).max(span.start);
    }
    (out, err)
}

/// Whether `span` lies in `source`, on char boundaries: the test a report
/// applies before underlining a label (otherwise it is a note).
fn fits(span: &span::Span, source: &str) -> bool {
    span.start <= span.end
        && span.end <= source.len()
        && source.is_char_boundary(span.start)
        && source.is_char_boundary(span.end)
}

fn floor_char(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// #1666: whether diagnostics on stderr are coloured: only when stderr is a
/// terminal and `NO_COLOR` is unset or empty (<https://no-color.org>). Colour
/// piped into a file or a CI log is noise, and ariadne colours per character,
/// so it multiplied the output's size by about twenty.
///
/// #1777: `FORCE_COLOR` (<https://force-color.org>) or `CLICOLOR_FORCE`
/// (<https://bixense.com/clicolors/>), set to anything but the empty string or
/// `0`, turns colour on without a terminal, for `less -R` or a CI log that
/// renders ANSI. `0` means "don't force", not "force off": `NO_COLOR` is the
/// off switch, and it wins over both. That is clap's order for `NO_COLOR` and
/// `CLICOLOR_FORCE`; clap doesn't read `FORCE_COLOR`, so only
/// `CLICOLOR_FORCE` also colours clap's own help and errors.
pub fn stderr_color() -> bool {
    use std::io::IsTerminal;
    color_allowed(std::io::stderr().is_terminal(), |name| {
        std::env::var_os(name)
    })
}

/// [`stderr_color`]'s decision, over whether stderr is a terminal and a
/// lookup of the environment.
fn color_allowed(is_terminal: bool, var: impl Fn(&str) -> Option<std::ffi::OsString>) -> bool {
    let set = |name| var(name).is_some_and(|v| !v.is_empty());
    let forced = |name| var(name).is_some_and(|v| !v.is_empty() && v != "0");
    if set("NO_COLOR") {
        false
    } else {
        is_terminal || forced("FORCE_COLOR") || forced("CLICOLOR_FORCE")
    }
}

/// Render project-level errors as plain `[category] message` lines — the
/// fallback for errors with no file attribution. Rich, source-context rendering
/// lives in the front-end's project-failure renderer (v0.24).
pub fn print_project_errors(root: &Path, errors: &[CompileError]) {
    let _ = root;
    for err in errors {
        eprintln!("[{}] {}", err.category, err.message);
        for note in &err.notes {
            eprintln!("  note: {note}");
        }
        // Finding #47: a label's *text* survives even with nowhere to
        // underline it (there is no single file to render against here).
        for (_, label) in &err.labels {
            eprintln!("  label: {label}");
        }
    }
}

/// v0.38 (ADR 0071): one terse line per diagnostic for tooling consumers
/// (`bynkc check --format short`):
/// `path:line:col: <severity>[<category>]: <message>`. Line/column are
/// 1-indexed, computed from the byte span against the source. The VS Code
/// `bynkc` problem-matcher keys off this exact shape — keep it stable.
pub fn print_errors_short(errors: &[CompileError], source: &str, filename: &str) {
    eprint!("{}", render_errors_short(errors, source, filename));
}

/// The string form of [`print_errors_short`] — one `…[category]: message` line
/// per error, each newline-terminated. The renderer behind the CLI's `--format
/// short`, exposed for testing.
///
/// Finding #47 doesn't reach this one: `tests/check_format_short.rs` locks
/// `short` to *exactly* one line per diagnostic (the VS Code problem-matcher's
/// contract), so notes/labels can't grow extra lines here without breaking a
/// real machine consumer — unlike [`print_project_errors`]/[`render_project_errors`],
/// which have no such one-line contract.
pub fn render_errors_short(errors: &[CompileError], source: &str, filename: &str) -> String {
    let mut out = String::new();
    for err in errors {
        out.push_str(&short_line(filename, source, err));
        out.push('\n');
    }
    out
}

/// One terse `path:line:col: severity[category]: message` line for a single
/// error against its source. The front-end's project-failure short renderer
/// flattens an attributed error to `(label, text, error)` and calls this.
pub fn short_line(filename: &str, source: &str, err: &CompileError) -> String {
    let (line, col) = span::line_col(source, err.span.start);
    format!(
        "{filename}:{line}:{col}: {}[{}]: {}",
        severity_word(err),
        err.category,
        err.message
    )
}

/// `"error"` / `"warning"` for an error's [`Severity`].
pub fn severity_word(err: &CompileError) -> &'static str {
    match Severity::for_error(err) {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

/// Render a list of compile errors as plain `[category] message` lines (with
/// notes and labels), for test assertion.
pub fn render_project_errors(errors: &[CompileError]) -> String {
    let mut out = String::new();
    for err in errors {
        out.push_str(&format!("[{}] {}\n", err.category, err.message));
        for note in &err.notes {
            out.push_str(&format!("  note: {note}\n"));
        }
        for (_, label) in &err.labels {
            out.push_str(&format!("  label: {label}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_syntax::span::Span;

    /// Spans are byte offsets; ariadne 0.6 defaults to character indexing.
    /// On a line with non-ASCII text before the span, the char-indexed
    /// underline lands past the target. Pin the byte-indexed placement by
    /// checking the caret column against the target's display column.
    #[test]
    fn underline_is_byte_indexed_on_non_ascii_lines() {
        // `é` is 2 bytes / 1 display column; `bad` starts at byte 11,
        // display column 10.
        let source = "// caféxyz bad\n";
        let start = source.find("bad").unwrap();
        let err = CompileError::new(
            "bynk.test.example",
            Span::new(start, start + 3),
            "bad thing",
        );
        let rendered = render_errors_plain(&[err], source, "probe.bynk");
        let source_line = rendered
            .lines()
            .find(|l| l.contains("caféxyz"))
            .expect("snippet line present");
        let marker_line = rendered
            .lines()
            .find(|l| l.contains('┬'))
            .expect("marker line present");
        let col_of = |line: &str, target: char| line.chars().take_while(|&c| c != target).count();
        // The `┬` sits within the underline under `bad` — same display
        // column as `b`, or one to its right for spans wider than 1.
        let b_col = col_of(source_line, 'b');
        let caret_col = col_of(marker_line, '┬');
        assert!(
            (b_col..b_col + 3).contains(&caret_col),
            "caret at display column {caret_col}, expected within `bad` at {b_col}..{}:\n{rendered}",
            b_col + 3
        );
    }

    /// A label whose span lies past the end of the rendered source belongs to
    /// another file; it must be demoted to a note, not underline unrelated
    /// text (or panic).
    #[test]
    fn out_of_bounds_label_demotes_to_note() {
        let source = "commons demo\n";
        let err = CompileError::new("bynk.test.example", Span::new(0, 7), "problem here")
            .with_label(
                Span::new(5_000, 5_010),
                "parameter declared here (in another file)",
            );
        let rendered = render_errors_plain(&[err], source, "probe.bynk");
        assert!(
            rendered.contains("parameter declared here"),
            "label text survives as a note:\n{rendered}"
        );
    }

    /// A cross-file label whose byte span is *in-bounds* but lands mid-codepoint
    /// (the file it really belongs to has non-ASCII text) must be demoted, not
    /// fed to ariadne — a byte offset splitting a codepoint panics its byte→char
    /// mapping (#716). The rendered source here is all multi-byte, so an odd
    /// offset is never a char boundary.
    #[test]
    fn mid_codepoint_label_demotes_to_note() {
        let source = "café ☕\n"; // `é` and `☕` are multi-byte
        let err = CompileError::new("bynk.test.example", Span::new(0, 3), "problem here")
            .with_label(
                Span::new(4, 5),
                "declared here (mid-codepoint, another file)",
            );
        // Must not panic, and the label survives as a note rather than a caret.
        let rendered = render_errors_plain(&[err], source, "probe.bynk");
        assert!(
            rendered.contains("declared here (mid-codepoint, another file)"),
            "a mid-codepoint label must survive as a note:\n{rendered}"
        );
    }

    /// #1666: colour needs a terminal, and `NO_COLOR` set to anything but
    /// the empty string turns it off. #1777: `FORCE_COLOR` or
    /// `CLICOLOR_FORCE`, set to anything but the empty string or `0`, turns
    /// it on without one; `NO_COLOR` wins over both.
    #[test]
    fn color_needs_a_terminal_or_a_force_and_no_no_color() {
        use std::ffi::OsString;
        let allowed = |is_terminal, vars: &[(&str, &str)]| {
            color_allowed(is_terminal, |name| {
                vars.iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| OsString::from(v))
            })
        };
        assert!(allowed(true, &[]));
        assert!(allowed(true, &[("NO_COLOR", "")]));
        assert!(!allowed(true, &[("NO_COLOR", "1")]));
        assert!(!allowed(false, &[]));
        assert!(!allowed(false, &[("NO_COLOR", "")]));

        for force in ["FORCE_COLOR", "CLICOLOR_FORCE"] {
            assert!(allowed(false, &[(force, "1")]), "{force}=1");
            assert!(allowed(false, &[(force, "true")]), "{force}=true");
            assert!(!allowed(false, &[(force, "")]), "{force}= is unset");
            assert!(!allowed(false, &[(force, "0")]), "{force}=0 doesn't force");
            assert!(allowed(true, &[(force, "0")]), "{force}=0 doesn't forbid");
            assert!(allowed(false, &[(force, "1"), ("NO_COLOR", "")]));
            assert!(!allowed(false, &[(force, "1"), ("NO_COLOR", "1")]));
            assert!(!allowed(true, &[(force, "1"), ("NO_COLOR", "1")]));
        }
    }

    /// #1666: a megabyte line renders as a window around the label, marked
    /// `…` both ends, with the header naming the label's real column.
    #[test]
    fn a_long_line_is_cut_around_the_label() {
        let pad = "x".repeat(500_000);
        let source = format!("commons c\n{pad}bad{pad}\n");
        let start = source.find("bad").unwrap();
        let err = CompileError::new("bynk.test.example", Span::new(start, start + 3), "bad");
        let rendered = render_errors_plain(&[err], &source, "probe.bynk");
        assert!(
            rendered.len() < 4_000,
            "{} bytes:\n{rendered}",
            rendered.len()
        );
        assert!(
            rendered.contains("probe.bynk:2:500001 "),
            "the header keeps the column in the file:\n{rendered}"
        );
        let line = rendered.lines().find(|l| l.contains("xbadx")).unwrap();
        assert!(line.contains('…') && line.ends_with('…'), "{line}");
        let caret = rendered.lines().find(|l| l.contains('┬')).unwrap();
        let col = |l: &str, s: &str| l[..l.find(s).unwrap()].chars().count();
        assert!((col(line, "bad")..col(line, "bad") + 3).contains(&col(caret, "┬")));
        // Coloured, the same report is just as small.
        let err = CompileError::new("bynk.test.example", Span::new(start, start + 3), "bad");
        assert!(render_errors(&[err], &source, "probe.bynk").len() < 8_000);
    }

    /// #1666: a cut lands on a char boundary, and a line no longer than
    /// [`MAX_LINE`] renders as it is.
    #[test]
    fn cuts_respect_chars_and_short_lines_are_kept() {
        let wide = "é".repeat(300); // 600 bytes
        let short = "y".repeat(MAX_LINE);
        let source = format!("commons c\n{short}\n{wide}bad{wide}\n");
        let start = source.find("bad").unwrap();
        let err = CompileError::new("bynk.test.example", Span::new(start, start + 3), "bad");
        let rendered = render_errors_plain(&[err], &source, "probe.bynk");
        assert!(rendered.contains("probe.bynk:3:301 "), "{rendered}");
        let line = rendered.lines().find(|l| l.contains("ébadé")).unwrap();
        assert!(line.contains("…éé") && line.contains("éé…"), "{line}");

        let span = Span::new(10, 11);
        let err = CompileError::new("bynk.test.example", span, "here");
        let rendered = render_errors_plain(&[err], &source, "probe.bynk");
        assert!(rendered.contains(&short), "{rendered}");
    }

    /// #1666: cutting a long line keeps another file's label a note.
    #[test]
    fn a_foreign_label_stays_a_note_on_a_cut_line() {
        let source = format!("commons c\n{}\n", "z".repeat(1_000));
        let err = CompileError::new("bynk.test.example", Span::new(10, 11), "here")
            .with_label(Span::new(5_000, 5_010), "declared in another file");
        let rendered = render_errors_plain(&[err], &source, "probe.bynk");
        assert!(
            rendered.contains("Note: declared in another file"),
            "{rendered}"
        );
    }

    /// #1666: labels too far apart for one window keep the primary span's
    /// neighbourhood; a label cut from it becomes a note rather than a caret
    /// on the `…`.
    #[test]
    fn a_label_cut_from_the_window_becomes_a_note() {
        let source = format!("commons c\nhere{}there\n", "1 ".repeat(100_000));
        let there = source.find("there").unwrap();
        let err = CompileError::new("bynk.test.example", Span::new(10, 14), "primary")
            .with_label(Span::new(there, there + 5), "far label");
        let rendered = render_errors_plain(&[err], &source, "probe.bynk");
        assert!(rendered.len() < 4_000, "{rendered}");
        assert!(rendered.contains("Note: far label"), "{rendered}");
        assert!(rendered.contains("probe.bynk:2:1 "), "{rendered}");
    }

    /// #1666: a CRLF line's `\r` is kept after a cut, outside the window.
    #[test]
    fn a_cut_crlf_line_keeps_its_line_ending() {
        let source = format!("commons c\r\n{}bad\r\n", "w".repeat(1_000));
        let start = source.find("bad").unwrap();
        let (shown, _) = cropped(
            &CompileError::new("bynk.test.example", Span::new(start, start + 3), "bad"),
            &source,
        );
        assert!(shown.starts_with("commons c\r\n…"), "{shown:?}");
        assert!(shown.ends_with("wbad\r\n"), "{shown:?}");
    }

    /// #1666 review: a label touching a window edge from outside keeps no
    /// byte, so it is a note, not a caret on the `…` or on the first kept
    /// character. The far label forces the primary span's window
    /// `[primary - CONTEXT, primary + MAX_LINE)`.
    #[test]
    fn a_label_touching_the_window_edge_from_outside_is_a_note() {
        let source = format!("commons c\n{}\n", "q".repeat(20_000));
        let p = 10 + 1_000;
        let err = CompileError::new("bynk.test.example", Span::new(p, p + 1), "primary")
            .with_label(Span::new(p + MAX_LINE, p + MAX_LINE + 3), "after the cut")
            .with_label(Span::new(p - CONTEXT - 3, p - CONTEXT), "before the cut")
            .with_label(Span::new(p + 10_000, p + 10_003), "far away");
        let rendered = render_errors_plain(&[err], &source, "probe.bynk");
        for (i, label) in ["after the cut", "before the cut", "far away"]
            .iter()
            .enumerate()
        {
            let note = format!("Note {}: {label}", i + 1);
            assert!(rendered.contains(&note), "{rendered}");
        }
    }

    /// #1666 review: errors over one source, one on a cut line and one on a
    /// short line, each get the right header; two long lines are cut
    /// independently.
    #[test]
    fn several_errors_and_long_lines_render_independently() {
        let pad = "x".repeat(5_000);
        let source = format!("commons c\n{pad}one{pad}\nshort\n{pad}two{pad}\n");
        let at = |word: &str| source.find(word).unwrap();
        let err = |word: &str| {
            CompileError::new("bynk.test.example", Span::new(at(word), at(word) + 3), word)
        };
        let errors = [
            err("one"),
            CompileError::new(
                "bynk.test.example",
                Span::new(at("short"), at("short") + 5),
                "s",
            ),
            err("two"),
        ];
        let rendered = render_errors_plain(&errors, &source, "probe.bynk");
        assert!(rendered.len() < 8_000, "{rendered}");
        for header in [
            "probe.bynk:2:5001 ",
            "probe.bynk:3:1 ",
            "probe.bynk:4:5001 ",
        ] {
            assert!(rendered.contains(header), "{header}:\n{rendered}");
        }
        assert!(
            rendered
                .lines()
                .any(|l| l.contains("xonex") && l.ends_with('…'))
        );
        assert!(
            rendered
                .lines()
                .any(|l| l.contains("xtwox") && l.ends_with('…'))
        );
    }

    /// #1666 review: the coloured report's header names the column in the
    /// file too.
    #[test]
    fn the_coloured_header_keeps_the_column_in_the_file() {
        let source = format!("commons c\n{}bad\n", "x".repeat(50_000));
        let start = source.find("bad").unwrap();
        let err = CompileError::new("bynk.test.example", Span::new(start, start + 3), "bad");
        let rendered = render_errors(&[err], &source, "probe.bynk");
        assert!(rendered.contains("probe.bynk:2:50001 "), "{rendered}");
    }

    /// #1666 review: a header fix touches only the header, not a message
    /// that quotes the same `path:line:col`.
    #[test]
    fn the_header_fix_leaves_the_message_alone() {
        let source = format!("commons c\n{}bad\n", "x".repeat(1_000));
        let start = source.find("bad").unwrap();
        // The cut puts `bad` at column 82 of the shown line.
        let err = CompileError::new(
            "bynk.test.example",
            Span::new(start, start + 3),
            "see probe.bynk:2:82",
        );
        let rendered = render_errors_plain(&[err], &source, "probe.bynk");
        assert!(
            rendered.contains("Error: see probe.bynk:2:82"),
            "{rendered}"
        );
        assert!(rendered.contains("[ probe.bynk:2:1001 ]"), "{rendered}");
    }

    /// #1666 review: a rendered report over CRLF source numbers the cut line
    /// as in the file.
    #[test]
    fn a_cut_crlf_report_keeps_line_numbers() {
        let source = format!("commons c\r\n\r\n{}bad\r\nnext\r\n", "w".repeat(1_000));
        let start = source.find("bad").unwrap();
        let err = CompileError::new("bynk.test.example", Span::new(start, start + 3), "bad");
        let rendered = render_errors_plain(&[err], &source, "probe.bynk");
        assert!(rendered.contains("[ probe.bynk:3:1001 ]"), "{rendered}");
        assert!(
            rendered
                .lines()
                .any(|l| l.starts_with(" 3 │ …") && l.contains("wbad")),
            "{rendered}"
        );
    }

    /// A label in another file is drawn in a snippet of that file; one with
    /// no text to show, or on one of that file's long lines (#1666, which cuts
    /// only the primary file), becomes a note.
    #[test]
    fn a_label_in_another_file_renders_against_that_file() {
        let primary = "context app\n\n\nservice api {}\n";
        let sibling = "context app\nservice api {}\n";
        let at = |text: &str| text.find("api").unwrap();
        let err = CompileError::new(
            "bynk.resolve.duplicate_service",
            Span::new(at(primary), at(primary) + 3),
            "already declared",
        )
        .with_label(
            Span::new(at(sibling), at(sibling) + 3),
            "previously declared here",
        )
        .with_label(Span::new(0, 1), "in an injected unit");
        let homes = [
            LabelHome::File {
                filename: "a.bynk",
                source: sibling,
            },
            LabelHome::Unavailable,
        ];
        let rendered = render_error_in_plain(&err, primary, "z.bynk", &homes);
        let (z, a) = rendered.split_once("─[ a.bynk:2:9 ]").expect(&rendered);
        assert!(z.contains("─[ z.bynk:4:9 ]"), "{rendered}");
        assert!(!z.contains("previously declared here"), "{rendered}");
        assert!(a.contains(" 2 │ service api {}"), "{rendered}");
        assert!(a.contains("previously declared here"), "{rendered}");
        assert!(rendered.contains("Note: in an injected unit"), "{rendered}");

        let long = format!("context app\n{} service api {{}}\n", "x".repeat(MAX_LINE));
        let err = CompileError::new("bynk.test", Span::new(0, 7), "msg")
            .with_label(Span::new(at(&long), at(&long) + 3), "on a long line");
        let homes = [LabelHome::File {
            filename: "a.bynk",
            source: &long,
        }];
        let rendered = render_error_in_plain(&err, primary, "z.bynk", &homes);
        assert!(!rendered.contains("a.bynk"), "{rendered}");
        assert!(rendered.contains("Note: on a long line"), "{rendered}");
    }

    /// #1956: a label in another file whose text has non-ASCII before it is
    /// headed by its own line and column. ariadne 0.6's byte-index mode looked
    /// the label's char offset up as a byte offset for that header — naming an
    /// earlier line, or panicking when the offset split a codepoint, as it does
    /// here (`bynk.bynk`, where first-party labels point, is full of `—`).
    #[test]
    fn a_label_in_another_file_after_non_ascii_text_is_headed_by_its_own_position() {
        let primary = "context app\n\n\nservice api {}\n";
        let sibling = "context app\n// ———\nservice api {} // é\n";
        let at = |text: &str| text.find("api").unwrap();
        let err = CompileError::new("bynk.test", Span::new(at(primary), at(primary) + 3), "msg")
            .with_label(
                Span::new(at(sibling), at(sibling) + 3),
                "previously declared here",
            );
        let homes = [LabelHome::File {
            filename: "a.bynk",
            source: sibling,
        }];
        let rendered = render_error_in_plain(&err, primary, "z.bynk", &homes);
        let (z, a) = rendered.split_once("─[ a.bynk:3:9 ]").expect(&rendered);
        assert!(z.contains("─[ z.bynk:4:9 ]"), "{rendered}");
        assert!(a.contains(" 3 │ service api {} // é"), "{rendered}");
        assert!(a.contains("previously declared here"), "{rendered}");
    }

    /// A long primary line is cut (#1666) even when a label lies in another
    /// file: the report heads with the primary span's column in the file, not
    /// in the cut text, and the sibling's snippet keeps its own header.
    #[test]
    fn a_cut_primary_line_keeps_its_column_beside_a_label_in_another_file() {
        let primary = format!(
            "context app\n{} service api {{}}\n",
            "x".repeat(2 * MAX_LINE)
        );
        let sibling = "context app\nservice api {}\n";
        let at = |text: &str| text.find("api").unwrap();
        let err = CompileError::new(
            "bynk.resolve.duplicate_service",
            Span::new(at(&primary), at(&primary) + 3),
            "already declared",
        )
        .with_label(
            Span::new(at(sibling), at(sibling) + 3),
            "previously declared here",
        );
        let homes = [LabelHome::File {
            filename: "a.bynk",
            source: sibling,
        }];
        let rendered = render_error_in_plain(&err, &primary, "z.bynk", &homes);
        let col = at(&primary) - primary.find('\n').unwrap();
        let (z, a) = rendered.split_once("─[ a.bynk:2:9 ]").expect(&rendered);
        assert!(z.contains(&format!("─[ z.bynk:2:{col} ]")), "{rendered}");
        assert!(z.contains('…'), "the long primary line is cut: {rendered}");
        assert!(!z.contains("previously declared here"), "{rendered}");
        assert!(a.contains("previously declared here"), "{rendered}");
    }
}
