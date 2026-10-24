//! #852 (capability-aware quick-fixes): the `codeAction` producers that repair
//! *resolution/boundary* diagnostics by editing a unit's header — `add
//! consumes` for an unconsumed cross-context call, and the Bynk analogue of
//! auto-import (`add uses`/`add consumes`) for an unresolved name that the
//! binding index places in a mixable commons or a consumable context.
//!
//! Unlike the [`crate::code_actions`] quick-fixes — which render structured
//! [`bynk_syntax::error::Suggestion`]s authored at the diagnosis site — these
//! are computed **here**: the fix's location is a unit-header edit that only
//! exists once the buffer is reparsed, and (for auto-import) the resolution is
//! a whole-project query over the committed [`ProjectIndex`], neither of which
//! is available at the per-unit checker diagnosis site. Like the `extract`
//! module, the current buffer is reparsed fresh each call (no cached AST);
//! edits are **versioned** against the analysed document version, so a drifted
//! buffer rejects them (§3.10's rule).
//!
//! Keying, as in `code_actions`, is on the **diagnostic's** span: the
//! unresolved name / unconsumed chain is read straight from the source at that
//! span (never re-derived from the message text), so the fix stays anchored to
//! exactly what the compiler flagged.

use bynk_check::firstparty::BYNK_SURFACE_CAPABILITIES;
use bynk_check::index::{ProjectIndex, SymbolKind};
use bynk_syntax::ast::{ConsumesDecl, SourceUnit, UsesDecl};
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::parse_unit_with_recovery;
use bynk_syntax::span::Span;
use tower_lsp::lsp_types::*;

/// Quick-fixes that add a `uses`/`consumes` clause to the current unit's
/// header, for every resolution diagnostic whose span intersects `requested`.
/// `index` is the committed round's binding index; the current unit's own
/// declarations are excluded as candidates (a name that already resolves
/// locally is not "unresolved").
pub fn header_quick_fixes(
    text: &str,
    diagnostics: &[bynk_ide::Diagnostic],
    requested: Span,
    uri: &Url,
    version: Option<i32>,
    index: &ProjectIndex,
) -> Vec<CodeActionOrCommand> {
    // Reparse the buffer to read the current unit's header (kind + existing
    // clauses). A file that no longer parses to a single header unit offers
    // nothing — the same posture as `extract`.
    let Ok(tokens) = tokenize(text) else {
        return Vec::new();
    };
    let (Some(unit), _errs) = parse_unit_with_recovery(&tokens, text) else {
        return Vec::new();
    };
    let Some(header) = Header::of(&unit) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for d in diagnostics {
        if !intersects(d.error.span, requested) {
            continue;
        }
        match d.error.category {
            // A dotted chain that looks like a cross-context call but is not
            // consumed → `consumes <chain>`. The chain is the diagnostic's own
            // span (`app.other`, not the trailing `.service`).
            "bynk.resolve.unconsumed_context" => {
                let chain = span_ident(text, d.error.span);
                if let Some(action) = header.add_consumes_unit(&chain, text, uri, version) {
                    out.push(action);
                }
            }
            // An unresolved name/type → one action per unit that declares it,
            // as `uses <commons>` or `consumes <context> { name }` (DECISION A:
            // per candidate, never a guess). `unknown_type` is restricted to
            // type-shaped candidates.
            "bynk.resolve.unknown_name" | "bynk.resolve.unknown_type" => {
                let type_only = d.error.category == "bynk.resolve.unknown_type";
                let name = span_ident(text, d.error.span);
                out.extend(header.import_actions(&name, type_only, text, uri, version, index));
            }
            _ => {}
        }
    }
    out
}

/// Closed intersection over half-open spans (mirrors `code_actions`).
fn intersects(a: Span, b: Span) -> bool {
    a.start <= b.end && b.start <= a.end
}

/// The identifier (or dotted chain) at `span`, whitespace-collapsed so a chain
/// written with spacing (`app . other`) still reads as `app.other`.
fn span_ident(text: &str, span: Span) -> String {
    text.get(span.start..span.end)
        .unwrap_or_default()
        .split_whitespace()
        .collect()
}

/// The current unit's header, extracted from the reparsed AST: what it can
/// import (`can_consume` is false for a commons — a commons has no `consumes`),
/// its name-clause anchor, and its existing `uses`/`consumes` clauses.
struct Header<'a> {
    unit_name: String,
    can_consume: bool,
    name_span: Span,
    uses: &'a [UsesDecl],
    consumes: &'a [ConsumesDecl],
}

const NO_CONSUMES: &[ConsumesDecl] = &[];

impl<'a> Header<'a> {
    fn of(unit: &'a SourceUnit) -> Option<Self> {
        match unit {
            SourceUnit::Commons(c) => Some(Header {
                unit_name: c.name.joined(),
                can_consume: false,
                name_span: c.name.span,
                uses: &c.uses,
                consumes: NO_CONSUMES,
            }),
            SourceUnit::Context(c) => Some(Header {
                unit_name: c.name.joined(),
                can_consume: true,
                name_span: c.name.span,
                uses: &c.uses,
                consumes: &c.consumes,
            }),
            SourceUnit::Adapter(a) => Some(Header {
                unit_name: a.name.joined(),
                can_consume: true,
                name_span: a.name.span,
                uses: &a.uses,
                consumes: &a.consumes,
            }),
            // A suite has no importing header of its own.
            SourceUnit::Suite(_) => None,
        }
    }

    /// One code action per unit that declares `name`, as an auto-import. A
    /// commons declaration (value vocabulary) → `uses`; a capability exported
    /// by a context/adapter → `consumes … { name }`. Candidates already in
    /// scope, or that the current unit cannot host, are dropped (never a no-op
    /// or an unsound offer); everything genuinely importable is offered
    /// (DECISION A).
    fn import_actions(
        &self,
        name: &str,
        type_only: bool,
        text: &str,
        uri: &Url,
        version: Option<i32>,
        index: &ProjectIndex,
    ) -> Vec<CodeActionOrCommand> {
        let mut targets: Vec<Candidate> = Vec::new();

        // The env-free `bynk` surface capabilities are first-party synthetic
        // symbols, excluded from the index — offered from the known list.
        if !type_only && BYNK_SURFACE_CAPABILITIES.contains(&name) {
            targets.push(Candidate::ConsumesCapability {
                unit: "bynk".to_string(),
            });
        }

        for (key, entry) in &index.symbols {
            if key.name != name || entry.def.is_none() || key.unit == self.unit_name {
                continue;
            }
            match key.kind {
                SymbolKind::Type if unit_is_commons(index, &key.unit) => {
                    targets.push(Candidate::Uses {
                        unit: key.unit.clone(),
                    });
                }
                // #1883: an asset (and #2387 an embed) is a value from a commons,
                // imported like a fn.
                SymbolKind::Fn | SymbolKind::Asset | SymbolKind::Embed
                    if !type_only && unit_is_commons(index, &key.unit) =>
                {
                    targets.push(Candidate::Uses {
                        unit: key.unit.clone(),
                    });
                }
                SymbolKind::Capability if !type_only => {
                    targets.push(Candidate::ConsumesCapability {
                        unit: key.unit.clone(),
                    });
                }
                _ => {}
            }
        }

        targets.sort();
        targets.dedup();

        let mut out = Vec::new();
        for cand in targets {
            let action = match cand {
                Candidate::Uses { unit } => self.add_uses(&unit, text, uri, version),
                Candidate::ConsumesCapability { unit } => {
                    self.add_consumes_capability(&unit, name, text, uri, version)
                }
            };
            if let Some(action) = action {
                out.push(action);
            }
        }
        out
    }

    /// A whole-unit `consumes <target>` clause (the cross-context call fix).
    /// `None` when the current unit cannot consume or already consumes it.
    fn add_consumes_unit(
        &self,
        target: &str,
        text: &str,
        uri: &Url,
        version: Option<i32>,
    ) -> Option<CodeActionOrCommand> {
        if !self.can_consume || self.consumes_target(target).is_some() {
            return None;
        }
        let (at, insert) = self.new_consumes_edit(&format!("consumes {target}"));
        Some(action(
            format!("add `consumes {target}`"),
            at,
            insert,
            text,
            uri,
            version,
        ))
    }

    /// A `consumes <unit> { <cap> }` clause, extending an existing braced
    /// clause for the same unit in place (DECISION C) when one exists. `None`
    /// when the current unit cannot consume or already lists the capability.
    fn add_consumes_capability(
        &self,
        unit: &str,
        cap: &str,
        text: &str,
        uri: &Url,
        version: Option<i32>,
    ) -> Option<CodeActionOrCommand> {
        if !self.can_consume {
            return None;
        }
        // Extend a matching braced clause, if any.
        if let Some(dec) = self
            .consumes
            .iter()
            .find(|c| c.target.joined() == unit && c.selected.is_some())
        {
            let selected = dec.selected.as_ref().unwrap();
            if selected.iter().any(|c| c.name == cap) {
                return None; // already listed
            }
            let (at, insert) = match selected.last() {
                // Non-empty list: append `, cap` after the last selected name.
                Some(last) => (Span::new(last.span.end, last.span.end), format!(", {cap}")),
                // `consumes unit { }` — the interior spacing is unknown, so
                // **replace** the whole clause with a canonical one (its only
                // content is the target and the new capability), rather than
                // inserting into braces of unknown width.
                None => (dec.span, format!("consumes {unit} {{ {cap} }}")),
            };
            return Some(action(
                format!("add `{cap}` to `consumes {unit}`"),
                at,
                insert,
                text,
                uri,
                version,
            ));
        }
        // A whole-unit consume of the same target already brings everything.
        if self.consumes_target(unit).is_some() {
            return None;
        }
        let (at, insert) = self.new_consumes_edit(&format!("consumes {unit} {{ {cap} }}"));
        Some(action(
            format!("add `consumes {unit} {{ {cap} }}`"),
            at,
            insert,
            text,
            uri,
            version,
        ))
    }

    /// A `uses <target>` clause. `None` when it is already used.
    fn add_uses(
        &self,
        target: &str,
        text: &str,
        uri: &Url,
        version: Option<i32>,
    ) -> Option<CodeActionOrCommand> {
        if self.uses.iter().any(|u| u.target.joined() == target) {
            return None;
        }
        // Append after the last `uses`, else after the last `consumes`, else on
        // a fresh line under the unit name.
        let (at, insert) = if let Some(last) = self.uses.last() {
            (last.span.end, format!("\nuses {target}"))
        } else if let Some(last) = self.consumes.last() {
            (last.span.end, format!("\nuses {target}"))
        } else {
            (self.name_span.end, format!("\n\nuses {target}"))
        };
        Some(action(
            format!("add `uses {target}`"),
            Span::new(at, at),
            insert,
            text,
            uri,
            version,
        ))
    }

    /// The `(anchor, text)` for a brand-new consumes clause `clause` (e.g.
    /// `consumes a.b` or `consumes bynk { Fetch }`): appended after the last
    /// existing `consumes`, else on a fresh line under the unit name — before
    /// any `uses` (the conventional consumes-first header order).
    fn new_consumes_edit(&self, clause: &str) -> (Span, String) {
        if let Some(last) = self.consumes.last() {
            let at = last.span.end;
            (Span::new(at, at), format!("\n{clause}"))
        } else {
            let at = self.name_span.end;
            (Span::new(at, at), format!("\n\n{clause}"))
        }
    }

    fn consumes_target(&self, target: &str) -> Option<&ConsumesDecl> {
        self.consumes
            .iter()
            .find(|c| c.target.joined() == target && c.selected.is_none())
    }
}

/// A resolved import target for an unresolved name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Candidate {
    Uses { unit: String },
    ConsumesCapability { unit: String },
}

/// True when `unit` declares only value vocabulary (types/fns/methods) — i.e.
/// it is a commons, not a context/adapter. A context/adapter additionally
/// declares services, agents, actors, capabilities, or providers; the presence
/// of any such symbol is the discriminator (no `UnitKind` is threaded into the
/// index). Conservative: an unclassifiable unit reads as not-a-commons and its
/// types are simply not offered for `uses`.
fn unit_is_commons(index: &ProjectIndex, unit: &str) -> bool {
    !index.symbols.keys().any(|k| {
        k.unit == unit
            && matches!(
                k.kind,
                SymbolKind::Service
                    | SymbolKind::Agent
                    | SymbolKind::Actor
                    | SymbolKind::Capability
                    | SymbolKind::Provider
                    | SymbolKind::Handler
                    | SymbolKind::CapabilityOp
            )
    })
}

/// Build a single-edit versioned quick-fix code action (the §3.10 shape).
fn action(
    title: String,
    at: Span,
    new_text: String,
    text: &str,
    uri: &Url,
    version: Option<i32>,
) -> CodeActionOrCommand {
    let edit = OneOf::Left(TextEdit {
        range: crate::position::span_to_range(text, at),
        new_text,
    });
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version,
                },
                edits: vec![edit],
            }])),
            change_annotations: None,
        }),
        ..Default::default()
    })
}
