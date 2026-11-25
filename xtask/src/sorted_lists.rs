//! The alphabetical-order check for marked reference lists (#4206).
//!
//! The long reference lists are meant to stay in alphabetical order, but nothing enforced it: #4202
//! had to reorder `reference/types.md` and `reference/glossary.md` by hand because additions had gone
//! at the end or beside related entries. A prose rule depends on everyone remembering it.
//!
//! A list opts in with a marker, so the check never guesses which lists are meant to be ordered:
//!
//! ```text
//! <!-- sorted: table -->      the region holds one Markdown table; its rows are ordered
//! <!-- sorted: headings -->   the region holds sections; its top-level headings are ordered
//! <!-- /sorted -->            ends the region
//! ```
//!
//! Regions nest: a section of a `headings` region can hold its own `headings` or `table` region. An
//! unmarked list is ignored. The check reports; it never rewrites.
//!
//! **The key.** A table row's key is its first cell's first code span, or its plain text when it
//! has none. A heading's key is its text without a trailing `{#anchor}`. Both drop backticks,
//! emphasis and link syntax, then leading symbols, and a leading `The `, and compare
//! case-insensitively, with a case-sensitive tie-break.

use std::fs;
use std::path::{Path, PathBuf};

/// What a marked region orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Table,
    Headings,
}

/// One marked region: where it opens and closes (0-based line indexes of the markers).
#[derive(Debug)]
struct Region {
    kind: Kind,
    open: usize,
    close: usize,
}

/// `<!-- sorted: KIND -->` as `Some(Ok(kind))`, an unknown kind as `Some(Err(word))`, anything else
/// as `None`.
fn open_marker(line: &str) -> Option<Result<Kind, String>> {
    let inner = line
        .trim()
        .strip_prefix("<!--")?
        .strip_suffix("-->")?
        .trim()
        .strip_prefix("sorted:")?
        .trim();
    Some(match inner {
        "table" => Ok(Kind::Table),
        "headings" => Ok(Kind::Headings),
        other => Err(other.to_string()),
    })
}

fn is_close_marker(line: &str) -> bool {
    line.trim() == "<!-- /sorted -->"
}

fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

/// The regions of a page, and each line's innermost region (an index into the regions).
type Regions = (Vec<Region>, Vec<Option<usize>>);

/// Each line's innermost region (an index into the returned regions), and the regions, or the
/// structural problems found: an unknown kind, an unclosed region, a stray close.
fn regions(file: &str, lines: &[&str]) -> Result<Regions, Vec<String>> {
    let mut regions: Vec<Region> = Vec::new();
    let mut innermost: Vec<Option<usize>> = vec![None; lines.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut errors = Vec::new();
    let mut fenced = false;
    for (i, line) in lines.iter().enumerate() {
        if is_fence(line) {
            fenced = !fenced;
        }
        if !fenced {
            match open_marker(line) {
                Some(Ok(kind)) => {
                    regions.push(Region {
                        kind,
                        open: i,
                        close: i,
                    });
                    stack.push(regions.len() - 1);
                    continue;
                }
                Some(Err(word)) => {
                    errors.push(format!(
                        "{file}:{}: `<!-- sorted: {word} -->` is not a region kind (use `table` or `headings`)",
                        i + 1
                    ));
                    continue;
                }
                None => {}
            }
            if is_close_marker(line) {
                match stack.pop() {
                    Some(r) => regions[r].close = i,
                    None => errors.push(format!(
                        "{file}:{}: `<!-- /sorted -->` closes no region",
                        i + 1
                    )),
                }
                continue;
            }
        }
        innermost[i] = stack.last().copied();
    }
    for r in stack {
        errors.push(format!(
            "{file}:{}: this `<!-- sorted -->` region is never closed",
            regions[r].open + 1
        ));
    }
    if errors.is_empty() {
        Ok((regions, innermost))
    } else {
        Err(errors)
    }
}

/// The ordering key of `raw`: see the module docs. `heading` selects the heading rules, which take
/// the whole text rather than the first code span.
fn sort_key(raw: &str, heading: bool) -> (String, String) {
    let mut text = raw.trim().to_string();
    if heading {
        if let Some(at) = text.rfind("{#")
            && text.ends_with('}')
        {
            text.truncate(at);
        }
    } else if let Some(start) = text.find('`')
        && let Some(len) = text[start + 1..].find('`')
    {
        text = text[start + 1..start + 1 + len].to_string();
    }
    // `[text](url)` reads as `text`.
    while let Some(open) = text.find("](") {
        let (Some(label_at), Some(close)) = (text[..open].rfind('['), text[open..].find(')'))
        else {
            break;
        };
        let label = text[label_at + 1..open].to_string();
        text = format!("{}{label}{}", &text[..label_at], &text[open + close + 1..]);
    }
    let plain: String = text
        .chars()
        .filter(|c| *c != '`' && *c != '*')
        .collect::<String>()
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .trim()
        .to_string();
    let lower = plain.to_lowercase();
    let primary = lower.strip_prefix("the ").unwrap_or(&lower).to_string();
    (primary, plain)
}

/// One entry to order: where it is, what it says, its key.
struct Entry {
    line: usize,
    shown: String,
    key: (String, String),
}

/// The entries of `region`, from its own lines only (not a nested region's).
fn entries(
    region: &Region,
    index: usize,
    lines: &[&str],
    innermost: &[Option<usize>],
) -> Vec<Entry> {
    let own: Vec<usize> = (region.open + 1..region.close)
        .filter(|i| innermost[*i] == Some(index))
        .collect();
    match region.kind {
        Kind::Table => {
            let rows: Vec<usize> = own
                .iter()
                .copied()
                .filter(|i| lines[*i].trim_start().starts_with('|'))
                .collect();
            // The header row and its separator are not entries.
            rows.into_iter()
                .skip(2)
                .map(|i| {
                    let cell = first_cell(lines[i]);
                    Entry {
                        line: i + 1,
                        shown: sort_key(cell, false).1,
                        key: sort_key(cell, false),
                    }
                })
                .collect()
        }
        Kind::Headings => {
            let heads: Vec<(usize, usize, &str)> = own
                .iter()
                .filter_map(|i| heading(lines[*i]).map(|(level, text)| (*i, level, text)))
                .collect();
            let Some(top) = heads.iter().map(|(_, level, _)| *level).min() else {
                return Vec::new();
            };
            heads
                .into_iter()
                .filter(|(_, level, _)| *level == top)
                .map(|(i, _, text)| Entry {
                    line: i + 1,
                    shown: sort_key(text, true).1,
                    key: sort_key(text, true),
                })
                .collect()
        }
    }
}

/// A table row's first cell, up to its closing pipe (an escaped `\|` stays in the cell).
fn first_cell(row: &str) -> &str {
    let body = row.trim_start().trim_start_matches('|');
    let mut prev = ' ';
    for (at, c) in body.char_indices() {
        if c == '|' && prev != '\\' {
            return &body[..at];
        }
        prev = c;
    }
    body
}

/// An ATX heading as `(level, text)`.
fn heading(line: &str) -> Option<(usize, &str)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    (1..=6)
        .contains(&hashes)
        .then(|| line[hashes..].strip_prefix(' ').map(|t| (hashes, t)))
        .flatten()
}

/// The ordering problems in `text`, one line each, naming the entry that is out of place and the
/// entry it should follow. `file` only labels them.
pub fn check_text(file: &str, text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let (regions, innermost) = match regions(file, &lines) {
        Ok(r) => r,
        Err(errors) => return errors,
    };
    let mut problems = Vec::new();
    for (index, region) in regions.iter().enumerate() {
        let found = entries(region, index, &lines, &innermost);
        let mut sorted: Vec<&Entry> = found.iter().collect();
        sorted.sort_by(|a, b| a.key.cmp(&b.key));
        for pair in found.windows(2) {
            let (before, after) = (&pair[0], &pair[1]);
            if after.key >= before.key {
                continue;
            }
            // Where the later entry belongs: after the last entry that sorts before it.
            let follows = sorted
                .iter()
                .rev()
                .find(|e| e.key < after.key)
                .map(|e| format!("it should follow `{}`", e.shown))
                .unwrap_or_else(|| "it should come first".to_string());
            problems.push(format!(
                "{file}:{}: `{}` is out of order: {follows} (it comes after `{}`)",
                after.line, after.shown, before.shown
            ));
        }
    }
    problems
}

/// Every Markdown file under `site/src/content/docs`, sorted.
fn markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![root.join("site/src/content/docs")];
    while let Some(dir) = dirs.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "md" || e == "mdx") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The ordering problems across every marked list in the Book and the Reference.
pub fn check_site(root: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    for path in markdown_files(root) {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        // Cheap skip: most pages carry no marker.
        if !text.contains("<!-- sorted") {
            continue;
        }
        let shown = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        problems.extend(check_text(&shown, &text));
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(rows: &[&str]) -> String {
        let mut out = String::from("<!-- sorted: table -->\n\n| Type | Notes |\n|---|---|\n");
        for r in rows {
            out.push_str(&format!("| {r} | x |\n"));
        }
        out.push_str("\n<!-- /sorted -->\n");
        out
    }

    #[test]
    fn an_in_order_table_passes() {
        assert!(check_text("t.md", &table(&["`Bool`", "`Bytes`", "`Decimal`"])).is_empty());
    }

    #[test]
    fn a_misplaced_row_names_itself_and_where_it_belongs() {
        let problems = check_text("t.md", &table(&["`Bool`", "`Decimal`", "`Bytes`"]));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].starts_with("t.md:7: `Bytes` is out of order: it should follow `Bool`"),
            "{problems:?}"
        );
    }

    #[test]
    fn a_row_that_belongs_first_says_so() {
        let problems = check_text("t.md", &table(&["`Bytes`", "`Bool`"]));
        assert!(problems[0].contains("it should come first"), "{problems:?}");
    }

    const SECTIONS: &str = "<!-- sorted: headings -->\n\n## Apple\n\nbody a\n\n### Z inner\n\n## Cherry {#c}\n\nbody c\n\n## Banana\n\nbody b\n\n<!-- /sorted -->\n";

    #[test]
    fn a_misplaced_section_is_reported_by_its_heading() {
        let problems = check_text("p.md", SECTIONS);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].starts_with("p.md:13: `Banana` is out of order: it should follow `Apple`"),
            "{problems:?}"
        );
    }

    #[test]
    fn nested_headings_are_not_the_regions_own_entries() {
        // `### Z inner` is a deeper heading of the section above it, not an entry.
        let ordered = SECTIONS
            .replace("## Cherry {#c}", "## Banana")
            .replace("## Banana\n\nbody b", "## Cherry\n\nbody b");
        assert!(check_text("p.md", &ordered).is_empty());
    }

    #[test]
    fn a_nested_region_is_checked_on_its_own() {
        let text = "<!-- sorted: headings -->\n\n## A\n\n<!-- sorted: headings -->\n\n### Y\n\n### X\n\n<!-- /sorted -->\n\n## B\n\n<!-- /sorted -->\n";
        let problems = check_text("n.md", text);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("`X` is out of order"), "{problems:?}");
    }

    #[test]
    fn an_unmarked_list_is_ignored() {
        assert!(check_text("u.md", "| T |\n|---|\n| `Z` |\n| `A` |\n\n## Z\n\n## A\n").is_empty());
    }

    #[test]
    fn a_marker_inside_a_code_fence_is_not_a_marker() {
        assert!(check_text("c.md", "```\n<!-- sorted: table -->\n```\n").is_empty());
    }

    #[test]
    fn the_key_ignores_case_symbols_backticks_anchors_links_and_a_leading_the() {
        let key = |raw: &str, heading: bool| sort_key(raw, heading).0;
        assert_eq!(key("`Bytes`", false), key("Bytes", false));
        assert_eq!(key("`Bytes`", false), "bytes");
        assert_eq!(key("**Zed**", false), "zed");
        assert_eq!(key("[`Option`](#option)", false), "option");
        assert_eq!(key("`Cap.op` subject", true), "cap.op subject");
        assert_eq!(key("String {#string}", true), "string");
        assert_eq!(key("The JSON codec", true), "json codec");
        // A table takes the first code span; a heading takes the whole text.
        assert_eq!(
            key("`Option` and `Result` methods", true),
            "option and result methods"
        );
        assert_eq!(key("`Option` and `Result`", false), "option");
    }

    #[test]
    fn ties_break_by_case() {
        let (a, b) = (sort_key("alpha", true), sort_key("Alpha", true));
        assert_eq!(a.0, b.0);
        assert!(
            b < a,
            "an upper-case entry sorts before its lower-case twin"
        );
        assert!(check_text("t.md", &table(&["`Alpha`", "`alpha`"])).is_empty());
        assert_eq!(check_text("t.md", &table(&["`alpha`", "`Alpha`"])).len(), 1);
    }

    #[test]
    fn a_bad_region_kind_an_unclosed_region_and_a_stray_close_are_reported() {
        assert!(check_text("k.md", "<!-- sorted: rows -->\n")[0].contains("not a region kind"));
        assert!(check_text("k.md", "<!-- sorted: table -->\n")[0].contains("never closed"));
        assert!(check_text("k.md", "<!-- /sorted -->\n")[0].contains("closes no region"));
    }
}
