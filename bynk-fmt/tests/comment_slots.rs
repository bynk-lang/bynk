//! #1834: a comment at every slot of every fixture.
//!
//! For every `.bynk` file of the positive corpus, and for each line, a comment
//! is inserted in two ways:
//!
//! - **trailing:** ` // c<N>` at the end of the line (skipped on a line that
//!   already has a `//`);
//! - **own line:** a `// c<N>` line before it, at the same indentation;
//! - **split:** after each `{` or `,` token on the line, a line break, the
//!   comment, and a line break. This reaches the slots inside a one-line body
//!   or list (`actor A { auth = …, identity = … }`, `exports { A, B }`,
//!   `enum { A, B }`), where #1786, #1788, #1794 and #1797 lived;
//! - **own line, blank after:** a `// c<N>` line and an empty line before a
//!   unit-level line (one starting with a keyword or a `---` fence at column 0). The
//!   blank line keeps the comment apart from the declaration below, so it must
//!   survive too (#1884).
//!
//! An insertion that no longer parses, or that lands inside a string or a doc
//! block (where it is text, not a comment), is skipped. Each remaining one must
//! end in exactly one of two outcomes:
//!
//! - **kept:** the output has the comment exactly once, re-parses, is
//!   idempotent, and differs from the formatted original only by the comment
//!   and the layout a comment may legitimately cause (see [`skeleton`] and
//!   [`strip_comment`]);
//! - **refused:** the formatter refuses with `bynk.fmt.comment_loss` and
//!   nothing else.
//!
//! Anything else (a dropped or duplicated comment, unparseable or
//! non-idempotent output, another diagnostic, or output perturbed beyond the
//! comment) fails, unless a [`KNOWN`] rule names it with its issue.
//!
//! **Refusals are counted, not listed.** Trivia attach at declaration and
//! statement granularity (#523), so a comment inside an expression (a match
//! arm, a call's arguments, a message entry) is refused by design: about a
//! quarter of all placements. The count is pinned per fixture and per insertion
//! kind, strictly, in one file per fixture under [`PIN_DIR`]
//! (`<fixture>.refused`: trailing, own line, split, own line with a blank after).
//! A fixture with no refusals has no file. More refusals than pinned is a
//! regression; fewer means the formatter learned a placement, and the pin must
//! come down so the gain cannot quietly reverse.
//!
//! The pins are per fixture so that adding a fixture adds its own pin file
//! instead of editing a count every other open change also edits.
//! `BYNK_BLESS=1 cargo test -p bynk-fmt --test comment_slots` rewrites the pin
//! files from the current run: after adding a fixture, or after a formatter
//! change, with the moved counts reviewed in the diff.
//!
//! **Sampling.** Checking every candidate takes well over half a minute in a
//! debug build, so by default
//! about one in four is checked, chosen by a stable hash of (file, line, kind).
//! The pins are for that sample. `BYNK_FMT_EXHAUSTIVE=1` checks every
//! candidate and reports the full counts instead of asserting the pins.
//! Candidates a [`KNOWN`] rule matches are always checked.
//!
//! **Known defects** are strict, the `behaviour_fixtures.rs` discipline: a rule
//! that matches no failure means its issue is fixed, and the test says to
//! delete the rule.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use bynk_fmt::{FormatOptions, format_source};
use bynk_syntax::lexer::{TokenKind, tokenize};
use bynk_syntax::parser::parse_units;

/// Refusals in the default sample, per insertion kind: `(trailing, own line,
/// split, own line with a blank after)`.
type Refused = (usize, usize, usize, usize);

/// Where the per-fixture refusal pins live, relative to this crate.
const PIN_DIR: &str = "tests/comment_slots";

/// The fixture a corpus file belongs to: its first path component (a positive
/// fixture's directory), or a repro's own name.
fn fixture_of(file: &str) -> &str {
    file.split('/').next().unwrap_or(file)
}

fn pin_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(PIN_DIR)
}

fn read_pins() -> BTreeMap<String, Refused> {
    let mut pins = BTreeMap::new();
    let Ok(entries) = fs::read_dir(pin_dir()) else {
        return pins;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let Some(name) = p
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".refused"))
        else {
            continue;
        };
        let text = fs::read_to_string(&p).expect("read pin file");
        let n: Vec<usize> = text
            .split_whitespace()
            .map(|t| t.parse().expect("pin file holds four counts"))
            .collect();
        assert_eq!(n.len(), 4, "{} must hold four counts", p.display());
        pins.insert(name.to_string(), (n[0], n[1], n[2], n[3]));
    }
    pins
}

/// Rewrites the pin directory to exactly `actual`: one file per fixture with
/// any refusal, and no file for the rest.
fn bless_pins(actual: &BTreeMap<String, Refused>) {
    let dir = pin_dir();
    fs::create_dir_all(&dir).expect("create pin dir");
    for name in read_pins().keys() {
        if !actual.contains_key(name) {
            fs::remove_file(dir.join(format!("{name}.refused"))).expect("remove stale pin");
        }
    }
    for (name, (t, o, s, b)) in actual {
        fs::write(
            dir.join(format!("{name}.refused")),
            format!("{t} {o} {s} {b}\n"),
        )
        .expect("write pin");
    }
}

/// Where a comment was inserted: the line it went on or before, the line
/// above that, and the construct a closing line ends (the nearest line above
/// at the same indentation; the line itself otherwise). With no defect open,
/// [`KNOWN`] is empty and no rule reads a field: the last rules read `line`
/// (#1808), `opener` (#1859) and `prev` (#1858).
#[allow(dead_code)]
struct Site<'a> {
    line: &'a str,
    prev: &'a str,
    opener: &'a str,
}

/// A known defect: `(issue, outcome, kind, the sites it covers)`, described by
/// the shape that triggers it rather than by file, so it covers every instance.
type Rule = (u32, Outcome, Kind, fn(&Site) -> bool);

const KNOWN: &[Rule] = &[];

/// Minimal programs for defects whose shape the corpus lacks.
const REPROS: &[(&str, &str)] = &[];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Trailing,
    OwnLine,
    /// After the `n`th `{`/`,` token of the line.
    Split(usize),
    OwnLineBlank,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Kept,
    Refused,
    Dropped,
    Duplicated,
    Unparseable,
    NotIdempotent,
    Perturbed,
    OtherError,
}

fn corpus() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("bynkc/tests/fixtures/positive");
    let mut files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read fixture dir").flatten() {
            let p = entry.path();
            if p.is_dir() {
                if !p.ends_with("expected") {
                    stack.push(p);
                }
            } else if p.extension().is_some_and(|e| e == "bynk") {
                files.push(p);
            }
        }
    }
    files.sort();
    let mut out: Vec<(String, String)> = files
        .into_iter()
        .map(|p| {
            let rel = p
                .strip_prefix(&root)
                .unwrap()
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            (rel, fs::read_to_string(&p).expect("read fixture"))
        })
        .collect();
    out.extend(REPROS.iter().map(|(n, s)| (n.to_string(), s.to_string())));
    out
}

fn parses(s: &str) -> bool {
    tokenize(s).is_ok_and(|t| parse_units(&t, s).is_ok())
}

/// Token texts, with a marker (and count) wherever blank lines separate two
/// tokens: the program and its vertical spacing, but not its line breaks. Commas and empty
/// `()` are dropped: a list a comment forces onto several lines gains a
/// trailing comma, and a unit tail the formatter would omit is kept to carry a
/// comment.
fn skeleton(s: &str) -> Vec<String> {
    let tokens = tokenize(s).expect("formatted output tokenizes");
    let mut out = Vec::new();
    let mut prev_end = 0;
    for t in &tokens {
        let newlines = s[prev_end..t.span.start].matches('\n').count();
        if newlines >= 2 {
            out.push(format!("<{} blank>", newlines - 1));
        }
        out.push(s[t.span.start..t.span.end].to_string());
        prev_end = t.span.end;
    }
    out.retain(|t| t != ",");
    let mut i = 0;
    while i + 1 < out.len() {
        if out[i] == "(" && out[i + 1] == ")" {
            out.drain(i..i + 2);
        } else {
            i += 1;
        }
    }
    out
}

/// `out` without the inserted `-- <tag>` comment. A line the comment leaves
/// empty goes too (a brace-line comment moves onto its own line, #1788), and
/// so does the blank line the formatter puts before a body's or file's
/// trailing comments ("one blank line before trailing comments if anything
/// came before them"). With `blank_after`, the blank line inserted after the
/// comment goes as well.
fn strip_comment(out: &str, tag: &str, blank_after: bool) -> String {
    let marker = format!("// {tag}");
    let lines: Vec<&str> = out.lines().collect();
    let mut kept: Vec<String> = Vec::new();
    let mut skip = false;
    for (i, line) in lines.iter().enumerate() {
        if std::mem::take(&mut skip) {
            continue;
        }
        match line.find(&marker) {
            Some(j) if line[..j].trim().is_empty() => {
                skip = blank_after;
                let closes = lines
                    .get(i + 1)
                    .is_none_or(|next| next.trim().starts_with('}'));
                if closes && kept.last().is_some_and(|k| k.trim().is_empty()) {
                    kept.pop();
                }
            }
            Some(j) => kept.push(line[..j].trim_end().to_string()),
            None => kept.push(line.to_string()),
        }
    }
    kept.into_iter().map(|l| l + "\n").collect()
}

fn classify(mutated: &str, tag: &str, kind: Kind, base: &str, opts: &FormatOptions) -> Outcome {
    let out = match format_source(mutated, opts) {
        Ok(out) => out,
        Err(e)
            if e.errors
                .iter()
                .all(|x| x.category == "bynk.fmt.comment_loss") =>
        {
            return Outcome::Refused;
        }
        Err(_) => return Outcome::OtherError,
    };
    match out.matches(&format!("// {tag}")).count() {
        0 => return Outcome::Dropped,
        1 => {}
        _ => return Outcome::Duplicated,
    }
    if !parses(&out) {
        return Outcome::Unparseable;
    }
    if format_source(&out, opts).ok().as_deref() != Some(out.as_str()) {
        return Outcome::NotIdempotent;
    }
    // The blank line after the comment is checked here: a lost one would strip
    // back to the base skeleton all the same.
    let blank_after = kind == Kind::OwnLineBlank;
    if blank_after && !out.contains(&format!("// {tag}\n\n")) {
        return Outcome::Perturbed;
    }
    if skeleton(&strip_comment(&out, tag, blank_after)) != skeleton(base) {
        return Outcome::Perturbed;
    }
    Outcome::Kept
}

/// A stable FNV-1a hash, so the sample is the same on every platform and run.
fn sampled(file: &str, line: usize, kind: Kind) -> bool {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in format!("{file}:{line}:{kind:?}").bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    h.is_multiple_of(4)
}

fn known_rule(outcome: Outcome, kind: Kind, site: &Site) -> Option<usize> {
    KNOWN
        .iter()
        .position(|&(_, o, k, covers)| o == outcome && k == kind && covers(site))
}

/// Whether some rule could match a candidate, whatever its outcome turns out
/// to be: those are checked even outside the sample.
fn known_candidate(kind: Kind, site: &Site) -> bool {
    KNOWN
        .iter()
        .any(|&(_, _, k, covers)| k == kind && covers(site))
}

#[test]
fn a_comment_at_every_slot_is_kept_or_refused() {
    let exhaustive = std::env::var_os("BYNK_FMT_EXHAUSTIVE").is_some();
    let opts = FormatOptions::default();
    let mut counts: BTreeMap<(Outcome, Kind), usize> = BTreeMap::new();
    let mut refused_by_fixture: BTreeMap<String, Refused> = BTreeMap::new();
    let mut matched = vec![0usize; KNOWN.len()];
    let mut failures: Vec<String> = Vec::new();

    for (file, src) in corpus() {
        let Ok(base) = format_source(&src, &opts) else {
            continue;
        };
        let tokens = tokenize(&src).expect("fixture tokenizes");
        let separators: Vec<usize> = tokens
            .iter()
            .filter(|t| matches!(t.kind, TokenKind::LBrace | TokenKind::Comma))
            .map(|t| t.span.end)
            .collect();
        let opaque: Vec<(usize, usize)> = tokens
            .iter()
            .filter(|t| {
                matches!(
                    t.kind,
                    TokenKind::StrLit
                        | TokenKind::InterpStr
                        | TokenKind::DocBlock
                        | TokenKind::DocLine
                )
            })
            .map(|t| (t.span.start, t.span.end))
            .collect();
        let lines: Vec<&str> = src.lines().collect();
        let mut line_start = 0;
        for (i, line) in lines.iter().enumerate() {
            let this_start = line_start;
            line_start += line.len() + 1;
            if line.trim().is_empty() {
                continue;
            }
            let indent = line.len() - line.trim_start().len();
            let opener = if line.trim_start().starts_with('}') {
                lines[..i]
                    .iter()
                    .rev()
                    .find(|l| !l.trim().is_empty() && l.len() - l.trim_start().len() == indent)
                    .copied()
                    .unwrap_or(line)
            } else {
                line
            };
            let site = Site {
                line,
                prev: if i > 0 { lines[i - 1] } else { "" },
                opener,
            };
            let line_end = this_start + line.len();
            let splits: Vec<usize> = separators
                .iter()
                .copied()
                .filter(|&o| {
                    this_start < o && o < line_end && !line[o - this_start..].trim().is_empty()
                })
                .collect();
            // A `---` fence at column 0, not a one-line `--- text` doc (#2203),
            // which may document a pipe-form variant there.
            let unit_level = line.starts_with(|c: char| c.is_ascii_alphabetic())
                || (line.starts_with("---") && line.trim_end().bytes().all(|b| b == b'-'));
            let kinds = [Kind::Trailing, Kind::OwnLine]
                .into_iter()
                .chain((0..splits.len()).map(Kind::Split))
                .chain(unit_level.then_some(Kind::OwnLineBlank));
            for kind in kinds {
                if !exhaustive && !sampled(&file, i + 1, kind) && !known_candidate(kind, &site) {
                    continue;
                }
                let tag = match kind {
                    Kind::Trailing => format!("c{}t", i + 1),
                    Kind::OwnLine => format!("c{}o", i + 1),
                    Kind::Split(n) => format!("c{}s{n}", i + 1),
                    Kind::OwnLineBlank => format!("c{}b", i + 1),
                };
                let mut mutated: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
                let offset = match kind {
                    Kind::Trailing => {
                        // A line with a comment already; a `//` in a string
                        // (a URL) is not one.
                        if line
                            .match_indices("//")
                            .any(|(at, _)| line[..at].matches('"').count() % 2 == 0)
                        {
                            continue;
                        }
                        mutated[i] = format!("{line} // {tag}");
                        this_start + line.len()
                    }
                    Kind::OwnLine => {
                        let indent: String =
                            line.chars().take_while(|c| c.is_whitespace()).collect();
                        mutated.insert(i, format!("{indent}// {tag}"));
                        this_start
                    }
                    Kind::Split(n) => {
                        let at = splits[n] - this_start;
                        mutated[i] = format!("{}\n// {tag}\n{}", &line[..at], &line[at..]);
                        splits[n]
                    }
                    Kind::OwnLineBlank => {
                        mutated.insert(i, format!("// {tag}\n"));
                        this_start
                    }
                };
                if opaque.iter().any(|&(a, b)| a < offset && offset < b) {
                    continue;
                }
                let mutated = mutated.join("\n") + "\n";
                if !parses(&mutated) {
                    continue;
                }
                let outcome = classify(&mutated, &tag, kind, &base, &opts);
                *counts.entry((outcome, kind)).or_default() += 1;
                if outcome == Outcome::Refused {
                    let r = refused_by_fixture
                        .entry(fixture_of(&file).to_string())
                        .or_default();
                    match kind {
                        Kind::Trailing => r.0 += 1,
                        Kind::OwnLine => r.1 += 1,
                        Kind::Split(_) => r.2 += 1,
                        Kind::OwnLineBlank => r.3 += 1,
                    }
                }
                if matches!(outcome, Outcome::Kept | Outcome::Refused) {
                    continue;
                }
                match known_rule(outcome, kind, &site) {
                    Some(r) => matched[r] += 1,
                    None => failures.push(format!(
                        "  {outcome:?} [{kind:?}] {file}:{}: {}",
                        i + 1,
                        line.trim()
                    )),
                }
            }
        }
    }

    let fixed: Vec<String> = KNOWN
        .iter()
        .zip(&matched)
        .filter(|(_, n)| **n == 0)
        .map(|((issue, outcome, kind, _), _)| format!("  #{issue}: {outcome:?} [{kind:?}]"))
        .collect();
    let summary = format!("{counts:?}");
    assert!(
        failures.is_empty() && fixed.is_empty(),
        "{summary}\n\
         Comments the formatter dropped, duplicated or mangled (fix, or add a KNOWN rule with its issue):\n{}\n\
         KNOWN rules that matched nothing (the issue is fixed: delete the rule):\n{}",
        failures.join("\n"),
        fixed.join("\n"),
    );
    if exhaustive {
        eprintln!("exhaustive run: {summary}");
        return;
    }
    if std::env::var_os("BYNK_BLESS").is_some() {
        bless_pins(&refused_by_fixture);
        eprintln!("blessed {} comment-slot pins", refused_by_fixture.len());
        return;
    }
    let pins = read_pins();
    let names: std::collections::BTreeSet<&String> =
        pins.keys().chain(refused_by_fixture.keys()).collect();
    let moved: Vec<String> = names
        .into_iter()
        .filter_map(|name| {
            let pinned = pins.get(name).copied().unwrap_or_default();
            let now = refused_by_fixture.get(name).copied().unwrap_or_default();
            (pinned != now).then(|| format!("  {name}: pinned {pinned:?}, now {now:?}"))
        })
        .collect();
    assert!(
        moved.is_empty(),
        "comment-slot refusals (trailing, own line, split, own line with a blank after) moved \
         from their per-fixture pins in `bynk-fmt/{PIN_DIR}`:\n{}\n\
         A count above its pin: the formatter now refuses a comment somewhere it kept one, which \
         is a regression.\n\
         A count below its pin: it learned a placement; lower the pin so the gain cannot quietly \
         reverse.\n\
         A new or edited fixture moves its own pin: rewrite the pins with \
         `BYNK_BLESS=1 cargo test -p bynk-fmt --test comment_slots` and review the diff.\n{summary}",
        moved.join("\n"),
    );
}
