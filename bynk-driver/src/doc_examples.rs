//! Doc-comment examples (#2461): a fenced `bynk` block in a doc comment is
//! checked by `bynk test`, so an example cannot silently fall out of step with
//! the code it documents. One marked `bynk test` also runs.
//!
//! Each example of statements becomes a `case` in a synthetic `suite` targeting
//! the unit that declares the doc comment, so it sees the unit's scope and the
//! ordinary test semantics. An example of declarations becomes a synthetic
//! sibling `commons` that `uses` the declaring unit. A context cannot be
//! `uses`d, so its declaration fence is checked in a synthetic copy of the
//! context instead: the fence's items are added to the copy, and one that
//! redeclares a name the context already has replaces that declaration, so a
//! doc showing `type Order = …` on `type Order` is no collision. Only the
//! diagnostics inside the fence count. Each such fence is checked in its own
//! project check (a *layer* holds the n-th such fence of every context), as
//! two fences cannot share one copy of a context. The synthetic files are
//! built in memory and added to the project's source overlay. A first project *check* reports every
//! diagnostic at the fence in the real file. A project that has `bynk test`
//! examples is then compiled again with those cases added, so they are emitted
//! and run like any other case.
//!
//! **Line-preserving.** A synthetic file has exactly as many lines as the real
//! file, and every line of an example's body sits on the line it has in the
//! real file, at the same column. The generated `suite`/`case` header is on the
//! opening fence's line. So a diagnostic or a failing `expect` names the right
//! line and column in the real file, and only the file name needs mapping back.
//!
//! **Fence info strings.** The first word is `bynk`; further words are
//! separated by spaces or commas (`bynk ignore` and `bynk,ignore` are the same
//! fence, matching the Book's own examples). `ignore`, `fragment` and `fail`
//! opt a fence out; `test` makes it a case that is run; any other fence (`ts`,
//! `text`, …) is not Bynk and is not looked at.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use bynk_emit::project::AttributedError;
use bynk_syntax::ast::{CommonsForm, Context, SourceUnit};
use bynk_syntax::lexer::{TokenKind, doc_block_body_range, tokenize};
use bynk_syntax::parser::parse_unit_with_recovery;

/// The tail of a synthetic suite's name: `<stem>.doc-examples.bynk` beside the
/// real `<stem>.bynk`.
const SYNTHETIC_SUFFIX: &str = ".doc-examples.bynk";

/// The middle of the name of a synthetic sibling unit checking a fence of
/// declarations: `<stem>_doc_example_<n>.bynk` beside `<stem>.bynk`.
const ITEMS_INFIX: &str = "_doc_example_";

/// How a fence's info string classifies it.
#[derive(Debug, PartialEq, Eq)]
enum Fence {
    /// Not a Bynk fence (`ts`, `text`, no language, …): not looked at.
    Foreign,
    /// A Bynk fence that opts out of checking (`ignore`, `fragment`, `fail`).
    Skipped,
    /// A Bynk fence to type-check.
    Checked,
    /// A Bynk fence to type-check and run (`bynk test`).
    Run,
}

fn classify(info: &str) -> Fence {
    let mut words = info
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|w| !w.is_empty());
    if words.next() != Some("bynk") {
        return Fence::Foreign;
    }
    let words: Vec<&str> = words.collect();
    if words
        .iter()
        .any(|w| matches!(*w, "ignore" | "fragment" | "fail") || w.starts_with("fail="))
    {
        Fence::Skipped
    } else if words.contains(&"test") {
        Fence::Run
    } else {
        Fence::Checked
    }
}

/// One line of a doc block and the byte offset in the real source where it starts.
struct DocLine<'a> {
    text: &'a str,
    start: usize,
}

/// The lines of every `---` doc block. A one-line doc (`--- text`) cannot hold
/// a fence, which needs at least two lines, so only blocks are looked at.
fn doc_blocks(source: &str) -> Vec<Vec<DocLine<'_>>> {
    let Ok(tokens) = tokenize(source) else {
        return Vec::new();
    };
    let mut blocks = Vec::new();
    for t in &tokens {
        if t.kind != TokenKind::DocBlock {
            continue;
        }
        let Some(range) = doc_block_body_range(source, t.span) else {
            continue;
        };
        let mut lines = Vec::new();
        let mut at = range.start;
        for raw in source[range.clone()].split('\n') {
            lines.push(DocLine {
                text: raw.trim_end_matches('\r'),
                start: at,
            });
            at += raw.len() + 1;
        }
        blocks.push(lines);
    }
    blocks
}

/// A fenced example to check.
struct Example<'a> {
    /// Marked `bynk test`: also run.
    run: bool,
    /// The 0-based line of the opening fence in the real source.
    open: usize,
    body: Vec<DocLine<'a>>,
}

impl Example<'_> {
    /// The 0-based line of the closing fence.
    fn close(&self) -> usize {
        self.open + 1 + self.body.len()
    }

    fn text(&self) -> String {
        self.body
            .iter()
            .map(|l| l.text)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A fence's opener: how many backticks, the info string after them.
fn fence_marker(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let ticks = trimmed.bytes().take_while(|&b| b == b'`').count();
    (ticks >= 3).then(|| (ticks, &trimmed[ticks..]))
}

fn examples(source: &str) -> Vec<Example<'_>> {
    let line_of = |offset: usize| source[..offset].matches('\n').count();
    let mut out = Vec::new();
    for block in doc_blocks(source) {
        // The open fence: its backtick count, how it classifies, its line and
        // its body so far.
        let mut open: Option<(usize, Fence, usize, Vec<DocLine<'_>>)> = None;
        for line in block {
            match (fence_marker(line.text), open.as_mut()) {
                (None, Some((_, _, _, body))) => body.push(line),
                (None, None) => {}
                (Some((ticks, info)), None) => {
                    open = Some((ticks, classify(info), line_of(line.start), Vec::new()));
                }
                // Only a bare run of at least as many backticks closes a
                // fence (CommonMark), so a fence shown inside a longer one
                // stays part of its body.
                (Some((ticks, info)), Some((n, ..))) if ticks >= *n && info.trim().is_empty() => {
                    let (_, kind, open_line, body) = open.take().expect("matched Some");
                    if matches!(kind, Fence::Checked | Fence::Run) {
                        out.push(Example {
                            run: kind == Fence::Run,
                            open: open_line,
                            body,
                        });
                    }
                }
                (Some(_), Some((_, _, _, body))) => body.push(line),
            }
        }
        // A fence the doc ends without closing runs to the end of the doc, as
        // Markdown renders it, so a lost closer doesn't quietly stop its
        // example being checked.
        if let Some((_, kind @ (Fence::Checked | Fence::Run), open_line, body)) = open {
            out.push(Example {
                run: kind == Fence::Run,
                open: open_line,
                body,
            });
        }
    }
    out
}

/// What a fence's body is, found by parsing it.
#[derive(Debug, PartialEq, Eq)]
enum Body {
    /// Statements and expressions: the body of a `case`.
    Statements,
    /// Top-level declarations (`type`, `fn`, `capability`, …).
    Items,
}

fn parses_clean(text: &str, wanted: impl Fn(&SourceUnit) -> bool) -> bool {
    let Ok(tokens) = tokenize(text) else {
        return false;
    };
    let (unit, errors) = parse_unit_with_recovery(&tokens, text);
    errors.is_empty() && unit.as_ref().is_some_and(wanted)
}

/// Statements first (the common case: an `expect`, a `let`), then
/// declarations. A body that is neither stays `Statements`, so the checker's
/// own parse error is reported at the fence.
fn classify_body(target: &str, body: &str) -> Body {
    let case = format!("suite {target} {{\n  case \"x\" {{\n{body}\n  }}\n}}\n");
    if parses_clean(&case, |u| matches!(u, SourceUnit::Suite(_))) {
        return Body::Statements;
    }
    let items = format!("commons {target}{ITEMS_INFIX}0\n\n{body}\n");
    if parses_clean(&items, |u| matches!(u, SourceUnit::Commons(_))) {
        Body::Items
    } else {
        Body::Statements
    }
}

/// The byte offset at which each line of `text` starts.
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// One synthetic file's map back to the real file it came from: both have the
/// same lines, so an offset maps by its line and column.
struct Table {
    /// The synthetic file's source-map key.
    synth_key: PathBuf,
    synth_starts: Vec<usize>,
    real_starts: Vec<usize>,
    real_len: usize,
    /// Set for a synthetic copy of a context, whose fence is appended to the
    /// end rather than sitting on its own lines.
    copy: Option<CopyMap>,
}

/// Where a context copy's appended fence body sits, and where it came from.
struct CopyMap {
    /// The synthetic byte offset at which the appended text starts.
    from: usize,
    /// The fence's `uses` hoisted into the header: where each sits in the
    /// copy, and its line in the body.
    hoisted: Vec<(Range<usize>, usize)>,
    /// The first appended body line, in the copy.
    synth_line: usize,
    /// The body's first line, in the real file.
    real_line: usize,
    /// The number of body lines.
    len: usize,
}

impl Table {
    /// The real line `offset` of the synthetic file falls on, as a column and
    /// the real line's byte range.
    fn locate(&self, offset: usize) -> (usize, Range<usize>) {
        let line = self
            .synth_starts
            .partition_point(|&s| s <= offset)
            .saturating_sub(1);
        let real_line = match &self.copy {
            // Past the body is generated text: the closing fence line.
            Some(c) if line >= c.synth_line => c.real_line + (line - c.synth_line).min(c.len),
            _ => line,
        }
        .min(self.real_starts.len() - 1);
        let start = self.real_starts[real_line];
        let end = self
            .real_starts
            .get(real_line + 1)
            .map_or(self.real_len, |next| next - 1);
        (offset - self.synth_starts[line], start..end)
    }

    fn map_range(&self, start: usize, end: usize) -> (usize, usize) {
        let (col, line) = self.locate(start);
        // Generated text (the `suite`/`case` header) runs past the end of the
        // fence line it sits on: that is the whole fence line.
        if line.start + col > line.end {
            return (line.start, line.end);
        }
        let from = line.start + col;
        let (end_col, end_line) = self.locate(end.max(start));
        (from, (end_line.start + end_col).min(end_line.end).max(from))
    }
}

/// The synthetic suite for `examples`, line for line with the real file.
fn suite_text(
    target: &str,
    file: &str,
    n_lines: usize,
    examples: &[&Example<'_>],
) -> Option<String> {
    let mut lines = vec![String::new(); n_lines];
    for (i, ex) in examples.iter().enumerate() {
        let head = if i == 0 {
            format!("suite {target} {{ ")
        } else {
            String::new()
        };
        lines[ex.open] = format!("{head}case \"doc example {file}:{}\" {{", ex.open + 1);
        for (k, l) in ex.body.iter().enumerate() {
            lines[ex.open + 1 + k] = l.text.to_string();
        }
        let tail = if i + 1 == examples.len() { "} }" } else { "}" };
        lines[ex.close()] = tail.to_string();
    }
    (!examples.is_empty()).then(|| lines.join("\n"))
}

/// The synthetic sibling `commons` for one example of declarations.
fn items_text(target: &str, n: usize, n_lines: usize, ex: &Example<'_>) -> String {
    let mut lines = vec![String::new(); n_lines];
    lines[0] = format!("commons {target}{ITEMS_INFIX}{n}");
    lines[ex.open] = format!("uses {target}");
    for (k, l) in ex.body.iter().enumerate() {
        lines[ex.open + 1 + k] = l.text.to_string();
    }
    lines.join("\n")
}

/// The names a fence of declarations declares.
fn fence_item_names(target: &str, body: &str) -> Vec<String> {
    let text = format!("commons {target}{ITEMS_INFIX}0\n\n{body}\n");
    let Ok(tokens) = tokenize(&text) else {
        return Vec::new();
    };
    match parse_unit_with_recovery(&tokens, &text).0 {
        Some(SourceUnit::Commons(c)) => c
            .items
            .iter()
            .filter_map(|i| i.name().map(|n| n.name.clone()))
            .collect(),
        _ => Vec::new(),
    }
}

/// The leading `uses` lines of a declaration fence: a `uses` after a
/// declaration is a parse error, so the copy declares them in the context's
/// header instead. Each is the line's index in the body and its `uses` text.
fn fence_uses(ex: &Example<'_>) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (k, l) in ex.body.iter().enumerate() {
        let t = l.text.trim();
        if t.starts_with("uses ") {
            out.push((k, t.to_string()));
        } else if !(t.is_empty() || t.starts_with("--")) {
            break;
        }
    }
    out
}

/// A copy of a context made to check a declaration fence.
struct Copy {
    text: String,
    /// The byte offset at which the appended fence body starts.
    from: usize,
    /// The `uses` the fence brought to the header: where each sits in the copy,
    /// and its line in the fence body.
    hoisted: Vec<(Range<usize>, usize)>,
}

/// A copy of the context `source` with the declarations in `blank` and the
/// `close` brace (of a brace-form context) blanked out, the fence's `uses`
/// added to the header after `header_end`, and the rest of the fence `ex`
/// appended. Blanking keeps every line, so the real file's lines are the copy's;
/// the body is the copy's last lines.
fn context_copy(
    source: &str,
    blank: &[Range<usize>],
    close: Option<usize>,
    header_end: usize,
    uses: &[(usize, String)],
    moved: &[usize],
    ex: &Example<'_>,
) -> Copy {
    let mut bytes = source.as_bytes().to_vec();
    for range in blank {
        for b in &mut bytes[range.clone()] {
            if !matches!(*b, b'\n' | b'\r') {
                *b = b' ';
            }
        }
    }
    if let Some(at) = close {
        bytes[at] = b' ';
    }
    let mut text = String::from_utf8(bytes).expect("blanking keeps whole characters");
    let mut hoisted = Vec::new();
    let mut at = header_end;
    for (k, line) in uses {
        let piece = format!(" {line}");
        text.insert_str(at, &piece);
        hoisted.push((at + 1..at + piece.len(), *k));
        at += piece.len();
    }
    text.push('\n');
    let from = text.len();
    let body: Vec<&str> = ex
        .body
        .iter()
        .enumerate()
        .map(|(k, l)| if moved.contains(&k) { "" } else { l.text })
        .collect();
    text.push_str(&body.join("\n"));
    if close.is_some() {
        text.push_str("\n}");
    }
    Copy {
        text,
        from,
        hoisted,
    }
}

/// The real file name a synthetic file's name stands for.
fn real_name_of(synthetic: &str) -> Option<String> {
    if let Some(stem) = synthetic.strip_suffix(SYNTHETIC_SUFFIX) {
        return Some(format!("{stem}.bynk"));
    }
    let stem = synthetic.strip_suffix(".bynk")?;
    let (real, n) = stem.rsplit_once(ITEMS_INFIX)?;
    (!n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).then(|| format!("{real}.bynk"))
}

/// The synthetic files for a project's doc examples, and the means to report
/// their diagnostics at the fences.
#[derive(Default)]
pub struct DocExamples {
    /// Every file a check adds.
    check: Vec<(PathBuf, String)>,
    /// The suites of the examples marked `bynk test`: what a run adds.
    run: Vec<(PathBuf, String)>,
    tables: Vec<Table>,
    /// The copies of contexts that check their declaration fences, by layer.
    layers: Vec<Vec<(PathBuf, String)>>,
    layer_tables: Vec<Vec<Table>>,
    /// Checked examples.
    count: usize,
}

impl DocExamples {
    /// Scan every unit in `sources` for checked doc examples.
    pub fn collect(sources: &HashMap<PathBuf, String>) -> Self {
        let mut keys: Vec<&PathBuf> = sources.keys().collect();
        keys.sort();
        let mut found = DocExamples::default();
        for key in keys {
            let source = &sources[key];
            let Ok(tokens) = tokenize(source) else {
                continue;
            };
            let (unit, errors) = parse_unit_with_recovery(&tokens, source);
            let (target, context) = match unit {
                Some(SourceUnit::Commons(c)) if errors.is_empty() => (c.name.joined(), None),
                Some(SourceUnit::Context(c)) if errors.is_empty() => (c.name.joined(), Some(c)),
                _ => continue,
            };
            let examples = examples(source);
            if examples.is_empty() {
                continue;
            }
            found.add_unit(sources, key, &target, context.as_ref(), &examples);
        }
        found
    }

    fn add_unit(
        &mut self,
        sources: &HashMap<PathBuf, String>,
        key: &Path,
        target: &str,
        context: Option<&Context>,
        examples: &[Example<'_>],
    ) {
        let source = &sources[key];
        let file = key.file_name().unwrap_or_default().to_string_lossy();
        let stem = key.file_stem().unwrap_or_default().to_string_lossy();
        let real_starts = line_starts(source);
        let n_lines = real_starts.len();
        let table = |synth_key: &PathBuf, text: &str| Table {
            synth_key: synth_key.clone(),
            synth_starts: line_starts(text),
            real_starts: real_starts.clone(),
            real_len: source.len(),
            copy: None,
        };
        let mut cases: Vec<&Example<'_>> = Vec::new();
        let mut items_n = 0;
        let mut copies = 0;
        for ex in examples {
            match classify_body(target, &ex.text()) {
                Body::Statements => {
                    cases.push(ex);
                    self.count += 1;
                }
                Body::Items if context.is_some() => {
                    let ctx = context.expect("matched Some");
                    if self.add_copy(key, source, ctx, target, ex, copies) {
                        copies += 1;
                        self.count += 1;
                    }
                }
                Body::Items => {
                    // A file the project really has is never overwritten.
                    let synth_key = loop {
                        items_n += 1;
                        let k = key.with_file_name(format!("{stem}{ITEMS_INFIX}{items_n}.bynk"));
                        if !sources.contains_key(&k) {
                            break k;
                        }
                    };
                    self.count += 1;
                    let text = items_text(target, items_n, n_lines, ex);
                    self.tables.push(table(&synth_key, &text));
                    self.check.push((synth_key, text));
                }
            }
        }
        let synth_key = key.with_file_name(format!("{stem}{SYNTHETIC_SUFFIX}"));
        // A real file already named like the suite keeps its name, and the
        // cases are left unchecked rather than replace it.
        if sources.contains_key(&synth_key) {
            self.count -= cases.len();
            return;
        }
        if let Some(text) = suite_text(target, &file, n_lines, &cases) {
            self.tables.push(table(&synth_key, &text));
            self.check.push((synth_key.clone(), text));
        }
        let runnable: Vec<&Example<'_>> = cases.iter().copied().filter(|e| e.run).collect();
        if let Some(text) = suite_text(target, &file, n_lines, &runnable) {
            self.run.push((synth_key, text));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.check.is_empty() && self.layers.is_empty()
    }

    /// Whether any file besides the context copies is added to a check.
    pub fn has_base(&self) -> bool {
        !self.check.is_empty()
    }

    /// The number of extra project checks the context copies need.
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// `sources` with the context copies of `layer` in place of the contexts.
    pub fn layer_overlay(
        &self,
        layer: usize,
        sources: &HashMap<PathBuf, String>,
    ) -> HashMap<PathBuf, String> {
        let mut out = sources.clone();
        out.extend(self.layers[layer].iter().cloned());
        out
    }

    /// The errors a context copy's fence caused, in the real file. Only those
    /// inside the fence count: one the replaced declaration caused elsewhere
    /// is not the example's to answer for.
    pub fn remap_layer(&self, layer: usize, errors: Vec<AttributedError>) -> Vec<AttributedError> {
        errors
            .into_iter()
            .filter(|ae| {
                bynk_syntax::Severity::for_error(&ae.error) == bynk_syntax::Severity::Error
            })
            .filter_map(|ae| {
                let path = ae.source_path.clone()?;
                let table = self.layer_tables[layer]
                    .iter()
                    .find(|t| t.synth_key.ends_with(&path))?;
                let copy = table.copy.as_ref()?;
                let at = ae.error.span.start;
                if let Some((_, k)) = copy.hoisted.iter().find(|(r, _)| r.contains(&at)) {
                    // An error in a `uses` moved to the header is the fence
                    // line's.
                    return Some(Self::at_fence_line(ae, table, copy.real_line + k, path));
                }
                (at >= copy.from).then(|| Self::remap_one(ae, table, path))
            })
            .collect()
    }

    /// Add the copy of `ctx` that checks the declaration fence `ex`, in the
    /// layer after the `n` fences of this context already added. False when the
    /// context cannot be copied.
    fn add_copy(
        &mut self,
        key: &Path,
        source: &str,
        ctx: &Context,
        target: &str,
        ex: &Example<'_>,
        n: usize,
    ) -> bool {
        let close = match ctx.form {
            CommonsForm::Fragment => None,
            CommonsForm::Brace => {
                let at = source[..ctx.span.end].rfind('}');
                debug_assert!(at.is_some(), "a brace-form context ends in a brace");
                match at {
                    Some(at) => Some(at),
                    None => return false,
                }
            }
        };
        // A `uses` the context already has is left out of the header.
        let have: Vec<String> = ctx.uses.iter().map(|u| u.target.joined()).collect();
        let all_uses = fence_uses(ex);
        let moved: Vec<usize> = all_uses.iter().map(|(k, _)| *k).collect();
        let uses: Vec<(usize, String)> = all_uses
            .into_iter()
            .filter(|(_, line)| !have.iter().any(|h| line.trim_end() == format!("uses {h}")))
            .collect();
        let names = fence_item_names(target, &ex.text());
        let blank: Vec<Range<usize>> = ctx
            .items
            .iter()
            .filter(|i| i.name().is_some_and(|n| names.contains(&n.name)))
            .map(|i| i.span().start..i.span().end)
            .collect();
        let Copy {
            text,
            from,
            hoisted,
        } = context_copy(source, &blank, close, ctx.name.span.end, &uses, &moved, ex);
        let real_starts = line_starts(source);
        let table = Table {
            synth_key: key.to_path_buf(),
            synth_starts: line_starts(&text),
            copy: Some(CopyMap {
                from,
                hoisted,
                synth_line: real_starts.len(),
                real_line: ex.open + 1,
                len: ex.body.len(),
            }),
            real_starts,
            real_len: source.len(),
        };
        while self.layers.len() <= n {
            self.layers.push(Vec::new());
            self.layer_tables.push(Vec::new());
        }
        self.layers[n].push((key.to_path_buf(), text));
        self.layer_tables[n].push(table);
        true
    }

    /// The number of examples that will be checked.
    pub fn example_count(&self) -> usize {
        self.count
    }

    /// Whether any example is marked `bynk test`.
    pub fn has_runnable(&self) -> bool {
        !self.run.is_empty()
    }

    /// `sources` plus every synthetic file, for the check.
    pub fn check_overlay(&self, sources: &HashMap<PathBuf, String>) -> HashMap<PathBuf, String> {
        let mut out = sources.clone();
        out.extend(self.check.iter().cloned());
        out
    }

    /// `sources` plus the suites of the examples marked `bynk test`.
    pub fn run_overlay(&self, sources: &HashMap<PathBuf, String>) -> HashMap<PathBuf, String> {
        let mut out = sources.clone();
        out.extend(self.run.iter().cloned());
        out
    }

    /// For each runnable suite, the synthetic file name and the real file name
    /// to show in its place in the run's output (`calc.doc-examples.bynk` ->
    /// `calc.bynk`). The line and column are already the real file's.
    pub fn run_aliases(&self) -> Vec<(String, String)> {
        self.run
            .iter()
            .filter_map(|(key, _)| {
                let name = key.file_name()?.to_string_lossy().into_owned();
                let real = real_name_of(&name)?;
                Some((name, real))
            })
            .collect()
    }

    /// The errors the examples caused, in the real file. One a synthetic file
    /// produced is mapped to its fence. One attributed to a real file, such as a
    /// duplicate declaration reported at the original, is kept as it is with a
    /// note that the examples brought it about: the project compiled clean
    /// without them, so it is theirs.
    pub fn remap(&self, errors: Vec<AttributedError>) -> Vec<AttributedError> {
        errors
            .into_iter()
            .filter(|ae| {
                bynk_syntax::Severity::for_error(&ae.error) == bynk_syntax::Severity::Error
            })
            .map(|ae| match self.table_for(&ae) {
                Some((table, real)) => Self::remap_one(ae, table, real),
                None => {
                    let mut ae = ae;
                    ae.error.notes.push(
                        "this appeared when the doc-comment examples were checked".to_string(),
                    );
                    ae
                }
            })
            .collect()
    }

    fn table_for(&self, ae: &AttributedError) -> Option<(&Table, PathBuf)> {
        let path = ae.source_path.as_deref()?;
        let name = path.file_name()?.to_string_lossy().into_owned();
        let real = path.with_file_name(real_name_of(&name)?);
        let table = self.tables.iter().find(|t| t.synth_key.ends_with(path))?;
        Some((table, real))
    }

    /// `ae` moved onto the whole of line `real_line` of the real file.
    fn at_fence_line(
        mut ae: AttributedError,
        table: &Table,
        real_line: usize,
        real: PathBuf,
    ) -> AttributedError {
        let start = table.real_starts[real_line];
        let end = table
            .real_starts
            .get(real_line + 1)
            .map_or(table.real_len, |next| next - 1);
        ae.error.span = bynk_syntax::span::Span {
            start,
            end,
            ..ae.error.span
        };
        ae.error.labels.clear();
        ae.error.suggestions.clear();
        ae.source_path = Some(real);
        ae
    }

    fn remap_one(mut ae: AttributedError, table: &Table, real: PathBuf) -> AttributedError {
        let map = |span: bynk_syntax::span::Span| {
            let (start, end) = table.map_range(span.start, span.end);
            bynk_syntax::span::Span { start, end, ..span }
        };
        ae.error.span = map(ae.error.span);
        for (i, (span, _)) in ae.error.labels.iter_mut().enumerate() {
            if matches!(
                ae.label_files.get(i),
                None | Some(bynk_emit::project::LabelFile::Primary)
            ) {
                *span = map(*span);
            }
        }
        // A suggestion's edits are offsets into the synthetic text.
        ae.error.suggestions.clear();
        ae.source_path = Some(real);
        ae
    }
}

/// Whether `c` can be part of a file name, so a name that continues past it is a
/// different file.
fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.')
}

/// The byte offsets in `text` at which `name` occurs as a whole file name: not
/// the tail of a longer one (`src/xcalc.doc-examples.bynk` is not
/// `calc.doc-examples.bynk`) and not the head of one.
fn name_occurrences<'t>(text: &'t str, name: &'t str) -> impl Iterator<Item = usize> + 't {
    text.match_indices(name).map(|(i, _)| i).filter(move |&i| {
        let before = text[..i].chars().next_back();
        let after = text[i + name.len()..].chars().next();
        !before.is_some_and(is_name_char) && !after.is_some_and(is_name_char)
    })
}

/// Whether `path` is one of the synthetic files in `aliases`, by its whole name.
pub fn is_synthetic_path(path: &str, aliases: &[(String, String)]) -> bool {
    aliases.iter().any(|(synthetic, _)| {
        path.ends_with(synthetic.as_str()) && name_occurrences(path, synthetic).count() > 0
    })
}

/// Show the real file's name in place of a synthetic one in `text`. Only a whole
/// file name is replaced, never part of a longer one or of other text.
pub fn apply_aliases<'a>(text: &'a str, aliases: &[(String, String)]) -> std::borrow::Cow<'a, str> {
    let mut out = std::borrow::Cow::Borrowed(text);
    for (synthetic, real) in aliases {
        let at: Vec<usize> = name_occurrences(&out, synthetic).collect();
        if at.is_empty() {
            continue;
        }
        let mut replaced = String::with_capacity(out.len());
        let mut last = 0;
        for i in at {
            replaced.push_str(&out[last..i]);
            replaced.push_str(real);
            last = i + synthetic.len();
        }
        replaced.push_str(&out[last..]);
        out = std::borrow::Cow::Owned(replaced);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_info_strings() {
        assert_eq!(classify("bynk"), Fence::Checked);
        assert_eq!(classify(" bynk "), Fence::Checked);
        assert_eq!(classify("bynk test"), Fence::Run);
        assert_eq!(classify("bynk,test"), Fence::Run);
        assert_eq!(classify("bynk run"), Fence::Checked);
        assert_eq!(classify("bynk ignore"), Fence::Skipped);
        assert_eq!(classify("bynk,ignore"), Fence::Skipped);
        assert_eq!(classify("bynk test ignore"), Fence::Skipped);
        assert_eq!(classify("bynk,fragment"), Fence::Skipped);
        assert_eq!(classify("bynk,fail=bynk.types.mismatch"), Fence::Skipped);
        assert_eq!(classify("ts"), Fence::Foreign);
        assert_eq!(classify(""), Fence::Foreign);
        assert_eq!(classify("bynkc"), Fence::Foreign);
    }

    fn sources(text: &str) -> HashMap<PathBuf, String> {
        HashMap::from([(PathBuf::from("src/m.bynk"), text.to_string())])
    }

    #[test]
    fn finds_doc_block_examples() {
        let src = "---\nSums.\n\n```bynk\nexpect add(1, 2) == 3\n```\n\n```bynk ignore\nnope\n```\n---\ncommons m\n\n---\n```bynk\nexpect true\n```\n---\nfn add(a: Int, b: Int) -> Int { a + b }\n";
        let found = DocExamples::collect(&sources(src));
        assert_eq!(found.example_count(), 2);
        let (_, text) = &found.check[0];
        assert!(text.starts_with("\n\n\nsuite m { case "), "{text}");
        assert!(text.contains("expect add(1, 2) == 3\n"), "{text}");
        assert!(text.contains("expect true\n"), "{text}");
        assert!(!text.contains("nope"), "{text}");
        assert!(!found.has_runnable());
    }

    #[test]
    fn a_synthetic_file_keeps_every_line_of_the_real_one() {
        let src =
            "---\nProse.\n\n```bynk test\n  let x = 1\n\texpect x == 1\n```\n---\ncommons m\n";
        let found = DocExamples::collect(&sources(src));
        assert!(found.has_runnable());
        let (_, text) = &found.run[0];
        let want: Vec<&str> = src.split('\n').collect();
        let got: Vec<&str> = text.split('\n').collect();
        assert_eq!(got.len(), want.len(), "{text}");
        // Each body line is on its real line, column for column.
        assert_eq!(got[4], want[4]);
        assert_eq!(got[5], want[5]);
        assert!(got[3].starts_with("suite m { case \"doc example m.bynk:4\" {"));
        assert_eq!(got[6], "} }");
    }

    #[test]
    fn maps_a_body_offset_back_to_the_fence() {
        let src = "---\n```bynk\nexpect 1 == 1\n```\n---\ncommons m\n";
        let found = DocExamples::collect(&sources(src));
        let (_, text) = &found.check[0];
        let synth = text.find("expect").unwrap();
        let table = &found.tables[0];
        let (start, end) = table.map_range(synth + 7, synth + 8);
        assert_eq!(&src[start..end], "1");
        // The generated header maps onto the opening fence line.
        let (hs, he) = table.map_range(text.find("case").unwrap(), text.find("case").unwrap() + 4);
        assert!(src[hs..he].starts_with("```"), "{:?}", &src[hs..he]);
    }

    #[test]
    fn classifies_bodies_by_parsing_them() {
        assert_eq!(classify_body("m", "expect 1 == 1"), Body::Statements);
        assert_eq!(
            classify_body("m", "let x = 1\nexpect x == 1"),
            Body::Statements
        );
        assert_eq!(
            classify_body("m", "type P = { a: Int }\n\nfn f(p: P) -> Int { p.a }"),
            Body::Items
        );
        // Neither: stays statements, so the checker reports the parse error.
        assert_eq!(classify_body("m", "this is nonsense ("), Body::Statements);
    }

    #[test]
    fn a_declaration_fence_is_a_sibling_commons_that_uses_a_commons() {
        let src = "---\n```bynk\nfn f() -> Int { 1 }\n```\n---\ncommons m\n";
        let found = DocExamples::collect(&sources(src));
        assert_eq!(found.check.len(), 1);
        let (key, text) = &found.check[0];
        assert!(key.ends_with("m_doc_example_1.bynk"), "{key:?}");
        assert!(text.starts_with("commons m_doc_example_1\n"), "{text}");
        assert!(text.contains("\nuses m\n"), "{text}");
        assert_eq!(
            real_name_of("m_doc_example_1.bynk").as_deref(),
            Some("m.bynk")
        );
        assert_eq!(
            real_name_of("m.doc-examples.bynk").as_deref(),
            Some("m.bynk")
        );
        assert_eq!(real_name_of("m.bynk"), None);
        assert_eq!(real_name_of("m_doc_example_x.bynk"), None);
    }

    #[test]
    fn a_declaration_fence_in_a_context_is_checked_in_a_copy() {
        let src = "---\n```bynk\ntype Order = { c: Int }\nfn f(o: Order) -> Int { 1 }\n```\n---\ncontext m\n\ntype Order = { c: Int }\n";
        let found = DocExamples::collect(&sources(src));
        assert!(!found.is_empty());
        assert!(!found.has_base());
        assert_eq!(found.layer_count(), 1);
        assert_eq!(found.example_count(), 1);
        let (key, text) = &found.layers[0][0];
        assert!(key.ends_with("m.bynk"), "{key:?}");
        // The documented `type Order` is blanked, the fence appended, and the
        // lines of the real file kept.
        assert!(text.starts_with(&src[..src.find("type Order = { c: Int }\n").unwrap()]));
        let code = &text[text.find("context m").unwrap()..];
        assert_eq!(code.matches("type Order").count(), 1, "{code}");
        assert!(
            text.ends_with("\ntype Order = { c: Int }\nfn f(o: Order) -> Int { 1 }"),
            "{text}"
        );
        let table = &found.layer_tables[0][0];
        let at = text.rfind("fn f").unwrap();
        let (start, _) = table.map_range(at, at + 2);
        assert_eq!(&src[start..start + 2], "fn");
    }

    #[test]
    fn a_fence_uses_moves_to_the_context_header() {
        let src = "---\n```bynk\nuses a.b\nuses m.c\n\nfn f() -> Int { 1 }\n```\n---\ncontext m uses m.c\n\ntype T = { c: Int }\n";
        let found = DocExamples::collect(&sources(src));
        let (_, text) = &found.layers[0][0];
        // `m.c` is already used; `a.b` moves to the header and the body keeps
        // its lines, with the `uses` lines blank.
        let code = &text[text.find("context m").unwrap()..];
        assert!(code.starts_with("context m uses a.b uses m.c\n"), "{code}");
        assert!(text.ends_with("\n\n\n\nfn f() -> Int { 1 }"), "{text}");
        assert_eq!(text.matches("uses a.b").count(), 2, "{text}");
        let copy = found.layer_tables[0][0].copy.as_ref().unwrap();
        assert_eq!(copy.hoisted.len(), 1);
        assert_eq!(&text[copy.hoisted[0].0.clone()], "uses a.b");
        assert_eq!(copy.hoisted[0].1, 0);
    }

    #[test]
    fn two_declaration_fences_in_one_context_take_a_layer_each() {
        let src = "---\n```bynk\ntype A = { c: Int }\n```\n\n```bynk\ntype A = { d: Int }\n```\n---\ncontext m\n";
        let found = DocExamples::collect(&sources(src));
        assert_eq!(found.layer_count(), 2);
        assert_eq!(found.example_count(), 2);
    }

    #[test]
    fn a_brace_form_context_closes_after_the_appended_fence() {
        let src =
            "---\n```bynk\nfn f() -> Int { 1 }\n```\n---\ncontext m {\n  fn g() -> Int { 2 }\n}\n";
        let found = DocExamples::collect(&sources(src));
        let (_, text) = &found.layers[0][0];
        assert!(text.ends_with("fn f() -> Int { 1 }\n}"), "{text}");
        // The real closing brace is blanked: the last one is the generated one.
        let code = &text[text.find("context m").unwrap()..];
        assert_eq!(code.matches('}').count(), 3, "{code}");
    }

    #[test]
    fn aliases_show_the_real_file_name() {
        let aliases = vec![("m.doc-examples.bynk".to_string(), "m.bynk".to_string())];
        assert_eq!(
            apply_aliases("at src/m.doc-examples.bynk:5:3", &aliases),
            "at src/m.bynk:5:3"
        );
        assert!(matches!(
            apply_aliases("nothing here", &aliases),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn a_longer_fence_keeps_a_shorter_one_shown_inside_it() {
        // A ````text fence showing a ```bynk example is one foreign fence.
        let src = "---\n````text\n```bynk\nnope nope\n```\n````\n---\ncommons m\n";
        assert!(DocExamples::collect(&sources(src)).is_empty());
    }

    #[test]
    fn an_unterminated_fence_is_checked_to_the_end_of_the_doc() {
        let src = "---\n```bynk\nexpect 1 == 1\n---\ncommons m\n";
        let found = DocExamples::collect(&sources(src));
        assert_eq!(found.example_count(), 1);
        let (_, text) = &found.check[0];
        assert!(text.contains("expect 1 == 1\n"), "{text}");
    }

    #[test]
    fn a_real_file_named_like_a_synthetic_one_is_never_replaced() {
        let src = "---\n```bynk\nexpect 1 == 1\n```\n---\ncommons m\n";
        let mut all = sources(src);
        all.insert(PathBuf::from("src/m.doc-examples.bynk"), "x".to_string());
        let found = DocExamples::collect(&all);
        assert!(
            found.is_empty(),
            "the suite is left unchecked, not written over"
        );
        assert_eq!(found.example_count(), 0);
        // A taken sibling name moves the next one along.
        let items = "---\n```bynk\nfn f() -> Int { 1 }\n```\n---\ncommons m\n";
        let mut all = sources(items);
        all.insert(PathBuf::from("src/m_doc_example_1.bynk"), "x".to_string());
        let found = DocExamples::collect(&all);
        assert!(found.check[0].0.ends_with("m_doc_example_2.bynk"));
    }

    #[test]
    fn aliases_only_touch_a_whole_file_name() {
        let aliases = vec![(
            "calc.doc-examples.bynk".to_string(),
            "calc.bynk".to_string(),
        )];
        assert_eq!(
            apply_aliases("at src/demo/calc.doc-examples.bynk:5:3", &aliases),
            "at src/demo/calc.bynk:5:3"
        );
        // A longer name that merely ends the same way is another file.
        assert_eq!(
            apply_aliases("src/xcalc.doc-examples.bynk", &aliases),
            "src/xcalc.doc-examples.bynk"
        );
        assert_eq!(
            apply_aliases("calc.doc-examples.bynk.map", &aliases),
            "calc.doc-examples.bynk.map"
        );
        assert!(is_synthetic_path(
            "src/demo/calc.doc-examples.bynk",
            &aliases
        ));
        assert!(is_synthetic_path("calc.doc-examples.bynk", &aliases));
        assert!(!is_synthetic_path("src/xcalc.doc-examples.bynk", &aliases));
    }
}
