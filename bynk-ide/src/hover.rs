//! #611 (ADR 0190): hover's resolution ladder, as one pure function.
//!
//! #2520: lives in `bynk-ide` so the language server and `bynk mcp` render the
//! same answer.
//!
//! `Backend::hover` is transport — it resolves the cursor position, gathers the
//! round's tables and the live buffer, and packages the result as an LSP
//! `Hover`. The *order* the rungs are tried in is the behaviour: #611's gap B was
//! a fall-through bug, where a rung that resolved the offset correctly but
//! rendered nothing let a later, name-matching rung answer instead — a
//! confidently wrong hover. That order lives here, once, so a test can **pin** it
//! rather than replicate it (a replica agrees with the original only until
//! someone reorders one of them).
//!
//! Two text sources, deliberately: the index rungs read the round's **analysed
//! snapshot** (the tables' spans index into it), while the lexical rungs read the
//! **live buffer**, which is what makes hover work mid-edit. They diverge while
//! the user types, so the ladder keeps them distinct rather than assuming one.
//!
//! The rungs, in trial order (see [`hover_content`]'s inline numbering):
//! 1. the binding index — a resolved symbol reference, described from its
//!    defining file (a resolved `Service` gets a #855 wire-contract appendix);
//! 2. a `store` field operation;
//! 3. a local/param/`self`;
//! 4. #855: a handler's HEADER (`on`, the method/kind token, the route
//!    literal) — guarded to that byte range so it cannot shadow rung 1;
//!    4b. #2460: a doc-comment `[Name]` link — the cursor inside a doc-link
//!    candidate span previews the link's target, resolved as the link itself
//!    is (#2454: the attached declaration's own parts first), or says it is
//!    ambiguous or unresolved;
//!    guarded to doc-link spans, which hold no index reference, so it cannot
//!    shadow rung 1;
//! 5. #1885: a unit name — the header's, or a `uses`/`consumes`/suite target —
//!    shown with the unit's module doc (lexical, first live-buffer rung);
//! 6. a top-level declaration by name;
//! 7. the `key`/`store` contextual keywords and agent-state references;
//! 8. a handler-position `@cache` annotation;
//! 9. a `Recv.member` name-receiver access;
//! 10. a project-wide name scan;
//! 11. the embedded first-party sources.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_check::checker::Types;
use bynk_check::index::SymbolKind;
use bynk_syntax::span::Span;

use crate::index_queries::{DocLinkResolution, resolve_doc_link_in};

/// The analysed round's tables, positioned at the cursor.
pub struct HoverAnalysis<'a> {
    pub index: &'a bynk_check::index::ProjectIndex,
    /// Project-relative path → the analysed text.
    pub snapshots: &'a HashMap<PathBuf, String>,
    pub locals: &'a bynk_check::locals::FileLocals,
    pub expr_types: &'a bynk_check::expr_types::FileExprTypes,
    /// T3.6b (R4.1): the table `expr_types`' `TyId`s resolve against.
    pub tys: &'a Types,
    /// The cursor's file (project-relative) and its offset **into the snapshot**.
    pub rel: &'a Path,
    pub offset: usize,
    /// #2469: the round's capability paths, for the `given`-entry hover.
    pub capability_paths: &'a bynk_check::capability_paths::FileCapabilityPaths,
    /// #848: the round's project root — resolves a doc-link's `SiteRef` into
    /// a `file://` hover-Markdown target.
    pub project_root: &'a Path,
    /// #848: qualified unit name → its doc-comment intra-doc-link search
    /// order, for rung 1's doc-link rewrite.
    pub doc_scope: &'a HashMap<String, Vec<String>>,
    /// #855: qualified context/adapter unit name → the combined type table
    /// and service/agent tables the wire-contract peek needs.
    pub boundary_info: &'a HashMap<String, crate::ContextBoundaryInfo>,
    /// #855: how many **real** contexts/adapters this project has — computed
    /// once per round by the transport with
    /// `crate::wire_contract::real_context_count`, never re-derived here
    /// as a bare `boundary_info.len()` (see that function's doc: the
    /// synthetic toolchain `bynk` capability unit would otherwise count as a
    /// second "context" and `NoCrossContextReason::SingleContext` would
    /// almost never fire).
    pub context_count: usize,
}

/// Everything the ladder reads.
pub struct HoverInput<'a> {
    /// The analysed round; `None` when the file is outside it — the lexical
    /// rungs still answer from the live buffer.
    pub analysis: Option<HoverAnalysis<'a>>,
    /// The **live** buffer and the cursor's offset into it; `None` when the
    /// document is not open — the index rungs still answer from the snapshot.
    pub doc: Option<(&'a str, usize)>,
    /// The cursor's file as the caller knows it: it is excluded from the
    /// cross-file declaration scan, which looks for the name *elsewhere*.
    /// `None` (a document with no file path) skips that scan.
    pub path: Option<&'a Path>,
    /// Content-ownership track (#1086): a pre-read `(path, content)` map for
    /// the project's `.bynk` files (the caller's overlay-then-disk sweep) —
    /// was a bare path list the resolution rungs below read from disk
    /// themselves. Every rung that reads project files (8's `resolve_label`,
    /// 9's `describe_symbol_cross_file`) takes this map directly now.
    pub files: Option<&'a HashMap<PathBuf, String>>,
}

/// The hover Markdown for the cursor, or `None` when no rung resolves it.
///
/// The rung order is the contract; see the module doc. Each rung is tried in
/// turn and the first `Some` wins.
pub fn hover_content(input: &HoverInput<'_>) -> Option<String> {
    if let Some(a) = &input.analysis {
        let tys = a.tys;
        // 1. v0.25: binding-index path — a resolved symbol reference, described
        //    from its *defining* file (names are unique per file, so the per-file
        //    lookup is exact). Binding-correct: a duplicate name in another unit
        //    describes the bound declaration, not the first name match.
        //
        //    v0.166 (ADR 0191): every `SymbolKind` now has a renderer arm, so a
        //    resolved key is answered here rather than falling through to a name
        //    match below. The `Some` guard remains — it is what a new kind added
        //    without an arm would fall through, silently, which is how
        //    `Method`/`CapabilityOp` came to render the wrong declaration.
        if let Some((key, def)) = crate::index_queries::definition_at(a.index, a.rel, a.offset)
            && let Some(def_text) = a.snapshots.get(&def.path)
            && let Some(content) = crate::symbols::describe_symbol(def_text, &key.name)
        {
            // #848: rewrite any resolvable intra-doc link in the rendered
            // doc comment into a Markdown link, scoped to `key.unit` (the
            // declaring unit doc-link resolution searches from).
            let ctx = crate::index_queries::DocLinkContext {
                path: &def.path,
                text: def_text,
                attached: def.span.start,
                locals: a.locals.get(&def.path).map_or(&[], |l| l.as_slice()),
            };
            let content = crate::doc_link_urls::linkify_doc_links(
                &content,
                a.index,
                a.doc_scope,
                a.project_root,
                a.snapshots,
                &key.unit,
                Some(&ctx),
            );
            // #855: a resolved `Service` symbol (the declaration's own name,
            // or a cross-context/test-service call site — both record this
            // kind, see `bynk-emit/src/project/symbols.rs`) gets its wire
            // contract appended after everything `describe_symbol` already
            // rendered. This changes only the *tail* of what rung 1 already
            // returns `Some` for — never whether it answers.
            let content = if key.kind == SymbolKind::Service {
                match wire_contract_for_service_key(a, key, &def.path, def_text) {
                    Some(model) => format!("{content}{}", wire_contract_appendix(&model)),
                    None => content,
                }
            } else {
                content
            };
            return Some(with_capability_path(content, a));
        }
        // 1a. #2469: a `given` entry whose capability rung 1 could not describe
        //    still explains why the declaration needs it.
        if let Some(section) = capability_path_section(a) {
            return Some(section);
        }
        // 2. #611 (gap C): a `store` field's operation (`items.put(…)`) — a
        //    structural match on the enclosing agent's declared field, so it
        //    outranks the locals rung below, which guesses by name in scope.
        //    Takes `locals` to honour the checker's by-provenance dispatch: a
        //    local shadowing the field makes this an ordinary value method.
        if let Some(text) = a.snapshots.get(a.rel)
            && let Some(content) = crate::symbols::describe_store_op_at(
                text,
                a.offset,
                a.locals.get(a.rel).map_or(&[], |l| l.as_slice()),
            )
        {
            return Some(content);
        }
        // 3. v0.122 (slice 1): a local / parameter → its inferred type, and
        //    `self` → its receiver/agent type. Both read the retained analysis
        //    tables and run before the lexical fallback, which knows only
        //    declarations by name.
        if let Some(text) = a.snapshots.get(a.rel) {
            let local = a
                .locals
                .get(a.rel)
                .and_then(|locals| crate::locals_nav::describe_local_at(locals, text, a.offset))
                .or_else(|| {
                    let entries = a.expr_types.get(a.rel)?;
                    crate::symbols::describe_self_at(text, a.offset, entries, tys)
                });
            if let Some(content) = local {
                return Some(content);
            }
        }
        // 3a. #2683, #2711: the `within` of `e.within(d)` or the `race` of
        //     `a.race(b)` on an `Effect`.
        if let Some(text) = a.snapshots.get(a.rel)
            && let Some(entries) = a.expr_types.get(a.rel)
            && let Some(content) =
                crate::symbols::describe_effect_method_at(text, a.offset, entries, tys)
        {
            return Some(content);
        }
        // 4. #855: the handler HEADER — the `on` keyword, the HTTP method /
        //    handler-kind token, and any route string literal. No earlier rung
        //    (nor any later lexical one) ever answers here today: `GET` lexes
        //    as a bare `Ident` with no index symbol, and no top-level
        //    declaration is named after it. Guarded to the header's own byte
        //    range — from the handler's span start up to (but excluding) its
        //    first param's span start, or its body's span start with no
        //    params — which contains only tokens that are never an index
        //    symbol, so this rung is provably unable to shadow rung 1's
        //    resolution of, say, a param's *type* name (`client: ClientId`),
        //    which always sits past that boundary. See `header_handler_at`.
        if let Some(text) = a.snapshots.get(a.rel)
            && let Some((service_name, handler)) = header_handler_at(text, a.offset)
            && let Some((unit, _)) = crate::symbols::own_declaration_name(text)
            && let Some(info) = a.boundary_info.get(&unit)
        {
            let expr_types = a.expr_types.get(a.rel).map(|v| v.as_slice()).unwrap_or(&[]);
            if let Some(model) = crate::wire_contract::wire_contract_for_service(
                &unit,
                text,
                &service_name,
                &handler,
                info,
                expr_types,
                tys,
                a.context_count,
            ) {
                return Some(wire_contract_standalone(&model));
            }
        }
        // 4b. #2460: the cursor inside a doc-comment `[Name]` link previews
        //     its target — `describe_symbol` of the resolved key, exactly as
        //     hovering the name in code would — or reports it unresolved.
        //     Doc-link spans are comment text, which no index reference or
        //     local covers, so this cannot shadow rungs 1-4.
        if let Some(text) = a.snapshots.get(a.rel)
            && let Some(content) = doc_link_hover(a, text)
        {
            return Some(content);
        }
    }

    // The remaining rungs are lexical, over the **live** buffer.
    let (text, offset) = input.doc?;
    let Some((name, span)) = identifier_at(text, offset) else {
        // v0.121 (ADR 0156): the mechanical coverage test requires every
        // lowercase-initial keyword to have *a* hover path. A bare keyword token
        // (`requires`, `suite`, …) never resolves as an identifier above, so it
        // falls here — its one-line `keywords` registry doc, the same text
        // completion shows. Richer per-declaration hover is `describe_symbol`'s
        // job, not this fallback's.
        return crate::symbols::describe_keyword_at(text, offset).map(str::to_string);
    };
    // 5. #1885: a unit name — the file's own header name, or a `uses` /
    //    `consumes` / suite target path — shows the unit's module doc. Guarded
    //    to those names' byte ranges, which hold only unit-path segments and
    //    never an index symbol, so it cannot shadow rung 1; and it runs before
    //    the bare-name rungs below, which would otherwise answer a segment
    //    (`model` in `uses compat.model`) with any declaration named `model`.
    //    A context's doc may live in a sibling file, so the search covers the
    //    project's other files: the caller's content map, or else the analysed
    //    round's snapshots, in path order so the answer is deterministic.
    let mut others: Vec<(&PathBuf, &str)> = match (input.files, &input.analysis) {
        (Some(files), _) => files.iter().map(|(p, t)| (p, t.as_str())).collect(),
        (None, Some(a)) => a
            .snapshots
            .iter()
            .filter(|(p, _)| p.as_path() != a.rel)
            .map(|(p, t)| (p, t.as_str()))
            .collect(),
        (None, None) => Vec::new(),
    };
    others.sort_by(|a, b| a.0.cmp(b.0));
    // #2223: `money.currency`, where `money` is a dependency alias, is the
    // dependency's `currency`: describe it from that package's files and say
    // where it comes from.
    if let Some(content) = describe_dependency_unit(input, text, span.start, &others) {
        return Some(content);
    }
    let others: Vec<&str> = others.into_iter().map(|(_, t)| t).collect();
    if let Some(content) =
        crate::symbols::describe_unit_at(text, span.start, others.iter().copied())
    {
        return Some(content);
    }
    // 5a. #2137: the contextual `advance` of `advance Clock by <Duration>`.
    if let Some(content) = crate::symbols::describe_advance_at(text, span.start) {
        return Some(content);
    }
    // 5a. Constrained generics slice 1: the contextual `derives` of a type's
    //    `derives Ord` clause, and a constraint name in a bound or that clause.
    if let Some(content) = crate::symbols::describe_derives_at(text, span.start) {
        return Some(content);
    }
    if let Some(content) = crate::symbols::describe_constraint_at(text, span.start) {
        return Some(content);
    }
    // 5a. #2640: the contextual `par` of a `par { … }` binding block.
    if let Some(content) = crate::symbols::describe_par_at(text, span.start) {
        return Some(content);
    }
    // 5a'. #2618: the contextual `private` modifier before a declaration.
    if let Some(content) = crate::symbols::describe_private_at(text, span.start) {
        return Some(content);
    }
    // 5b. #2136: a `schedule` / `message` address in a `system`-tier case,
    //    which runs through the Worker's own entry point. Neither name is a
    //    declaration, so this shadows nothing.
    if let Some(content) =
        crate::symbols::describe_system_job_address_at(text, span.start, others.iter().copied())
    {
        return Some(content);
    }
    // 5b. #2134: a suite's or case's `env { … }` clause — the `env` word or an
    //    entry's name. Guarded to those byte ranges, which hold no index
    //    symbol, so it cannot shadow rung 1.
    if let Some(content) = crate::symbols::describe_env_at(text, span.start) {
        return Some(content);
    }
    // 5c'. #2138: `run.id` in a case body that hasn't bound its own `run`.
    if let Some(content) = crate::symbols::describe_run_id_at(text, span.start) {
        return Some(content);
    }
    // 5c. #2135: a reserved call-site principal (`by Nobody`/`Forged`/`Stale`)
    //    — no declaration, so nothing below would answer it.
    if let Some(content) = crate::symbols::describe_reserved_principal_at(text, span.start) {
        return Some(content);
    }
    // 6. A top-level declaration in this file (fast path).
    if let Some(content) = crate::symbols::describe_symbol(text, &name) {
        return Some(content);
    }
    // 7. v0.137.0 (ADR 0161) + #611 (gap A): the `key`/`store` contextual
    //    keywords, the agent state fields they declare, and — since #611 — a
    //    *reference* to one from the agent's body. Single-file-local, so it
    //    resolves before any project-wide scan.
    if let Some(content) = crate::symbols::describe_agent_state_at(text, span.start) {
        return Some(content);
    }
    // 8. v0.140 (ADR 0163): a handler-position `@cache` annotation — not a symbol
    //    and no local, so it resolves here beside the agent state.
    if let Some(content) = crate::symbols::describe_handler_annotation_at(text, span.start) {
        return Some(content);
    }
    // 9. v0.123 (slice 2, DECISION B): a `Recv.member` name-receiver access — a
    //    capability op (`Clock.now`), a refined/opaque `of`/`unsafe`, or a type
    //    static — via the same path signature help uses, over the project and the
    //    embedded surface. Before the cross-file / first-party name scans.
    //    #1888: a `Sum.Variant` callee hovers as the variant, with its doc.
    crate::symbols::qualified_callee_at(text, span)
        .and_then(|callee| {
            describe_variant_anywhere(text, input.files, |src| {
                crate::symbols::describe_variant_key(src, &callee)
            })
            .or_else(|| {
                crate::signature_help::resolve_label(&callee, text, input.files)
                    .map(|sig| format!("```bynk\n{sig}\n```"))
            })
        })
        // 10. A project-wide scan (v1.1), then 11. the embedded first-party
        //    sources (slice 9) — so `uses`/`consumes` names resolve across
        //    file boundaries (§3.4) and stdlib/surface symbols surface too.
        //    Content-ownership track (#1086) slice 1: `describe_symbol_cross_file`
        //    now takes `input.files`'s content map directly — no more deriving
        //    a path list to hand it.
        .or_else(|| {
            input
                .files
                .zip(input.path)
                .and_then(|(content, path)| {
                    crate::symbols::describe_symbol_cross_file(content, path, &name)
                })
                .map(|(_other_path, desc)| desc)
        })
        // #1888: a bare variant name (`Circle(r)` in a pattern, `Dot` as a
        // value) hovers as its variant, after every declaration rung, so a
        // variant never shadows a type of the same name. Only a name exactly
        // one sum declares answers.
        .or_else(|| {
            describe_variant_anywhere(text, input.files, |src| {
                crate::symbols::describe_variant_named(src, &name)
            })
        })
        .or_else(|| crate::symbols::describe_firstparty_symbol(&name))
        // #2620: the ambient `unreachable`/`todo`, last, so a declaration of the
        // same name (which shadows the built-in) is answered by a rung above.
        .or_else(|| describe_fault_builtin(&name, &text[span.end..]))
}

/// #2620: hover for a call to the ambient `unreachable(…)` / `todo(…)`: the
/// signature (its type is `never returns`) and the registry's one-line doc.
fn describe_fault_builtin(name: &str, after: &str) -> Option<String> {
    let info = bynk_syntax::keywords::BUILTIN_FUNCTIONS
        .iter()
        .find(|k| k.word == name)?;
    after.trim_start().starts_with('(').then(|| {
        format!(
            "```bynk\n{name}(message: String) -> never returns\n```\n\n{}",
            info.meaning
        )
    })
}

/// #2469: the shortest path to the capability's use, when the cursor is on a
/// `given` entry the checker recorded one for.
fn capability_path_section(a: &HoverAnalysis<'_>) -> Option<String> {
    crate::capability_path::hover_section(
        &crate::capability_path::PathView {
            graphs: a.capability_paths,
            snapshots: a.snapshots,
            project_root: a.project_root,
            rel: a.rel,
        },
        a.offset,
    )
}

/// Rung 1's content with the capability path appended below the capability's
/// own description.
fn with_capability_path(content: String, a: &HoverAnalysis<'_>) -> String {
    match capability_path_section(a) {
        Some(section) => format!("{content}\n\n---\n\n{section}"),
        None => content,
    }
}

/// #2223: hover for a `uses`/`consumes` target written `<alias>.<unit>`: the
/// dependency unit's kind and module doc, from the dependency's own files,
/// and the package and alias it comes through. `files` are the project's
/// files with their paths (the content map, or the round's snapshots, which
/// are relative to the project root).
fn describe_dependency_unit(
    input: &HoverInput<'_>,
    text: &str,
    offset: usize,
    files: &[(&PathBuf, &str)],
) -> Option<String> {
    let written = crate::symbols::unit_name_at(text, offset)?;
    // The analysed round knows the project root; read its manifest from the
    // live content map, so an unsaved alias edit is honoured as the round
    // honours it.
    let empty = HashMap::new();
    let view = match &input.analysis {
        Some(a) => {
            crate::packages::DependencyView::for_root(a.project_root, input.files.unwrap_or(&empty))
        }
        None => input
            .files
            .and_then(crate::packages::DependencyView::from_files),
    }?;
    let target = view.resolve(&written)?;
    let root = input.analysis.as_ref().map(|a| a.project_root);
    let sources = files.iter().filter(|(p, _)| {
        let path = match root {
            Some(r) if p.is_relative() => r.join(p),
            _ => (*p).clone(),
        };
        view.package_of(&path) == Some(&target.package)
    });
    let mut out = crate::symbols::describe_unit(sources.map(|(_, t)| *t), &target.unit)?;
    out.push('\n');
    out.push_str(&crate::packages::hover_note(&target));
    out.push('\n');
    Some(out)
}

/// #1888: `describe` over the live buffer, then over each other project file
/// (sorted, as the cross-file declaration scan is), first answer winning.
fn describe_variant_anywhere(
    text: &str,
    files: Option<&HashMap<PathBuf, String>>,
    describe: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    describe(text).or_else(|| {
        let files = files?;
        let mut paths: Vec<&PathBuf> = files.keys().collect();
        paths.sort();
        paths
            .into_iter()
            .map(|p| &files[p])
            .filter(|src| src.as_str() != text)
            .find_map(|src| describe(src))
    })
}

/// The identifier-ish token covering `offset` — its text and span.
///
/// Hole-aware (issue #473): interpolation holes are expanded, so a cursor inside
/// `"… \(name) …"` lands on the hole's identifier token rather than the opaque
/// `InterpStr` token. The token-kind filter is wider than `Ident` because the
/// literal kinds carry hover-worthy names too (`Result`, `Option`, `Effect`).
pub fn identifier_at(text: &str, offset: usize) -> Option<(String, Span)> {
    use bynk_syntax::lexer::TokenKind;
    let tokens = bynk_syntax::lexer::tokenize_expanding_holes(text).ok()?;
    tokens
        .iter()
        .find(|t| {
            t.span.start <= offset
                && offset < t.span.end
                && matches!(
                    t.kind,
                    TokenKind::Ident
                        | TokenKind::Int
                        | TokenKind::String
                        | TokenKind::Bool
                        | TokenKind::Float
                        | TokenKind::Decimal
                        | TokenKind::Result
                        | TokenKind::Option
                        | TokenKind::Effect
                )
        })
        .map(|t| (text[t.span.start..t.span.end].to_string(), t.span))
}

// ---------------------------------------------------------------------
// #855: wire-contract rendering — rung 1's appendix and rung 4's standalone
// content share the same body renderer ([`wire_contract_body`]); they differ
// only in what comes before it (an existing `describe_symbol` block, versus
// nothing).
// ---------------------------------------------------------------------

/// Resolve a resolved-`Service`-symbol's wire contract, for rung 1's
/// appendix. A service may declare more than one handler (several HTTP
/// routes on one service, say); this module has no way to tell *which* one
/// the author meant by hovering the service's bare name (a declaration site,
/// or a cross-context/test-service call site — neither carries a handler
/// selector). Rather than guess, this answers only the unambiguous case: a
/// service with **exactly one** handler. That covers every worked example in
/// the issue (a single-route HTTP service, a single-handler `on call`
/// service) and every #855 fixture; a multi-handler service simply gets no
/// appendix, which is rung 1's pre-#855 behaviour unchanged — never a wrong
/// answer, only sometimes an absent one.
fn wire_contract_for_service_key(
    a: &HoverAnalysis<'_>,
    key: &bynk_check::index::SymbolKey,
    def_path: &Path,
    def_text: &str,
) -> Option<crate::wire_contract::WireContractModel> {
    let tys = a.tys;
    let info = a.boundary_info.get(&key.unit)?;
    let svc = info.services.get(&key.name)?;
    let [handler] = svc.handlers.as_slice() else {
        return None;
    };
    let expr_types = a
        .expr_types
        .get(def_path)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    crate::wire_contract::wire_contract_for_service(
        &key.unit,
        def_text,
        &key.name,
        handler,
        info,
        expr_types,
        tys,
        a.context_count,
    )
}

/// Locate the `Handler` (and its owning service's name) whose **header**
/// contains `offset` — the `on` keyword, the handler-kind/HTTP-method token,
/// and any route string literal, but *none* of its params or body. Mirrors
/// `crate::wire_contract::wire_contract_at`'s re-parse-and-find-the-
/// enclosing-handler convention; duplicated (rather than reused) because the
/// guard below needs the raw `Handler`'s param/body spans, which
/// `wire_contract_at`'s return value (a `WireContractModel`, not an AST node)
/// does not carry.
fn header_handler_at(text: &str, offset: usize) -> Option<(String, bynk_syntax::ast::Handler)> {
    use bynk_syntax::ast::{CommonsItem, SourceUnit};
    let tokens = bynk_syntax::lexer::tokenize(text).ok()?;
    let (parsed, _errs) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, text);
    let items: &[CommonsItem] = match parsed.as_ref()? {
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Commons(_) | SourceUnit::Suite(_) => return None,
    };
    for item in items {
        let CommonsItem::Service(s) = item else {
            continue;
        };
        for h in &s.handlers {
            // The safety property: this range holds only `on`, the
            // kind/method token, and the route literal — never a param name,
            // a type name, or anything else an earlier rung (or a later
            // lexical one) could already resolve. See the module doc and
            // rung 4's own comment.
            let prefix_end = h
                .params
                .first()
                .map(|p| p.span.start)
                .unwrap_or(h.body.span.start);
            if h.span.start <= offset && offset < prefix_end {
                return Some((s.name.name.clone(), h.clone()));
            }
        }
    }
    None
}

/// The Markdown facts shared by rung 1's appendix and rung 4's standalone
/// render: the request envelope (all three cases), the cross-context
/// contract form + hash (`on call` only), and — HTTP handlers only — the
/// reachable response status set.
fn wire_contract_body(model: &crate::wire_contract::WireContractModel) -> String {
    use crate::wire_contract::{BoundaryKind, Envelope};

    let mut out = String::new();
    if let BoundaryKind::Http { method, path } = &model.kind {
        out.push_str(&format!("`{} {}`\n\n", method.as_str(), path));
    }

    out.push_str("**Request:** ");
    match &model.envelope {
        Envelope::Empty => out.push_str("no body — the handler never reads it.\n\n"),
        Envelope::Bare { param, .. } => out.push_str(&format!(
            "the request body **is** the value of `{param}` — not wrapped in an object.\n\n"
        )),
        Envelope::Raw {
            param,
            content_type,
        } => out.push_str(&format!(
            "the request body is the raw octets of `{param}` — not JSON, not base64 — sent as \
             `{content_type}`.\n\n"
        )),
        Envelope::Keyed { params } => {
            let names: Vec<&str> = params.iter().map(|(n, _)| n.as_str()).collect();
            out.push_str(&format!(
                "an object keyed by `{}`, in declaration order.\n\n",
                names.join("`, `")
            ));
        }
    }

    if let Some(contract) = &model.contract {
        out.push_str(&format!(
            "**Cross-context contract** (hash `{}`) — change any of this and \
             already-deployed callers get a 409:\n\n```\n{}\n```\n\n",
            contract.hash,
            // #2087: each qualified key shown as its declaration's name.
            bynk_check::symbols::display_type_names_in(&contract.normal_form)
        ));
    }

    if !model.responses.is_empty() {
        out.push_str("**Responses:**\n\n");
        for r in &model.responses {
            out.push_str(&format!("- `{} {}`\n", r.status, r.variant));
        }
        out.push('\n');
    }

    out
}

/// Rung 1's wire-contract appendix — separated from whatever
/// `describe_symbol` already rendered by a rule, so it reads as a distinct
/// section rather than a continuation of the declaration's own doc comment.
fn wire_contract_appendix(model: &crate::wire_contract::WireContractModel) -> String {
    format!(
        "\n\n---\n\n**Wire contract**\n\n{}",
        wire_contract_body(model)
    )
}

/// Rung 4's standalone render — hovering the header *is* the question, so
/// this is the whole answer, headed by the service/handler name rather than
/// appended after one.
fn wire_contract_standalone(model: &crate::wire_contract::WireContractModel) -> String {
    format!(
        "**Wire contract** — `{}`\n\n{}",
        model.service,
        wire_contract_body(model)
    )
}

/// #2460: hover for a cursor inside a doc-comment `[Name]` link, or `None`
/// when the cursor is not in one (or the file has no unit to scope from).
///
/// Resolves exactly as the link itself does (`resolve_doc_link_in`, with the
/// declaration the comment is attached to as context, #2454), so the preview
/// and the click-through can never disagree: an own field or parameter
/// previews, and a unit-qualified name previews.
fn doc_link_hover(a: &HoverAnalysis<'_>, text: &str) -> Option<String> {
    // The span test is the real guard and the cheap one; the unit lookup only
    // runs for a cursor already inside a link.
    let link = crate::symbols::doc_link_spans_attached(text)
        .into_iter()
        .find(|l| l.span.start <= a.offset && a.offset < l.span.end)?;
    let name = link.name;
    let owner_unit = crate::symbols::doc_link_unit(text)?;
    let ctx = link
        .attached
        .map(|attached| crate::index_queries::DocLinkContext {
            path: a.rel,
            text,
            attached,
            locals: a.locals.get(a.rel).map_or(&[], |l| l.as_slice()),
        });
    let resolution = resolve_doc_link_in(a.index, a.doc_scope, &owner_unit, &name, ctx.as_ref());
    let resolved = match resolution {
        DocLinkResolution::Resolved(resolved) => Some(resolved),
        DocLinkResolution::Ambiguous { unit, kinds } => {
            let kinds: Vec<String> = kinds.iter().map(|k| format!("{k:?}")).collect();
            return Some(format!(
                "Ambiguous doc link `{name}` in `{unit}` (matches: {})",
                kinds.join(", ")
            ));
        }
        DocLinkResolution::Unresolved => None,
    };
    if let Some(resolved) = &resolved {
        let def_text = a.snapshots.get(&resolved.def.path);
        match (&resolved.key, def_text) {
            (Some(key), Some(def_text)) => {
                if let Some(content) = crate::symbols::describe_symbol(def_text, &key.name) {
                    // The target's own doc links resolve from the target.
                    let target_ctx = crate::index_queries::DocLinkContext {
                        path: &resolved.def.path,
                        text: def_text,
                        attached: resolved.def.span.start,
                        locals: a
                            .locals
                            .get(&resolved.def.path)
                            .map_or(&[], |l| l.as_slice()),
                    };
                    return Some(crate::doc_link_urls::linkify_doc_links(
                        &content,
                        a.index,
                        a.doc_scope,
                        a.project_root,
                        a.snapshots,
                        &key.unit,
                        Some(&target_ctx),
                    ));
                }
            }
            // No index key: a parameter, which is a local binding — described
            // from the locals table, as hovering it in code would.
            (None, Some(def_text)) => {
                let described = a.locals.get(&resolved.def.path).and_then(|locals| {
                    crate::locals_nav::describe_local_at(locals, def_text, resolved.def.span.start)
                });
                return Some(described.unwrap_or_else(|| format!("parameter `{name}`")));
            }
            _ => {}
        }
        // The link resolves (the click-through opens it) but its target has
        // no richer rendering: never report it unresolved.
        return Some(match &resolved.key {
            Some(key) => format!("`{}` — {:?} in `{}`", key.name, key.kind, key.unit),
            None => format!("parameter `{name}`"),
        });
    }
    // Nothing resolved (the ambiguous case returned above).
    let searched = a
        .doc_scope
        .get(&owner_unit)
        .map_or_else(|| owner_unit.clone(), |s| s.join(", "));
    Some(format!(
        "Unresolved doc link `{name}` — searched: {searched}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hover_at(src: &str, needle: &str, nth: usize) -> Option<String> {
        let offset = src.match_indices(needle).nth(nth).unwrap().0;
        hover_content(&HoverInput {
            analysis: None,
            doc: Some((src, offset)),
            path: None,
            files: None,
        })
    }

    /// #2620: the ambient fault built-ins hover with their signature and doc; a
    /// declaration of the same name shadows the built-in.
    #[test]
    fn fault_builtins_hover_unless_shadowed() {
        let src = "commons demo.m\n\nfn f(n: Int) -> Int { if n > 0 { n } else { unreachable(\"never\") } }\n";
        let h = hover_at(src, "unreachable", 0).expect("`unreachable` hovers");
        assert!(
            h.contains("unreachable(message: String) -> never returns"),
            "{h}"
        );
        let shadowed =
            "commons demo.m\n\nfn todo(n: Int) -> Int { n }\n\nfn g() -> Int { todo(1) }\n";
        let h = hover_at(shadowed, "todo", 1).expect("the user's `todo` hovers");
        assert!(!h.contains("never returns"), "{h}");
    }

    /// Constrained generics slice 1: `derives` and the constraint names it and
    /// a bound carry each hover; an `Ord` lookalike elsewhere does not.
    #[test]
    fn derives_and_constraint_names_hover() {
        let src = "commons demo.m\n\ntype P = Int derives Ord\n\nfn top[T: Ord](xs: List[T]) -> Option[T] { None }\n\nfn same[T: Eq](a: T) -> T { a }\n";
        let kw = hover_at(src, "derives", 0).expect("`derives` hovers");
        assert!(kw.contains("declaration order"), "{kw}");
        // `Ord` in the clause and in the bound; `Eq` in a bound.
        for nth in 0..2 {
            let ord = hover_at(src, "Ord", nth).expect("`Ord` hovers");
            assert!(ord.contains("constraint Ord"), "{ord}");
        }
        assert!(hover_at(src, "Eq", 0).unwrap().contains("constraint Eq"));
        // The type's own hover shows the clause.
        assert!(hover_at(src, "P =", 0).unwrap().contains("derives Ord"));
        // A generic's hover shows its bound.
        assert!(
            hover_at(src, "top", 0)
                .unwrap()
                .contains("fn top[T: Ord](xs: List[T])")
        );
    }
}
