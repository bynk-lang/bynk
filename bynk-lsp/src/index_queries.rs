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

use bynk_check::checker::{Ty, TyId, Types};
use bynk_check::index::{ProjectIndex, SiteRef, SymbolKey, SymbolKind};
use bynk_check::locals::{LocalBinding, LocalKind};
use bynk_syntax::ast::BaseType;
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
const DOC_LINK_MEMBER_KINDS: &[SymbolKind] = &[
    SymbolKind::Field,
    SymbolKind::Method,
    SymbolKind::CapabilityOp,
    SymbolKind::Handler,
];

/// #848: the top-level kinds a *bare* doc-link name matches.
const DOC_LINK_BARE_KINDS: &[SymbolKind] = &[
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
    let (_, def) = resolve_in_unit_scope(index, doc_scope, owner_unit, name)?;
    Some(def)
}

/// #2454: the declaration a doc comment is attached to, as far as link
/// resolution needs it: `attached` is the byte offset of its name in `path`
/// (`bynk_ide::symbols::DocLinkSpan::attached`), and `locals` that file's
/// local bindings, from which a fn / handler / op's parameters are read.
pub struct DocLinkContext<'a> {
    pub path: &'a Path,
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
    if let Some((key, def)) = resolve_in_unit_scope(index, doc_scope, owner_unit, name) {
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

/// The unit-scope walk of [`resolve_doc_link`], also returning the key.
fn resolve_in_unit_scope<'a>(
    index: &'a ProjectIndex,
    doc_scope: &HashMap<String, Vec<String>>,
    owner_unit: &str,
    name: &str,
) -> Option<(SymbolKey, &'a SiteRef)> {
    let kinds: &[SymbolKind] = if name.contains('.') {
        DOC_LINK_MEMBER_KINDS
    } else {
        DOC_LINK_BARE_KINDS
    };
    let scope = doc_scope.get(owner_unit)?;
    for unit in scope {
        let mut hit = None;
        let mut count = 0usize;
        for &kind in kinds {
            let key = SymbolKey {
                unit: unit.clone(),
                kind,
                name: name.to_string(),
            };
            if let Some(def) = index.symbols.get(&key).and_then(|e| e.def.as_ref()) {
                count += 1;
                hit = Some((key, def));
            }
        }
        match count {
            0 => continue,
            1 => return hit,
            _ => return None, // ambiguous in the first matching unit — never guess
        }
    }
    None
}

/// Step 1 of [`resolve_doc_link_in`]: `name` as a member or parameter of the
/// declaration whose name sits at `ctx.attached`.
fn resolve_own_part(
    index: &ProjectIndex,
    ctx: &DocLinkContext<'_>,
    name: &str,
) -> Option<ResolvedDocLink> {
    let at = |d: &SiteRef| d.path == ctx.path && d.span.start == ctx.attached;
    let (owner, _) = index
        .symbols
        .iter()
        .find(|(_, e)| e.def.as_ref().is_some_and(at))?;
    let member = format!("{}.{name}", owner.name);
    if let Some((key, def)) = unique_hit(index, &owner.unit, &member, DOC_LINK_MEMBER_KINDS) {
        return Some(ResolvedDocLink {
            def: def.clone(),
            key: Some(key),
        });
    }
    // The declaration's parameters: `Param` bindings after its name and before
    // the next definition in the file, and not nested inside another binding's
    // scope (which excludes a lambda's parameters in a body).
    let next_def = index
        .symbols
        .values()
        .filter_map(|e| e.def.as_ref())
        .filter(|d| d.path == ctx.path && d.span.start > ctx.attached)
        .map(|d| d.span.start)
        .min()
        .unwrap_or(usize::MAX);
    let param = ctx.locals.iter().find(|b| {
        b.kind == LocalKind::Param
            && b.name == name
            && b.def_span.start > ctx.attached
            && b.def_span.start < next_def
            && !ctx.locals.iter().any(|o| {
                o.scope.start <= b.def_span.start
                    && b.def_span.end <= o.scope.end
                    && o.def_span != b.def_span
            })
    })?;
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
    pre.equals_modulo_rename(post, &plan.key, &target, |s| remap_site(s, plan))
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

// -- v0.28 (ADR 0057): semantic tokens --

/// The frozen semantic-tokens legend. **Array order is the wire encoding**
/// (clients index into these arrays): entries are append-only, never
/// reordered — pinned by the legend-stability test. Token types: standard
/// where faithful (`type`, `function`), custom for the Bynk-distinctive
/// kinds (`capability`, `service`, `agent`, `provider`).
pub fn semantic_tokens_legend() -> tower_lsp::lsp_types::SemanticTokensLegend {
    use tower_lsp::lsp_types::{SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::TYPE,
            SemanticTokenType::FUNCTION,
            SemanticTokenType::new("capability"),
            SemanticTokenType::new("service"),
            SemanticTokenType::new("agent"),
            SemanticTokenType::new("provider"),
            // v0.31 (ADR 0064): local bindings + params. Standard LSP type —
            // VS Code themes it by default, no extension declaration needed.
            SemanticTokenType::VARIABLE,
            // v0.36 (ADR 0069): instance methods. Appended (never reordered) so
            // existing legend indices are unchanged. Standard LSP type.
            SemanticTokenType::METHOD,
            // v0.36 (ADR 0069, slice 2): record fields. Appended. Standard LSP
            // type. (Capability ops reuse `method` — they're operation calls.)
            SemanticTokenType::PROPERTY,
            // v0.45: actor declarations. Appended at index 9 (never reordered).
            // Custom type — the VS Code extension declares it in package.json.
            SemanticTokenType::new("actor"),
            // v0.140 (ADR 0163): handler-position annotations (`@cache` and its
            // argument labels). Appended at index 10. Standard LSP type — VS Code
            // themes `decorator` by default.
            SemanticTokenType::DECORATOR,
            // message-bundles slice 1: `messages <tag> { ... }` bundles.
            // Appended at index 11 (never reordered). Custom type — the VS
            // Code extension declares it in package.json.
            SemanticTokenType::new("messages"),
            // #1888: a `--|` doc line. Appended at index 12 (never
            // reordered); standard LSP type, with the `documentation`
            // modifier. #2203: no longer emitted (one-line docs, like `---`
            // blocks, are left to the editor grammars), but the legend is
            // append-only, so the entry stays.
            SemanticTokenType::COMMENT,
            // #1883: `asset` names (static files). Appended at index 13
            // (never reordered). Custom type — the VS Code extension declares
            // it in package.json, as a `variable` subtype.
            SemanticTokenType::new("asset"),
            // #2387: `embed` names (embedded text). Appended at index 14
            // (never reordered); a custom type the VS Code extension declares.
            SemanticTokenType::new("embed"),
        ],
        token_modifiers: vec![
            SemanticTokenModifier::DECLARATION,
            SemanticTokenModifier::new("refined"),
            SemanticTokenModifier::new("opaque"),
            SemanticTokenModifier::new("platformNative"),
            // #1888: on a `--|` doc line's `comment` token. Appended at bit 4.
            // #2203: no longer emitted; kept, as the legend is append-only.
            SemanticTokenModifier::DOCUMENTATION,
        ],
    }
}

/// Legend indices/bits — must mirror [`semantic_tokens_legend`]'s order.
fn token_type_index(kind: SymbolKind) -> u32 {
    match kind {
        SymbolKind::Type => 0,
        SymbolKind::Fn => 1,
        SymbolKind::Capability => 2,
        SymbolKind::Service => 3,
        SymbolKind::Agent => 4,
        SymbolKind::Provider => 5,
        // 6 is `variable` (locals; TOK_LOCAL); methods append at 7.
        SymbolKind::Method => 7,
        // v0.36 slice 2: ops reuse `method` (7); fields append `property` at 8.
        SymbolKind::CapabilityOp => 7,
        SymbolKind::Field => 8,
        // v0.45: actors append `actor` at 9.
        SymbolKind::Actor => 9,
        // #304: agent handlers reuse `method` (7), same as capability ops.
        SymbolKind::Handler => 7,
        // message-bundles slice 1: `messages` bundles append `messages` at 11.
        SymbolKind::Messages => 11,
        // #1883: asset names append `asset` at 13.
        SymbolKind::Asset => 13,
        // #2387: embed names append `embed` at 14.
        SymbolKind::Embed => 14,
    }
}

/// Legend index of the `variable` token type (locals; ADR 0064).
const TOK_LOCAL: u32 = 6;

/// Legend index of the `decorator` token type (handler annotations; ADR 0163).
const TOK_DECORATOR: u32 = 10;

const MOD_DECLARATION: u32 = 1 << 0;
const MOD_REFINED: u32 = 1 << 1;
const MOD_OPAQUE: u32 = 1 << 2;
const MOD_PLATFORM_NATIVE: u32 = 1 << 3;

fn modifier_bits(m: bynk_check::index::SymbolModifiers) -> u32 {
    (if m.refined { MOD_REFINED } else { 0 })
        | (if m.opaque { MOD_OPAQUE } else { 0 })
        | (if m.platform_native {
            MOD_PLATFORM_NATIVE
        } else {
            0
        })
}

/// Semantic tokens for `path`, delta-encoded over the frozen legend —
/// a pure read of the cached index's two sources: `symbols` (user
/// defs+refs; a def site carries `declaration`) and `foreign_refs`
/// (first-party references). `range` (byte offsets into `text`, the
/// analysed snapshot) filters to overlapping tokens for the `…/range`
/// request; `None` is the full document.
pub fn semantic_tokens(
    index: &ProjectIndex,
    local_tokens: &[(Span, bool)],
    decorator_tokens: &[Span],
    path: &Path,
    text: &str,
    range: Option<Span>,
) -> Vec<tower_lsp::lsp_types::SemanticToken> {
    let in_scope = |span: Span| {
        span.end <= text.len() && range.is_none_or(|r| span.end > r.start && span.start < r.end)
    };
    let mut raw: Vec<(Span, u32, u32)> = Vec::new();
    for (key, entry) in &index.symbols {
        let ty = token_type_index(key.kind);
        let mods = modifier_bits(entry.modifiers);
        if let Some(def) = &entry.def
            && def.path == path
            && in_scope(def.span)
        {
            raw.push((def.span, ty, mods | MOD_DECLARATION));
        }
        for site in &entry.refs {
            if site.path == path && in_scope(site.span) {
                raw.push((site.span, ty, mods));
            }
        }
    }
    for fr in &index.foreign_refs {
        if fr.site.path == path && in_scope(fr.site.span) {
            raw.push((
                fr.site.span,
                token_type_index(fr.kind),
                modifier_bits(fr.modifiers),
            ));
        }
    }
    // v0.31 (ADR 0064): local bindings + their uses (precomputed by the caller
    // via `locals_nav`, so this stays free of that dependency for the
    // `#[path]`-include tests). Disjoint from the index tokens — locals are
    // never top-level symbols — so they merge into the same sorted stream.
    for &(span, is_decl) in local_tokens {
        if in_scope(span) {
            raw.push((span, TOK_LOCAL, if is_decl { MOD_DECLARATION } else { 0 }));
        }
    }
    // v0.140 (ADR 0163): handler-annotation name + argument-label spans (precomputed
    // by the caller from the parsed unit, keeping this a parse-free index read).
    // Disjoint from the index/local tokens — annotations are neither symbols nor
    // locals — so they merge into the same sorted stream.
    for &span in decorator_tokens {
        if in_scope(span) {
            raw.push((span, TOK_DECORATOR, 0));
        }
    }
    // Name segments never overlap (the index invariant), so a position
    // sort fully determines the protocol's relative encoding.
    raw.sort_by_key(|(span, _, _)| (span.start, span.end));
    let mut data = Vec::with_capacity(raw.len());
    // One line index for the whole snapshot: a `semanticTokens/full` request
    // emits a position per token, so scanning from byte 0 each time is O(n²)
    // (#732). Build it once and binary-search per token instead.
    let positions = crate::position::PositionMap::new(text);
    let (mut prev_line, mut prev_start) = (0u32, 0u32);
    for (span, token_type, token_modifiers_bitset) in raw {
        let pos = positions.position(span.start);
        let delta_line = pos.line - prev_line;
        let delta_start = if delta_line == 0 {
            pos.character - prev_start
        } else {
            pos.character
        };
        data.push(tower_lsp::lsp_types::SemanticToken {
            delta_line,
            delta_start,
            // The protocol counts in the negotiated encoding (UTF-16, as
            // positions are) — not bytes.
            length: text[span.range()].encode_utf16().count() as u32,
            token_type,
            token_modifiers_bitset,
        });
        prev_line = pos.line;
        prev_start = pos.character;
    }
    data
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

    // -- v0.28 (ADR 0057): semantic tokens --

    /// The legend's array order IS the wire encoding: this test freezes it.
    /// New entries APPEND — a failure here means a silent recolour of every
    /// client; never fix it by reordering.
    #[test]
    fn legend_is_frozen() {
        let legend = semantic_tokens_legend();
        let types: Vec<&str> = legend.token_types.iter().map(|t| t.as_str()).collect();
        assert_eq!(
            types,
            [
                "type",
                "function",
                "capability",
                "service",
                "agent",
                "provider",
                "variable",  // v0.31 (ADR 0064): locals — appended, never reordered
                "method",    // v0.36 (ADR 0069): instance methods — appended
                "property",  // v0.36 (ADR 0069, slice 2): record fields — appended
                "actor",     // v0.45: actor declarations — appended
                "decorator", // v0.140 (ADR 0163): handler annotations — appended
                "messages",  // message-bundles slice 1 (#859): messages bundles — appended
                "comment",   // #1888: `--|` doc lines — appended
                "asset",     // #1883: static asset names — appended
                "embed",     // #2387: embedded-text names — appended
            ]
        );
        let modifiers: Vec<&str> = legend.token_modifiers.iter().map(|m| m.as_str()).collect();
        assert_eq!(
            modifiers,
            [
                "declaration",
                "refined",
                "opaque",
                "platformNative",
                "documentation"
            ]
        );
    }

    /// #2203: a one-line doc gets no semantic token, as a `---` block gets
    /// none, so the editor grammar's Markdown highlighting shows through; the
    /// legend keeps its `comment` type and `documentation` modifier.
    #[test]
    fn doc_lines_get_no_semantic_token() {
        let text = "commons m\n\n  --- A doc.\n// a comment\ntype T = Int\n";
        let tokens = semantic_tokens(
            &ProjectIndex::default(),
            &[],
            &[],
            Path::new("a.bynk"),
            text,
            None,
        );
        assert!(tokens.is_empty(), "{tokens:?}");
        let legend = semantic_tokens_legend();
        assert_eq!(legend.token_types[12].as_str(), "comment");
        assert_eq!(legend.token_modifiers[4].as_str(), "documentation");
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

    #[test]
    fn tokens_are_delta_encoded_with_modifier_bitsets() {
        // text:  line 0: "type Age = Int"   (def `Age` at 5..8, refined)
        //        line 1: "fn f(a: Age) ..." (ref `Age` at 23..26)
        let text = "type Age = Int\nfn f(a: Age) -> Age {}\n";
        let mut index = index_with(vec![(
            key("shop", SymbolKind::Type, "Age"),
            site("a.bynk", 5, 8),
            vec![site("a.bynk", 23, 26), site("a.bynk", 31, 34)],
        )]);
        index
            .symbols
            .get_mut(&key("shop", SymbolKind::Type, "Age"))
            .unwrap()
            .modifiers = bynk_check::index::SymbolModifiers {
            refined: true,
            ..Default::default()
        };
        let tokens = semantic_tokens(&index, &[], &[], Path::new("a.bynk"), text, None);
        assert_eq!(tokens.len(), 3);
        // Def: line 0 char 5, length 3, type `type` (0), declaration|refined.
        assert_eq!(
            (
                tokens[0].delta_line,
                tokens[0].delta_start,
                tokens[0].length
            ),
            (0, 5, 3)
        );
        assert_eq!(tokens[0].token_type, 0);
        assert_eq!(tokens[0].token_modifiers_bitset, 0b0011);
        // First ref: next line, char 8 (absolute — line changed), refined only.
        assert_eq!(
            (
                tokens[1].delta_line,
                tokens[1].delta_start,
                tokens[1].length
            ),
            (1, 8, 3)
        );
        assert_eq!(tokens[1].token_modifiers_bitset, 0b0010);
        // Second ref: same line, char delta from the previous token's start.
        assert_eq!(
            (
                tokens[2].delta_line,
                tokens[2].delta_start,
                tokens[2].length
            ),
            (0, 8, 3)
        );
    }

    #[test]
    fn foreign_refs_emit_tokens_and_range_filters() {
        let text = "given Kv {\n  Kv.get(k)\n}\n";
        let mut index = ProjectIndex::default();
        index.foreign_refs.push(bynk_check::index::ForeignRef {
            site: site("a.bynk", 6, 8),
            kind: SymbolKind::Capability,
            modifiers: bynk_check::index::SymbolModifiers {
                platform_native: true,
                ..Default::default()
            },
        });
        index.foreign_refs.push(bynk_check::index::ForeignRef {
            site: site("a.bynk", 13, 15),
            kind: SymbolKind::Capability,
            modifiers: bynk_check::index::SymbolModifiers {
                platform_native: true,
                ..Default::default()
            },
        });
        let all = semantic_tokens(&index, &[], &[], Path::new("a.bynk"), text, None);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].token_type, 2); // capability
        assert_eq!(all[0].token_modifiers_bitset, 0b1000); // platformNative
        // Range covering only line 0 keeps only the first token.
        let ranged = semantic_tokens(
            &index,
            &[],
            &[],
            Path::new("a.bynk"),
            text,
            Some(Span::new(0, 10)),
        );
        assert_eq!(ranged.len(), 1);
        // Other files and empty indexes yield nothing.
        assert!(semantic_tokens(&index, &[], &[], Path::new("b.bynk"), text, None).is_empty());
        assert!(
            semantic_tokens(
                &ProjectIndex::default(),
                &[],
                &[],
                Path::new("a.bynk"),
                text,
                None
            )
            .is_empty()
        );
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
        let index = index_with(vec![
            (
                key("u", SymbolKind::Fn, "f"),
                site("u.bynk", 10, 11),
                vec![],
            ),
            (
                key("u", SymbolKind::Fn, "g"),
                site("u.bynk", 200, 201),
                vec![],
            ),
        ]);
        let scope = doc_scope_with(vec![("u", vec!["u"])]);
        let locals = vec![
            ctx_locals("order", (12, 17), (40, 120)),
            // a lambda parameter nested in `f`'s body
            ctx_locals("inner", (60, 65), (66, 80)),
            // a parameter of the next fn
            ctx_locals("other", (202, 207), (230, 260)),
        ];
        let ctx = DocLinkContext {
            path: Path::new("u.bynk"),
            attached: 10,
            locals: &locals,
        };
        let hit = resolve_doc_link_in(&index, &scope, "u", "order", Some(&ctx)).unwrap();
        assert_eq!(hit.def.span, Span::new(12, 17));
        assert!(hit.key.is_none());
        assert!(resolve_doc_link_in(&index, &scope, "u", "inner", Some(&ctx)).is_none());
        assert!(resolve_doc_link_in(&index, &scope, "u", "other", Some(&ctx)).is_none());
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
