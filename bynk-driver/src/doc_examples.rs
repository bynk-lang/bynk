//! Doc-comment examples (#2461): a fenced `bynk` block in a doc comment is
//! checked by `bynk test`, so an example cannot silently fall out of step with
//! the code it documents.
//!
//! Each example becomes a synthetic `case` in a synthetic `suite` targeting the
//! unit that declares the doc comment. The case therefore sees the unit's scope
//! and the ordinary test semantics. The suites are built in memory, added to the
//! project's source overlay, and the project is *checked* (never emitted or run:
//! type-checking is the default, ADR `doc-examples-checked`). A diagnostic that
//! lands in a synthetic file is mapped back to the fence in the real file.
//!
//! **Fence info strings.** The first word is `bynk`; further words are
//! separated by spaces or commas (`bynk ignore` and `bynk,ignore` are the same
//! fence, matching the Book's own examples). `ignore`, `fragment` and `fail`
//! opt a fence out; any other fence (`ts`, `text`, …) is not Bynk and is not
//! looked at.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use bynk_emit::project::AttributedError;
use bynk_syntax::ast::SourceUnit;
use bynk_syntax::lexer::{TokenKind, doc_block_body_range, tokenize};
use bynk_syntax::parser::parse_unit_with_recovery;

/// The tail of a synthetic file's name: `<stem>.doc-examples.bynk` beside the
/// real `<stem>.bynk`.
const SYNTHETIC_SUFFIX: &str = ".doc-examples.bynk";

/// How a fence's info string classifies it.
#[derive(Debug, PartialEq, Eq)]
enum Fence {
    /// Not a Bynk fence (`ts`, `text`, no language, …): not looked at.
    Foreign,
    /// A Bynk fence that opts out of checking (`ignore`, `fragment`, `fail`).
    Skipped,
    /// A Bynk fence to check.
    Checked,
}

fn classify(info: &str) -> Fence {
    let mut words = info
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|w| !w.is_empty());
    if words.next() != Some("bynk") {
        return Fence::Foreign;
    }
    if words.any(|w| matches!(w, "ignore" | "fragment") || w == "fail" || w.starts_with("fail=")) {
        Fence::Skipped
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

/// A fenced example: where the opening fence line sits, and its body lines.
struct Example<'a> {
    /// The opening fence line's text range in the real source.
    fence: Range<usize>,
    body: Vec<DocLine<'a>>,
}

fn examples(source: &str) -> Vec<Example<'_>> {
    let mut out = Vec::new();
    for run in doc_blocks(source) {
        let mut open: Option<(Fence, Range<usize>, Vec<DocLine<'_>>)> = None;
        for line in run {
            let trimmed = line.text.trim_start();
            if let Some(info) = trimmed.strip_prefix("```") {
                match open.take() {
                    None => {
                        let lead = line.text.len() - trimmed.len();
                        let start = line.start + lead;
                        open = Some((
                            classify(info),
                            start..line.start + line.text.len(),
                            Vec::new(),
                        ));
                    }
                    Some((Fence::Checked, fence, body)) => out.push(Example { fence, body }),
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

/// The synthetic suites for a project, and the means to report their
/// diagnostics at the fences.
#[derive(Default)]
pub struct DocExamples {
    files: Vec<(PathBuf, String)>,
    tables: Vec<Table>,
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
            let (target, target_is_commons) = match unit {
                Some(SourceUnit::Commons(c)) if errors.is_empty() => (c.name.joined(), true),
                Some(SourceUnit::Context(c)) if errors.is_empty() => (c.name.joined(), false),
                _ => continue,
            };
            let examples = examples(source);
            if examples.is_empty() {
                continue;
            }
            found.count += examples.len();
            let made = synthesise(&target, target_is_commons, key, source, &examples);
            found.files.extend(made.files);
            found.tables.extend(made.tables);
        }
        found
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The number of examples that will be checked.
    pub fn example_count(&self) -> usize {
        self.count
    }

    /// `sources` plus the synthetic suites.
    pub fn overlay(&self, sources: &HashMap<PathBuf, String>) -> HashMap<PathBuf, String> {
        let mut out = sources.clone();
        out.extend(self.files.iter().cloned());
        out
    }

    /// Keep only the errors a synthetic suite produced, mapped to the fence in
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

impl Table {
    fn map_range(&self, start: usize, end: usize) -> (usize, usize) {
        let Some(seg) = self
            .segments
            .iter()
            .find(|s| s.synth.contains(&start) || (start == s.synth.end && s.exact))
        else {
            let fallback = self.segments.first().map_or(0..0, |s| s.real.clone());
            return (fallback.start, fallback.end);
        };
        if !seg.exact {
            return (seg.real.start, seg.real.end);
        }
        let from = seg.real.start + (start - seg.synth.start);
        let to = seg.real.start + (end.min(seg.synth.end).max(start) - seg.synth.start);
        (from, to.max(from))
    }
}

/// A piece of a synthetic file and where it maps in the real file.
struct Segment {
    synth: Range<usize>,
    real: Range<usize>,
    /// Offsets inside map one-to-one (an example's body line). Otherwise the
    /// piece (the generated header) maps to the whole fence line.
    exact: bool,
}

/// One synthetic file and the map back to the real file it came from.
struct Table {
    /// The synthetic file's source-map key.
    synth_key: PathBuf,
    segments: Vec<Segment>,
}

/// What a fence's body is, found by parsing it.
#[derive(Debug, PartialEq, Eq)]
enum Body {
    /// Statements and expressions: the body of a `case`.
    Statements,
    /// Top-level declarations (`type`, `fn`, `capability`, …).
    Items,
}

/// The middle of the name of a synthetic sibling unit checking a fence of
/// declarations: `<stem>_doc_example_<n>.bynk` beside `<stem>.bynk`.
const ITEMS_INFIX: &str = "_doc_example_";

/// The real file name a synthetic file's name stands for.
fn real_name_of(synthetic: &str) -> Option<String> {
    if let Some(stem) = synthetic.strip_suffix(SYNTHETIC_SUFFIX) {
        return Some(format!("{stem}.bynk"));
    }
    let stem = synthetic.strip_suffix(".bynk")?;
    let (real, n) = stem.rsplit_once(ITEMS_INFIX)?;
    (!n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).then(|| format!("{real}.bynk"))
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

fn body_text(ex: &Example<'_>) -> String {
    ex.body
        .iter()
        .map(|l| l.text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn push_body(text: &mut String, segments: &mut Vec<Segment>, ex: &Example<'_>) {
    for l in &ex.body {
        let at = text.len();
        text.push_str(l.text);
        segments.push(Segment {
            synth: at..text.len(),
            real: l.start..l.start + l.text.len(),
            exact: true,
        });
        text.push('\n');
    }
}

/// The synthetic files for the examples of one unit, each with its map back.
struct Synthesised {
    files: Vec<(PathBuf, String)>,
    tables: Vec<Table>,
}

/// The statement examples together become one `suite` on `target`. Each
/// example of declarations becomes a sibling `commons` of its own, which
/// `uses` `target` when that is itself a commons (a `uses` names a commons).
fn synthesise(
    target: &str,
    target_is_commons: bool,
    key: &Path,
    source: &str,
    examples: &[Example<'_>],
) -> Synthesised {
    let file = key.file_name().unwrap_or_default().to_string_lossy();
    let stem = key.file_stem().unwrap_or_default().to_string_lossy();
    let mut out = Synthesised {
        files: Vec::new(),
        tables: Vec::new(),
    };
    let mut suite = format!("suite {target} {{\n");
    let mut suite_segments = Vec::new();
    let mut items_n = 0;
    for ex in examples {
        match classify_body(target, &body_text(ex)) {
            Body::Statements => {
                let line = source[..ex.fence.start].matches('\n').count() + 1;
                let header = format!("  case \"doc example {file}:{line}\" {{\n");
                suite_segments.push(Segment {
                    synth: suite.len()..suite.len() + header.len(),
                    real: ex.fence.clone(),
                    exact: false,
                });
                suite.push_str(&header);
                push_body(&mut suite, &mut suite_segments, ex);
                // The closing brace maps to the fence too, so a parse error
                // that stops there (an unfinished example) finds its example.
                suite_segments.push(Segment {
                    synth: suite.len()..suite.len() + 4,
                    real: ex.fence.clone(),
                    exact: false,
                });
                suite.push_str("  }\n");
            }
            Body::Items => {
                items_n += 1;
                let uses = if target_is_commons {
                    format!("uses {target}\n\n")
                } else {
                    String::new()
                };
                let mut text = format!("commons {target}{ITEMS_INFIX}{items_n}\n\n{uses}");
                let mut segments = vec![Segment {
                    synth: 0..text.len(),
                    real: ex.fence.clone(),
                    exact: false,
                }];
                push_body(&mut text, &mut segments, ex);
                let synth_key = key.with_file_name(format!("{stem}{ITEMS_INFIX}{items_n}.bynk"));
                out.files.push((synth_key.clone(), text));
                out.tables.push(Table {
                    synth_key,
                    segments,
                });
            }
        }
    }
    if !suite_segments.is_empty() {
        suite.push_str("}\n");
        let synth_key = key.with_file_name(format!("{stem}{SYNTHETIC_SUFFIX}"));
        out.files.push((synth_key.clone(), suite));
        out.tables.push(Table {
            synth_key,
            segments: suite_segments,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_info_strings() {
        assert_eq!(classify("bynk"), Fence::Checked);
        assert_eq!(classify("bynk test"), Fence::Checked);
        assert_eq!(classify(" bynk "), Fence::Checked);
        assert_eq!(classify("bynk ignore"), Fence::Skipped);
        assert_eq!(classify("bynk,ignore"), Fence::Skipped);
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
    fn finds_block_and_line_doc_examples() {
        let src = "---\nSums.\n\n```bynk\nexpect add(1, 2) == 3\n```\n\n```bynk ignore\nnope\n```\n---\ncommons m\n\n---\n```bynk\nexpect true\n```\n---\nfn add(a: Int, b: Int) -> Int { a + b }\n";
        let found = DocExamples::collect(&sources(src));
        assert_eq!(found.example_count(), 2);
        let (_, text) = &found.files[0];
        assert!(text.starts_with("suite m {\n"), "{text}");
        assert!(text.contains("expect add(1, 2) == 3\n"), "{text}");
        assert!(text.contains("expect true\n"), "{text}");
        assert!(!text.contains("nope"), "{text}");
    }

    #[test]
    fn maps_a_body_offset_back_to_the_fence() {
        let src = "---\n```bynk\nexpect 1 == 1\n```\n---\ncommons m\n";
        let found = DocExamples::collect(&sources(src));
        let (_, text) = &found.files[0];
        let synth = text.find("expect").unwrap();
        let table = &found.tables[0];
        let (start, end) = table.map_range(synth + 7, synth + 8);
        assert_eq!(&src[start..end], "1");
        // The generated header maps to the opening fence line.
        let (hs, he) = table.map_range(text.find("case").unwrap(), text.find("case").unwrap() + 4);
        assert_eq!(&src[hs..he], "```bynk");
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
        assert_eq!(found.files.len(), 1);
        let (key, text) = &found.files[0];
        assert!(key.ends_with("m_doc_example_1.bynk"), "{key:?}");
        assert!(
            text.starts_with("commons m_doc_example_1\n\nuses m\n"),
            "{text}"
        );
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
}
