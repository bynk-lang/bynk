//! Doc-comment examples (#2461): a fenced `bynk` block in a doc comment is
//! checked by `bynk test`, so an example cannot silently fall out of step with
//! the code it documents. One marked `bynk test` also runs.
//!
//! Each example of statements becomes a `case` in a synthetic `suite` targeting
//! the unit that declares the doc comment, so it sees the unit's scope and the
//! ordinary test semantics. An example of declarations becomes a synthetic
//! sibling `commons` that `uses` the declaring unit (when that is a commons; a
//! context's declarations are not checked, as nothing reachable from a sibling
//! would resolve them). The synthetic files are built in memory and added to
//! the project's source overlay. A first project *check* reports every
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
use bynk_syntax::ast::SourceUnit;
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

fn examples(source: &str) -> Vec<Example<'_>> {
    let line_of = |offset: usize| source[..offset].matches('\n').count();
    let mut out = Vec::new();
    for block in doc_blocks(source) {
        let mut open: Option<(Fence, usize, Vec<DocLine<'_>>)> = None;
        for line in block {
            let trimmed = line.text.trim_start();
            if let Some(info) = trimmed.strip_prefix("```") {
                match open.take() {
                    None => open = Some((classify(info), line_of(line.start), Vec::new())),
                    Some((kind @ (Fence::Checked | Fence::Run), open, body)) => {
                        out.push(Example {
                            run: kind == Fence::Run,
                            open,
                            body,
                        });
                    }
                    Some(_) => {}
                }
            } else if let Some((_, _, body)) = open.as_mut() {
                body.push(line);
            }
        }
        // An unterminated fence is not an example: Markdown renders it as one,
        // but a doc comment cut off mid-fence is more likely a typo than code.
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
}

impl Table {
    /// The real line `offset` of the synthetic file falls on, as a column and
    /// the real line's byte range.
    fn locate(&self, offset: usize) -> (usize, Range<usize>) {
        let line = self
            .synth_starts
            .partition_point(|&s| s <= offset)
            .saturating_sub(1);
        let start = self.real_starts[line];
        let end = self
            .real_starts
            .get(line + 1)
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
            if key
                .file_name()
                .is_some_and(|n| real_name_of(&n.to_string_lossy()).is_some())
            {
                continue;
            }
            let Ok(tokens) = tokenize(source) else {
                continue;
            };
            let (unit, errors) = parse_unit_with_recovery(&tokens, source);
            let (target, is_commons) = match unit {
                Some(SourceUnit::Commons(c)) if errors.is_empty() => (c.name.joined(), true),
                Some(SourceUnit::Context(c)) if errors.is_empty() => (c.name.joined(), false),
                _ => continue,
            };
            let examples = examples(source);
            if examples.is_empty() {
                continue;
            }
            found.add_unit(key, source, &target, is_commons, &examples);
        }
        found
    }

    fn add_unit(
        &mut self,
        key: &Path,
        source: &str,
        target: &str,
        is_commons: bool,
        examples: &[Example<'_>],
    ) {
        let file = key.file_name().unwrap_or_default().to_string_lossy();
        let stem = key.file_stem().unwrap_or_default().to_string_lossy();
        let real_starts = line_starts(source);
        let n_lines = real_starts.len();
        let table = |synth_key: &PathBuf, text: &str| Table {
            synth_key: synth_key.clone(),
            synth_starts: line_starts(text),
            real_starts: real_starts.clone(),
            real_len: source.len(),
        };
        let mut cases: Vec<&Example<'_>> = Vec::new();
        let mut items_n = 0;
        for ex in examples {
            match classify_body(target, &ex.text()) {
                Body::Statements => {
                    cases.push(ex);
                    self.count += 1;
                }
                // A context's own types are not reachable from a sibling unit
                // (a `uses` names a commons), and adding the items to a copy
                // of the context collides with the declaration the doc usually
                // shows. So a declaration fence in a context is not checked:
                // any mode that checked it would report false errors on
                // correct docs.
                Body::Items if !is_commons => {}
                Body::Items => {
                    items_n += 1;
                    self.count += 1;
                    let text = items_text(target, items_n, n_lines, ex);
                    let synth_key =
                        key.with_file_name(format!("{stem}{ITEMS_INFIX}{items_n}.bynk"));
                    self.tables.push(table(&synth_key, &text));
                    self.check.push((synth_key, text));
                }
            }
        }
        let synth_key = key.with_file_name(format!("{stem}{SYNTHETIC_SUFFIX}"));
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
        self.check.is_empty()
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

    /// Keep only the errors a synthetic file produced, mapped to the fence in
    /// the real file. Everything else belongs to the project proper, which
    /// compiled clean before the examples were looked at.
    pub fn remap(&self, errors: Vec<AttributedError>) -> Vec<AttributedError> {
        errors
            .into_iter()
            .filter(|ae| {
                bynk_syntax::Severity::for_error(&ae.error) == bynk_syntax::Severity::Error
            })
            .filter_map(|ae| self.remap_one(ae))
            .collect()
    }

    fn remap_one(&self, mut ae: AttributedError) -> Option<AttributedError> {
        let path = ae.source_path.as_deref()?;
        let name = path.file_name()?.to_string_lossy().into_owned();
        let real_identity = path.with_file_name(real_name_of(&name)?);
        let table = self.tables.iter().find(|t| t.synth_key.ends_with(path))?;
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
        ae.source_path = Some(real_identity);
        Some(ae)
    }
}

/// Show the real file's name in place of a synthetic one in `text`.
pub fn apply_aliases<'a>(text: &'a str, aliases: &[(String, String)]) -> std::borrow::Cow<'a, str> {
    let mut out = std::borrow::Cow::Borrowed(text);
    for (synthetic, real) in aliases {
        if out.contains(synthetic.as_str()) {
            out = std::borrow::Cow::Owned(out.replace(synthetic.as_str(), real));
        }
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
    fn a_declaration_fence_in_a_context_is_not_checked() {
        let src = "---\n```bynk\nfn f(o: Order) -> Int { 1 }\n```\n---\ncontext m\n\ntype Order = { c: Int }\n";
        let found = DocExamples::collect(&sources(src));
        assert!(found.is_empty());
        assert_eq!(found.example_count(), 0);
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
}
