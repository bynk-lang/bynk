//! v0.25 (ADR 0053): pure queries over the project binding index.
//!
//! Everything here is a pure function over [`ProjectIndex`] + analysed
//! snapshot texts — the unit-testable core behind `textDocument/references`
//! and `rename`/`prepareRename`. Transport-side handlers in `main.rs` only
//! convert positions and package results.
//!
//! Rename is validated two ways, both correct-by-construction:
//! 1. **Collisions** — apply the edits to an overlay, re-run
//!    `diagnose_project`, refuse if a new diagnostic appears
//!    ([`no_new_diagnostics`]).
//! 2. **Capture/escape** — re-analysis alone misses silent re-binding
//!    (declared fns shadow fn-typed locals in call position), so the
//!    re-built index must equal the pre-index *modulo the rename*
//!    ([`index_unchanged_modulo_rename`]).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::symbols::is_dot_preceded;
use bynk_check::checker::{Ty, TyId, Types};
use bynk_check::index::{ProjectIndex, SiteRef, SymbolKey, SymbolKind};
use bynk_check::locals::{FileLocals, LocalBinding, LocalKind};
use bynk_syntax::ast::BaseType;
use bynk_syntax::lexer;
use bynk_syntax::span::Span;

/// Definition first, then references — the `references` surface.
pub fn sites_for<'a>(
    index: &'a ProjectIndex,
    path: &Path,
    offset: usize,
    include_declaration: bool,
) -> Option<Vec<&'a SiteRef>> {
    let (key, _) = index.symbol_at(path, offset)?;
    let mut sites = index.sites(key);
    if !include_declaration && !sites.is_empty() {
        sites.remove(0); // definition is always first.
    }
    Some(sites)
}

/// The definition site for the symbol at the cursor (an index-backed,
/// binding-correct go-to-definition).
pub fn definition_at<'a>(
    index: &'a ProjectIndex,
    path: &Path,
    offset: usize,
) -> Option<(&'a SymbolKey, &'a SiteRef)> {
    let (key, _) = index.symbol_at(path, offset)?;
    let entry = index.symbols.get(key)?;
    Some((key, entry.def.as_ref()?))
}

/// `prepareRename`: the renameable range under the cursor, or `None` for
/// out-of-scope symbols (locals, unit names) —
/// the request is refused rather than falling through to a partial rename.
pub fn prepare_rename<'a>(
    index: &'a ProjectIndex,
    path: &Path,
    offset: usize,
) -> Option<(&'a SymbolKey, &'a SiteRef)> {
    index.symbol_at(path, offset)
}

/// v0.26 rider (ADR 0055): `workspace/symbol` — every index definition whose
/// name contains the query, case-insensitive (an empty query lists all),
/// sorted by (name, unit) for a stable order.
pub fn workspace_symbols<'a>(
    index: &'a ProjectIndex,
    query: &str,
) -> Vec<(&'a SymbolKey, &'a SiteRef)> {
    let q = query.to_lowercase();
    let mut out: Vec<_> = index
        .symbols
        .iter()
        .filter(|(k, _)| q.is_empty() || k.name.to_lowercase().contains(&q))
        .filter_map(|(k, e)| e.def.as_ref().map(|d| (k, d)))
        .collect();
    out.sort_by(|a, b| (&a.0.name, &a.0.unit).cmp(&(&b.0.name, &b.0.unit)));
    out
}

/// v0.33 (ADR 0066): `codeLens` — one reference-count lens per top-level
/// definition in `path`, as `(def site, reference sites)`. The count is
/// `refs.len()`; the reference sites feed the `showReferences` action. Sorted
/// by definition position (a stable, top-to-bottom lens order).
pub fn code_lenses<'a>(index: &'a ProjectIndex, path: &Path) -> Vec<(&'a SiteRef, &'a [SiteRef])> {
    let mut out: Vec<(&SiteRef, &[SiteRef])> = index
        .symbols
        .values()
        .filter_map(|e| {
            let def = e.def.as_ref()?;
            (def.path == path).then_some((def, e.refs.as_slice()))
        })
        .collect();
    out.sort_by_key(|(def, _)| (def.span.start, def.span.end));
    out
}

/// v0.127 (editor-currency slice 6): a provider-count lens per `capability`
/// definition in `path`, as `(capability def, provider def sites)`. The count is
/// `providers.len()`; the provider sites feed the same `showReferences` peek the
/// reference lens uses. Reuses the `implementations` relation (the `impls_of`
/// edge behind go-to-implementation), so a capability with no provider yields no
/// lens. Sorted by definition position, alongside the reference lenses.
pub fn capability_provider_lenses<'a>(
    index: &'a ProjectIndex,
    path: &Path,
) -> Vec<(&'a SiteRef, Vec<&'a SiteRef>)> {
    let mut out: Vec<(&SiteRef, Vec<&SiteRef>)> = index
        .symbols
        .iter()
        .filter(|(key, _)| key.kind == SymbolKind::Capability)
        .filter_map(|(key, entry)| {
            let def = entry.def.as_ref()?;
            if def.path != path {
                return None;
            }
            let providers = implementations(index, key);
            (!providers.is_empty()).then_some((def, providers))
        })
        .collect();
    out.sort_by_key(|(def, _)| (def.span.start, def.span.end));
    out
}

/// v0.129 (#259): a refinement-family lens per refined/opaque `Type` definition
/// in `path`, as `(type def, base, family def sites)` — every refined/opaque type
/// (and plain alias) declared over the same builtin `base`, across the project.
/// The family includes the type itself, so a lens is emitted only for a family of
/// **≥ 2** (a lone refinement has nothing to navigate to — no lens, mirroring
/// `capability_provider_lenses`). Sorted by definition position, alongside the
/// reference lenses.
pub fn refinement_family_lenses<'a>(
    index: &'a ProjectIndex,
    path: &Path,
) -> Vec<(&'a SiteRef, BaseType, Vec<&'a SiteRef>)> {
    let mut out: Vec<(&SiteRef, BaseType, Vec<&SiteRef>)> = index
        .symbols
        .iter()
        .filter(|(key, _)| key.kind == SymbolKind::Type)
        .filter_map(|(key, entry)| {
            let def = entry.def.as_ref()?;
            if def.path != path {
                return None;
            }
            let base = index.refined_base(key)?;
            let mut family: Vec<&SiteRef> = index
                .refinements_over(base)
                .filter_map(|e| index.symbols.get(&e.ty)?.def.as_ref())
                .collect();
            family.sort_by_key(|d| (d.path.clone(), d.span.start, d.span.end));
            family.dedup();
            (family.len() >= 2).then_some((def, base, family))
        })
        .collect();
    out.sort_by_key(|(def, _, _)| (def.span.start, def.span.end));
    out
}

/// v0.34 (ADR 0067): one end of a call-hierarchy relation — the related
/// symbol (`key` + its definition site) and the call sites linking it to the
/// queried symbol. For incoming calls `key` is a caller and `sites` are where
/// it calls the queried symbol; for outgoing, `key` is a callee and `sites`
/// are where the queried symbol calls it. The sites double as the LSP
/// `fromRanges` (identical for both directions).
pub struct CallRelation<'a> {
    pub key: &'a SymbolKey,
    pub def: &'a SiteRef,
    pub sites: Vec<&'a SiteRef>,
}

/// v0.34 (ADR 0067): `prepareCallHierarchy` — the symbol under the cursor and
/// its definition site (the goto-def resolution; an item is anchored on the
/// definition). `None` for out-of-scope positions.
pub fn prepare_call_hierarchy<'a>(
    index: &'a ProjectIndex,
    path: &Path,
    offset: usize,
) -> Option<(&'a SymbolKey, &'a SiteRef)> {
    definition_at(index, path, offset)
}

/// Group `edges` by the key returned by `pick`, attach each grouped symbol's
/// definition, and collect the call sites — the shared core of incoming and
/// outgoing calls. Groups with no indexed definition are dropped (defensive;
/// every call-edge endpoint is an index symbol by construction). Groups are
/// ordered by definition position for a stable, top-to-bottom listing.
fn group_calls<'a>(
    index: &'a ProjectIndex,
    edges: impl Iterator<Item = &'a bynk_check::index::CallEdge>,
    pick: impl Fn(&'a bynk_check::index::CallEdge) -> &'a SymbolKey,
) -> Vec<CallRelation<'a>> {
    let mut by_key: BTreeMap<&SymbolKey, Vec<&SiteRef>> = BTreeMap::new();
    for edge in edges {
        by_key.entry(pick(edge)).or_default().push(&edge.site);
    }
    let mut out: Vec<CallRelation<'a>> = by_key
        .into_iter()
        .filter_map(|(key, mut sites)| {
            let def = index.symbols.get(key)?.def.as_ref()?;
            sites.sort();
            Some(CallRelation { key, def, sites })
        })
        .collect();
    out.sort_by_key(|r| (r.def.path.clone(), r.def.span.start, r.def.span.end));
    out
}

/// v0.34 (ADR 0067): `callHierarchy/incomingCalls` — the callers of `key`,
/// each with the call sites at which it calls `key`.
pub fn incoming_calls<'a>(index: &'a ProjectIndex, key: &SymbolKey) -> Vec<CallRelation<'a>> {
    group_calls(index, index.calls_into(key), |e| &e.caller)
}

/// v0.34 (ADR 0067): `callHierarchy/outgoingCalls` — what `key` calls, each
/// with the call sites within `key` at which the callee is called.
pub fn outgoing_calls<'a>(index: &'a ProjectIndex, key: &SymbolKey) -> Vec<CallRelation<'a>> {
    group_calls(index, index.calls_from(key), |e| &e.callee)
}

/// v0.35 (ADR 0068): `textDocument/implementation` — the definition sites of
/// every provider implementing the capability `key`, sorted by definition
/// position. Empty for a non-capability or unknown key (the request then falls
/// through; goto-def still serves the reverse, provider → capability).
pub fn implementations<'a>(index: &'a ProjectIndex, key: &SymbolKey) -> Vec<&'a SiteRef> {
    let mut defs: Vec<&SiteRef> = index
        .impls_of(key)
        .filter_map(|e| index.symbols.get(&e.provider)?.def.as_ref())
        .collect();
    defs.sort_by_key(|d| (d.path.clone(), d.span.start, d.span.end));
    defs.dedup();
    defs
}

/// The unit a source file belongs to, read off the first index symbol defined
/// in it. `None` for a file that defines nothing.
pub fn unit_of_file<'a>(index: &'a ProjectIndex, path: &Path) -> Option<&'a str> {
    index
        .symbols
        .iter()
        .find(|(_, e)| e.def.as_ref().is_some_and(|d| d.path == path))
        .map(|(k, _)| k.unit.as_str())
}

/// Whether the refine edge's representation is visible to a request made from
/// `from_unit`: always for a refined type or alias, only inside the declaring
/// unit for an `opaque` one.
fn representation_visible(edge: &bynk_check::index::RefineEdge, from_unit: Option<&str>) -> bool {
    !edge.opaque || from_unit == Some(edge.ty.unit.as_str())
}

/// What `prepareTypeHierarchy` resolved the cursor to.
pub enum HierarchyTarget<'a> {
    /// A refined/opaque/alias type, with its definition site.
    Type(&'a SymbolKey, &'a SiteRef),
    /// A primitive written where a type is, such as `Int` in `type Qty = Int
    /// where …` or in a field type, anchored at that occurrence.
    Primitive(BaseType, SiteRef),
}

/// `textDocument/prepareTypeHierarchy`: the refined/opaque/alias type under the
/// cursor, or the primitive named by the token under it (#2530) — a
/// declaration's base, a field or parameter type, anywhere it is written. `text`
/// is `path`'s analysed snapshot. `None` for any other position, including a
/// record or sum, which has no base.
pub fn prepare_type_hierarchy<'a>(
    index: &'a ProjectIndex,
    path: &Path,
    text: &str,
    offset: usize,
) -> Option<HierarchyTarget<'a>> {
    if let Some((key, def)) = definition_at(index, path, offset) {
        return index
            .refined_base(key)
            .map(|_| HierarchyTarget::Type(key, def));
    }
    // `Int`, `String` and `Bool` lex as keywords, the rest as identifiers; a
    // comment or string token is never exactly a built-in name.
    let tok = lexer::tokenize(text)
        .ok()?
        .into_iter()
        .find(|t| t.span.start <= offset && offset <= t.span.end)?;
    // A member name (`x.Int`) is not the built-in type.
    if is_dot_preceded(text, tok.span.start) {
        return None;
    }
    let base = BaseType::from_name(&text[tok.span.start..tok.span.end])?;
    Some(HierarchyTarget::Primitive(
        base,
        SiteRef {
            path: path.to_path_buf(),
            span: Span::new(tok.span.start, tok.span.end),
        },
    ))
}

/// `typeHierarchy/supertypes` of a refined/opaque type: its builtin base and
/// where the declaration writes it — the only supertype Bynk has (a base is
/// always a primitive, never another declared type). `None` when the type has
/// no base, or it is `opaque` and the request does not come from the declaring
/// unit.
pub fn type_supertype<'a>(
    index: &'a ProjectIndex,
    key: &SymbolKey,
    from_unit: Option<&str>,
) -> Option<(BaseType, &'a SiteRef)> {
    let edge = index.refinements.iter().find(|e| &e.ty == key)?;
    representation_visible(edge, from_unit).then_some((edge.base, &edge.base_site))
}

/// `typeHierarchy/subtypes` of a primitive: every refined, opaque or alias type
/// declared over `base` across the project, sorted by definition position.
/// An `opaque` type is listed only to requests from its declaring unit.
pub fn type_subtypes<'a>(
    index: &'a ProjectIndex,
    base: BaseType,
    from_unit: Option<&str>,
) -> Vec<(&'a SymbolKey, &'a SiteRef)> {
    let mut out: Vec<_> = index
        .refinements_over(base)
        .filter(|e| representation_visible(e, from_unit))
        .filter_map(|e| Some((&e.ty, index.symbols.get(&e.ty)?.def.as_ref()?)))
        .collect();
    out.sort_by_key(|(_, d)| (d.path.clone(), d.span.start, d.span.end));
    out
}

/// Slice 6: `textDocument/typeDefinition` — the definition site(s) of the type
/// named `name` (a `Type` symbol). The checker's `Ty::Named.name` and the index
/// both use bare names, so this is a bare-name match; a name shared across units
/// yields several locations (the LSP-conventional resolution — the client lets
/// the user choose). Sorted by definition position.
pub fn type_definitions_named<'a>(index: &'a ProjectIndex, name: &str) -> Vec<&'a SiteRef> {
    let mut defs: Vec<&SiteRef> = index
        .symbols
        .iter()
        .filter(|(k, _)| k.kind == SymbolKind::Type && k.name == name)
        .filter_map(|(_, e)| e.def.as_ref())
        .collect();
    defs.sort_by_key(|d| (d.path.clone(), d.span.start, d.span.end));
    defs.dedup();
    defs
}

/// #848: the flat compound-name member kinds (`"Owner.member"`, built by
/// `bynk-emit`'s `assemble_index`). A dotted doc-link name matches these
/// verbatim — never split into owner/member.
pub const DOC_LINK_MEMBER_KINDS: &[SymbolKind] = &[
    SymbolKind::Field,
    SymbolKind::Method,
    SymbolKind::CapabilityOp,
    SymbolKind::Handler,
];

/// #848: the top-level kinds a *bare* doc-link name matches.
pub const DOC_LINK_BARE_KINDS: &[SymbolKind] = &[
    SymbolKind::Type,
    SymbolKind::Fn,
    SymbolKind::Capability,
    SymbolKind::Service,
    SymbolKind::Agent,
    SymbolKind::Provider,
    SymbolKind::Actor,
    SymbolKind::Asset,
    SymbolKind::Embed,
];

/// #848: resolve one intra-doc-link candidate name (from
/// `bynk_ide::symbols::scan_doc_link_candidates`) against `owner_unit`'s
/// doc-link scope order in `doc_scope` — the unit itself, then its `uses`
/// targets, then its `consumes` targets (mirrors
/// `bynk_check::index::IndexBuilder::qualify_with`'s bare-name
/// qualification).
///
/// A `name` containing `.` matches the flat compound member kinds
/// (`DOC_LINK_MEMBER_KINDS`, private below) verbatim; otherwise it matches the
/// bare top-level kinds (`DOC_LINK_BARE_KINDS`). The **first** scope-order unit
/// with any match decides the answer: more than one candidate there is
/// unresolved (no `kind@` disambiguation in this increment — convention-only
/// resolution) rather than a guess, even if a later unit in scope order would
/// answer unambiguously. Synthetic (first-party) units are never in
/// `index.symbols` (their defs are dropped at assembly), so a name that only
/// matches through one — e.g. `Clock.now` — naturally comes back `None`,
/// exactly like any other unresolved name.
pub fn resolve_doc_link<'a>(
    index: &'a ProjectIndex,
    doc_scope: &HashMap<String, Vec<String>>,
    owner_unit: &str,
    name: &str,
) -> Option<&'a SiteRef> {
    resolve_doc_link_key(index, doc_scope, owner_unit, name).map(|(_, def)| def)
}

/// #2460: [`resolve_doc_link`], also returning the resolved [`SymbolKey`] so a
/// caller (hover) can describe the target, not just locate it.
pub fn resolve_doc_link_key<'a>(
    index: &'a ProjectIndex,
    doc_scope: &HashMap<String, Vec<String>>,
    owner_unit: &str,
    name: &str,
) -> Option<(&'a SymbolKey, &'a SiteRef)> {
    match doc_link_matches(index, doc_scope, owner_unit, name)?.as_slice() {
        [hit] => Some(*hit),
        _ => None, // ambiguous in the first matching unit — never guess
    }
}

/// #2454: the declaration a doc comment is attached to, as far as link
/// resolution needs it: `attached` is the byte offset of its name in `path`
/// (`bynk_ide::symbols::DocLinkSpan::attached`), and `locals` that file's
/// local bindings, from which a fn / handler / op's parameters are read, and
/// `text` that file's analysed source, which bounds the parameter list.
pub struct DocLinkContext<'a> {
    pub path: &'a Path,
    pub text: &'a str,
    pub attached: usize,
    pub locals: &'a [LocalBinding],
}

/// #2454: a resolved doc link — the definition site, and its index key when
/// it has one (a parameter is a local binding, not an indexed symbol).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDocLink {
    pub def: SiteRef,
    pub key: Option<SymbolKey>,
}

/// #2454: context-aware [`resolve_doc_link`]. A candidate resolves in order:
///
/// 1. the attached declaration's **own parts** — a record's fields, a
///    capability's ops, an agent's handlers, a type's methods (the index's
///    `"Owner.member"` entries), and a fn / handler / op's parameters;
/// 2. the unit scope, exactly as [`resolve_doc_link`] reads it;
/// 3. a **unit-qualified** name (`window.decide`, `a.b.Name`): the last
///    segment as a top-level declaration of the unit named by the rest,
///    whether or not that unit is in scope.
///
/// An own part shadows a unit-level name of the same spelling. A dotted name
/// still tries `Owner.member` (step 2) before the unit-qualified reading, so
/// every link that resolved before resolves to the same target. With no
/// `ctx` step 1 is skipped.
pub fn resolve_doc_link_in(
    index: &ProjectIndex,
    doc_scope: &HashMap<String, Vec<String>>,
    owner_unit: &str,
    name: &str,
    ctx: Option<&DocLinkContext<'_>>,
) -> Option<ResolvedDocLink> {
    if let Some(ctx) = ctx
        && !name.contains('.')
        && let Some(hit) = resolve_own_part(index, ctx, name)
    {
        return Some(hit);
    }
    if let Some((key, def)) = resolve_doc_link_key(index, doc_scope, owner_unit, name) {
        return Some(ResolvedDocLink {
            def: def.clone(),
            key: Some(key.clone()),
        });
    }
    let (unit, last) = name.rsplit_once('.')?;
    let (key, def) = unique_hit(index, unit, last, DOC_LINK_BARE_KINDS)?;
    Some(ResolvedDocLink {
        def: def.clone(),
        key: Some(key),
    })
}

/// The one definition of `name` among `kinds` in `unit`; `None` for none or
/// for more than one (never a guess).
fn unique_hit<'a>(
    index: &'a ProjectIndex,
    unit: &str,
    name: &str,
    kinds: &[SymbolKind],
) -> Option<(SymbolKey, &'a SiteRef)> {
    let mut hit = None;
    let mut count = 0usize;
    for &kind in kinds {
        let key = SymbolKey {
            unit: unit.to_string(),
            kind,
            name: name.to_string(),
        };
        if let Some(def) = index.symbols.get(&key).and_then(|e| e.def.as_ref()) {
            count += 1;
            hit = Some((key, def));
        }
    }
    if count == 1 { hit } else { None }
}

/// #2460: every definition `name` matches in the **first** scope-order unit
/// that has any — the walk [`resolve_doc_link_key`] answers from. One entry
/// is a resolution; more is the ambiguous case a caller can report as such
/// (rather than as "not found"); `None` is nothing matching anywhere.
pub fn doc_link_matches<'a>(
    index: &'a ProjectIndex,
    doc_scope: &HashMap<String, Vec<String>>,
    owner_unit: &str,
    name: &str,
) -> Option<Vec<(&'a SymbolKey, &'a SiteRef)>> {
    let kinds: &[SymbolKind] = if name.contains('.') {
        DOC_LINK_MEMBER_KINDS
    } else {
        DOC_LINK_BARE_KINDS
    };
    let scope = doc_scope.get(owner_unit)?;
    for unit in scope {
        let hits: Vec<_> = kinds
            .iter()
            .filter_map(|&kind| {
                let key = SymbolKey {
                    unit: unit.clone(),
                    kind,
                    name: name.to_string(),
                };
                let (k, entry) = index.symbols.get_key_value(&key)?;
                Some((k, entry.def.as_ref()?))
            })
            .collect();
        if !hits.is_empty() {
            return Some(hits);
        }
    }
    None
}

/// #2456: one doc-link mention in a source file that resolves to a definition
/// (see [`doc_link_mentions`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocLinkMention {
    /// The file (project-relative, as in `snapshots`) holding the mention.
    pub path: PathBuf,
    /// The whole link construct, e.g. `[the limiter][Limiter]` or `` [`Limiter`] ``.
    pub span: Span,
    /// The link's target name text, e.g. `Limiter` or `Limiter.decide`. Display
    /// text and backticks are not part of it.
    pub name: String,
    /// The span of exactly [`Self::name`] inside the link — the text a rename
    /// rewrites. The label of `[text][Name]`, the inside of `` [`Name`] ``.
    pub name_span: Span,
}

/// #2456: every doc-link mention in `snapshots` that resolves (through
/// [`resolve_doc_link_in`], with the declaration each comment is attached to
/// as context, so a bare `[field]` in its own record's doc counts) to the
/// definition at `target`. `locals` is the round's per-file local bindings.
///
/// This is the reverse of [`resolve_doc_link`]: rename uses it to rewrite the
/// mention, and it is the lookup that find-references and document-highlight
/// are meant to report doc-link mentions through (#2457). `target` is a definition site, as held in
/// `SymbolEntry::def`.
///
/// Only files whose doc-link scope contains the defining unit are scanned, and a
/// file whose text does not contain the name's last segment is skipped before
/// it is tokenised. A suite file has no own unit name and is skipped, matching
/// `textDocument/documentLink`. Mentions come back in file then source order.
pub fn doc_link_mentions(
    index: &ProjectIndex,
    doc_scope: &HashMap<String, Vec<String>>,
    snapshots: &HashMap<PathBuf, String>,
    locals: &FileLocals,
    target: &SiteRef,
) -> Vec<DocLinkMention> {
    doc_link_mentions_of(index, doc_scope, snapshots, locals, &[target])
}

/// [`doc_link_mentions`] over several definitions at once: a mention is kept when
/// it resolves to any of `targets`. Rename uses it to also catch `[Type.member]`
/// links when the owning type is renamed.
pub fn doc_link_mentions_of(
    index: &ProjectIndex,
    doc_scope: &HashMap<String, Vec<String>>,
    snapshots: &HashMap<PathBuf, String>,
    locals: &FileLocals,
    targets: &[&SiteRef],
) -> Vec<DocLinkMention> {
    // The defining units and name segments the targets can be spelt with.
    let mut units: Vec<&str> = Vec::new();
    let mut segments: Vec<&str> = Vec::new();
    for t in targets {
        if let Some((k, _)) = index.symbol_at(&t.path, t.span.start) {
            units.push(k.unit.as_str());
            segments.extend(k.name.split('.'));
        }
    }
    if units.is_empty() {
        return Vec::new();
    }
    let mut paths: Vec<&PathBuf> = snapshots.keys().collect();
    paths.sort();
    let mut out = Vec::new();
    for path in paths {
        let text = &snapshots[path];
        if !segments.iter().any(|s| text.contains(s)) {
            continue;
        }
        let Some(owner_unit) = crate::symbols::doc_link_unit(text) else {
            continue;
        };
        let Some(scope) = doc_scope.get(&owner_unit) else {
            continue;
        };
        // A unit-qualified link (`[unit.Name]`) reaches outside the scope list,
        // so a scope miss only rules the file out when no link is qualified.
        if !units.iter().any(|u| scope.iter().any(|s| s == u)) && !text.contains('.') {
            continue;
        }
        for link in crate::symbols::doc_link_spans_attached(text) {
            let (name, span) = (link.name, link.span);
            let ctx = link.attached.map(|attached| DocLinkContext {
                path,
                text,
                attached,
                locals: locals.get(path).map_or(&[][..], Vec::as_slice),
            });
            let Some(resolved) =
                resolve_doc_link_in(index, doc_scope, &owner_unit, &name, ctx.as_ref())
            else {
                continue;
            };
            if !targets.contains(&&resolved.def) {
                continue;
            }
            // The name is the last thing in the construct: before the closing
            // `]`, or before `` `] `` in the code-span form.
            let tail = if text[..span.end].ends_with("`]") {
                2
            } else {
                1
            };
            let Some(name_start) = span.end.checked_sub(tail + name.len()) else {
                continue;
            };
            if text.get(name_start..name_start + name.len()) != Some(name.as_str()) {
                continue;
            }
            out.push(DocLinkMention {
                path: path.clone(),
                span,
                name_span: Span::new(name_start, name_start + name.len()),
                name,
            });
        }
    }
    out
}

/// #2458: one name a doc link could be completed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocLinkSuggestion {
    /// The text to insert: the whole name, or just the member segment when
    /// completing after `Owner.`.
    pub label: String,
    /// The full link name `resolve_doc_link_in` would be given.
    pub name: String,
    /// The symbol kind; `None` for a parameter (a local binding, not indexed).
    pub kind: Option<SymbolKind>,
    /// The unit that declares it.
    pub unit: String,
}

/// #2458: the names a doc link in `owner_unit` could resolve, **in resolution
/// order** — the single place completion defines that order:
///
/// 1. the attached declaration's own parts (`ctx`): a record's fields, a
///    capability's ops, an agent's handlers, a type's methods, then a fn /
///    handler / op's parameters;
/// 2. the unit scope — the same `doc_scope` list `resolve_doc_link` walks (so a
///    change to scope order or contents flows through), by position in that
///    list and then by name.
///
/// A name is kept only when `resolve_doc_link_in` resolves it to that very
/// declaration, so suggestions and resolution agree. A bare name that does not
/// (shadowed by an own part or an earlier unit) is instead offered as
/// `unit.Name` when that qualified form resolves to it; one ambiguous within
/// its unit has no resolving form and is left out.
///
/// With `owner: Some("Type")` it offers that owner's members (`Type.member`,
/// labelled by the member segment) from the unit scope only.
pub fn doc_link_suggestions(
    index: &ProjectIndex,
    doc_scope: &HashMap<String, Vec<String>>,
    owner_unit: &str,
    owner: Option<&str>,
    ctx: Option<&DocLinkContext<'_>>,
) -> Vec<DocLinkSuggestion> {
    let Some(scope) = doc_scope.get(owner_unit) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if owner.is_none()
        && let Some(ctx) = ctx
    {
        own_part_suggestions(index, doc_scope, owner_unit, ctx, &mut out);
    }
    let kinds: &[SymbolKind] = if owner.is_some() {
        DOC_LINK_MEMBER_KINDS
    } else {
        DOC_LINK_BARE_KINDS
    };
    let prefix = owner.map(|o| format!("{o}."));
    let mut found: Vec<(usize, &SymbolKey)> = index
        .symbols
        .iter()
        .filter(|(k, e)| e.def.is_some() && kinds.contains(&k.kind))
        .filter(|(k, _)| match &prefix {
            Some(p) => k
                .name
                .strip_prefix(p.as_str())
                .is_some_and(|m| !m.contains('.')),
            None => !k.name.contains('.'),
        })
        .filter_map(|(k, _)| Some((scope.iter().position(|u| *u == k.unit)?, k)))
        .collect();
    found.sort_by(|a, b| (a.0, &a.1.name, a.1.kind).cmp(&(b.0, &b.1.name, b.1.kind)));
    for (_, key) in found {
        let resolves_to = |name: &str| {
            resolve_doc_link_in(index, doc_scope, owner_unit, name, ctx)
                .and_then(|r| r.key)
                .is_some_and(|k| k == *key)
        };
        let (label, name) = if resolves_to(&key.name) {
            let label = match &prefix {
                Some(p) => key.name[p.len()..].to_string(),
                None => key.name.clone(),
            };
            (label, key.name.clone())
        } else if prefix.is_none() && resolves_to(&format!("{}.{}", key.unit, key.name)) {
            // #2458 (decision C): the bare name reaches something else, so
            // offer the form that reaches this one.
            let qualified = format!("{}.{}", key.unit, key.name);
            (qualified.clone(), qualified)
        } else {
            continue;
        };
        out.push(DocLinkSuggestion {
            label,
            name,
            kind: Some(key.kind),
            unit: key.unit.clone(),
        });
    }
    out
}

/// Tier 1 of [`doc_link_suggestions`]: the attached declaration's members (by
/// source position), then its parameters. Each is checked against
/// [`resolve_doc_link_in`], so a parameter a same-named field would shadow is
/// not offered.
fn own_part_suggestions(
    index: &ProjectIndex,
    doc_scope: &HashMap<String, Vec<String>>,
    owner_unit: &str,
    ctx: &DocLinkContext<'_>,
    out: &mut Vec<DocLinkSuggestion>,
) {
    let resolves_at = |name: &str, def: &SiteRef| {
        resolve_doc_link_in(index, doc_scope, owner_unit, name, Some(ctx))
            .is_some_and(|r| r.def == *def)
    };
    if let Some(owner) = attached_owner(index, ctx) {
        let prefix = format!("{}.", owner.name);
        let mut members: Vec<_> = index
            .symbols
            .iter()
            .filter(|(k, _)| k.unit == owner.unit && DOC_LINK_MEMBER_KINDS.contains(&k.kind))
            .filter_map(|(k, e)| {
                let member = k.name.strip_prefix(prefix.as_str())?;
                (!member.contains('.')).then_some((member, k, e.def.as_ref()?))
            })
            .collect();
        members.sort_by_key(|(m, k, d)| (d.span.start, *m, k.kind));
        for (member, key, def) in members {
            if resolves_at(member, def) {
                out.push(DocLinkSuggestion {
                    label: member.to_string(),
                    name: member.to_string(),
                    kind: Some(key.kind),
                    unit: key.unit.clone(),
                });
            }
        }
    }
    let mut params: Vec<_> = attached_params(ctx).collect();
    params.sort_by_key(|b| b.def_span.start);
    for param in params {
        let def = SiteRef {
            path: ctx.path.to_path_buf(),
            span: param.def_span,
        };
        if resolves_at(&param.name, &def) {
            out.push(DocLinkSuggestion {
                label: param.name.clone(),
                name: param.name.clone(),
                kind: None,
                unit: owner_unit.to_string(),
            });
        }
    }
}

/// #2455: whether any unit in `owner_unit`'s doc-link scope declares a
/// top-level symbol named `owner` (the part of a doc-link name before any `.`).
/// A project declaration shadows a first-party name of the same spelling, so
/// the first-party fallback must not run when this is true — even if
/// `resolve_doc_link` declined (ambiguous, or a member miss).
pub fn doc_link_owner_declared(
    index: &ProjectIndex,
    doc_scope: &HashMap<String, Vec<String>>,
    owner_unit: &str,
    owner: &str,
) -> bool {
    doc_scope.get(owner_unit).is_some_and(|scope| {
        scope.iter().any(|unit| {
            DOC_LINK_BARE_KINDS.iter().any(|&kind| {
                let key = SymbolKey {
                    unit: unit.clone(),
                    kind,
                    name: owner.to_string(),
                };
                index.symbols.get(&key).is_some_and(|e| e.def.is_some())
            })
        })
    })
}

/// The byte range of the `( … )` parameter list that follows the declaration
/// name at `attached`: `None` when a `{` or `=` comes first (a record, agent or
/// capability has no parameters) or the parentheses are unbalanced.
fn param_header(text: &str, attached: usize) -> Option<std::ops::Range<usize>> {
    let rest = text.get(attached..)?;
    let open = rest.find(['(', '{', '='])?;
    if !rest[open..].starts_with('(') {
        return None;
    }
    let mut depth = 0usize;
    for (i, c) in rest[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(attached + open..attached + open + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// The index entry for the declaration whose name sits at `ctx.attached`.
fn attached_owner<'a>(index: &'a ProjectIndex, ctx: &DocLinkContext<'_>) -> Option<&'a SymbolKey> {
    let at = |d: &SiteRef| d.path == ctx.path && d.span.start == ctx.attached;
    index
        .symbols
        .iter()
        .find(|(_, e)| e.def.as_ref().is_some_and(at))
        .map(|(k, _)| k)
}

/// The `Param` bindings inside the attached declaration's parenthesised header.
fn attached_params<'a>(ctx: &'a DocLinkContext<'_>) -> impl Iterator<Item = &'a LocalBinding> {
    let header = param_header(ctx.text, ctx.attached);
    ctx.locals.iter().filter(move |b| {
        b.kind == LocalKind::Param
            && header
                .as_ref()
                .is_some_and(|h| b.def_span.start >= h.start && b.def_span.end <= h.end)
    })
}

/// #2459: the names [`resolve_doc_link_in`] resolves as the attached
/// declaration's own parts (member and parameter names, bare), for the
/// did-you-mean candidates of the unresolved-link hint. Only names that
/// resolve (a name defined as two member kinds is ambiguous, so never one).
pub fn own_part_names(index: &ProjectIndex, ctx: &DocLinkContext<'_>) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(owner) = attached_owner(index, ctx) {
        let prefix = format!("{}.", owner.name);
        names.extend(
            index
                .symbols
                .iter()
                .filter(|(k, e)| {
                    k.unit == owner.unit
                        && e.def.is_some()
                        && DOC_LINK_MEMBER_KINDS.contains(&k.kind)
                })
                .filter_map(|(k, _)| k.name.strip_prefix(&prefix))
                .map(str::to_string),
        );
    }
    names.extend(attached_params(ctx).map(|b| b.name.clone()));
    names.sort();
    names.dedup();
    names.retain(|n| resolve_own_part(index, ctx, n).is_some());
    names
}

/// Step 1 of [`resolve_doc_link_in`]: `name` as a member or parameter of the
/// declaration whose name sits at `ctx.attached`.
fn resolve_own_part(
    index: &ProjectIndex,
    ctx: &DocLinkContext<'_>,
    name: &str,
) -> Option<ResolvedDocLink> {
    let owner = attached_owner(index, ctx)?;
    let member = format!("{}.{name}", owner.name);
    if let Some((key, def)) = unique_hit(index, &owner.unit, &member, DOC_LINK_MEMBER_KINDS) {
        return Some(ResolvedDocLink {
            def: def.clone(),
            key: Some(key),
        });
    }
    // The declaration's parameters (see `attached_params`): a lambda's sit in
    // the body, past the header, so a declaration with no parameters of its own
    // never claims one.
    let param = attached_params(ctx).find(|b| b.name == name)?;
    Some(ResolvedDocLink {
        def: SiteRef {
            path: ctx.path.to_path_buf(),
            span: param.def_span,
        },
        key: None,
    })
}

/// The user-declared type a value's type points at, for go-to-type-definition:
/// a `Named` directly, or the element of a single-parameter container
/// (`Option`/`Effect`/`List`/`HttpResult`) unwrapped to it. Built-in, function,
/// actor, and two-parameter (`Result`/`Map`) types have no single
/// type-declaration target and yield `None`.
pub fn named_type_target(ty: TyId, tys: &Types) -> Option<String> {
    match &*tys.get(ty) {
        Ty::Named { name, .. } => Some(name.clone()),
        Ty::Option(t) | Ty::Effect(t) | Ty::List(t) | Ty::HttpResult(t) => {
            named_type_target(*t, tys)
        }
        _ => None,
    }
}

/// v0.26 rider (ADR 0055): `documentHighlight` — the symbol-at-cursor's
/// occurrences within that same file (the `references` query, file-scoped).
/// The index does not distinguish read from write references, so the LSP
/// layer omits the highlight `kind`.
pub fn document_highlights<'a>(
    index: &'a ProjectIndex,
    path: &Path,
    offset: usize,
) -> Option<Vec<&'a SiteRef>> {
    let sites = sites_for(index, path, offset, true)?;
    Some(sites.into_iter().filter(|s| s.path == path).collect())
}

/// A planned rename: every name-segment edit, grouped per file, spans
/// ascending. The definition site is edited along with every reference.
#[derive(Debug, Clone)]
pub struct RenamePlan {
    pub key: SymbolKey,
    pub new_name: String,
    pub edits: BTreeMap<PathBuf, Vec<Span>>,
}

/// Build the rename plan for the symbol at the cursor. Errors are
/// human-readable strings surfaced as LSP request failures.
pub fn plan_rename(
    index: &ProjectIndex,
    path: &Path,
    offset: usize,
    new_name: &str,
) -> Result<RenamePlan, String> {
    validate_new_name(new_name)?;
    let (key, _) = index.symbol_at(path, offset).ok_or_else(|| {
        "no renameable symbol at the cursor — types, fns, methods, record fields, \
         capability ops, capabilities, services, agents and providers rename; \
         local bindings and unit names are not yet supported"
            .to_string()
    })?;
    if key_segment(&key.name) == new_name {
        return Err(format!("`{new_name}` is already the symbol's name"));
    }
    let mut edits: BTreeMap<PathBuf, Vec<Span>> = BTreeMap::new();
    for site in index.sites(key) {
        edits.entry(site.path.clone()).or_default().push(site.span);
    }
    for spans in edits.values_mut() {
        spans.sort();
        spans.dedup();
    }
    Ok(RenamePlan {
        key: key.clone(),
        new_name: new_name.to_string(),
        edits,
    })
}

/// #2456: extend `plan` with the doc links that resolved to the renamed symbol
/// before the rename, across every file in `snapshots`. A link to the symbol
/// itself has its last segment rewritten (`[Limiter]`, `[Limiter.decide]` when
/// `decide` is renamed); when a type-like symbol is renamed, a link to one of
/// its members has its owner segment rewritten (`[Limiter.decide]` when
/// `Limiter` is). Display text and backticks are untouched, and a link that
/// resolves to some other definition of the same name is left alone.
pub fn add_doc_link_edits(
    plan: &mut RenamePlan,
    index: &ProjectIndex,
    doc_scope: &HashMap<String, Vec<String>>,
    snapshots: &HashMap<PathBuf, String>,
    locals: &FileLocals,
) {
    let Some(entry) = index.symbols.get(&plan.key) else {
        return;
    };
    let Some(def) = entry.def.as_ref() else {
        return;
    };
    let own = doc_link_mentions(index, doc_scope, snapshots, locals, def);
    let seg_len = key_segment(&plan.key.name).len();
    let mut extra: Vec<(PathBuf, Span)> = own
        .iter()
        .map(|m| {
            (
                m.path.clone(),
                Span::new(m.name_span.end - seg_len, m.name_span.end),
            )
        })
        .collect();
    if !plan.key.name.contains('.') {
        let prefix = format!("{}.", plan.key.name);
        let members: Vec<&SiteRef> = index
            .symbols
            .iter()
            .filter(|(k, _)| k.unit == plan.key.unit && k.name.starts_with(&prefix))
            .filter_map(|(_, e)| e.def.as_ref())
            .collect();
        let owner_len = plan.key.name.len();
        for m in doc_link_mentions_of(index, doc_scope, snapshots, locals, &members) {
            // A bare `[member]` (in its owner's own doc) spells no owner; a
            // dotted one names it just before the member segment.
            let Some((before, member)) = m.name.rsplit_once('.') else {
                continue;
            };
            let end = m.name_span.end - member.len() - 1;
            if !before.ends_with(&plan.key.name) || end < owner_len {
                continue;
            }
            extra.push((m.path.clone(), Span::new(end - owner_len, end)));
        }
    }
    for (path, span) in extra {
        let spans = plan.edits.entry(path).or_default();
        // Spans order by file id first, so a doc-link span must carry the id its
        // file's index spans already have for the per-file list to sort by offset.
        let file = spans.first().map(|s| s.file).unwrap_or(span.file);
        spans.push(Span::new_in(file, span.start, span.end));
    }
    for spans in plan.edits.values_mut() {
        spans.sort();
        spans.dedup();
    }
}

/// #2456 (decision C): after a rename, every doc link must resolve to the same
/// definition it did before — modulo the rename itself. `post_*` describe the
/// re-analysed project with the edits applied, `post_texts` the edited sources.
/// A link that now resolves elsewhere, stops resolving, or starts resolving is
/// a silent re-binding, reported like any other.
pub fn doc_links_rebound(
    pre: (&ProjectIndex, &HashMap<String, Vec<String>>, &FileLocals),
    post: (&ProjectIndex, &HashMap<String, Vec<String>>, &FileLocals),
    pre_texts: &HashMap<PathBuf, String>,
    post_texts: &HashMap<PathBuf, String>,
    plan: &RenamePlan,
) -> Result<(), String> {
    let renamed = renamed_key_name(&plan.key.name, &plan.new_name);
    let owner_prefix = format!("{}.", plan.key.name);
    let expect = |k: &SymbolKey| -> SymbolKey {
        let mut k = k.clone();
        if k == plan.key {
            k.name = renamed.clone();
        } else if k.unit == plan.key.unit
            && !plan.key.name.contains('.')
            && k.name.starts_with(&owner_prefix)
        {
            k.name = format!("{}.{}", plan.new_name, &k.name[owner_prefix.len()..]);
        }
        k
    };
    // What every doc link in `text` resolves to, as a definition: its index
    // key, or for a parameter (no key) the site.
    let resolve_all =
        |(index, scope, locals): (&ProjectIndex, &HashMap<String, Vec<String>>, &FileLocals),
         path: &PathBuf,
         text: &str|
         -> Option<Vec<(String, Option<ResolvedDocLink>)>> {
            let owner = crate::symbols::doc_link_unit(text)?;
            Some(
                crate::symbols::doc_link_spans_attached(text)
                    .into_iter()
                    .map(|link| {
                        let ctx = link.attached.map(|attached| DocLinkContext {
                            path,
                            text,
                            attached,
                            locals: locals.get(path).map_or(&[][..], Vec::as_slice),
                        });
                        let r = resolve_doc_link_in(index, scope, &owner, &link.name, ctx.as_ref());
                        (link.name, r)
                    })
                    .collect(),
            )
        };
    let mut paths: Vec<&PathBuf> = pre_texts.keys().collect();
    paths.sort();
    for path in paths {
        let (Some(before), Some(after)) = (
            resolve_all(pre, path, &pre_texts[path]),
            resolve_all(post, path, &post_texts[path]),
        ) else {
            continue;
        };
        debug_assert_eq!(
            before.len(),
            after.len(),
            "a rename edit never adds or removes a link"
        );
        if before.len() != after.len() {
            continue;
        }
        for ((name, was), (_, now)) in before.iter().zip(&after) {
            // Compare by key; a parameter has none, so it is compared by whether
            // it is still a parameter (its site shifts with the edits).
            let was_key = was.as_ref().map(|r| r.key.as_ref().map(&expect));
            let now_key = now.as_ref().map(|r| r.key.clone());
            if was_key != now_key {
                return Err(format!(
                    "renaming `{}` to `{}` would change what the doc link `[{name}]` in {} resolves to — refused",
                    plan.key.name,
                    plan.new_name,
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

/// A new name must lex as exactly one identifier (keywords lex as their own
/// token kinds, so they fail this check).
pub fn validate_new_name(name: &str) -> Result<(), String> {
    let err = || format!("`{name}` is not a valid Bynk identifier");
    let tokens = bynk_syntax::lexer::tokenize(name).map_err(|_| err())?;
    match tokens.as_slice() {
        [t] if matches!(t.kind, bynk_syntax::lexer::TokenKind::Ident)
            && t.span.start == 0
            && t.span.end == name.len() =>
        {
            Ok(())
        }
        _ => Err(err()),
    }
}

/// Apply one file's edits (spans ascending) to its snapshot text.
pub fn apply_edits(text: &str, spans: &[Span], new_name: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for s in spans {
        out.push_str(&text[last..s.start]);
        out.push_str(new_name);
        last = s.end;
    }
    out.push_str(&text[last..]);
    out
}

/// The post-edit position of a pre-edit site — rename edits shift spans
/// within edited files. An edited span maps to the new name's span.
pub fn remap_site(site: &SiteRef, plan: &RenamePlan) -> SiteRef {
    let Some(spans) = plan.edits.get(&site.path) else {
        return site.clone();
    };
    // The edit replaces the member segment only (`"m"` of `"Type.m"`), so the
    // length delta is against that segment, not the whole compound key name.
    let delta = plan.new_name.len() as isize - key_segment(&plan.key.name).len() as isize;
    let shift: isize = spans.iter().filter(|s| s.end <= site.span.start).count() as isize * delta;
    let start = (site.span.start as isize + shift) as usize;
    let end = if spans.binary_search(&site.span).is_ok() {
        start + plan.new_name.len()
    } else {
        (site.span.end as isize + shift) as usize
    };
    SiteRef {
        path: site.path.clone(),
        span: Span::new_in(site.span.file, start, end),
    }
}

/// Validator (2): the re-built index must equal the pre-index modulo the
/// rename — every other symbol's reference set identical (after remapping
/// shifted spans), the renamed symbol's sites exactly the edited ones.
/// Catches silent re-binding (capture/escape) that produces no diagnostic.
pub fn index_unchanged_modulo_rename(
    pre: &ProjectIndex,
    post: &ProjectIndex,
    plan: &RenamePlan,
) -> bool {
    // v0.36 (ADR 0069): a member key carries a compound name (`"Type.method"`),
    // but the edit replaces only the member segment — so the post-rename key is
    // the prefix plus the new segment, not the bare new name.
    let target = renamed_key_name(&plan.key.name, &plan.new_name);
    // #2456: renaming a type-like symbol also re-keys its members
    // (`Type.member` -> `New.member`), which the post-index carries.
    if plan.key.name.contains('.') {
        return pre.equals_modulo_rename(post, &plan.key, &target, |s| remap_site(s, plan));
    }
    let prefix = format!("{}.", plan.key.name);
    let mut rekeyed = pre.clone();
    rekeyed.symbols = pre
        .symbols
        .iter()
        .map(|(k, e)| {
            let mut k = k.clone();
            if k.unit == plan.key.unit && k.name.starts_with(&prefix) {
                k.name = format!("{}.{}", plan.new_name, &k.name[prefix.len()..]);
            }
            (k, e.clone())
        })
        .collect();
    rekeyed.equals_modulo_rename(post, &plan.key, &target, |s| remap_site(s, plan))
}

/// The post-rename value of a (possibly compound) key name: for a member key
/// `"Type.method"`, replace the segment after the last `.`; for a bare name,
/// the new name as-is.
fn renamed_key_name(old: &str, new_segment: &str) -> String {
    match old.rfind('.') {
        Some(i) => format!("{}.{new_segment}", &old[..i]),
        None => new_segment.to_string(),
    }
}

/// The member segment of a (possibly compound) key name — the text the rename
/// actually edits (every site span covers exactly this).
fn key_segment(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// Validator (1): refuse when the edited project carries a diagnostic the
/// original did not — compared as per-(file, category) counts, robust to
/// span shifts. Removals are tolerated; additions refuse.
pub fn no_new_diagnostics(
    pre: &[(PathBuf, String)],
    post: &[(PathBuf, String)],
) -> Result<(), String> {
    let mut budget: HashMap<(&Path, &str), isize> = HashMap::new();
    for (p, c) in pre {
        *budget.entry((p.as_path(), c.as_str())).or_default() += 1;
    }
    for (p, c) in post {
        let n = budget.entry((p.as_path(), c.as_str())).or_default();
        *n -= 1;
        if *n < 0 {
            return Err(format!(
                "rename would introduce `{c}` in {} — refused",
                p.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_check::index::{SymbolEntry, SymbolKind};

    fn site(path: &str, start: usize, end: usize) -> SiteRef {
        SiteRef {
            path: PathBuf::from(path),
            span: Span::new(start, end),
        }
    }

    fn key(unit: &str, kind: SymbolKind, name: &str) -> SymbolKey {
        SymbolKey {
            unit: unit.into(),
            kind,
            name: name.into(),
        }
    }

    fn index_with(entries: Vec<(SymbolKey, SiteRef, Vec<SiteRef>)>) -> ProjectIndex {
        let mut index = ProjectIndex::default();
        for (k, def, refs) in entries {
            index.symbols.insert(
                k,
                SymbolEntry {
                    def: Some(def),
                    refs,
                    ..Default::default()
                },
            );
        }
        index
    }

    #[test]
    fn new_name_validation() {
        assert!(validate_new_name("Money2").is_ok());
        assert!(validate_new_name("snake_case").is_ok());
        assert!(validate_new_name("fn").is_err(), "keyword");
        assert!(validate_new_name("two words").is_err());
        assert!(validate_new_name("1abc").is_err());
        assert!(validate_new_name("a.b").is_err());
        assert!(validate_new_name("").is_err());
    }

    #[test]
    fn apply_and_remap_agree() {
        // text: "fn helper(x: Int) -> Int { helper(x) }"
        //        3..9 def                  27..33 ref
        let text = "fn helper(x: Int) -> Int { helper(x) }";
        let k = key("demo.a", SymbolKind::Fn, "helper");
        let index = index_with(vec![(
            k.clone(),
            site("a.bynk", 3, 9),
            vec![site("a.bynk", 27, 33)],
        )]);
        let plan = plan_rename(&index, Path::new("a.bynk"), 4, "do_it").unwrap();
        let edited = apply_edits(text, &plan.edits[Path::new("a.bynk")], "do_it");
        assert_eq!(edited, "fn do_it(x: Int) -> Int { do_it(x) }");

        // Remap maps both old sites onto the new spellings.
        for (old, expected) in [
            (site("a.bynk", 3, 9), "do_it"),
            (site("a.bynk", 27, 33), "do_it"),
        ] {
            let new = remap_site(&old, &plan);
            assert_eq!(&edited[new.span.range()], expected);
        }
        // An unedited later site shifts by the accumulated delta.
        let unrelated = site("a.bynk", 34, 35); // `x` argument
        let new = remap_site(&unrelated, &plan);
        assert_eq!(&edited[new.span.range()], "x");
    }

    #[test]
    fn references_listing_orders_definition_first() {
        let k = key("demo.a", SymbolKind::Type, "Money");
        let index = index_with(vec![(
            k,
            site("a.bynk", 5, 10),
            vec![site("b.bynk", 1, 6), site("a.bynk", 20, 25)],
        )]);
        let all = sites_for(&index, Path::new("b.bynk"), 3, true).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].path, PathBuf::from("a.bynk"));
        assert_eq!(all[0].span, Span::new(5, 10));
        let without_decl = sites_for(&index, Path::new("b.bynk"), 3, false).unwrap();
        assert_eq!(without_decl.len(), 2);
    }

    #[test]
    fn rename_refuses_unindexed_positions_and_same_name() {
        let k = key("demo.a", SymbolKind::Fn, "helper");
        let index = index_with(vec![(k, site("a.bynk", 3, 9), vec![])]);
        assert!(plan_rename(&index, Path::new("a.bynk"), 100, "x").is_err());
        assert!(plan_rename(&index, Path::new("a.bynk"), 4, "helper").is_err());
    }

    #[test]
    fn index_equality_detects_escape() {
        // Pre: `helper` has no references; some other symbol unchanged.
        let helper = key("demo.a", SymbolKind::Fn, "helper");
        let money = key("demo.a", SymbolKind::Type, "Money");
        let pre = index_with(vec![
            (helper.clone(), site("a.bynk", 3, 9), vec![]),
            (money.clone(), site("a.bynk", 50, 55), vec![]),
        ]);
        let plan = plan_rename(&pre, Path::new("a.bynk"), 4, "shadow").unwrap();

        // Post (honest): def renamed in place, still no refs.
        let honest = index_with(vec![
            (
                key("demo.a", SymbolKind::Fn, "shadow"),
                site("a.bynk", 3, 9),
                vec![],
            ),
            (money.clone(), site("a.bynk", 50, 55), vec![]),
        ]);
        assert!(index_unchanged_modulo_rename(&pre, &honest, &plan));

        // Post (escape): a site that used to bind elsewhere now resolves to
        // the renamed symbol — an extra reference appears.
        let escape = index_with(vec![
            (
                key("demo.a", SymbolKind::Fn, "shadow"),
                site("a.bynk", 3, 9),
                vec![site("a.bynk", 70, 76)],
            ),
            (money, site("a.bynk", 50, 55), vec![]),
        ]);
        assert!(!index_unchanged_modulo_rename(&pre, &escape, &plan));
    }

    #[test]
    fn method_rename_edits_the_member_segment_and_remaps_the_compound_key() {
        // v0.36: a method key is compound (`"Counter.bump"`), but the edit
        // touches only the `bump` segment — so the plan's new name is the bare
        // segment, the post key is `"Counter.increment"`, and the span delta is
        // against the segment length (4), not the compound length (12).
        let bump = key("demo.a", SymbolKind::Method, "Counter.bump");
        let other = key("demo.a", SymbolKind::Type, "Counter");
        let pre = index_with(vec![
            (other.clone(), site("a.bynk", 0, 5), vec![]),
            // def `bump` at 11..15, one call at 40..44.
            (
                bump.clone(),
                site("a.bynk", 11, 15),
                vec![site("a.bynk", 40, 44)],
            ),
        ]);

        // Cursor on the def segment; rename to a longer name.
        let plan = plan_rename(&pre, Path::new("a.bynk"), 12, "increment").unwrap();
        assert_eq!(plan.key.name, "Counter.bump");
        assert_eq!(plan.new_name, "increment");
        // Renaming to the same segment is refused (segment-aware, not key-aware).
        assert!(plan_rename(&pre, Path::new("a.bynk"), 12, "bump").is_err());

        // Honest post: the compound key becomes `Counter.increment`; the def
        // grows in place (11..20) and the call shifts by +5 (45..54).
        let post = index_with(vec![
            (other, site("a.bynk", 0, 5), vec![]),
            (
                key("demo.a", SymbolKind::Method, "Counter.increment"),
                site("a.bynk", 11, 20),
                vec![site("a.bynk", 45, 54)],
            ),
        ]);
        assert!(
            index_unchanged_modulo_rename(&pre, &post, &plan),
            "compound key remaps to Counter.increment and segment-based delta lines the spans up"
        );
    }

    #[test]
    fn workspace_symbols_filters_and_orders() {
        let index = index_with(vec![
            (
                key("demo.a", SymbolKind::Type, "Money"),
                site("a.bynk", 5, 10),
                vec![],
            ),
            (
                key("demo.b", SymbolKind::Fn, "moneyMaker"),
                site("b.bynk", 3, 13),
                vec![],
            ),
            (
                key("demo.a", SymbolKind::Fn, "helper"),
                site("a.bynk", 40, 46),
                vec![],
            ),
        ]);
        // Case-insensitive substring match.
        let hits = workspace_symbols(&index, "money");
        assert_eq!(
            hits.iter()
                .map(|(k, _)| k.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Money", "moneyMaker"]
        );
        // Empty query lists everything, (name, unit)-ordered.
        assert_eq!(workspace_symbols(&index, "").len(), 3);
        assert!(workspace_symbols(&index, "nothing").is_empty());
    }

    #[test]
    fn document_highlights_are_file_scoped() {
        let k = key("demo.a", SymbolKind::Type, "Money");
        let index = index_with(vec![(
            k,
            site("a.bynk", 5, 10),
            vec![site("b.bynk", 1, 6), site("a.bynk", 20, 25)],
        )]);
        // From a.bynk: the definition + the in-file reference, not b.bynk's.
        let highlights = document_highlights(&index, Path::new("a.bynk"), 7).unwrap();
        assert_eq!(highlights.len(), 2);
        assert!(highlights.iter().all(|s| s.path == Path::new("a.bynk")));
        // No symbol at the cursor → None.
        assert!(document_highlights(&index, Path::new("a.bynk"), 100).is_none());
    }

    #[test]
    fn diagnostic_budget_allows_removals_refuses_additions() {
        let pre = vec![
            (PathBuf::from("a.bynk"), "bynk.x".to_string()),
            (PathBuf::from("a.bynk"), "bynk.x".to_string()),
        ];
        let same = pre.clone();
        assert!(no_new_diagnostics(&pre, &same).is_ok());
        assert!(no_new_diagnostics(&pre, &pre[..1]).is_ok());
        let mut more = pre.clone();
        more.push((PathBuf::from("b.bynk"), "bynk.resolve.duplicate_fn".into()));
        assert!(no_new_diagnostics(&pre, &more).is_err());
    }

    #[test]
    fn code_lenses_count_references_per_definition_in_the_file() {
        let index = index_with(vec![
            // `foo` defined in a.bynk with two references.
            (
                key("u", SymbolKind::Fn, "foo"),
                site("a.bynk", 3, 6),
                vec![site("a.bynk", 20, 23), site("b.bynk", 4, 7)],
            ),
            // `Bar` defined in a.bynk with no references (a 0-ref lens).
            (
                key("u", SymbolKind::Type, "Bar"),
                site("a.bynk", 40, 43),
                vec![],
            ),
            // `qux` defined in another file — no lens for a.bynk.
            (
                key("u", SymbolKind::Fn, "qux"),
                site("b.bynk", 0, 3),
                vec![],
            ),
        ]);
        let lenses = code_lenses(&index, Path::new("a.bynk"));
        assert_eq!(lenses.len(), 2, "two a.bynk defs get lenses");
        // Sorted by def position: foo (3..6) before Bar (40..43).
        assert_eq!((lenses[0].0.span.start, lenses[0].1.len()), (3, 2));
        assert_eq!((lenses[1].0.span.start, lenses[1].1.len()), (40, 0));
        assert!(code_lenses(&index, Path::new("c.bynk")).is_empty());
    }

    #[test]
    fn call_hierarchy_groups_incoming_and_outgoing_by_symbol() {
        use bynk_check::index::CallEdge;
        // `a` and `b` both call `c`; `a` calls `c` twice. So `c`'s incoming
        // groups by caller (a with two sites, b with one), and `a`'s outgoing
        // is the single callee `c`.
        let mut index = index_with(vec![
            (key("u", SymbolKind::Fn, "a"), site("f.bynk", 3, 4), vec![]),
            (
                key("u", SymbolKind::Fn, "b"),
                site("f.bynk", 40, 41),
                vec![],
            ),
            (
                key("u", SymbolKind::Fn, "c"),
                site("f.bynk", 80, 81),
                vec![],
            ),
        ]);
        let edge = |caller: &str, cs: usize, ce: usize| CallEdge {
            caller: key("u", SymbolKind::Fn, caller),
            callee: key("u", SymbolKind::Fn, "c"),
            site: site("f.bynk", cs, ce),
        };
        index.calls = vec![edge("a", 10, 11), edge("a", 20, 21), edge("b", 50, 51)];

        let into_c = incoming_calls(&index, &key("u", SymbolKind::Fn, "c"));
        // Sorted by caller def position: a (3) before b (40).
        assert_eq!(into_c.len(), 2);
        assert_eq!(
            (into_c[0].key.name.as_str(), into_c[0].sites.len()),
            ("a", 2)
        );
        assert_eq!(
            (into_c[1].key.name.as_str(), into_c[1].sites.len()),
            ("b", 1)
        );

        let from_a = outgoing_calls(&index, &key("u", SymbolKind::Fn, "a"));
        assert_eq!(from_a.len(), 1);
        assert_eq!(
            (from_a[0].key.name.as_str(), from_a[0].sites.len()),
            ("c", 2)
        );

        // `c` calls nothing; an unknown key yields nothing.
        assert!(outgoing_calls(&index, &key("u", SymbolKind::Fn, "c")).is_empty());
        assert!(incoming_calls(&index, &key("u", SymbolKind::Fn, "ghost")).is_empty());
    }

    #[test]
    fn implementations_lists_provider_defs_for_a_capability() {
        use bynk_check::index::ImplEdge;
        // `Cap` is provided by `P1` and `P2`; `Other` (a capability) has none.
        let mut index = index_with(vec![
            (
                key("u", SymbolKind::Capability, "Cap"),
                site("a.bynk", 10, 13),
                vec![],
            ),
            (
                key("u", SymbolKind::Provider, "P1"),
                site("a.bynk", 50, 52),
                vec![],
            ),
            (
                key("u", SymbolKind::Provider, "P2"),
                site("b.bynk", 5, 7),
                vec![],
            ),
            (
                key("u", SymbolKind::Capability, "Other"),
                site("a.bynk", 80, 85),
                vec![],
            ),
        ]);
        let edge = |provider: &str, file: &str, s: usize, e: usize| ImplEdge {
            capability: key("u", SymbolKind::Capability, "Cap"),
            provider: key("u", SymbolKind::Provider, provider),
            site: site(file, s, e),
        };
        index.impls = vec![edge("P1", "a.bynk", 30, 33), edge("P2", "b.bynk", 20, 23)];

        // Provider defs, sorted by position: P1 (a.bynk:50) before P2 (b.bynk:5).
        let impls = implementations(&index, &key("u", SymbolKind::Capability, "Cap"));
        assert_eq!(impls.len(), 2);
        assert_eq!(
            (&impls[0].path, impls[0].span.start),
            (&PathBuf::from("a.bynk"), 50)
        );
        assert_eq!(
            (&impls[1].path, impls[1].span.start),
            (&PathBuf::from("b.bynk"), 5)
        );

        // A capability with no providers, and an unknown key, yield nothing.
        assert!(implementations(&index, &key("u", SymbolKind::Capability, "Other")).is_empty());
        assert!(implementations(&index, &key("u", SymbolKind::Capability, "Ghost")).is_empty());
    }

    #[test]
    fn capability_provider_lenses_pair_a_capability_with_its_providers() {
        use bynk_check::index::ImplEdge;
        // `Cap` (defined in a.bynk) is provided by `P1`/`P2`; `Other` (a.bynk)
        // has no provider — so no lens; `Elsewhere` lives in b.bynk — no a.bynk lens.
        let mut index = index_with(vec![
            (
                key("u", SymbolKind::Capability, "Cap"),
                site("a.bynk", 10, 13),
                vec![],
            ),
            (
                key("u", SymbolKind::Provider, "P1"),
                site("a.bynk", 50, 52),
                vec![],
            ),
            (
                key("u", SymbolKind::Provider, "P2"),
                site("b.bynk", 5, 7),
                vec![],
            ),
            (
                key("u", SymbolKind::Capability, "Other"),
                site("a.bynk", 80, 85),
                vec![],
            ),
            (
                key("u", SymbolKind::Capability, "Elsewhere"),
                site("b.bynk", 90, 99),
                vec![],
            ),
        ]);
        index.impls = vec![
            ImplEdge {
                capability: key("u", SymbolKind::Capability, "Cap"),
                provider: key("u", SymbolKind::Provider, "P1"),
                site: site("a.bynk", 30, 33),
            },
            ImplEdge {
                capability: key("u", SymbolKind::Capability, "Cap"),
                provider: key("u", SymbolKind::Provider, "P2"),
                site: site("b.bynk", 20, 23),
            },
        ];

        let lenses = capability_provider_lenses(&index, Path::new("a.bynk"));
        // Only `Cap` qualifies: `Other` has no providers, `Elsewhere` is off-file.
        assert_eq!(lenses.len(), 1);
        assert_eq!(lenses[0].0.span.start, 10);
        assert_eq!(lenses[0].1.len(), 2, "Cap has two providers");
        // Off-file query and a provider-less file yield nothing.
        assert!(capability_provider_lenses(&index, Path::new("c.bynk")).is_empty());
    }

    #[test]
    fn refinement_family_lenses_group_refined_types_by_builtin_base() {
        use bynk_check::index::RefineEdge;
        // Email/UserId (a.bynk) + Slug (b.bynk) refine String — a family of 3;
        // Age refines Int alone (no family); Order is a record (no base).
        let mut index = index_with(vec![
            (
                key("u", SymbolKind::Type, "Email"),
                site("a.bynk", 10, 15),
                vec![],
            ),
            (
                key("u", SymbolKind::Type, "UserId"),
                site("a.bynk", 40, 46),
                vec![],
            ),
            (
                key("u", SymbolKind::Type, "Slug"),
                site("b.bynk", 5, 9),
                vec![],
            ),
            (
                key("u", SymbolKind::Type, "Age"),
                site("a.bynk", 70, 73),
                vec![],
            ),
            (
                key("u", SymbolKind::Type, "Order"),
                site("a.bynk", 90, 95),
                vec![],
            ),
        ]);
        let refine = |name: &str, base: BaseType| RefineEdge {
            base,
            opaque: false,
            base_site: site("a.bynk", 0, 0),
            ty: key("u", SymbolKind::Type, name),
        };
        index.refinements = vec![
            refine("Email", BaseType::String),
            refine("UserId", BaseType::String),
            refine("Slug", BaseType::String),
            refine("Age", BaseType::Int),
        ];

        let lenses = refinement_family_lenses(&index, Path::new("a.bynk"));
        // Email + UserId qualify (the String family has 3 members, ≥ 2). `Age` is
        // alone over Int — no lens. `Order` has no base. `Slug` is off-file.
        assert_eq!(lenses.len(), 2);
        // Sorted by def position: Email (10) before UserId (40).
        assert_eq!(
            (lenses[0].0.span.start, lenses[0].1),
            (10, BaseType::String)
        );
        assert_eq!(lenses[0].2.len(), 3, "the String family spans 3 types");
        assert_eq!(lenses[1].0.span.start, 40);
        // Off-file query yields nothing.
        assert!(refinement_family_lenses(&index, Path::new("c.bynk")).is_empty());
    }

    #[test]
    fn type_definitions_named_collects_type_defs_by_bare_name() {
        // Two units each declare an `Order` type; a fn shares the name.
        let index = index_with(vec![
            (
                key("a", SymbolKind::Type, "Order"),
                site("a.bynk", 10, 15),
                vec![],
            ),
            (
                key("b", SymbolKind::Type, "Order"),
                site("b.bynk", 4, 9),
                vec![],
            ),
            (
                key("a", SymbolKind::Fn, "Order"),
                site("a.bynk", 40, 45),
                vec![],
            ),
        ]);
        // Both `Type` defs (not the fn), sorted by position.
        let defs = type_definitions_named(&index, "Order");
        assert_eq!(defs.len(), 2);
        assert_eq!(
            (&defs[0].path, defs[0].span.start),
            (&PathBuf::from("a.bynk"), 10)
        );
        assert_eq!(
            (&defs[1].path, defs[1].span.start),
            (&PathBuf::from("b.bynk"), 4)
        );
        // An unknown type name yields nothing.
        assert!(type_definitions_named(&index, "Nope").is_empty());
    }

    fn doc_scope_with(entries: Vec<(&str, Vec<&str>)>) -> HashMap<String, Vec<String>> {
        entries
            .into_iter()
            .map(|(unit, scope)| {
                (
                    unit.to_string(),
                    scope.into_iter().map(String::from).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn doc_link_suggestions_follow_resolution_order() {
        let index = index_with(vec![
            (
                key("window", SymbolKind::Fn, "decide"),
                site("w.bynk", 1, 7),
                vec![],
            ),
            (
                key("ratelimit", SymbolKind::Type, "Zed"),
                site("r.bynk", 1, 4),
                vec![],
            ),
            (
                key("ratelimit", SymbolKind::Agent, "Limiter"),
                site("r.bynk", 9, 16),
                vec![],
            ),
            (
                key("platform", SymbolKind::Capability, "Clock2"),
                site("p.bynk", 1, 7),
                vec![],
            ),
        ]);
        let scope = doc_scope_with(vec![("ratelimit", vec!["ratelimit", "window", "platform"])]);
        let got = doc_link_suggestions(&index, &scope, "ratelimit", None, None);
        let names: Vec<_> = got.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Limiter", "Zed", "decide", "Clock2"]);
    }

    #[test]
    fn doc_link_suggestions_drop_ambiguous_names_and_qualify_shadowed_ones() {
        let index = index_with(vec![
            (
                key("ratelimit", SymbolKind::Type, "Dup"),
                site("r.bynk", 1, 4),
                vec![],
            ),
            (
                key("ratelimit", SymbolKind::Fn, "Dup"),
                site("r.bynk", 9, 12),
                vec![],
            ),
            (
                key("ratelimit", SymbolKind::Type, "Shared"),
                site("r.bynk", 20, 26),
                vec![],
            ),
            (
                key("window", SymbolKind::Type, "Shared"),
                site("w.bynk", 1, 7),
                vec![],
            ),
        ]);
        let scope = doc_scope_with(vec![("ratelimit", vec!["ratelimit", "window"])]);
        let got = doc_link_suggestions(&index, &scope, "ratelimit", None, None);
        // `Dup` is ambiguous in its unit (no form resolves); `Shared` resolves
        // bare to `ratelimit`'s, and `window`'s is offered qualified.
        let names: Vec<_> = got.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Shared", "window.Shared"], "{got:?}");
        assert_eq!(got[1].unit, "window");
    }

    #[test]
    fn doc_link_suggestions_offer_own_fields_first_and_shadow_unit_names() {
        let index = index_with(vec![
            (
                key("u", SymbolKind::Type, "View"),
                site("u.bynk", 10, 14),
                vec![],
            ),
            (
                key("u", SymbolKind::Field, "View.remaining"),
                site("u.bynk", 30, 39),
                vec![],
            ),
            (
                key("u", SymbolKind::Field, "View.id"),
                site("u.bynk", 20, 22),
                vec![],
            ),
            (
                key("u", SymbolKind::Fn, "id"),
                site("u.bynk", 90, 92),
                vec![],
            ),
        ]);
        let scope = doc_scope_with(vec![("u", vec!["u"])]);
        let ctx = DocLinkContext {
            path: Path::new("u.bynk"),
            text: "",
            attached: 10,
            locals: &[],
        };
        let got = doc_link_suggestions(&index, &scope, "u", None, Some(&ctx));
        let names: Vec<_> = got.iter().map(|s| s.name.as_str()).collect();
        // Fields in source order, then the unit scope; the fn `id` is shadowed
        // by the field, so it is offered qualified.
        assert_eq!(names, ["id", "remaining", "View", "u.id"], "{got:?}");
        assert_eq!(got[0].kind, Some(SymbolKind::Field));
        // Every offered name resolves, in this context, to what was offered.
        for s in &got {
            assert!(resolve_doc_link_in(&index, &scope, "u", &s.name, Some(&ctx)).is_some());
        }
        // Without a context there are no own parts and `id` is the fn.
        let got = doc_link_suggestions(&index, &scope, "u", None, None);
        let names: Vec<_> = got.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["View", "id"]);
    }

    #[test]
    fn doc_link_suggestions_offer_a_fns_parameters_first() {
        let text = "fn f(order: Int, n: Int) -> Int { xs.fold(0, (acc, x) => acc + x) }\n";
        let at = |needle: &str| text.find(needle).unwrap();
        let span = |n: &str| (at(n), at(n) + n.len());
        let body = (at("{"), text.len());
        let index = index_with(vec![
            (key("u", SymbolKind::Fn, "f"), site("u.bynk", 3, 4), vec![]),
            (
                key("u", SymbolKind::Fn, "order"),
                site("u.bynk", 200, 205),
                vec![],
            ),
        ]);
        let scope = doc_scope_with(vec![("u", vec!["u"])]);
        let locals = vec![
            ctx_locals("n", (at("n:"), at("n:") + 1), body),
            ctx_locals("order", span("order"), body),
            ctx_locals("acc", span("acc"), (body.0 + 20, body.1)),
        ];
        let ctx = DocLinkContext {
            path: Path::new("u.bynk"),
            text,
            attached: 3,
            locals: &locals,
        };
        let got = doc_link_suggestions(&index, &scope, "u", None, Some(&ctx));
        let names: Vec<_> = got.iter().map(|s| s.name.as_str()).collect();
        // Source order; the lambda's `acc` is not a parameter of `f`; the
        // unit-level `order` is shadowed by the parameter, so is qualified.
        assert_eq!(names, ["order", "n", "f", "u.order"], "{got:?}");
        assert_eq!(got[0].kind, None);
    }

    #[test]
    fn doc_link_suggestions_after_an_owner_offer_its_members() {
        let index = index_with(vec![
            (
                key("ratelimit", SymbolKind::Field, "RateView.remaining"),
                site("r.bynk", 1, 9),
                vec![],
            ),
            (
                key("ratelimit", SymbolKind::Field, "RateView.limit"),
                site("r.bynk", 12, 17),
                vec![],
            ),
            (
                key("ratelimit", SymbolKind::Field, "Other.x"),
                site("r.bynk", 20, 21),
                vec![],
            ),
        ]);
        let scope = doc_scope_with(vec![("ratelimit", vec!["ratelimit"])]);
        let got = doc_link_suggestions(&index, &scope, "ratelimit", Some("RateView"), None);
        let labels: Vec<_> = got.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["limit", "remaining"]);
        assert!(doc_link_suggestions(&index, &scope, "nope", None, None).is_empty());
    }

    #[test]
    fn resolve_doc_link_finds_a_bare_local_hit() {
        let index = index_with(vec![(
            key("ratelimit", SymbolKind::Agent, "Limiter"),
            site("ratelimit.bynk", 10, 17),
            vec![],
        )]);
        let scope = doc_scope_with(vec![("ratelimit", vec!["ratelimit"])]);
        let def = resolve_doc_link(&index, &scope, "ratelimit", "Limiter").unwrap();
        assert_eq!(def.path, PathBuf::from("ratelimit.bynk"));
    }

    #[test]
    fn resolve_doc_link_finds_a_dotted_local_hit() {
        let index = index_with(vec![(
            key("ratelimit", SymbolKind::Field, "RateView.remaining"),
            site("ratelimit.bynk", 20, 29),
            vec![],
        )]);
        let scope = doc_scope_with(vec![("ratelimit", vec!["ratelimit"])]);
        let def = resolve_doc_link(&index, &scope, "ratelimit", "RateView.remaining").unwrap();
        assert_eq!(def.path, PathBuf::from("ratelimit.bynk"));
    }

    #[test]
    fn resolve_doc_link_follows_the_uses_chain() {
        // `decide` isn't declared in `ratelimit`, only in `window`, which
        // `ratelimit` `uses` — the scope order (built the same way
        // `doc_scope` is assembled) puts `window` after the local unit.
        let index = index_with(vec![(
            key("window", SymbolKind::Fn, "decide"),
            site("window.bynk", 5, 11),
            vec![],
        )]);
        let scope = doc_scope_with(vec![("ratelimit", vec!["ratelimit", "window"])]);
        let def = resolve_doc_link(&index, &scope, "ratelimit", "decide").unwrap();
        assert_eq!(def.path, PathBuf::from("window.bynk"));
    }

    #[test]
    fn resolve_doc_link_finds_an_asset_through_uses() {
        // #1883: an asset is a bare top-level name like a fn.
        let index = index_with(vec![(
            key("page", SymbolKind::Asset, "style"),
            site("page.bynk", 5, 10),
            vec![],
        )]);
        let scope = doc_scope_with(vec![("board", vec!["board", "page"])]);
        let def = resolve_doc_link(&index, &scope, "board", "style").unwrap();
        assert_eq!(def.path, PathBuf::from("page.bynk"));
    }

    #[test]
    fn resolve_doc_link_finds_an_embed_through_uses() {
        // #2387: an embed is a bare top-level name like an asset.
        let index = index_with(vec![(
            key("page", SymbolKind::Embed, "shell"),
            site("page.bynk", 5, 10),
            vec![],
        )]);
        let scope = doc_scope_with(vec![("board", vec!["board", "page"])]);
        let def = resolve_doc_link(&index, &scope, "board", "shell").unwrap();
        assert_eq!(def.path, PathBuf::from("page.bynk"));
    }

    #[test]
    fn resolve_doc_link_follows_the_consumes_chain_after_uses() {
        let index = index_with(vec![(
            key("platform", SymbolKind::Capability, "Clock"),
            site("platform.bynk", 0, 5),
            vec![],
        )]);
        // `uses` targets come before `consumes` targets in scope order; a name
        // only found via `consumes` still resolves, just later in the search.
        let scope = doc_scope_with(vec![("ratelimit", vec!["ratelimit", "window", "platform"])]);
        let def = resolve_doc_link(&index, &scope, "ratelimit", "Clock").unwrap();
        assert_eq!(def.path, PathBuf::from("platform.bynk"));
    }

    #[test]
    fn resolve_doc_link_is_none_when_nothing_in_scope_matches() {
        let index = index_with(vec![(
            key("ratelimit", SymbolKind::Agent, "Limiter"),
            site("ratelimit.bynk", 10, 17),
            vec![],
        )]);
        let scope = doc_scope_with(vec![("ratelimit", vec!["ratelimit"])]);
        assert!(resolve_doc_link(&index, &scope, "ratelimit", "Nope").is_none());
    }

    #[test]
    fn resolve_doc_link_is_none_for_a_synthetic_unit_never_in_scope() {
        // A synthetic (first-party) unit's symbols never make it into
        // `index.symbols` at all (their defs are dropped at assembly), so a
        // name that only exists through one comes back unresolved even
        // though `bynk` is a `consumes` target in scope.
        let index = ProjectIndex::default();
        let scope = doc_scope_with(vec![("ratelimit", vec!["ratelimit", "bynk"])]);
        assert!(resolve_doc_link(&index, &scope, "ratelimit", "Clock.now").is_none());
    }

    #[test]
    fn doc_link_owner_declared_sees_a_shadowing_project_declaration() {
        let index = index_with(vec![(
            key("window", SymbolKind::Type, "Clock"),
            site("window.bynk", 0, 5),
            vec![],
        )]);
        let scope = doc_scope_with(vec![("ratelimit", vec!["ratelimit", "bynk", "window"])]);
        assert!(doc_link_owner_declared(
            &index,
            &scope,
            "ratelimit",
            "Clock"
        ));
        assert!(!doc_link_owner_declared(&index, &scope, "ratelimit", "Kv"));
    }

    #[test]
    fn resolve_doc_link_is_none_when_the_owner_unit_has_no_scope_entry() {
        let index = ProjectIndex::default();
        let scope = doc_scope_with(vec![]);
        assert!(resolve_doc_link(&index, &scope, "ratelimit", "Limiter").is_none());
    }

    #[test]
    fn resolve_doc_link_never_guesses_an_ambiguous_first_matching_unit() {
        // Two different candidate kinds share the bare name `Order` in the
        // *first* scope-order unit — even though a later unit in scope order
        // would resolve unambiguously, the ambiguity in the first matching
        // unit wins: unresolved, never a guess.
        let index = index_with(vec![
            (
                key("ratelimit", SymbolKind::Type, "Order"),
                site("ratelimit.bynk", 10, 15),
                vec![],
            ),
            (
                key("ratelimit", SymbolKind::Fn, "Order"),
                site("ratelimit.bynk", 40, 45),
                vec![],
            ),
            (
                key("window", SymbolKind::Type, "Order"),
                site("window.bynk", 0, 5),
                vec![],
            ),
        ]);
        let scope = doc_scope_with(vec![("ratelimit", vec!["ratelimit", "window"])]);
        assert!(resolve_doc_link(&index, &scope, "ratelimit", "Order").is_none());
    }

    #[test]
    fn doc_link_matches_reports_every_kind_in_the_first_matching_unit() {
        let index = index_with(vec![
            (
                key("ratelimit", SymbolKind::Type, "Order"),
                site("ratelimit.bynk", 10, 15),
                vec![],
            ),
            (
                key("ratelimit", SymbolKind::Fn, "Order"),
                site("ratelimit.bynk", 40, 45),
                vec![],
            ),
        ]);
        let scope = doc_scope_with(vec![("ratelimit", vec!["ratelimit"])]);
        let kinds = |name| {
            doc_link_matches(&index, &scope, "ratelimit", name)
                .map(|m| m.iter().map(|(k, _)| k.kind).collect::<Vec<_>>())
        };
        assert_eq!(kinds("Order"), Some(vec![SymbolKind::Type, SymbolKind::Fn]));
        assert_eq!(kinds("Nope"), None);
    }

    #[test]
    fn named_type_target_unwraps_single_param_containers() {
        use bynk_check::checker::{NamedKind, Types};
        use bynk_syntax::ast::BaseType;
        let tys = &Types::new();
        let order = || {
            tys.intern(Ty::Named {
                name: "Order".into(),
                kind: NamedKind::Record,
                args: Vec::new(),
            })
        };
        let target = |t| named_type_target(t, tys);
        assert_eq!(target(order()).as_deref(), Some("Order"));
        assert_eq!(
            target(tys.intern(Ty::Option(order()))).as_deref(),
            Some("Order")
        );
        // Nested single-param containers unwrap all the way.
        let effect = tys.intern(Ty::Effect(order()));
        assert_eq!(
            target(tys.intern(Ty::List(effect))).as_deref(),
            Some("Order")
        );
        // Built-in, two-parameter, and unit types have no single target.
        assert_eq!(target(tys.intern(Ty::Base(BaseType::Int))), None);
        assert_eq!(target(tys.intern(Ty::Result(order(), order()))), None);
        assert_eq!(target(tys.intern(Ty::Unit)), None);
    }

    fn ctx_locals(name: &str, def: (usize, usize), scope: (usize, usize)) -> LocalBinding {
        LocalBinding {
            name: name.to_string(),
            def_span: Span::new(def.0, def.1),
            kind: LocalKind::Param,
            ty: "Int".to_string(),
            scope: Span::new(scope.0, scope.1),
        }
    }

    #[test]
    fn resolve_doc_link_in_prefers_an_own_field_over_a_unit_name() {
        // `id` is both `View`'s field and a unit-level fn: inside `View`'s doc
        // the field wins; with no context the fn does.
        let index = index_with(vec![
            (
                key("u", SymbolKind::Type, "View"),
                site("u.bynk", 10, 14),
                vec![],
            ),
            (
                key("u", SymbolKind::Field, "View.id"),
                site("u.bynk", 20, 22),
                vec![],
            ),
            (
                key("u", SymbolKind::Fn, "id"),
                site("u.bynk", 90, 92),
                vec![],
            ),
        ]);
        let scope = doc_scope_with(vec![("u", vec!["u"])]);
        let ctx = DocLinkContext {
            path: Path::new("u.bynk"),
            text: "",
            attached: 10,
            locals: &[],
        };
        let hit = resolve_doc_link_in(&index, &scope, "u", "id", Some(&ctx)).unwrap();
        assert_eq!(hit.def.span.start, 20);
        assert_eq!(hit.key.unwrap().kind, SymbolKind::Field);
        let hit = resolve_doc_link_in(&index, &scope, "u", "id", None).unwrap();
        assert_eq!(hit.def.span.start, 90);
    }

    #[test]
    fn resolve_doc_link_in_finds_a_parameter_but_not_a_lambdas() {
        let text = "fn f(order: Int) -> Int { xs.fold(0, (acc, x) => acc + x) }\n\
                    fn total() -> Int { xs.fold(0, (acc, x) => acc + x) }\n";
        let at = |needle: &str, from: usize| from + text[from..].find(needle).unwrap();
        let (f, total) = (at("f(", 0), at("total", 0));
        let span = |start: usize, n: &str| (start, start + n.len());
        let order = span(at("order", 0), "order");
        let lambda = |from: usize| {
            let acc = at("acc", from);
            let x = at("x)", acc);
            (span(acc, "acc"), span(x, "x"))
        };
        let (acc1, x1) = lambda(f);
        let (acc2, x2) = lambda(total);
        let body1 = (at("{", f), at("}", f) + 1);
        let body2 = (at("{", total), at("}", total) + 1);
        let index = index_with(vec![
            (
                key("u", SymbolKind::Fn, "f"),
                site("u.bynk", f, f + 1),
                vec![],
            ),
            (
                key("u", SymbolKind::Fn, "total"),
                site("u.bynk", total, total + 5),
                vec![],
            ),
        ]);
        let scope = doc_scope_with(vec![("u", vec!["u"])]);
        let locals = vec![
            ctx_locals("order", order, body1),
            ctx_locals("acc", acc1, (body1.0 + 20, body1.1)),
            ctx_locals("x", x1, (body1.0 + 20, body1.1)),
            ctx_locals("acc", acc2, (body2.0 + 20, body2.1)),
            ctx_locals("x", x2, (body2.0 + 20, body2.1)),
        ];
        let ctx = |attached| DocLinkContext {
            path: Path::new("u.bynk"),
            text,
            attached,
            locals: &locals,
        };
        let hit = resolve_doc_link_in(&index, &scope, "u", "order", Some(&ctx(f))).unwrap();
        assert_eq!(hit.def.span, Span::new(order.0, order.1));
        assert!(hit.key.is_none());
        // A lambda's binder is not the fn's parameter, with or without parameters of its own.
        assert!(resolve_doc_link_in(&index, &scope, "u", "x", Some(&ctx(f))).is_none());
        assert!(resolve_doc_link_in(&index, &scope, "u", "x", Some(&ctx(total))).is_none());
        assert!(resolve_doc_link_in(&index, &scope, "u", "acc", Some(&ctx(total))).is_none());
        // Another declaration's parameter is not this one's.
        assert!(resolve_doc_link_in(&index, &scope, "u", "order", Some(&ctx(total))).is_none());
    }

    #[test]
    fn resolve_doc_link_in_reads_a_unit_qualified_name_outside_scope() {
        let index = index_with(vec![
            (
                key("a.b", SymbolKind::Fn, "decide"),
                site("b.bynk", 5, 11),
                vec![],
            ),
            (
                key("u", SymbolKind::Field, "View.decide"),
                site("u.bynk", 20, 26),
                vec![],
            ),
        ]);
        let scope = doc_scope_with(vec![("u", vec!["u"])]);
        let hit = resolve_doc_link_in(&index, &scope, "u", "a.b.decide", None).unwrap();
        assert_eq!(hit.def.path, PathBuf::from("b.bynk"));
        // `Owner.member` is still read first.
        let hit = resolve_doc_link_in(&index, &scope, "u", "View.decide", None).unwrap();
        assert_eq!(hit.def.path, PathBuf::from("u.bynk"));
        assert!(resolve_doc_link_in(&index, &scope, "u", "zz.decide", None).is_none());
    }
}
