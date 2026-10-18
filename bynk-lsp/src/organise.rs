//! #2100 slice 5: the `source.organizeImports` code action — organise a unit
//! header's `uses` and `consumes` clauses.
//!
//! The action sorts the `uses` clauses by target, then the `consumes` clauses
//! by target (the order `bynk-fmt` prints them in: every `uses` before every
//! `consumes`), drops exact duplicates, sorts and deduplicates the names in a
//! braced `consumes U { … }` selection, and drops what the unit never uses:
//!
//! - a `uses U` or whole-unit `consumes U` (or `consumes U as A`) whose target
//!   no reference in the unit resolves into;
//! - a name in a braced selection that no reference in the unit names, and the
//!   whole braced clause once every name in it is gone.
//!
//! "Unused" is read from the committed binding index (ADR 0053), over every
//! file of the unit (a `uses` is unit-wide: the driver merges the clauses of
//! all of a unit's files, and a suite resolves through its target's), not
//! just the one being organised. A reference inside a `uses`/`consumes`
//! clause is not a use — the braced form records one for every name it
//! selects. A target that is not a project unit (the
//! first-party `bynk` surface, a `bynk.list`-style commons) has no index
//! symbols to test against, so a whole-unit clause naming one is never
//! dropped; a braced selection from one is pruned against the index's
//! first-party capability references, matched on their source text.
//!
//! The edit is one replacement of the header's clause run. Each clause moves
//! with its own comments (the comment lines directly above it and a trailing
//! comment on its line); a clause carrying a comment is never dropped. The
//! action declines (offers nothing) when the file doesn't parse cleanly, when
//! the clauses don't form one run (something other than whitespace between
//! two of them), when a clause shares its line with other code, or when the
//! file is `bynk-fmt`-canonical but the organised text would not be. It is
//! also never an empty edit: an already-organised header offers nothing.
//!
//! The pure computation lives here; the `code_action` handler adds DECISION
//! B's safety gate (re-analyse the edited project, offer the edit only if it
//! introduces no diagnostic), falling back from [`Mode::Full`] to
//! [`Mode::SortOnly`] when a drop would break something.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use bynk_check::index::{ProjectIndex, SymbolKind};
use bynk_syntax::ast::{ConsumesDecl, SourceUnit, UsesDecl};
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::parse_unit_with_recovery;
use bynk_syntax::span::Span;
use tower_lsp::lsp_types::*;

/// What the organise pass may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Sort, deduplicate, and drop unused clauses and selected names.
    Full,
    /// Sort and deduplicate only — the fallback when [`Mode::Full`]'s drops
    /// fail the re-check.
    SortOnly,
}

/// The project facts the unused test reads: the committed binding index, the
/// round's snapshots (project-relative path → text), and each unit's source
/// files.
pub struct Project<'a> {
    pub index: &'a ProjectIndex,
    pub snapshots: &'a HashMap<PathBuf, String>,
    pub unit_sources: &'a HashMap<String, Vec<PathBuf>>,
}

/// The organised header for `text` (the snapshot of `rel`), as one edit:
/// `(span, replacement)`. `None` when there is nothing to change or the action
/// declines (see the module docs).
pub fn organise(text: &str, rel: &Path, mode: Mode, project: &Project) -> Option<(Span, String)> {
    let unit = parse_clean(text)?;
    let (unit_name, uses, consumes) = header(&unit)?;
    if uses.is_empty() && consumes.is_empty() {
        return None;
    }
    let usage = match mode {
        Mode::Full => Some(Usage::of(&unit_name, rel, text, project)),
        Mode::SortOnly => None,
    };

    // Every clause's extent: its whole line(s), with the comment lines above.
    let mut clauses: Vec<Clause> = Vec::new();
    for u in uses {
        clauses.push(Clause::new(text, u.span, Kind::Uses(u))?);
    }
    for c in consumes {
        clauses.push(Clause::new(text, c.span, Kind::Consumes(c))?);
    }
    clauses.sort_by_key(|c| c.extent.start);
    for pair in clauses.windows(2) {
        if pair[1].extent.start < pair[0].extent.end
            || !text[pair[0].extent.end..pair[1].extent.start]
                .chars()
                .all(char::is_whitespace)
        {
            return None;
        }
    }
    let region = Span::new(clauses.first()?.extent.start, clauses.last()?.extent.end);

    // Render, prune, sort, dedupe.
    let mut kept: Vec<Rendered> = Vec::new();
    for c in &clauses {
        let rendered = c.render(text, usage.as_ref())?;
        if let Some(r) = rendered {
            kept.push(r);
        }
    }
    kept.sort_by(|a, b| a.key.cmp(&b.key));
    let mut seen: HashSet<(u8, String)> = HashSet::new();
    let mut lines: Vec<String> = Vec::new();
    for r in kept {
        if !seen.insert(r.key.clone()) {
            if r.has_comment {
                return None; // a duplicate carrying a comment: never drop it
            }
            continue;
        }
        lines.push(r.text);
    }

    let (span, replacement) = if lines.is_empty() {
        // Everything went: take the clauses' trailing blank line with them,
        // or (at end of file) the blank line before them.
        let after = &text[region.end..];
        let ws = after.len() - after.trim_start().len();
        if region.end + ws == text.len() {
            let start = text[..region.start].trim_end().len();
            (Span::new(start, text.len()), "\n".to_string())
        } else {
            let end = region.end + after[..ws].rfind('\n').map_or(0, |i| i + 1);
            (Span::new(region.start, end), String::new())
        }
    } else {
        (region, lines.join("\n"))
    };

    let organised = format!(
        "{}{}{}",
        &text[..span.start],
        replacement,
        &text[span.end..]
    );
    if organised == text {
        return None;
    }
    // Formatting is preserved: a canonical file stays canonical.
    let opts = bynk_fmt::FormatOptions::default();
    let canonical = |s: &str| bynk_fmt::format_source(s, &opts).is_ok_and(|f| f == s);
    if canonical(text) && !canonical(&organised) {
        return None;
    }
    Some((span, replacement))
}

/// The versioned `source.organizeImports` action for one organise edit.
pub fn action(
    text: &str,
    edit: (Span, String),
    uri: &Url,
    version: Option<i32>,
) -> CodeActionOrCommand {
    let (span, new_text) = edit;
    CodeActionOrCommand::CodeAction(CodeAction {
        title: "Organise `uses` and `consumes`".to_string(),
        kind: Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS),
        edit: Some(WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version,
                },
                edits: vec![OneOf::Left(TextEdit {
                    range: crate::position::span_to_range(text, span),
                    new_text,
                })],
            }])),
            change_annotations: None,
        }),
        ..Default::default()
    })
}

/// The text with one organise edit applied.
pub fn apply(text: &str, edit: &(Span, String)) -> String {
    format!("{}{}{}", &text[..edit.0.start], edit.1, &text[edit.0.end..])
}

fn parse_clean(text: &str) -> Option<SourceUnit> {
    let tokens = tokenize(text).ok()?;
    match parse_unit_with_recovery(&tokens, text) {
        (Some(unit), errs) if errs.is_empty() => Some(unit),
        _ => None,
    }
}

/// The unit's name and its header clauses. A suite has none to organise.
fn header(unit: &SourceUnit) -> Option<(String, &[UsesDecl], &[ConsumesDecl])> {
    match unit {
        SourceUnit::Commons(c) => Some((c.name.joined(), &c.uses, &[])),
        SourceUnit::Context(c) => Some((c.name.joined(), &c.uses, &c.consumes)),
        SourceUnit::Adapter(a) => Some((a.name.joined(), &a.uses, &a.consumes)),
        SourceUnit::Suite(_) => None,
    }
}

/// The spans of every `uses`/`consumes` clause in `text`'s header.
fn header_spans(text: &str) -> Vec<Span> {
    let Some(unit) = parse_clean(text) else {
        return Vec::new();
    };
    let Some((_, uses, consumes)) = header(&unit) else {
        return Vec::new();
    };
    uses.iter()
        .map(|u| u.span)
        .chain(consumes.iter().map(|c| c.span))
        .collect()
}

/// What the unit's references resolve into, header clauses excluded.
struct Usage {
    /// The project units some reference resolves into.
    units: HashSet<String>,
    /// `(unit, capability)` pairs some reference names (a capability or one
    /// of its operations).
    caps: HashSet<(String, String)>,
    /// Names of first-party capabilities some reference names.
    foreign_caps: HashSet<String>,
    /// The project's units.
    project_units: HashSet<String>,
}

impl Usage {
    fn of(unit_name: &str, rel: &Path, text: &str, project: &Project) -> Self {
        // The unit's files, each with its own header spans to exclude. The
        // file being organised reads from `text` (the caller's snapshot).
        let mut files: Vec<PathBuf> = project
            .unit_sources
            .get(unit_name)
            .cloned()
            .unwrap_or_default();
        if !files.iter().any(|f| f == rel) {
            files.push(rel.to_path_buf());
        }
        let file_text = |f: &Path| -> Option<&str> {
            if f == rel {
                Some(text)
            } else {
                project.snapshots.get(f).map(String::as_str)
            }
        };
        let headers: HashMap<&Path, Vec<Span>> = files
            .iter()
            .map(|f| {
                (
                    f.as_path(),
                    file_text(f).map(header_spans).unwrap_or_default(),
                )
            })
            .collect();
        let counts = |path: &Path, span: Span| {
            headers.get(path).is_some_and(|hs| {
                !hs.iter()
                    .any(|h| h.start <= span.start && span.end <= h.end)
            })
        };

        let mut units = HashSet::new();
        let mut caps = HashSet::new();
        for (key, entry) in &project.index.symbols {
            if !entry.refs.iter().any(|r| counts(&r.path, r.span)) {
                continue;
            }
            units.insert(key.unit.clone());
            if matches!(key.kind, SymbolKind::Capability | SymbolKind::CapabilityOp) {
                let cap = key.name.split('.').next().unwrap_or(&key.name);
                caps.insert((key.unit.clone(), cap.to_string()));
            }
        }
        let mut foreign_caps = HashSet::new();
        for fr in &project.index.foreign_refs {
            if fr.kind == SymbolKind::Capability
                && counts(&fr.site.path, fr.site.span)
                && let Some(name) = file_text(&fr.site.path)
                    .and_then(|t| t.get(fr.site.span.start..fr.site.span.end))
            {
                foreign_caps.insert(name.to_string());
            }
        }
        Usage {
            units,
            caps,
            foreign_caps,
            project_units: project.unit_sources.keys().cloned().collect(),
        }
    }

    /// A whole-unit clause on `target` is used. Never false for a target
    /// outside the project: nothing in the index can show it unused.
    fn unit_used(&self, target: &str) -> bool {
        !self.project_units.contains(target) || self.units.contains(target)
    }

    /// The capability `cap`, selected from `target`, is used.
    fn cap_used(&self, target: &str, cap: &str) -> bool {
        if self.project_units.contains(target) {
            self.caps.contains(&(target.to_string(), cap.to_string()))
        } else {
            self.foreign_caps.contains(cap)
        }
    }
}

enum Kind<'a> {
    Uses(&'a UsesDecl),
    Consumes(&'a ConsumesDecl),
}

/// One header clause and the text that moves with it.
struct Clause<'a> {
    kind: Kind<'a>,
    span: Span,
    /// From the start of its first comment line (or its own line) to the end
    /// of its line, newline excluded.
    extent: Span,
}

impl<'a> Clause<'a> {
    fn new(text: &str, span: Span, kind: Kind<'a>) -> Option<Self> {
        let line_start = text[..span.start].rfind('\n').map_or(0, |i| i + 1);
        if !text[line_start..span.start].trim().is_empty() {
            return None; // shares its line with code before it
        }
        let line_end = text[span.end..]
            .find('\n')
            .map_or(text.len(), |i| span.end + i);
        let rest = text[span.end..line_end].trim();
        if !rest.is_empty() && !rest.starts_with("--") {
            return None; // shares its line with code after it
        }
        // Comment lines directly above (no blank line between) move with it.
        let mut start = line_start;
        while start > 0 {
            let prev_start = text[..start - 1].rfind('\n').map_or(0, |i| i + 1);
            if text[prev_start..start - 1].trim_start().starts_with("--") {
                start = prev_start;
            } else {
                break;
            }
        }
        Some(Clause {
            kind,
            span,
            extent: Span::new(start, line_end),
        })
    }

    /// The clause as it will be printed, or `Ok(None)` when it is dropped.
    /// The outer `None` declines the whole action.
    fn render(&self, text: &str, usage: Option<&Usage>) -> Option<Option<Rendered>> {
        let has_comment = text[self.extent.start..self.extent.end].contains("--");
        let mut clause = text[self.span.start..self.span.end].to_string();
        let key = match self.kind {
            Kind::Uses(u) => {
                let target = u.target.joined();
                if usage.is_some_and(|us| !us.unit_used(&target)) {
                    if has_comment {
                        return None;
                    }
                    return Some(None);
                }
                (0, target)
            }
            Kind::Consumes(c) => {
                let target = c.target.joined();
                match (&c.alias, &c.selected) {
                    (_, Some(selected)) if !selected.is_empty() && !clause.contains("--") => {
                        let mut names: Vec<&str> = selected
                            .iter()
                            .map(|i| i.name.as_str())
                            .filter(|n| usage.is_none_or(|us| us.cap_used(&target, n)))
                            .collect();
                        names.sort_unstable();
                        names.dedup();
                        if names.is_empty() {
                            if has_comment {
                                return None;
                            }
                            return Some(None);
                        }
                        let original: Vec<&str> =
                            selected.iter().map(|i| i.name.as_str()).collect();
                        // Rewrite only a selection that changes, so an
                        // organised clause keeps its own spacing.
                        if names != original {
                            clause = format!("consumes {target} {{ {} }}", names.join(", "));
                        }
                        (1, format!("{target} {{ {} }}", names.join(", ")))
                    }
                    (_, Some(_)) => (1, format!("{target} {{ ... }}#{}", self.span.start)),
                    (alias, None) => {
                        if usage.is_some_and(|us| !us.unit_used(&target)) {
                            if has_comment {
                                return None;
                            }
                            return Some(None);
                        }
                        match alias {
                            Some(a) => (1, format!("{target} as {}", a.name)),
                            None => (1, target),
                        }
                    }
                }
            }
        };
        let mut out = String::new();
        out.push_str(&text[self.extent.start..self.span.start]);
        out.push_str(&clause);
        out.push_str(&text[self.span.end..self.extent.end]);
        Some(Some(Rendered {
            key,
            text: out,
            has_comment,
        }))
    }
}

/// A kept clause: its sort/dedupe key (`0` for `uses`, `1` for `consumes`,
/// then the normalised clause) and its printed extent.
struct Rendered {
    key: (u8, String),
    text: String,
    has_comment: bool,
}
