//! Name resolution (spec §5.1, v0.1 §4.1, v0.2 §4.1).
//!
//! Builds symbol tables for the commons and validates that:
//! - No two top-level items share a name (types, fns, methods are all named).
//! - Every `TypeRef::Named` resolves to a declared type.
//! - Every free function call resolves to a function declaration.
//! - Every identifier in expression position resolves to a parameter, a
//!   `let` binding, or `self` (inside a method).
//! - Constructor / static calls (`TypeName.method(args)`) resolve either to
//!   the built-in `T.of` of a refined type, a static method on `T`, or a
//!   variant constructor when `T` is a sum type.
//! - Record construction targets a declared record type and uses only
//!   declared fields.
//! - Method calls resolve via the receiver's nominal type (the actual type
//!   check happens in the type checker).
//!
//! On success returns a [`ResolvedCommons`] — the original AST plus
//! symbol tables the type checker consumes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::index::{RefSink, SymbolKind};
use bynk_project::UnitKind;
use bynk_syntax::ast::*;
use bynk_syntax::error::{Applicability, CompileError};
use bynk_syntax::span::Span;

/// Is `name` a type imported via `uses` of a *commons* specifically — the
/// exact predicate [`ResolvedCommons::is_uses_commons_type`] caches as
/// `uses_commons_type_names`, and `bynk-emit` needs at *two* real call
/// sites that must never disagree: `emit_context_rebrands`'s own two steps,
/// "alias the import" and "rebrand the type" (its own doc comment, step 1
/// "Done in imports", step 2 the rebrand itself, both in
/// `bynk-emit/src/emitter.rs`) — an import narrower than the rebrand leaves
/// an undefined name in the generated module; a rebrand narrower than the
/// import leaves an alias imported and never used.
///
/// R4.10/R8.2 (`design/bynk-greenfield-compiler.md`): before this function
/// existed, `prepare_unit_check_ctx` (`check_pipeline.rs`) and *both*
/// `bynk-emit` call sites above each independently inlined this same
/// two-condition check, linked only by a doc comment promising they all
/// matched exactly — a real risk ADR 0226 names (#655: "a single named
/// binder took the entire test run down, pointing at generated code the
/// author never wrote"). One definition, every caller reads it — an edit to
/// either condition can no longer silently update only one side.
pub fn compute_is_uses_commons_type(
    imported_from_kind: &HashMap<String, UnitKind>,
    types: &HashMap<String, Arc<TypeDecl>>,
    name: &str,
) -> bool {
    matches!(imported_from_kind.get(name), Some(UnitKind::Commons)) && types.contains_key(name)
}

#[cfg(test)]
mod compute_is_uses_commons_type_tests {
    use super::compute_is_uses_commons_type;
    use bynk_project::UnitKind;
    use bynk_syntax::ast::{Ident, RecordBody, Trivia, TypeBody, TypeDecl};
    use bynk_syntax::span::Span;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn bare_record_type(name: &str) -> Arc<TypeDecl> {
        Arc::new(TypeDecl {
            type_params: Vec::new(),
            name: Ident {
                name: name.to_string(),
                span: Span::default(),
            },
            body: TypeBody::Record(RecordBody {
                trailing_comments: Default::default(),
                fields: Vec::new(),
                span: Span::default(),
            }),
            documentation: None,
            span: Span::default(),
            trivia: Trivia::default(),
        })
    }

    /// The four combinations the real callers' own doc comments describe:
    /// a `uses`-imported commons type (the only `true` case), a
    /// `uses`-imported commons *function* (v0.20b's own carve-out — not
    /// rebranded, a value not a type), a name imported from a non-commons
    /// unit kind (a consumed context, say), and a name absent from
    /// `imported_from_kind` entirely (a local declaration).
    #[test]
    fn matches_a_commons_imported_type_only() {
        let mut kinds = HashMap::new();
        kinds.insert("Money".to_string(), UnitKind::Commons);
        kinds.insert("traverse".to_string(), UnitKind::Commons);
        kinds.insert("Order".to_string(), UnitKind::Context);
        let mut types = HashMap::new();
        types.insert("Money".to_string(), bare_record_type("Money"));
        types.insert("Order".to_string(), bare_record_type("Order"));

        assert!(compute_is_uses_commons_type(&kinds, &types, "Money"));
        assert!(
            !compute_is_uses_commons_type(&kinds, &types, "traverse"),
            "a uses-imported commons function is a value, not a type — v0.20b"
        );
        assert!(
            !compute_is_uses_commons_type(&kinds, &types, "Order"),
            "imported from a context, not a commons"
        );
        assert!(
            !compute_is_uses_commons_type(&kinds, &types, "Local"),
            "absent from imported_from_kind entirely — a local declaration"
        );
    }
}

/// The resolver's two collection points, bundled so the reference walk
/// threads one parameter (v0.25, ADR 0053). `push` forwards to the error
/// list, keeping the walk's error sites unchanged; binding edges record
/// via `refs` at the site that resolved them.
pub(crate) struct Sinks<'a> {
    errs: &'a mut Vec<CompileError>,
    pub(crate) refs: &'a mut RefSink,
}

impl Sinks<'_> {
    fn push(&mut self, e: CompileError) {
        self.errs.push(e);
    }
}

/// Per-type method table built during resolution: keyed by method name,
/// values are clones of the [`FnDecl`] for that method.
#[derive(Debug, Default, Clone)]
pub struct MethodTable {
    pub instance: HashMap<String, Arc<FnDecl>>,
    pub statics: HashMap<String, Arc<FnDecl>>,
}

/// Output of resolution: the AST plus the symbol tables the checker needs.
pub struct ResolvedCommons {
    pub commons: Commons,
    /// Finding #10/#51: `Arc`-wrapped (not owned) so cloning this map — done
    /// once per synthetic per-handler `ResolvedCommons` during emission — is
    /// a pointer bump, not a deep copy of every declaration body in the unit.
    pub types: HashMap<String, Arc<TypeDecl>>,
    /// Finding #10/#51: `Arc`-wrapped for the same reason as `types`.
    pub fns: HashMap<String, Arc<FnDecl>>,
    /// Per-type method tables (instance + static).
    pub methods: HashMap<String, MethodTable>,
    /// Names of types declared in *this* commons (as opposed to imported via
    /// `uses`). Used by the checker to gate access to `.raw` and `.unsafe()`
    /// on opaque types. Private: this field's contract is specifically
    /// "declared here, not merely visible here", and a builder outside this
    /// crate that populates it from the wrong (merged, rather than
    /// pre-merge) table silently over-widens those gates — read it via
    /// [`ResolvedCommons::is_local_type`], and build a `ResolvedCommons` via
    /// [`ResolvedCommons::new`], which derives it correctly by construction.
    pub(crate) local_type_names: std::collections::HashSet<String>,
    /// Cross-context call information for v0.6. None for commons and for
    /// single-file mode. For contexts, supplies the set of consumed contexts
    /// and any aliases introduced via `consumes ... as Alias`.
    pub cross_context: CrossContextInfo,
    /// Agents declared in this context. Used to recognise the `Agent(key)`
    /// construction shape and the `agent_instance.handler(args)` method-call
    /// shape in handler bodies that mention other agents.
    pub agents: HashMap<String, AgentDecl>,
    /// v0.91 (ADR 0116 D6): for each imported function name, the qualified unit
    /// it came from (`map` → `bynk.list`). Lets the checker flag deprecated
    /// first-party free functions at their call sites. Empty in single-file
    /// mode and in synthetic handler-validation resolveds.
    pub imported_from: HashMap<String, String>,
    /// True iff this unit is a `context` (as opposed to a commons, adapter, or
    /// test/integration scaffold). `bynk-check` has no dependency on
    /// `bynk-emit`'s `UnitKind`, so callers set this directly from their own
    /// unit-kind knowledge. Used to gate the context-rebrand construction
    /// check (#907): only a context's emission rebrands a `uses`-sourced
    /// commons sum type's variant constructors out of value scope.
    pub is_context: bool,
    /// Names of types brought into scope via `uses` of a *commons*
    /// specifically (as opposed to a local declaration, or a type surfaced
    /// via `consumes`) — every name [`compute_is_uses_commons_type`] accepts
    /// against this unit's own `imported_from_kind`/`combined_types`, the
    /// single shared definition both `bynk-emit`'s `emit_context_rebrands`
    /// (rebrand + its own import-aliasing step) and this crate's own
    /// `prepare_unit_check_ctx` (which populates this set) read (R4.10/R8.2,
    /// closing what used to be two independently hand-maintained copies
    /// linked only by a doc comment promising they matched). A type surfaced
    /// via `consumes` (a capability signature from an adapter or another
    /// context) is *not* rebranded and must not be gated by #907's check —
    /// only this narrower set may be. Private for the same reason as
    /// `local_type_names`; read via [`ResolvedCommons::is_uses_commons_type`].
    pub(crate) uses_commons_type_names: std::collections::HashSet<String>,
    /// Events track, slice 0 (spine #936): names of `event` declarations in
    /// *this* commons specifically — as opposed to `local_type_names`, which
    /// answers "declared here" for any type, event-derived or not. Backs the
    /// `Events.emit[E]` check that `E` names a real event, not merely any
    /// local type (owner-only emission alone can't tell the two apart, since
    /// an event's synthetic `TypeDecl` sits in the same `types` table as
    /// every ordinary type). Private for the same reason as
    /// `local_type_names`; read via [`ResolvedCommons::is_local_event`].
    pub(crate) event_type_names: std::collections::HashSet<String>,
    /// #1883, slice 1a: every `asset` visible in this unit — its own and its
    /// `uses`d commons' ([`crate::assets::compose_unit_assets`]). An asset
    /// name is a value of type `Asset`. Empty unless set with
    /// [`ResolvedCommons::with_assets`].
    pub assets: HashMap<String, AssetDecl>,
    /// #2387, slice 1: every `embed` visible in this unit — its own and its
    /// `uses`d commons' ([`crate::embeds::compose_unit_embeds`]). An embed
    /// name is a `String` value. Empty unless set with
    /// [`ResolvedCommons::with_embeds`].
    pub embeds: HashMap<String, EmbedDecl>,
    /// #1925: the `uses`-imported values this unit's own declarations of
    /// another kind shadow, by name. Empty unless set with
    /// [`ResolvedCommons::with_shadowed_imports`]; read through
    /// [`ResolvedCommons::shadowed_import_error`].
    pub shadowed_imports: HashMap<String, ShadowedImport>,
    /// Standard-library S3 (#1963): the names in scope that are a first-party
    /// unit's extensible enums (`FetchError`, `Method`), whose `match` needs a
    /// wildcard. Types carry no owning unit, so this is decided once per unit
    /// from its provenance (`check_pipeline`) and carried to every check built
    /// from it. Empty unless set with [`ResolvedCommons::with_extensible_enums`].
    pub extensible_enums: HashSet<String>,
    /// Standard-library S3 (#1963): the unit's stability facts — the project's
    /// `[unstable]` policy and the imports it blocks. Set with
    /// [`ResolvedCommons::with_stability`]; the default blocks nothing.
    pub stability: crate::stability::UnitStability,
}

/// Static information about the consuming context: the set of contexts it
/// `consumes`, and any aliases introduced via `as Alias` clauses. Used by
/// the resolver to recognise cross-context service calls and by the checker
/// to type them (v0.6 §4.2).
#[derive(Debug, Default, Clone)]
pub struct CrossContextInfo {
    /// The qualified name of the consuming context, if this unit is a context.
    pub self_context: Option<String>,
    /// Qualified names of every consumed context.
    pub consumed_contexts: Vec<String>,
    /// alias → consumed-context qualified name.
    pub aliases: HashMap<String, String>,
    /// For each consumed context, its service surface plus the structural
    /// shapes of each service handler's params and return type (as seen
    /// from the consumed context's own namespace). Populated by the project
    /// driver; empty in single-file mode.
    pub consumed_services: HashMap<String, HashMap<String, CrossContextService>>,
    /// For each consumed context, its full type table (the consumed
    /// context's local types, plus the types it brings in via `uses`).
    /// Used by the checker for structural shape comparisons across the
    /// boundary (v0.6 §4.3).
    pub consumed_types: HashMap<String, HashMap<String, Arc<TypeDecl>>>,
    /// #1932: for each consumed context, the renames (`Box` → `shop__Box`)
    /// under which the consumer generates codecs for the consumed context's
    /// types it neither imports nor names (#1846). Set by the emitter, which
    /// computes them once (`emitter::unexported_type_renames`) so a call site
    /// names the same codecs the consumer generates. Empty in the checker.
    pub consumed_type_renames: HashMap<String, HashMap<String, String>>,
    /// v0.15: for each consumed context, the capabilities it `exports
    /// capability { … }` — keyed by capability name. Used to resolve and
    /// type-check `given B.Cap` references and `B.Cap.op(…)` calls, and by
    /// the emitter to instantiate the provider locally.
    pub consumed_capabilities: HashMap<String, HashMap<String, CrossContextCapability>>,
    /// v0.17: `consumes U { Cap, … }` flattens selected capabilities into the
    /// consumer's local namespace under their bare names (§3.3). Maps each bare
    /// capability name to the consumed unit (context or adapter) providing it,
    /// so bare `given Cap` / `Cap.op(…)` resolve, the deps type imports from the
    /// right module, and compose instantiates the provider.
    pub flattened_caps: HashMap<String, String>,
    /// Events track, slice 0 (spine #936): for each consumed context, the
    /// names of its own `event` declarations. Lets a subscriber's `from
    /// Events(E)` header be checked against a foreign owner too — `E` is
    /// legitimate if it's a local event *or* a declared event of some
    /// consumed context, mirroring how `discover_event_subscribers`
    /// (`bynk-emit/src/project.rs`) already resolves ownership for wiring.
    pub consumed_event_names: HashMap<String, HashSet<String>>,
    /// #2073: for each consumed context, the types in its `consumed_types`
    /// table that it does not declare but consumes from another context,
    /// mapped to that declaring context (`Order` → `shop.order` for a consumed
    /// `shop.ledger`). A boundary cast or codec names such a type through its
    /// declaring context, not the consumed one.
    pub consumed_type_owners: HashMap<String, HashMap<String, String>>,
}

/// Snapshot of one exported capability in a consumed context, as needed for
/// v0.15 cross-context capability resolution. Operation signatures are
/// expressed in the consumed context's own namespace (resolved against
/// `consumed_types` at the call site, mirroring [`CrossContextService`]).
#[derive(Debug, Clone)]
pub struct CrossContextCapability {
    pub name: String,
    /// Each operation's parameter type-refs and return type-ref.
    pub ops: Vec<CrossContextCapabilityOp>,
    /// The provider that implements this capability in the providing context
    /// (its generated class name), so the consumer can instantiate it.
    pub provider_name: String,
    /// The provider's own `given` capabilities (intra-providing-context),
    /// needed to wire the provider's constructor when instantiated locally.
    pub provider_given: Vec<String>,
    pub span: bynk_syntax::span::Span,
}

#[derive(Debug, Clone)]
pub struct CrossContextCapabilityOp {
    pub name: String,
    /// #926: the op's own type parameters (empty for a non-generic op),
    /// spelled the same as the consumed context's own declaration. A cross-
    /// context call resolves these from an explicit call-site type argument,
    /// same as the local-capability path.
    pub type_params: Vec<String>,
    pub params: Vec<(String, TypeRef)>,
    pub return_type: TypeRef,
}

/// Snapshot of one service in a consumed context, as needed for v0.6
/// cross-context type checking. The params and return type are expressed
/// in the consumed context's own namespace.
#[derive(Debug, Clone)]
pub struct CrossContextService {
    pub name: String,
    /// Surface (parsed) type-refs of the `on call` handler's parameters.
    pub params: Vec<(String, TypeRef)>,
    pub return_type: TypeRef,
    pub span: bynk_syntax::span::Span,
    /// I3 (#1971): the foreign callers the handler admits, by actor name. A
    /// system test that calls as one of them signs as that actor. Not part
    /// of the contract.
    pub foreign_callers: Vec<String>,
}

/// Project one local `on call` handler into the [`CrossContextService`] shape
/// both sides of a cross-context contract check need — a caller resolving a
/// *consumed* service ([`crate::symbols::build_cross_context_info`]) and a
/// callee stamping its *own* `X-Bynk-Contract` constant
/// ([`crate::contract::own_contract_hashes`]). Sharing this one projection is
/// the whole correctness argument for that symmetry: if the two sides ever
/// diverged, a working deployment would 409 on every call instead of only on
/// real skew. `None` when `sdecl` has no `on call` handler (e.g. an
/// events-only or queue-only service).
pub fn cross_context_service_for(name: &str, sdecl: &ServiceDecl) -> Option<CrossContextService> {
    let handler = sdecl
        .handlers
        .iter()
        .find(|h| matches!(h.kind, HandlerKind::Call))?;
    Some(CrossContextService {
        name: name.to_string(),
        params: handler
            .params
            .iter()
            .map(|p| (p.name.name.clone(), p.type_ref.clone()))
            .collect(),
        return_type: handler.return_type.clone(),
        span: sdecl.span,
        foreign_callers: Vec::new(),
    })
}

impl CrossContextInfo {
    /// Returns the qualified name of the consumed context this prefix refers
    /// to, treating `prefix` as either an alias or a full qualified name.
    pub fn resolve_prefix(&self, prefix: &str) -> Option<String> {
        if let Some(q) = self.aliases.get(prefix) {
            return Some(q.clone());
        }
        if self.consumed_contexts.iter().any(|c| c == prefix) {
            return Some(prefix.to_string());
        }
        None
    }

    /// v0.15: resolve a dotted receiver chain like `platform.time.Clock` or
    /// `Time.Clock` to `(consumed_context, capability)` when the leading
    /// segments name a consumed context (or alias) that exports the trailing
    /// capability. Returns `None` if the chain is not a cross-context
    /// capability reference.
    pub fn resolve_cross_capability(&self, chain: &str) -> Option<(String, String)> {
        let (prefix, cap) = chain.rsplit_once('.')?;
        let ctx = self.resolve_prefix(prefix)?;
        let caps = self.consumed_capabilities.get(&ctx)?;
        if caps.contains_key(cap) {
            Some((ctx, cap.to_string()))
        } else {
            None
        }
    }
}

impl ResolvedCommons {
    /// Returns true if `name` is a type declared in the current commons
    /// (rather than imported via `uses`). Local types alone may reach into
    /// their opaque representation (`.raw`) or call `.unsafe(value)`.
    pub fn is_local_type(&self, name: &str) -> bool {
        self.local_type_names.contains(name)
    }

    /// Events track, slice 0: is `name` a declared `event` in this commons —
    /// not merely any local type?
    pub fn is_local_event(&self, name: &str) -> bool {
        self.event_type_names.contains(name)
    }

    /// Is `name` in scope via `uses` of a *commons* specifically? See
    /// `uses_commons_type_names`'s field doc for the exact predicate.
    pub fn is_uses_commons_type(&self, name: &str) -> bool {
        self.uses_commons_type_names.contains(name)
    }

    /// Build a `ResolvedCommons` from a merged (local + `uses`/`consumes`)
    /// symbol table, deriving `local_type_names`/`event_type_names` from
    /// `local_types`/`local_events` — the *pre-merge* tables — rather than
    /// from `types`/`agents` (already merged). This is the one thing every
    /// hand-rolled construction outside this crate got a chance to disagree
    /// on: the pre-merge/merged distinction is exactly what backs
    /// `.raw`/`.unsafe()`/owner-only-event-emission gating, and reusing the
    /// merged table there silently widens all three to any consumed/used
    /// type or event (found during the events track, slice 0, spine #936).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        commons: Commons,
        types: HashMap<String, Arc<TypeDecl>>,
        local_types: &HashMap<String, Arc<TypeDecl>>,
        fns: HashMap<String, Arc<FnDecl>>,
        methods: HashMap<String, MethodTable>,
        agents: HashMap<String, AgentDecl>,
        local_events: &HashMap<String, EventDecl>,
        cross_context: CrossContextInfo,
        imported_from: HashMap<String, String>,
        is_context: bool,
        uses_commons_type_names: HashSet<String>,
    ) -> Self {
        Self {
            commons,
            local_type_names: local_types.keys().cloned().collect(),
            event_type_names: local_events.keys().cloned().collect(),
            types,
            fns,
            methods,
            cross_context,
            agents,
            imported_from,
            is_context,
            uses_commons_type_names,
            assets: HashMap::new(),
            embeds: HashMap::new(),
            shadowed_imports: HashMap::new(),
            extensible_enums: HashSet::new(),
            stability: Default::default(),
        }
    }

    /// #1883, slice 1a: the assets visible in this unit. A builder rather
    /// than a [`ResolvedCommons::new`] parameter, so the constructions that
    /// can never see an asset (a synthetic test scaffold) need not mention
    /// them.
    pub fn with_assets(mut self, assets: HashMap<String, AssetDecl>) -> Self {
        self.assets = assets;
        self
    }

    /// #2387, slice 1: the embeds visible in this unit, a builder like
    /// [`ResolvedCommons::with_assets`].
    pub fn with_embeds(mut self, embeds: HashMap<String, EmbedDecl>) -> Self {
        self.embeds = embeds;
        self
    }

    /// #1925: the `uses`-imported values this unit's own declarations of
    /// another kind shadow ([`crate::project_model::shadowed_imports`]).
    pub fn with_shadowed_imports(mut self, shadowed: HashMap<String, ShadowedImport>) -> Self {
        self.shadowed_imports = shadowed;
        self
    }

    /// S3 (#1963): set [`ResolvedCommons::extensible_enums`].
    pub fn with_extensible_enums(mut self, names: HashSet<String>) -> Self {
        self.extensible_enums = names;
        self
    }

    /// S3 (#1963): set [`ResolvedCommons::stability`].
    pub fn with_stability(mut self, stability: crate::stability::UnitStability) -> Self {
        self.stability = stability;
        self
    }

    /// #1925: the conflict to report for a reference to `name` that the
    /// unit's own declaration could not answer, when `name` is a shadowed
    /// import — or `None`, and the caller reports its usual diagnostic.
    pub fn shadowed_import_error(&self, name: &str, span: Span) -> Option<CompileError> {
        self.shadowed_imports
            .get(name)
            .map(|s| shadowed_import_error(name, span, s))
    }
}

/// #1925: a `uses`-imported function or asset that the unit hides behind its
/// own declaration of the same name and another kind. A local declaration
/// wins, as it does in the emitted module, so the import is out of scope; a
/// reference the local declaration cannot answer (a call to a type, a bare
/// service name) is reported as this conflict, not as an unknown name.
#[derive(Debug, Clone)]
pub struct ShadowedImport {
    /// The commons that declares the import.
    pub unit: String,
    /// The import's kind: `"function"`, `"asset"`, or `"type"` (#1927).
    pub imported: &'static str,
    /// The local declaration's kind, as `symbols::local_value_kind` names it.
    pub local: &'static str,
    /// The local declaration's name.
    pub local_span: Span,
}

/// `bynk.uses.name_conflict` for a reference to `name`, a shadowed import.
pub(crate) fn shadowed_import_error(name: &str, span: Span, s: &ShadowedImport) -> CompileError {
    CompileError::new(
        "bynk.uses.name_conflict",
        span,
        format!(
            "`{name}` names this unit's {}, which hides the {} `{name}` from `{}`",
            s.local, s.imported, s.unit
        ),
    )
    .with_label(s.local_span, format!("{} declared here", s.local))
    .with_note(format!(
        "a unit's own declaration shadows a `uses`-imported one of the same name; rename one to use the {}",
        s.imported
    ))
}

/// Resolve names in a single-file (or already-merged) commons. Use this
/// entry point only for self-contained Bynk programs. For multi-file
/// projects and `uses`-resolving commons, use [`resolve_file`] against a
/// pre-built combined symbol table.
pub fn resolve(commons: Commons) -> Result<ResolvedCommons, Vec<CompileError>> {
    let (resolved, errors) = resolve_recovering(commons);
    if errors.is_empty() {
        Ok(resolved)
    } else {
        Err(errors)
    }
}

/// [`resolve`], returning the symbol table *with* every resolve error rather
/// than instead of it (#1663, Decision A). A resolve error is local to the
/// declaration it is in, so the checker can still check every declaration —
/// an unknown name in `b` no longer hides a type error in `a`. The table is
/// complete apart from what the errors name (a duplicate keeps its first
/// declaration). A caller that emits must still refuse on any error.
pub fn resolve_recovering(commons: Commons) -> (ResolvedCommons, Vec<CompileError>) {
    let mut errors = Vec::new();
    let mut types: HashMap<String, Arc<TypeDecl>> = HashMap::new();
    let mut fns: HashMap<String, Arc<FnDecl>> = HashMap::new();
    let mut methods: HashMap<String, MethodTable> = HashMap::new();

    // First pass: collect declarations and detect duplicates / name overlap.
    for item in &commons.items {
        match item {
            // v0.5 declaration kinds — these don't introduce types/fns into
            // the symbol space. They go through the context-level v0.5 path
            // in project.rs. Skip them at the per-commons level.
            CommonsItem::Capability(_)
            | CommonsItem::Provider(_)
            | CommonsItem::Service(_)
            | CommonsItem::Agent(_)
            | CommonsItem::Actor(_)
            // `messages` entries are plain string literals with no type refs
            // to resolve here; commons-only legality and the reference/
            // duplicate-code checks live in bynk-emit's project validation.
            | CommonsItem::Messages(_)
            // #1883, slice 1a: an asset's placement, path and name checks
            // live in `symbols::build_unit_table`; it has no type refs.
            | CommonsItem::Asset(_)
            // #2387, slice 1: likewise an embed (`embeds::path_errors`).
            | CommonsItem::Embed(_) => {}
            CommonsItem::Type(t) => {
                if let Some(prev) = types.get(&t.name.name) {
                    errors.push(
                        CompileError::new(
                            "bynk.resolve.duplicate_type",
                            t.name.span,
                            format!("type `{}` is already declared", t.name.name),
                        )
                        .with_label(prev.name.span, "previously declared here"),
                    );
                } else if let Some(prev) = fns.get(&t.name.name) {
                    errors.push(
                        CompileError::new(
                            "bynk.resolve.name_conflict",
                            t.name.span,
                            format!(
                                "type `{}` conflicts with a function of the same name",
                                t.name.name
                            ),
                        )
                        .with_label(prev.name.ident().span, "function declared here"),
                    );
                } else {
                    types.insert(t.name.name.clone(), Arc::new(t.clone()));
                    methods.insert(t.name.name.clone(), MethodTable::default());
                }
            }
            // Events track, slice 0 (spine #936): an `event` registers into
            // the same `types` table as an ordinary `type` — via the
            // synthetic `TypeDecl` `EventDecl::as_type_decl` builds — so it
            // reuses every existing type-reference/construction check.
            // Context-only legality (`bynk.event.outside_context`) and
            // event-vs-plain-type distinctions live in bynk-emit's project
            // validation, the same split `messages` already uses.
            CommonsItem::Event(e) => {
                let t = e.as_type_decl();
                if let Some(prev) = types.get(&t.name.name) {
                    errors.push(
                        CompileError::new(
                            "bynk.resolve.duplicate_type",
                            t.name.span,
                            format!("type `{}` is already declared", t.name.name),
                        )
                        .with_label(prev.name.span, "previously declared here"),
                    );
                } else if let Some(prev) = fns.get(&t.name.name) {
                    errors.push(
                        CompileError::new(
                            "bynk.resolve.name_conflict",
                            t.name.span,
                            format!(
                                "type `{}` conflicts with a function of the same name",
                                t.name.name
                            ),
                        )
                        .with_label(prev.name.ident().span, "function declared here"),
                    );
                } else {
                    methods.insert(t.name.name.clone(), MethodTable::default());
                    types.insert(t.name.name.clone(), Arc::new(t));
                }
            }
            CommonsItem::Fn(f) => match &f.name {
                FnName::Free(id) => {
                    if let Some(prev) = fns.get(&id.name) {
                        errors.push(
                            CompileError::new(
                                "bynk.resolve.duplicate_fn",
                                id.span,
                                format!("function `{}` is already declared", id.name),
                            )
                            .with_label(prev.name.ident().span, "previously declared here"),
                        );
                    } else if let Some(prev) = types.get(&id.name) {
                        errors.push(
                            CompileError::new(
                                "bynk.resolve.name_conflict",
                                id.span,
                                format!(
                                    "function `{}` conflicts with a type of the same name",
                                    id.name
                                ),
                            )
                            .with_label(prev.name.span, "type declared here"),
                        );
                    } else {
                        fns.insert(id.name.clone(), Arc::new(f.clone()));
                    }
                }
                FnName::Method {
                    type_name,
                    method_name,
                } => {
                    // The type the method is attached to must be declared.
                    if !types.contains_key(&type_name.name) {
                        errors.push(
                            CompileError::new(
                                "bynk.resolve.method_unknown_type",
                                type_name.span,
                                format!(
                                    "method `{}.{}` attached to an unknown type `{}`",
                                    type_name.name, method_name.name, type_name.name
                                ),
                            )
                            .with_note(
                                "methods can only be declared on types defined in the same commons",
                            ),
                        );
                        continue;
                    }
                    // #594: an *instance* method on a generic type is a generic
                    // method — the receiver's type arguments supply the type's
                    // parameters (`self: Box[A]`), so it resolves and emits as an
                    // erased TS generic method. A *static* method has no receiver
                    // to supply those parameters, so it stays deferred (it would
                    // need free-function-style inference of the type's params);
                    // reject it rather than emit an under-applied `Box` signature.
                    if !f.has_self
                        && types
                            .get(&type_name.name)
                            .is_some_and(|d| !d.type_params.is_empty())
                    {
                        errors.push(
                            CompileError::new(
                                "bynk.generics.method_on_generic_type",
                                type_name.span,
                                format!(
                                    "static method `{}.{}` is attached to generic type `{}` — static methods on generic types are deferred (instance methods are supported)",
                                    type_name.name, method_name.name, type_name.name
                                ),
                            )
                            .with_note(
                                "give the method a `self` receiver, or use a free function taking the generic value as a parameter instead",
                            ),
                        );
                        continue;
                    }
                    let table = methods.entry(type_name.name.clone()).or_default();
                    let bucket = if f.has_self {
                        &mut table.instance
                    } else {
                        &mut table.statics
                    };
                    if let Some(prev) = bucket.get(&method_name.name) {
                        errors.push(
                            CompileError::new(
                                "bynk.resolve.duplicate_method",
                                method_name.span,
                                format!(
                                    "method `{}.{}` is already declared",
                                    type_name.name, method_name.name
                                ),
                            )
                            .with_label(prev.name.ident().span, "previously declared here"),
                        );
                    } else {
                        bucket.insert(method_name.name.clone(), Arc::new(f.clone()));
                    }
                }
            },
        }
    }

    // #1883, slice 1a: single-file mode has no `uses`, so the visible assets
    // are exactly the ones declared here.
    // The first declaration of a name wins, as in the unit table; a second is
    // `bynk.asset.duplicate` (`assets::single_file_asset_errors`).
    let mut local_assets: HashMap<String, AssetDecl> = HashMap::new();
    for item in &commons.items {
        if let CommonsItem::Asset(a) = item {
            local_assets
                .entry(a.name.name.clone())
                .or_insert_with(|| a.clone());
        }
    }

    // #2387, slice 1: likewise the embeds, first declaration winning
    // (`embeds::single_file_embed_errors` reports a second).
    let mut local_embeds: HashMap<String, EmbedDecl> = HashMap::new();
    for item in &commons.items {
        if let CommonsItem::Embed(e) = item {
            local_embeds
                .entry(e.name.name.clone())
                .or_insert_with(|| e.clone());
        }
    }

    // Second pass: validate references inside type-refs and function bodies.
    let mut refs = RefSink::new(); // single-file mode: no recording context.
    let mut sinks = Sinks {
        errs: &mut errors,
        refs: &mut refs,
    };
    for item in &commons.items {
        check_reserved_host_name(item, &mut sinks);
        match item {
            CommonsItem::Type(t) => {
                check_type_decl_refs(t, &types, &mut sinks);
            }
            CommonsItem::Event(e) => {
                check_type_decl_refs(&e.as_type_decl(), &types, &mut sinks);
            }
            CommonsItem::Fn(f) => {
                check_fn_refs(
                    f,
                    &types,
                    &fns,
                    &methods,
                    &local_assets,
                    &local_embeds,
                    // A single file has no `uses` imports to shadow.
                    &HashMap::new(),
                    &mut sinks,
                );
            }
            // v0.5 items' bodies are resolved via a separate context-level
            // pass; their signatures here (#1679).
            CommonsItem::Capability(_)
            | CommonsItem::Service(_)
            | CommonsItem::Agent(_)
            | CommonsItem::Provider(_)
            | CommonsItem::Actor(_) => {
                check_signature_refs(item, &types, &mut sinks);
            }
            // `messages` entries are plain string literals with no type refs
            // to resolve here; commons-only legality and the reference/
            // duplicate-code checks live in bynk-emit's project validation.
            // #1883, slice 1a: likewise an asset (a string-literal path).
            CommonsItem::Messages(_) | CommonsItem::Asset(_) | CommonsItem::Embed(_) => {}
        }
    }

    {
        let local_type_names = types.keys().cloned().collect();
        let event_type_names = commons
            .items
            .iter()
            .filter_map(|item| match item {
                CommonsItem::Event(e) => Some(e.name.name.clone()),
                _ => None,
            })
            .collect();
        let resolved = ResolvedCommons {
            commons,
            types,
            fns,
            methods,
            local_type_names,
            cross_context: CrossContextInfo::default(),
            agents: HashMap::new(),
            // Single-file mode has no `uses`-imported functions.
            imported_from: HashMap::new(),
            // Single-file mode has no `uses` at all — the rebrand this flag
            // gates is unreachable here.
            is_context: false,
            uses_commons_type_names: HashSet::new(),
            event_type_names,
            assets: local_assets,
            embeds: local_embeds,
            shadowed_imports: HashMap::new(),
            extensible_enums: HashSet::new(),
            stability: Default::default(),
        };
        (resolved, errors)
    }
}

/// #1663 (Decision B): split resolve errors into those to report and those
/// that only echo a declaration the parser had to skip.
///
/// A declaration that fails to parse is dropped from the AST, so every
/// reference to its name — `Money` in a signature, `Money.zero` as a method's
/// owner, a call to a skipped `fn` — resolves as unknown, and one missing comma
/// becomes twenty diagnostics. Its name is known (the parser recorded it in
/// `broken_decl_names`); only its declaration is broken, and that is already
/// reported. So an unknown-name diagnostic naming one is hidden.
///
/// The hidden errors still count as resolve errors: pass *both* halves to
/// [`without_resolve_echoes`], so the declarations they are in reject the
/// checker's follow-on diagnostics too.
pub fn split_broken_decl_echoes(
    resolve_errors: Vec<CompileError>,
    broken_decl_names: &[String],
) -> (Vec<CompileError>, Vec<CompileError>) {
    const ECHO_CODES: &[&str] = &[
        "bynk.resolve.unknown_type",
        "bynk.resolve.method_unknown_type",
        "bynk.resolve.unknown_name",
        "bynk.resolve.unknown_function",
        "bynk.resolve.unknown_static_member",
        // #1710: echoes the checker reports (so the project path splits the
        // checker's diagnostics too), each naming a declaration recovery may
        // skip: a consumed context's service, a method, a capability (as a
        // provider's target, in `given`, or unused for want of one), an actor.
        "bynk.consumes.unknown_service",
        "bynk.types.method_not_found",
        "bynk.provider.unknown_capability",
        "bynk.given.unknown_capability",
        "bynk.given.unused_capability",
        "bynk.actor.unknown_actor",
    ];
    // The *subject* each of these diagnostics is about, spelled as the parser
    // records a broken declaration (`T`, `f`, or a method as `T.m`):
    // - `unknown type `T``, `unknown name `x``, `unknown function `f``: the
    //   one backticked name;
    // - `method `T.m` attached to an unknown type `T``: the type, its last;
    // - `type `T` has no static method or variant named `m``: the member,
    //   qualified by its type (`T.m`), its first and last;
    // - `context `c` has no service named `s``: the service, its last;
    // - `type `T` has no instance method named `m``: the method, as `T.m`;
    // - `capability `C` is declared in `given` but never used`: its first;
    // - any other capability or actor diagnostic: its one backticked name.
    // Matching only the subject keeps a broken `fn m` from hiding an unrelated
    // `Other.m` that merely shares the name.
    let names = |message: &str| -> Vec<String> {
        message
            .split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect()
    };
    let subject = |e: &CompileError| -> Option<String> {
        let ns = names(&e.message);
        match e.category {
            "bynk.resolve.unknown_static_member" | "bynk.types.method_not_found" => {
                Some(format!("{}.{}", ns.first()?, ns.last()?))
            }
            // `capability `C` is declared in `given` but never used`: the
            // capability comes first (`given` is the keyword, quoted).
            "bynk.given.unused_capability" => ns.first().cloned(),
            _ => ns.last().cloned(),
        }
    };
    resolve_errors.into_iter().partition(|e| {
        !(ECHO_CODES.contains(&e.category)
            && subject(e).is_some_and(|s| broken_decl_names.contains(&s)))
    })
}

/// #1663 (Decision A): the checker's diagnostics after a resolve that reported
/// errors, keeping only those in declarations the resolver found clean.
///
/// Resolve-then-check is per *declaration*: an unknown name in `b` must not
/// hide a type error in `a`, so the checker runs over every declaration. Inside
/// a declaration the resolver rejected, the checker would only meet the same
/// fault again — under the same code (`unknown_function`), under its own twin
/// code (`types.unknown_static_member`), or as a follow-on (`unknown_name` for a
/// misplaced `self`, `type_in_expr` beside an unknown variant). The resolver's
/// report is the one that names the fault, so that declaration keeps only it.
/// `item_spans` are the unit's top-level declarations' spans
/// ([`CommonsItem::span`]). A diagnostic outside every declaration is kept
/// unless it overlaps a resolve error.
pub fn without_resolve_echoes(
    checked: Vec<CompileError>,
    resolve_errors: &[CompileError],
    item_spans: &[Span],
) -> Vec<CompileError> {
    let within = |outer: Span, at: Span| {
        outer.file == at.file
            && outer.start <= at.start
            && at.start < outer.end.max(outer.start + 1)
    };
    let rejected: Vec<Span> = item_spans
        .iter()
        .copied()
        .filter(|item| resolve_errors.iter().any(|r| within(*item, r.span)))
        .collect();
    checked
        .into_iter()
        .filter(|c| {
            !rejected.iter().any(|item| within(*item, c.span))
                && !resolve_errors.iter().any(|r| within(r.span, c.span))
        })
        .collect()
}

/// Validate name references inside a single file's items against an
/// already-built symbol table (`resolved.types`, `resolved.fns`,
/// `resolved.methods`). Used by the project-level driver after combining
/// declarations from every file in a multi-file commons and from every
/// commons brought in by `uses`.
pub fn resolve_file(resolved: &ResolvedCommons) -> Result<(), Vec<CompileError>> {
    resolve_file_record(resolved, &mut RefSink::new())
}

/// [`resolve_file`], recording binding edges into `refs` as the walk
/// resolves them (v0.25). The project pass sets the sink's per-file context;
/// a fresh sink records nothing.
pub fn resolve_file_record(
    resolved: &ResolvedCommons,
    refs: &mut RefSink,
) -> Result<(), Vec<CompileError>> {
    let mut errors = Vec::new();
    let mut sinks = Sinks {
        errs: &mut errors,
        refs,
    };
    for item in &resolved.commons.items {
        check_reserved_host_name(item, &mut sinks);
        match item {
            CommonsItem::Type(t) => {
                sinks.refs.set_owner(&t.name.name);
                check_type_decl_refs(t, &resolved.types, &mut sinks);
            }
            CommonsItem::Event(e) => {
                sinks.refs.set_owner(&e.name.name);
                check_type_decl_refs(&e.as_type_decl(), &resolved.types, &mut sinks);
            }
            CommonsItem::Fn(f) => {
                sinks.refs.set_owner(f.name.display());
                check_fn_refs(
                    f,
                    &resolved.types,
                    &resolved.fns,
                    &resolved.methods,
                    &resolved.assets,
                    &resolved.embeds,
                    &resolved.shadowed_imports,
                    &mut sinks,
                );
            }
            CommonsItem::Capability(_)
            | CommonsItem::Service(_)
            | CommonsItem::Agent(_)
            | CommonsItem::Provider(_)
            | CommonsItem::Actor(_) => {
                check_signature_refs(item, &resolved.types, &mut sinks);
            }
            // `messages` entries are plain string literals with no type refs
            // to resolve here; commons-only legality and the reference/
            // duplicate-code checks live in bynk-emit's project validation.
            CommonsItem::Messages(_) | CommonsItem::Asset(_) | CommonsItem::Embed(_) => {}
        }
        sinks.refs.clear_owner();
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// #1679 (runtime-semantics track S12): every type named in a handler or
/// capability **signature** must resolve, exactly as in a `fn` signature
/// (`bynk.resolve.unknown_type`). Their bodies are resolved by the
/// context-level pass, but nothing walked the signatures, so `on call(v:
/// Bogus)` and `Effect[Unit]` (Bynk's unit is `()`) were accepted and the
/// emitter wrote a `/* unknown */` placeholder. Covered:
/// - service and agent handler parameters and return types;
/// - an agent's key type and its `store` fields' kind arguments
///   (`Cell[T]`, `Map[K, V]`, …);
/// - capability operation parameters and return types, with the operation's
///   own type parameters in scope;
/// - a service header's types: a `from websocket(in: …, out: …)` frame pair
///   and a `from events(…)` event type;
/// - provider operation parameters and return types;
/// - an actor's `identity` type.
fn check_signature_refs(
    item: &CommonsItem,
    types: &HashMap<String, Arc<TypeDecl>>,
    sinks: &mut Sinks,
) {
    let handler = |h: &Handler, sinks: &mut Sinks| {
        for p in &h.params {
            check_type_ref_resolves(&p.type_ref, types, sinks);
        }
        check_type_ref_resolves(&h.return_type, types, sinks);
    };
    match item {
        CommonsItem::Service(s) => {
            match &s.protocol {
                ServiceProtocol::WebSocket { in_type, out_type } => {
                    check_type_ref_resolves(in_type, types, sinks);
                    check_type_ref_resolves(out_type, types, sinks);
                }
                ServiceProtocol::Events { event_type, .. } => {
                    check_type_ref_resolves(event_type, types, sinks);
                }
                ServiceProtocol::Call
                | ServiceProtocol::Http
                | ServiceProtocol::Cron
                | ServiceProtocol::Queue { .. } => {}
            }
            for h in &s.handlers {
                handler(h, sinks);
            }
        }
        CommonsItem::Provider(p) => {
            for op in &p.ops {
                for param in &op.params {
                    check_type_ref_resolves(&param.type_ref, types, sinks);
                }
                check_type_ref_resolves(&op.return_type, types, sinks);
            }
        }
        CommonsItem::Actor(a) => {
            if let Some(identity) = &a.identity {
                check_type_ref_resolves(identity, types, sinks);
            }
        }
        CommonsItem::Agent(a) => {
            check_type_ref_resolves(&a.key_type, types, sinks);
            for f in &a.store_fields {
                for arg in &f.kind.args {
                    check_type_ref_resolves(arg, types, sinks);
                }
            }
            for h in &a.handlers {
                handler(h, sinks);
            }
        }
        CommonsItem::Capability(c) => {
            for op in &c.ops {
                let type_params: HashSet<String> = op
                    .type_params
                    .iter()
                    .map(|tp| tp.name.name.clone())
                    .collect();
                for p in &op.params {
                    check_type_ref_resolves_in(&p.type_ref, types, &type_params, sinks);
                }
                check_type_ref_resolves_in(&op.return_type, types, &type_params, sinks);
            }
        }
        CommonsItem::Type(_)
        | CommonsItem::Event(_)
        | CommonsItem::Fn(_)
        | CommonsItem::Messages(_)
        | CommonsItem::Asset(_)
        | CommonsItem::Embed(_) => {}
    }
}

/// v0.157 (ADR 0183): the name a record field *directly contains* — a top-level
/// `Named` (`f: A`) or a generic application (`f: A[T]`). Both are direct
/// containment edges for the cycle guards; a `List[…]`/`Option[…]` wrapper is
/// not (its empty/`None` inhabitant breaks the cycle).
fn direct_record_head(tr: &TypeRef) -> Option<&str> {
    match tr {
        TypeRef::Named(id) => Some(&id.name),
        TypeRef::App { name, .. } => Some(&name.name),
        _ => None,
    }
}

/// Whether `target` is reachable from `start` over direct record-field edges
/// (bare `Named` or generic `App` heads) — the record-containment graph. Used
/// to reject indirect record cycles (`A = { b: B }`, `B = { a: A }`); a
/// `visited` set bounds the walk on graphs that already contain cycles
/// elsewhere.
fn record_field_reaches(start: &str, target: &str, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    let mut visited: HashSet<String> = HashSet::new();
    let mut stack = vec![start.to_string()];
    while let Some(name) = stack.pop() {
        if name == target {
            return true;
        }
        if !visited.insert(name.clone()) {
            continue;
        }
        if let Some(decl) = types.get(&name)
            && let TypeBody::Record(r) = &decl.body
        {
            for f in &r.fields {
                if let Some(head) = direct_record_head(&f.type_ref) {
                    stack.push(head.to_string());
                }
            }
        }
    }
    false
}

/// v0.157 (ADR 0183): reject a repeated type-parameter name — a duplicate would
/// collapse silently in the substitution map (the later argument winning), so a
/// `Pair[T, T]` mis-checks its fields. Shared by `type` and `fn` declarations.
fn check_duplicate_type_params(params: &[TypeParam], owner: &str, errors: &mut Sinks) {
    let mut seen: HashMap<&str, bynk_syntax::span::Span> = HashMap::new();
    for tp in params {
        if let Some(prev) = seen.get(tp.name.name.as_str()) {
            errors.push(
                CompileError::new(
                    "bynk.generics.duplicate_type_param",
                    tp.span,
                    format!(
                        "type parameter `{}` is declared more than once on {owner}",
                        tp.name.name
                    ),
                )
                .with_label(*prev, "previously declared here"),
            );
        } else {
            seen.insert(tp.name.name.as_str(), tp.span);
        }
    }
}

/// #1653: the generated TypeScript reaches host globals as `globalThis.<name>`,
/// so a module-scope declaration of that name (a type, function, agent,
/// provider, …) would hide every one of them in its module. Parameters and
/// locals are renamed by the emitter instead; a declaration is rejected.
fn check_reserved_host_name(item: &CommonsItem, errors: &mut Sinks) {
    if let Some(name) = item.name()
        && name.name == "globalThis"
    {
        errors.push(
            CompileError::new(
                "bynk.resolve.reserved_host_name",
                name.span,
                "`globalThis` cannot be used as a declaration name",
            )
            .with_note(
                "the generated TypeScript uses `globalThis` to reach the host's built-in objects; rename the declaration",
            ),
        );
    }
}

/// Recursively walk a type declaration to check that every type reference
/// inside it resolves.
fn check_type_decl_refs(t: &TypeDecl, types: &HashMap<String, Arc<TypeDecl>>, errors: &mut Sinks) {
    // A `type` declaration may not reuse a compiler-known built-in type name
    // (`List`, `Map`, `Query`, …). Those names are dispatched on by the type
    // parser (`parser/types.rs`), so any *reference* to the alias would be
    // intercepted as the built-in — the declaration would be silently shadowed
    // (`QueueResult`) or fail with an incoherent message at the use site. Reject
    // it here, at the declaration, with a message the user can act on. Base
    // types and other reserved *keywords* (`Int`, `Result`, …) are already
    // rejected earlier, by `expect_ident` at parse time.
    if bynk_syntax::keywords::is_builtin_type_name(&t.name.name) {
        errors.push(
            CompileError::new(
                "bynk.resolve.reserved_builtin_type",
                t.name.span,
                format!(
                    "`{}` is a built-in type name and cannot be redeclared",
                    t.name.name
                ),
            )
            .with_note("rename the type — built-in type names are reserved in type position"),
        );
    }
    // v0.157 (ADR 0183): a record body may be generic. #593: a sum body may too
    // — its variant payloads resolve the parameters as rigid vars, exactly as
    // record fields do. Type parameters on a refined / opaque body are still
    // rejected; a parameter shadowing a declared type is diagnosed (mirrors the
    // function-generics rule).
    let type_params: HashSet<String> = t.type_params.iter().map(|p| p.name.name.clone()).collect();
    if !t.type_params.is_empty() {
        check_duplicate_type_params(&t.type_params, &format!("type `{}`", t.name.name), errors);
        if !matches!(t.body, TypeBody::Record(_) | TypeBody::Sum(_)) {
            errors.push(
                CompileError::new(
                    "bynk.generics.generic_non_record",
                    t.type_params[0].span,
                    format!(
                        "type `{}` declares type parameters, but only a record (`{{ … }}`) or sum (`| … | …`) type may be generic",
                        t.name.name
                    ),
                )
                .with_note("refined and opaque types cannot be generic — their base is a fixed primitive"),
            );
        }
        // #593: a generic sum may not carry an `embeds` clause. Embedding folds
        // another sum's variants in by name; composing that with per-parameter
        // substitution (the embedded source could itself be generic, or mention
        // the host's parameters) is out of scope for this increment.
        if let TypeBody::Sum(s) = &t.body
            && let Some(clause) = s.embeds.first()
        {
            errors.push(
                CompileError::new(
                    "bynk.generics.generic_sum_embeds",
                    clause.span,
                    format!("generic sum `{}` cannot use an `embeds` clause", t.name.name),
                )
                .with_note("embedding into a generic sum is not supported — declare the variants directly, or make the sum non-generic"),
            );
        }
        for tp in &t.type_params {
            if types.contains_key(&tp.name.name) {
                errors.push(
                    CompileError::new(
                        "bynk.generics.type_arg_mismatch",
                        tp.span,
                        format!(
                            "type parameter `{}` shadows the declared type of the same name",
                            tp.name.name
                        ),
                    )
                    .with_note("rename the type parameter"),
                );
            }
        }
    }
    match &t.body {
        TypeBody::Refined { .. } => {
            // Refined-type bodies only reference base types directly.
        }
        TypeBody::Opaque { .. } => {
            // Opaque-type bodies only reference base types directly.
        }
        TypeBody::Record(r) => {
            let mut seen = HashMap::new();
            for f in &r.fields {
                if let Some(prev_span) = seen.get(&f.name.name) {
                    errors.push(
                        CompileError::new(
                            "bynk.resolve.duplicate_field",
                            f.name.span,
                            format!("field `{}` is declared more than once", f.name.name),
                        )
                        .with_label(*prev_span, "previously declared here"),
                    );
                } else {
                    seen.insert(f.name.name.clone(), f.name.span);
                }
                // Detect containment cycles: a direct `type A = { f: A }`,
                // and indirect cycles through direct record fields
                // (`A = { b: B }`, `B = { a: A }`). Such a cycle admits no finite
                // value, and defeats every structural walk downstream (zero-value
                // emission, codecs). A `List[...]`/`Option[...]` wrapper (whose
                // empty/`None` inhabitant breaks the cycle) is not a direct edge.
                // v0.157 (ADR 0183): a generic self-reference `f: A[T]` is a
                // `TypeRef::App` direct edge — caught here in the checker (and so
                // in the standalone LSP), not only by the emit-side boundary pass.
                if let Some(head) = direct_record_head(&f.type_ref) {
                    if head == t.name.name {
                        errors.push(
                            CompileError::new(
                                "bynk.resolve.recursive_record_field",
                                f.name.span,
                                format!(
                                    "record `{}` cannot directly contain a field of its own type",
                                    t.name.name
                                ),
                            )
                            .with_label(t.name.span, "type declared here")
                            .with_note(
                                "wrap the recursive reference in `Option[...]` to break the cycle",
                            ),
                        );
                    } else if record_field_reaches(head, &t.name.name, types) {
                        errors.push(
                            CompileError::new(
                                "bynk.resolve.recursive_record_field",
                                f.name.span,
                                format!(
                                    "record `{}` contains itself through this field — `{}` leads back to `{}`",
                                    t.name.name, head, t.name.name
                                ),
                            )
                            .with_label(t.name.span, "type declared here")
                            .with_note(
                                "wrap one field in the cycle in `Option[...]` to break it",
                            ),
                        );
                    }
                }
                check_type_ref_resolves_in(&f.type_ref, types, &type_params, errors);
            }
        }
        TypeBody::Sum(s) => {
            let mut seen = HashMap::new();
            for v in &s.variants {
                if let Some(prev_span) = seen.get(&v.name.name) {
                    errors.push(
                        CompileError::new(
                            "bynk.resolve.duplicate_variant",
                            v.name.span,
                            format!("variant `{}` is declared more than once", v.name.name),
                        )
                        .with_label(*prev_span, "previously declared here"),
                    );
                } else {
                    seen.insert(v.name.name.clone(), v.name.span);
                }
                let mut payload_seen = HashMap::new();
                for f in &v.payload {
                    if let Some(prev) = payload_seen.get(&f.name.name) {
                        errors.push(
                            CompileError::new(
                                "bynk.resolve.duplicate_field",
                                f.name.span,
                                format!(
                                    "payload field `{}` is declared more than once in variant `{}`",
                                    f.name.name, v.name.name
                                ),
                            )
                            .with_label(*prev, "previously declared here"),
                        );
                    } else {
                        payload_seen.insert(f.name.name.clone(), f.name.span);
                    }
                    // #1653: a variant is a flat `{ "kind": "<Variant>", ... }`
                    // object on the wire, so a payload field named `kind` would
                    // collide with the discriminant itself.
                    if f.name.name == "kind" {
                        errors.push(
                            CompileError::new(
                                "bynk.resolve.reserved_payload_field",
                                f.name.span,
                                format!(
                                    "variant `{}` cannot have a payload field named `kind`",
                                    v.name.name
                                ),
                            )
                            .with_note(
                                "`kind` carries the variant's name when a sum is encoded as JSON; rename the field (e.g. `category`)",
                            ),
                        );
                    }
                    // #593: a generic sum's declared type parameters are in scope
                    // in its variant payloads, resolving as rigid vars (empty set
                    // for a non-generic sum — the same reference walk as before).
                    check_type_ref_resolves_in(&f.type_ref, types, &type_params, errors);
                }
            }
            // v0.154 (ADR 0178): the `embeds E as V` clauses' source types must
            // resolve (the target variant is checked in `check_embeds`).
            for clause in &s.embeds {
                check_type_ref_resolves(&clause.source_type, types, errors);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_fn_refs(
    f: &FnDecl,
    types: &HashMap<String, Arc<TypeDecl>>,
    fns: &HashMap<String, Arc<FnDecl>>,
    methods: &HashMap<String, MethodTable>,
    assets: &HashMap<String, AssetDecl>,
    embeds: &HashMap<String, EmbedDecl>,
    shadowed: &HashMap<String, ShadowedImport>,
    errors: &mut Sinks,
) {
    // Parameter types resolve.
    // v0.20a: the fn's type parameters are legal named references in its
    // own signature and body annotations.
    let mut type_params: HashSet<String> = f
        .type_params
        .iter()
        .map(|tp| tp.name.name.clone())
        .collect();
    check_duplicate_type_params(
        &f.type_params,
        &format!("function `{}`", f.name.display()),
        errors,
    );
    // #594: an instance method on a generic type inherits the receiver type's
    // parameters into scope, so `fn Box.map[U](self, f: A -> U) -> Box[U]` may
    // name the type's own parameter `A` alongside the method's `U`. A method
    // parameter that reuses one of the type's parameter names would shadow it
    // ambiguously in the substitution — diagnose the collision.
    if let FnName::Method { type_name, .. } = &f.name
        && let Some(recv) = types.get(&type_name.name)
    {
        for tp in &recv.type_params {
            if type_params.contains(&tp.name.name) {
                errors.push(
                    CompileError::new(
                        "bynk.generics.duplicate_type_param",
                        f.type_params
                            .iter()
                            .find(|mp| mp.name.name == tp.name.name)
                            .map_or(tp.span, |mp| mp.span),
                        format!(
                            "type parameter `{}` is already a parameter of the receiver type `{}`",
                            tp.name.name, type_name.name
                        ),
                    )
                    .with_label(tp.span, "declared on the type here"),
                );
            }
            type_params.insert(tp.name.name.clone());
        }
    }
    let mut seen_params: HashMap<&str, &Ident> = HashMap::new();
    for p in &f.params {
        check_type_ref_resolves_in(&p.type_ref, types, &type_params, errors);
        if let Some(prev) = seen_params.get(p.name.name.as_str()) {
            errors.push(
                CompileError::new(
                    "bynk.resolve.duplicate_param",
                    p.name.span,
                    format!("parameter `{}` is declared more than once", p.name.name),
                )
                .with_label(prev.span, "previously declared here"),
            );
        } else {
            seen_params.insert(p.name.name.as_str(), &p.name);
        }
    }
    check_type_ref_resolves_in(&f.return_type, types, &type_params, errors);

    // Build the initial scope: parameters plus `self` (for instance methods).
    let mut params: HashMap<String, ()> =
        f.params.iter().map(|p| (p.name.name.clone(), ())).collect();
    if f.has_self {
        params.insert("self".to_string(), ());
    }
    let in_method = matches!(f.name, FnName::Method { .. });
    let mut cx = RefCheckCtx {
        params: &params,
        in_method,
        types,
        type_params: &type_params,
        fns,
        methods,
        assets,
        embeds,
        shadowed,
        scopes: Vec::new(),
        errors,
    };
    check_block_references(&f.body, &mut cx);
}

/// #1807, #1814: the walk the naming gates share: every place source writes
/// a type name, reporting each one that names a hidden type. See
/// [`check_hidden_type_names`] for what is walked.
struct Gate<'a> {
    /// Each gated type name, with its owning unit.
    hidden: &'a std::collections::BTreeMap<String, String>,
    /// #1814: the error for a gated name written at `id`, given its owner. A
    /// unit and a suite name the `uses` that would admit a hidden type
    /// differently; a shadowed import is a conflict instead (#1927).
    report: &'a dyn Fn(&Ident, &str) -> CompileError,
    /// #1927: each bare variant whose construction is gated, with its sum's
    /// name. Empty for #1807's hidden types, whose bare variants are values
    /// typed by their position; a shadowed sum's construction emits its type's
    /// name, which the local declaration has taken.
    variants: std::collections::BTreeMap<String, String>,
    /// #1927: for a shadowed type, the local declaration of each name, which
    /// answers a receiver it can serve (`svc.GET(…)` in a suite, `logo.url`).
    /// `None` for #1807's hidden types, which nothing local shares a name with.
    locals: Option<&'a HashMap<String, ShadowedImport>>,
    errors: Vec<CompileError>,
}
impl Gate<'_> {
    /// #1927: whether the receiver `id` of a method call (`method`) or a
    /// field read (`field`) is the local declaration that hides the type:
    /// a service addressed in a suite, or an asset's `url`.
    fn local_receiver(&self, id: &Ident, method: Option<&Ident>, field: Option<&Ident>) -> bool {
        let Some(s) = self.locals.and_then(|l| l.get(&id.name)) else {
            return false;
        };
        match s.local {
            "service" => method.is_some(),
            "asset" => field.is_some_and(|f| f.name == "url"),
            _ => false,
        }
    }
    fn name(&mut self, id: &Ident) {
        if let Some(owner) = self.hidden.get(&id.name) {
            self.errors.push((self.report)(id, owner));
        }
    }
    /// #1927: a bare variant constructed (`Done`, `Done(x)`) of a gated sum.
    /// It names the sum in the emitted module (`Status.Done`), so it is the
    /// conflict; the error says so, since matching the same variant is legal.
    fn bare_variant(&mut self, id: &Ident) {
        let Some(sum) = self.variants.get(&id.name) else {
            return;
        };
        let Some(s) = self.locals.and_then(|l| l.get(sum)) else {
            return;
        };
        self.errors.push(
            CompileError::new(
                "bynk.uses.name_conflict",
                id.span,
                format!(
                    "`{}` constructs a variant of `{sum}` from `{}`, which this unit's {} `{sum}` hides",
                    id.name, s.unit, s.local
                ),
            )
            .with_label(s.local_span, format!("{} declared here", s.local))
            .with_note(format!(
                "constructing a variant bare still names its type (`{sum}.{}`); match on it instead, or rename one of the two",
                id.name
            )),
        );
    }
    fn type_ref(&mut self, r: &TypeRef) {
        match r {
            TypeRef::Named(id) => self.name(id),
            TypeRef::App { name, args, .. } => {
                self.name(name);
                for a in args {
                    self.type_ref(a);
                }
            }
            TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
                self.type_ref(a);
                self.type_ref(b);
            }
            TypeRef::Option(t, _)
            | TypeRef::Effect(t, _)
            | TypeRef::HttpResult(t, _)
            | TypeRef::List(t, _)
            | TypeRef::Set(t, _)
            | TypeRef::Query(t, _)
            | TypeRef::Stream(t, _)
            | TypeRef::Connection(t, _)
            | TypeRef::History(t, _) => self.type_ref(t),
            TypeRef::Fn(params, ret, _) => {
                for p in params {
                    self.type_ref(p);
                }
                self.type_ref(ret);
            }
            TypeRef::Base(..)
            | TypeRef::QueueResult(_)
            | TypeRef::Asset(_)
            | TypeRef::RequestHeaders(_)
            | TypeRef::TokenClaims(_)
            | TypeRef::BoundaryRejection(_)
            | TypeRef::ValidationError(_)
            | TypeRef::JsonError(_)
            | TypeRef::Unit(_) => {}
        }
    }
    fn type_decl(&mut self, t: &TypeDecl) {
        match &t.body {
            TypeBody::Record(r) => {
                for f in &r.fields {
                    self.type_ref(&f.type_ref);
                    // Review of #1813: an `event` field default.
                    if let Some(init) = &f.init {
                        self.expr(init);
                    }
                }
            }
            TypeBody::Sum(s) => {
                for v in &s.variants {
                    for p in &v.payload {
                        self.type_ref(&p.type_ref);
                    }
                }
            }
            TypeBody::Refined { .. } | TypeBody::Opaque { .. } => {}
        }
    }
    fn signature(&mut self, params: &[Param], ret: &TypeRef) {
        for p in params {
            self.type_ref(&p.type_ref);
        }
        self.type_ref(ret);
    }
    fn pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Variant {
                type_name,
                variant,
                bindings,
                ..
            } => {
                if let Some(tn) = type_name {
                    self.name(tn);
                } else if bindings.is_empty() {
                    // A bare, payload-less pattern may name a refined type
                    // rather than a variant: `s is Repo` tests a refinement.
                    self.name(variant);
                }
                for b in bindings {
                    match &b.kind {
                        PatternBindingKind::Positional { pattern }
                        | PatternBindingKind::Named { pattern, .. } => self.pattern(pattern),
                    }
                }
            }
            Pattern::Refined { inner, .. } => self.pattern(inner),
            Pattern::Or(alts, _) => {
                for a in alts {
                    self.pattern(a);
                }
            }
            Pattern::Wildcard(_) | Pattern::Binding(_) | Pattern::Literal { .. } => {}
        }
    }
    fn block(&mut self, b: &Block) {
        for s in &b.statements {
            if let Statement::Let(l) | Statement::EffectLet(l) = s
                && let Some(t) = &l.type_annot
            {
                self.type_ref(t);
            }
        }
        // `expr_children` of a block expression is exactly its statements'
        // expressions and its tail.
        for e in &b.statements {
            let mut exprs = Vec::new();
            statement_exprs(e, &mut exprs);
            for x in exprs {
                self.expr(x);
            }
        }
        self.expr(&b.tail);
    }
    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Block(b) => return self.block(b),
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.expr(cond);
                self.block(then_block);
                return self.block(else_block);
            }
            ExprKind::Match { discriminant, arms } => {
                self.expr(discriminant);
                for arm in arms {
                    self.pattern(&arm.pattern);
                    if let Some(g) = &arm.guard {
                        self.expr(g);
                    }
                    match &arm.body {
                        MatchBody::Expr(b) => self.expr(b),
                        MatchBody::Block(b) => self.block(b),
                    }
                }
                return;
            }
            ExprKind::Is { pattern, .. } => self.pattern(pattern),
            ExprKind::RecordConstruction { type_name, .. } => self.name(type_name),
            ExprKind::RecordSpread {
                type_name: Some(tn),
                ..
            } => self.name(tn),
            ExprKind::FieldAccess { receiver, field } => {
                if let ExprKind::Ident(id) = &receiver.kind
                    && !self.local_receiver(id, None, Some(field))
                {
                    self.name(id);
                }
            }
            ExprKind::MethodCall {
                receiver,
                method,
                type_args,
                ..
            } => {
                if let ExprKind::Ident(id) = &receiver.kind
                    && !self.local_receiver(id, Some(method), None)
                {
                    self.name(id);
                }
                for t in type_args {
                    self.type_ref(t);
                }
            }
            ExprKind::Call {
                name, type_args, ..
            } => {
                self.bare_variant(name);
                for t in type_args {
                    self.type_ref(t);
                }
            }
            ExprKind::Ident(id) => self.bare_variant(id),
            ExprKind::Val { type_ref, .. } => self.type_ref(type_ref),
            ExprKind::Lambda(l) => {
                for p in &l.params {
                    if let Some(t) = &p.type_ref {
                        self.type_ref(t);
                    }
                }
            }
            _ => {}
        }
        for c in expr_children(e) {
            self.expr(c);
        }
    }
}

/// #1807: the naming gate for *hidden* types. A hidden type is one this unit's
/// composed table carries only because an imported declaration reaches it
/// (`crate::project_model::close_reachable_types`): resolvable here, never
/// nameable, since `uses` is one level (ADR 0278). Every place the unit's
/// source writes a type name is walked: type references in declarations,
/// signatures, `let` annotations, lambda parameters and type arguments; a
/// record, constructor or spread's type; a type used as a receiver
/// (`Repo.of(…)`, `Status.Pending`); a method declared on a type; a
/// pattern's qualifier; and a refinement test (`s is Repo`). Bodies are
/// walked, and so are an agent's invariants, transitions and `store`
/// initialisers, and an `event` field's default. A hidden name in any of them is
/// `bynk.resolve.unknown_type`, the error the name gave before the closure
/// existed, with a note naming the commons to `uses`.
///
/// A bare variant (`Pending`) is a value, not a type name, so it is not gated:
/// like a literal admitted as a hidden refined type, it is typed by the
/// position it fills. (A context still cannot construct one: the rebrand
/// construction check covers a hidden commons sum like a `uses`d one.)
pub fn check_hidden_type_names(
    items: &[CommonsItem],
    hidden: &std::collections::BTreeMap<String, String>,
) -> Vec<CompileError> {
    let report = |id: &Ident, owner: &str| {
        hidden_type_error(
            id,
            format!(
                "`{}` reaches this unit only through an imported declaration. It is \
                 declared in `{owner}`, which this unit does not `uses`; add `uses \
                 {owner}` to name it",
                id.name
            ),
        )
    };
    let mut g = Gate {
        hidden,
        report: &report,
        variants: Default::default(),
        locals: None,
        errors: Vec::new(),
    };
    gate_items(&mut g, items);
    g.errors
}

/// `bynk.resolve.unknown_type` for a hidden type written at `id`. Not
/// `unknown_type_error`: its note lists what is in scope, which a hidden
/// type, reached but unnameable, is not.
fn hidden_type_error(id: &Ident, note: String) -> CompileError {
    CompileError::new(
        "bynk.resolve.unknown_type",
        id.span,
        format!("unknown type `{}`", id.name),
    )
    .with_note(note)
}

/// #1927: the naming gate for a `uses`-imported type the unit hides behind
/// its own declaration of another kind (a function, asset, service, agent or
/// provider: `crate::project_model::shadowed_imports`). The type stays in the
/// unit's table, so an imported declaration that uses it still checks, and a
/// value of it still flows through the unit's code. But the emitted module
/// binds the name to the local declaration, so the unit may not write it: every
/// position [`check_hidden_type_names`] gates is `bynk.uses.name_conflict`
/// here, and so is constructing one of the sum's variants bare (`Done`), which
/// emits the type's name too. Matching a variant does not.
pub fn check_shadowed_type_names(
    items: &[CommonsItem],
    shadowed: &HashMap<String, ShadowedImport>,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Vec<CompileError> {
    let (gated, variants) = shadowed_type_gate(shadowed, types);
    let report = |id: &Ident, _owner: &str| {
        let s = &shadowed[&id.name];
        shadowed_import_error(&id.name, id.span, s)
    };
    let mut g = Gate {
        hidden: &gated,
        report: &report,
        variants,
        locals: Some(shadowed),
        errors: Vec::new(),
    };
    gate_items(&mut g, items);
    g.errors
}

/// #1927: the shadowed imported types, by name with their owning unit, and
/// each bare variant of a shadowed sum, with the sum's name.
fn shadowed_type_gate(
    shadowed: &HashMap<String, ShadowedImport>,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> (
    std::collections::BTreeMap<String, String>,
    std::collections::BTreeMap<String, String>,
) {
    let mut gated = std::collections::BTreeMap::new();
    let mut variants = std::collections::BTreeMap::new();
    for (name, s) in shadowed.iter().filter(|(_, s)| s.imported == "type") {
        gated.insert(name.clone(), s.unit.clone());
        if let Some(TypeBody::Sum(sum)) = types.get(name).map(|t| &t.body) {
            for v in &sum.variants {
                variants.insert(v.name.name.clone(), name.clone());
            }
        }
    }
    (gated, variants)
}

/// Walk every item of a unit through `g`.
fn gate_items(g: &mut Gate<'_>, items: &[CommonsItem]) {
    if g.hidden.is_empty() {
        return;
    }
    for item in items {
        match item {
            CommonsItem::Type(t) => g.type_decl(t),
            CommonsItem::Event(e) => g.type_decl(&e.as_type_decl()),
            CommonsItem::Fn(f) => {
                if let FnName::Method { type_name, .. } = &f.name {
                    g.name(type_name);
                }
                g.signature(&f.params, &f.return_type);
                g.block(&f.body);
            }
            CommonsItem::Capability(c) => {
                for op in &c.ops {
                    g.signature(&op.params, &op.return_type);
                }
            }
            CommonsItem::Provider(p) => {
                for op in &p.ops {
                    g.signature(&op.params, &op.return_type);
                    g.block(&op.body);
                }
            }
            CommonsItem::Service(s) => {
                match &s.protocol {
                    ServiceProtocol::WebSocket { in_type, out_type } => {
                        g.type_ref(in_type);
                        g.type_ref(out_type);
                    }
                    ServiceProtocol::Events { event_type, .. } => g.type_ref(event_type),
                    ServiceProtocol::Call
                    | ServiceProtocol::Http
                    | ServiceProtocol::Cron
                    | ServiceProtocol::Queue { .. } => {}
                }
                for h in &s.handlers {
                    g.signature(&h.params, &h.return_type);
                    g.block(&h.body);
                }
            }
            CommonsItem::Agent(a) => {
                g.type_ref(&a.key_type);
                for f in &a.store_fields {
                    for arg in &f.kind.args {
                        g.type_ref(arg);
                    }
                    if let Some(init) = &f.init {
                        g.expr(init);
                    }
                }
                for h in &a.handlers {
                    g.signature(&h.params, &h.return_type);
                    g.block(&h.body);
                }
                for inv in &a.invariants {
                    g.expr(&inv.predicate);
                }
                for t in &a.transitions {
                    g.expr(&t.predicate);
                }
            }
            CommonsItem::Actor(a) => {
                if let Some(id) = &a.identity {
                    g.type_ref(id);
                }
            }
            CommonsItem::Messages(_) | CommonsItem::Asset(_) | CommonsItem::Embed(_) => {}
        }
    }
}

/// #1814: the naming gate for a test suite's view. A suite resolves names in
/// its target's privileged view (a unit suite) or its harness table (a
/// `system` suite), each closed over the types its imported declarations reach
/// (`crate::project_model::close_view_types`). A hidden type is resolvable
/// there, so a case can build and read a reached field, but a suite may no
/// more name it than its target may. Walks every `stub` clause's argument
/// patterns and values, every case body, and every property's `for all`
/// bindings, `where` filter and predicate, as [`check_hidden_type_names`]
/// walks a unit's items. `target` is the unit whose `uses` would admit a
/// hidden name: the target of a unit suite, or the suite itself (`None`) for a
/// `system` suite, which declares its own `uses`.
pub fn check_hidden_type_names_in_suite(
    suite: &SuiteDecl,
    target: Option<&str>,
    hidden: &std::collections::BTreeMap<String, String>,
) -> Vec<CompileError> {
    let report = |id: &Ident, owner: &str| {
        let name = &id.name;
        hidden_type_error(
            id,
            match target {
                Some(target) => format!(
                    "`{name}` reaches this suite only through an imported declaration. It is \
                     declared in `{owner}`, which `{target}` does not `uses`; add `uses {owner}` \
                     to `{target}` to name it"
                ),
                None => format!(
                    "`{name}` reaches this suite only through an imported declaration. It is \
                     declared in `{owner}`, which this suite does not `uses`; add `uses {owner}` \
                     to name it"
                ),
            },
        )
    };
    let mut g = Gate {
        hidden,
        report: &report,
        variants: Default::default(),
        locals: None,
        errors: Vec::new(),
    };
    gate_suite(&mut g, suite);
    g.errors
}

/// #1927: [`check_shadowed_type_names`] for a unit suite: a case may no more
/// write a type its target hides than the target may.
pub fn check_shadowed_type_names_in_suite(
    suite: &SuiteDecl,
    shadowed: &HashMap<String, ShadowedImport>,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Vec<CompileError> {
    let (gated, variants) = shadowed_type_gate(shadowed, types);
    let report =
        |id: &Ident, _owner: &str| shadowed_import_error(&id.name, id.span, &shadowed[&id.name]);
    let mut g = Gate {
        hidden: &gated,
        report: &report,
        variants,
        locals: Some(shadowed),
        errors: Vec::new(),
    };
    gate_suite(&mut g, suite);
    g.errors
}

/// Walk every stub, case and property of `suite` through `g`.
fn gate_suite(g: &mut Gate<'_>, suite: &SuiteDecl) {
    if g.hidden.is_empty() {
        return;
    }
    let stub = |g: &mut Gate<'_>, s: &StubClause| {
        for a in &s.args {
            if let ArgPattern::Value(e) = a {
                g.expr(e);
            }
        }
        match &s.rhs {
            StubRhs::Returns(e) => g.expr(e),
            StubRhs::ReturnsEach(outcomes, _) => {
                for o in outcomes {
                    if let SeqOutcome::Value(e) = o {
                        g.expr(e);
                    }
                }
            }
            StubRhs::Fails(_) => {}
        }
    };
    for s in &suite.stubs {
        stub(g, s);
    }
    for case in &suite.cases {
        for s in &case.stubs {
            stub(g, s);
        }
        g.block(&case.body);
    }
    for prop in &suite.properties {
        for b in &prop.forall.bindings {
            g.type_ref(&b.type_ref);
        }
        if let Some(w) = &prop.forall.where_pred {
            g.expr(w);
        }
        g.block(&prop.forall.body);
    }
}

fn unknown_type_error(id: &Ident) -> CompileError {
    CompileError::new(
        "bynk.resolve.unknown_type",
        id.span,
        format!("unknown type `{}`", id.name),
    )
    .with_note(
        "in scope are the base types (`Int`, `Float`, `String`, `Bool`, `Duration`, \
         `Instant`, `Bytes`), the built-in generics (`List`, `Map`, `Option`, `Result`, …), \
         `ValidationError`, and the types this unit declares or imports",
    )
}

/// v0.157 (ADR 0183): a generic type named without its `[…]` arguments.
fn bare_generic_type_error(id: &Ident, arity: usize) -> CompileError {
    CompileError::new(
        "bynk.generics.type_arg_count",
        id.span,
        format!(
            "generic type `{}` must be applied to {} type argument{} — write `{}[…]`",
            id.name,
            arity,
            if arity == 1 { "" } else { "s" },
            id.name
        ),
    )
    .with_note("a generic type is used only through a concrete instantiation")
}

/// Recursively check that every type reference resolves.
fn check_type_ref_resolves(
    r: &TypeRef,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Sinks,
) {
    check_type_ref_resolves_in(r, types, &HashSet::new(), errors)
}

/// v0.20a: like [`check_type_ref_resolves`], with the enclosing function's
/// type parameters in scope — a `Named` reference matching one is a type
/// variable, not an unknown type.
fn check_type_ref_resolves_in(
    r: &TypeRef,
    types: &HashMap<String, Arc<TypeDecl>>,
    type_params: &HashSet<String>,
    errors: &mut Sinks,
) {
    match r {
        TypeRef::Base(_, _) => {}
        // v0.20a: a function type's components must each resolve.
        TypeRef::Fn(params, ret, _) => {
            for p in params {
                check_type_ref_resolves_in(p, types, type_params, errors);
            }
            check_type_ref_resolves_in(ret, types, type_params, errors);
        }
        TypeRef::Named(id) => {
            if let Some(decl) = types.get(&id.name) {
                errors.refs.record(id.span, SymbolKind::Type, &id.name);
                // v0.157 (ADR 0183): a generic type must be applied to its type
                // arguments — a bare `Paginated` (declared `Paginated[T]`) is an
                // under-application.
                if !decl.type_params.is_empty() {
                    errors.push(bare_generic_type_error(id, decl.type_params.len()));
                }
            } else if !type_params.contains(&id.name) {
                errors.push(unknown_type_error(id));
            }
        }
        // v0.157 (ADR 0183): `Name[Arg, …]` — a user generic-type application.
        // Validate existence, that the target is generic, and arity; then walk
        // the arguments.
        TypeRef::App { name, args, span } => {
            match types.get(&name.name) {
                None if type_params.contains(&name.name) => {
                    // A type parameter applied to arguments (`T[Int]`) — a type
                    // parameter is not itself generic (no higher-kinded types).
                    errors.push(
                        CompileError::new(
                            "bynk.generics.type_arg_count",
                            *span,
                            format!(
                                "type parameter `{}` cannot take type arguments — it is not a generic type",
                                name.name
                            ),
                        )
                        .with_note("higher-kinded type parameters are not supported"),
                    );
                }
                None => errors.push(unknown_type_error(name)),
                Some(decl) => {
                    errors.refs.record(name.span, SymbolKind::Type, &name.name);
                    let expected = decl.type_params.len();
                    // Finding #46: `decl` comes from the combined cross-file
                    // symbol table (`uses`/multi-file siblings), so its span
                    // may belong to a different file than `name` — a label
                    // can't express that without per-label file identity (a
                    // Wave 8 follow-up). A note keeps the same conservative
                    // choice `bynk-emit/src/project/consistency.rs` already
                    // makes for its own always-cross-file diagnostics,
                    // rather than risk underlining unrelated text.
                    if expected == 0 {
                        errors.push(
                            CompileError::new(
                                "bynk.generics.type_arg_count",
                                *span,
                                format!(
                                    "type `{}` is not generic — it takes no type arguments",
                                    name.name
                                ),
                            )
                            .with_note("type declared here"),
                        );
                    } else if expected != args.len() {
                        errors.push(
                            CompileError::new(
                                "bynk.generics.type_arg_count",
                                *span,
                                format!(
                                    "type `{}` expects {} type argument{}, but {} {} given",
                                    name.name,
                                    expected,
                                    if expected == 1 { "" } else { "s" },
                                    args.len(),
                                    if args.len() == 1 { "was" } else { "were" },
                                ),
                            )
                            .with_note("type declared here"),
                        );
                    }
                }
            }
            for a in args {
                check_type_ref_resolves_in(a, types, type_params, errors);
            }
        }
        TypeRef::Result(t, e, _) => {
            check_type_ref_resolves_in(t, types, type_params, errors);
            check_type_ref_resolves_in(e, types, type_params, errors);
        }
        TypeRef::Option(t, _) => {
            check_type_ref_resolves_in(t, types, type_params, errors);
        }
        TypeRef::Effect(t, _) => {
            check_type_ref_resolves_in(t, types, type_params, errors);
        }
        TypeRef::HttpResult(t, _) => {
            check_type_ref_resolves_in(t, types, type_params, errors);
        }
        TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_) => {}
        TypeRef::List(t, _) => {
            check_type_ref_resolves_in(t, types, type_params, errors);
        }
        // K6a (#2596): a `Set`'s element follows the `Map`-key rule.
        TypeRef::Set(t, _) => {
            check_type_ref_resolves_in(t, types, type_params, errors);
            check_keyable(t, types, type_params, errors, KeyRole::SetElement);
        }
        TypeRef::Query(t, _) => {
            check_type_ref_resolves_in(t, types, type_params, errors);
        }
        TypeRef::Stream(t, _) => {
            check_type_ref_resolves_in(t, types, type_params, errors);
        }
        TypeRef::Connection(t, _) => {
            check_type_ref_resolves_in(t, types, type_params, errors);
        }
        // v0.119 (ADR 0155): `History[Agent]` is a test-only generator, legal only
        // as a `for all` binding inside a `property` (validated in
        // `check_property_body`). A `History[…]` reaching this declared-type walk —
        // a field, parameter, return, or local annotation — is out of place.
        TypeRef::History(_, span) => {
            errors.push(
                CompileError::new(
                    "bynk.history.outside_property",
                    *span,
                    "`History[…]` is only valid as a `for all` generator inside a `property`",
                )
                .with_note(
                    "bind a driven call-history with `for all run: History[Agent]` in a `property`",
                ),
            );
        }
        TypeRef::Map(k, v, _) => {
            check_type_ref_resolves_in(k, types, type_params, errors);
            check_type_ref_resolves_in(v, types, type_params, errors);
            check_keyable(k, types, type_params, errors, KeyRole::MapKey);
        }
        TypeRef::ValidationError(_) | TypeRef::JsonError(_) => {}
        TypeRef::Unit(_) => {}
    }
}

/// v0.20b: `Map` keys are confined to value-keyable types — `String`, `Int`,
/// and refined/opaque types over them — so the emitted `ReadonlyMap` keeps
/// value equality (object keys would compare by reference). A type parameter
/// is admitted in key position: it can only ever be instantiated through a
/// concrete `Map[K, V]` reference elsewhere, and that site is checked.
/// Which value-keyable position [`check_keyable`] is checking.
#[derive(Clone, Copy)]
enum KeyRole {
    MapKey,
    /// K6a (#2596): a `Set`'s element.
    SetElement,
}

fn check_keyable(
    k: &TypeRef,
    types: &HashMap<String, Arc<TypeDecl>>,
    type_params: &HashSet<String>,
    errors: &mut Sinks,
    role: KeyRole,
) {
    let keyable = match k {
        TypeRef::Base(BaseType::String | BaseType::Int, _) => true,
        TypeRef::Named(id) => {
            // A type parameter is admitted (see above). An unknown name has
            // already been reported by the resolution walk; don't pile a
            // keyability error on top of it.
            if type_params.contains(&id.name) || !types.contains_key(&id.name) {
                return;
            }
            matches!(
                types.get(&id.name).map(|t| &t.body),
                Some(TypeBody::Refined { base, .. } | TypeBody::Opaque { base, .. })
                    if matches!(base, BaseType::String | BaseType::Int)
            )
        }
        _ => false,
    };
    if !keyable {
        let (code, what) = match role {
            KeyRole::MapKey => ("bynk.types.unkeyable_map_key", "a `Map` key"),
            KeyRole::SetElement => ("bynk.types.unkeyable_set_element", "a `Set` element"),
        };
        errors.push(
            CompileError::new(
                code,
                k.span(),
                format!("{what} must be value-keyable — `String`, `Int`, or a refined/opaque type over them"),
            )
            .with_note(
                "record, sum, collection, and function keys are rejected in v0.20b; value-equality keys need bounded generics",
            ),
        );
    }
}

/// Lookup a name across scopes. Returns true if it's bound somewhere
/// (param, self, or any let-scope).
fn name_in_scope(name: &str, params: &HashMap<String, ()>, scopes: &[HashMap<String, ()>]) -> bool {
    if params.contains_key(name) {
        return true;
    }
    scopes.iter().rev().any(|s| s.contains_key(name))
}

/// Validate a record construction's *field set* — every required field present,
/// no undeclared extra field, no field initialised twice, and every shorthand
/// `{ name }` bound in scope. Pure over the declaration and the provided fields;
/// the caller supplies its own scope predicate (the resolver's lexical scope via
/// [`name_in_scope`], the checker's binding table via `Ctx::lookup`) and its own
/// diagnostic sink.
///
/// #711: this walk skips `Service`/`Agent`/`Actor` items, so their handler
/// bodies never pass through it — the checker's `check_record_construction` is
/// their only backstop and calls this same function. A single implementation is
/// the point: an earlier fix copied three of these four checks into the checker
/// and dropped the shorthand one, re-opening the gap for shorthand fields. Both
/// callers now share this, so the two cannot re-diverge.
pub(crate) fn check_record_field_set(
    type_name: &Ident,
    fields: &[FieldInit],
    record: &RecordBody,
    // #852: the span of the whole `TypeName { … }` literal, so the missing-field
    // quick-fix knows where to insert a new field (before the closing brace when
    // the literal is empty).
    construction_span: Span,
    in_scope: impl Fn(&str) -> bool,
    errors: &mut Vec<CompileError>,
) {
    let declared: HashMap<&str, &RecordField> = record
        .fields
        .iter()
        .map(|f| (f.name.name.as_str(), f))
        .collect();
    let mut provided: HashMap<&str, bynk_syntax::span::Span> = HashMap::new();
    for f in fields {
        if !declared.contains_key(f.name.name.as_str()) {
            errors.push(
                CompileError::new(
                    "bynk.resolve.unknown_field",
                    f.name.span,
                    format!(
                        "record type `{}` has no field `{}`",
                        type_name.name, f.name.name
                    ),
                )
                // Finding #46: `decl_name_span` may name a declaration in a
                // different file than this construction site (both callers
                // resolve against the combined cross-file symbol table) — a
                // note instead of a label, matching the same conservative
                // choice made elsewhere for cross-file provenance without
                // per-label file identity (a Wave 8 follow-up).
                .with_note("type declared here"),
            );
        }
        if let Some(prev) = provided.get(f.name.name.as_str()) {
            errors.push(
                CompileError::new(
                    "bynk.resolve.duplicate_field_init",
                    f.name.span,
                    format!("field `{}` is initialised more than once", f.name.name),
                )
                .with_label(*prev, "previously initialised here"),
            );
        } else {
            provided.insert(f.name.name.as_str(), f.name.span);
        }
        // A shorthand `{ name }` (no `: value`) reads the binding `name` from
        // scope — it must exist. The full `field: value` form is checked by the
        // caller (the resolver recurses into the value, the checker types it).
        if f.value.is_none() && !in_scope(&f.name.name) {
            errors.push(
                CompileError::new(
                    "bynk.resolve.unknown_name",
                    f.name.span,
                    format!(
                        "shorthand field initialiser `{}` requires a binding of that name in scope",
                        f.name.name
                    ),
                )
                .with_note("either bring `{name}` into scope or use the full `field: value` form"),
            );
        }
    }
    // Missing required fields. Each is a diagnostic anchored at the type name;
    // a field whose type has a safe default additionally carries a
    // machine-applicable "add field `x`" quick-fix (#852, DECISIONS B/C) that
    // inserts `x: <default>` at a fmt-stable position, and — when more than one
    // field is missing and every missing field is defaultable — the first such
    // diagnostic also carries an "add all missing fields" convenience.
    let missing: Vec<&RecordField> = record
        .fields
        .iter()
        .filter(|f| !provided.contains_key(f.name.name.as_str()))
        .collect();
    // The edit for a `body` of one or more `name: default` entries. With
    // existing fields it appends `, body` right after the last one. With an
    // *empty* literal there is no field span to anchor to and the interior
    // spacing/trailing punctuation is unknown, so instead the whole ` { … }`
    // tail (from the end of the type name through the closing brace) is
    // **replaced** with a canonical ` { body }` — fmt-stable regardless of how
    // the empty braces were originally spelled (`{}`, `{ }`, `{  }`).
    let field_edit = |body: &str| -> (Span, String) {
        match fields.iter().map(|f| f.span.end).max() {
            Some(end) => (Span::new(end, end), format!(", {body}")),
            None => (
                Span::new(type_name.span.end, construction_span.end),
                format!(" {{ {body} }}"),
            ),
        }
    };
    // Defaultable missing fields, in declaration order, as `name: default`.
    let defaultable: Vec<String> = missing
        .iter()
        .filter_map(|f| field_default_init(f))
        .collect();
    let all_defaultable = defaultable.len() == missing.len();

    for (i, decl_field) in missing.iter().enumerate() {
        let mut err = CompileError::new(
            "bynk.resolve.missing_field",
            type_name.span,
            format!(
                "missing required field `{}` for record `{}`",
                decl_field.name.name, type_name.name
            ),
        )
        .with_label(decl_field.name.span, "field declared here");
        if let Some(piece) = field_default_init(decl_field) {
            err = err.with_suggestion(
                format!("add field `{}`", decl_field.name.name),
                vec![field_edit(&piece)],
                Applicability::MachineApplicable,
            );
        }
        // The "add all missing fields" convenience rides on the first missing
        // diagnostic (they all share `type_name.span`, so it surfaces together
        // with the single-field fixes), and only when the whole set is
        // defaultable and there is more than one to add.
        if i == 0 && missing.len() > 1 && all_defaultable {
            err = err.with_suggestion(
                "add all missing fields",
                vec![field_edit(&defaultable.join(", "))],
                Applicability::MachineApplicable,
            );
        }
        errors.push(err);
    }
}

/// The `name: <default>` initialiser for a missing record field, or `None` when
/// the field's type has no value that is guaranteed to re-check clean (#852,
/// DECISION B). Deliberately conservative: an inline-refined field or a
/// user-named type (which may itself be refined, a sum, or opaque) has no
/// synthesised default — only the unrefined built-in scalars, `Option` (`None`),
/// and `List` (`[]`) do, so the inserted value always type-checks.
fn field_default_init(field: &RecordField) -> Option<String> {
    if field.refinement.is_some() {
        return None;
    }
    let default = match &field.type_ref {
        TypeRef::Base(BaseType::Int, _) => "0",
        TypeRef::Base(BaseType::Float, _) => "0.0",
        TypeRef::Base(BaseType::String, _) => "\"\"",
        TypeRef::Base(BaseType::Bool, _) => "false",
        TypeRef::Option(..) => "None",
        TypeRef::List(..) => "[]",
        _ => return None,
    };
    Some(format!("{}: {}", field.name.name, default))
}

#[allow(clippy::too_many_arguments)]
/// Bundles the reference-walk's read-only lookup tables and mutable
/// traversal state (finding #37): threading nine positional parameters
/// through a ~900-line walk meant 313 of resolver.rs's 2,346 lines were
/// argument names at recursive call sites.
struct RefCheckCtx<'a, 'b> {
    params: &'a HashMap<String, ()>,
    in_method: bool,
    types: &'a HashMap<String, Arc<TypeDecl>>,
    type_params: &'a HashSet<String>,
    fns: &'a HashMap<String, Arc<FnDecl>>,
    methods: &'a HashMap<String, MethodTable>,
    /// #1883, slice 1a: the asset names in scope, values of type `Asset`.
    assets: &'a HashMap<String, AssetDecl>,
    /// #2387, slice 1: the embed names in scope, `String` values.
    embeds: &'a HashMap<String, EmbedDecl>,
    /// #1925: imports a local declaration of another kind hides.
    shadowed: &'a HashMap<String, ShadowedImport>,
    scopes: Vec<HashMap<String, ()>>,
    errors: &'a mut Sinks<'b>,
}

fn check_block_references(block: &Block, cx: &mut RefCheckCtx) {
    cx.scopes.push(HashMap::new());
    for stmt in &block.statements {
        match stmt {
            Statement::Let(l) | Statement::EffectLet(l) => {
                check_expr_references(&l.value, cx);
                if let Some(annot) = &l.type_annot {
                    check_type_ref_resolves_in(annot, cx.types, cx.type_params, cx.errors);
                }
                if let Some(prev) = cx.types.get(&l.name.name) {
                    cx.errors.push(
                        CompileError::new(
                            "bynk.resolve.let_shadows_type",
                            l.name.span,
                            format!(
                                "`let {}` shadows the declared type `{}`",
                                l.name.name, l.name.name
                            ),
                        )
                        .with_label(prev.name.span, "type declared here")
                        .with_note("choose a different name for the let binding"),
                    );
                } else if let Some(prev) = cx.fns.get(&l.name.name) {
                    cx.errors.push(
                        CompileError::new(
                            "bynk.resolve.let_shadows_fn",
                            l.name.span,
                            format!(
                                "`let {}` shadows the declared function `{}`",
                                l.name.name, l.name.name
                            ),
                        )
                        .with_label(prev.name.ident().span, "function declared here")
                        .with_note("choose a different name for the let binding"),
                    );
                } else if l.name.name != "_" {
                    cx.scopes
                        .last_mut()
                        .unwrap()
                        .insert(l.name.name.clone(), ());
                }
            }
            Statement::Expect(a) => {
                check_expr_references(&a.value, cx);
            }
            Statement::Send(s) => {
                check_expr_references(&s.value, cx);
            }
            Statement::Do(d) => {
                check_expr_references(&d.value, cx);
            }
            Statement::Assign(a) => {
                // v0.81: walk the RHS for references; the target resolves to a
                // `store` field, handled in the storage-track checker slice.
                check_expr_references(&a.value, cx);
            }
            Statement::Advance(a) => {
                check_expr_references(&a.amount, cx);
            }
        }
    }
    check_expr_references(&block.tail, cx);
    cx.scopes.pop();
}

#[allow(clippy::too_many_lines)]
fn check_expr_references(expr: &Expr, cx: &mut RefCheckCtx) {
    match &expr.kind {
        // v0.43: resolve names referenced inside each interpolation hole.
        ExprKind::InterpStr(parts) => {
            for part in parts {
                if let InterpPart::Hole(hole) = part {
                    check_expr_references(hole, cx);
                }
            }
        }
        ExprKind::IntLit { .. }
        | ExprKind::FloatLit { .. }
        | ExprKind::DurationLit { .. }
        | ExprKind::StrLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::None
        | ExprKind::UnitLit => {}
        // v0.20b: a list literal — each element resolves as a value.
        ExprKind::ListLit(elems) => {
            for el in elems {
                check_expr_references(el, cx);
            }
        }
        // Slice C: `Wire(<String>)` — the raw inner expression resolves as an
        // ordinary value (a string literal in practice).
        ExprKind::Wire(inner) => {
            check_expr_references(inner, cx);
        }
        // v0.20a: a lambda introduces a scope frame holding its params; the
        // body walks with the frame in place. Annotated param types resolve
        // through the ordinary type-ref check.
        ExprKind::Lambda(lambda) => {
            for p in &lambda.params {
                if let Some(tr) = &p.type_ref {
                    check_type_ref_resolves_in(tr, cx.types, cx.type_params, cx.errors);
                }
            }
            let mut frame: HashMap<String, ()> = HashMap::new();
            for p in &lambda.params {
                frame.insert(p.name.name.clone(), ());
            }
            cx.scopes.push(frame);
            check_expr_references(&lambda.body, cx);
            cx.scopes.pop();
        }
        ExprKind::EffectPure(inner) => {
            check_expr_references(inner, cx);
        }
        ExprKind::Expect(inner) => {
            check_expr_references(inner, cx);
        }
        ExprKind::Faults(f) => {
            // #1812: a call-site principal's identity (`by User(who)`) is an
            // ordinary expression, resolved like the call's arguments.
            if let Some(identity) = f.principal.as_ref().and_then(|p| p.identity.as_deref()) {
                check_expr_references(identity, cx);
            }
            check_expr_references(&f.call, cx);
        }
        ExprKind::Val { args, .. } => {
            // v0.9.4: the mocked type is validated by the checker; resolve any
            // pin-argument references here.
            for a in args {
                check_expr_references(a, cx);
            }
        }
        ExprKind::Observation(_) => {
            // v0.117: a `with` predicate's free names are the operation's
            // parameters, bound during type checking and not visible to name
            // resolution; a count is a literal. Nothing to resolve here.
        }
        ExprKind::Trace { .. } => {
            // v0.117: `Cap.op` names a capability seam, not value references.
        }
        ExprKind::RecordSpread {
            type_name,
            base,
            overrides,
        } => {
            if let Some(tn) = type_name
                && !cx.types.contains_key(&tn.name)
            {
                cx.errors.push(unknown_type_error(tn));
            }
            check_expr_references(base, cx);
            for f in overrides {
                if let Some(v) = &f.value {
                    check_expr_references(v, cx);
                }
            }
        }
        ExprKind::Ident(id) => {
            if id.name == "self" {
                if !cx.in_method {
                    cx.errors.push(
                        CompileError::new(
                            "bynk.resolve.self_outside_method",
                            id.span,
                            "`self` can only be used inside a method body",
                        )
                        .with_note(
                            "declare the function as `fn TypeName.method(self, ...)` if you intended a method",
                        ),
                    );
                }
                return;
            }
            if name_in_scope(&id.name, cx.params, &cx.scopes) {
                // OK.
            } else if http_variant(&id.name).is_some() {
                // v0.9: predeclared HttpResult variant (e.g. `NoContent`,
                // `Unauthorized`). The checker validates payload arity and
                // expected-type disambiguation.
            } else if let Some(sum_owner) = find_unique_variant_owner(&id.name, cx.types) {
                // It's a bare variant reference. We treat it as a valid
                // expression in resolver — the type checker will assign
                // the correct sum type. Mark with no error.
                let _ = sum_owner;
            } else if cx.fns.contains_key(&id.name)
                && cx
                    .shadowed
                    .get(&id.name)
                    .is_some_and(|s| s.imported == "type")
            {
                // Review of #1933: a local function hiding an imported type of
                // its name answers a bare reference (a function value), as in
                // the emitted module; the checker judges the position
                // (`check_ident`'s function arm, met before types).
                cx.errors.refs.record(id.span, SymbolKind::Fn, &id.name);
            } else if (cx.assets.contains_key(&id.name) || cx.embeds.contains_key(&id.name))
                && cx
                    .shadowed
                    .get(&id.name)
                    .is_some_and(|s| s.imported == "type")
            {
                // #1927: a local asset (or #2387 embed) hiding an imported
                // type of its name is the asset here, as in the emitted
                // module; the checker types it (`check_ident` meets assets
                // and embeds before types).
            } else if let Some(s) = cx.shadowed.get(&id.name)
                && !matches!(s.local, "function" | "asset" | "embed")
            {
                // #1925: the local declaration (a type, event, service,
                // agent or provider) is no value here, and it hides the import.
                cx.errors.push(shadowed_import_error(&id.name, id.span, s));
            } else if cx.types.contains_key(&id.name) {
                cx.errors.push(
                    CompileError::new(
                        "bynk.resolve.type_in_expr",
                        id.span,
                        format!("`{}` is a type, not a value", id.name),
                    )
                    .with_note(
                        "types cannot appear in expression position; \
                         use `TypeName.of(value)` or `TypeName { ... }` to construct values",
                    ),
                );
            } else if cx.fns.contains_key(&id.name) {
                // v0.20a: a bare named-function reference may be a function
                // VALUE where a function type is expected. The resolver has
                // no type information, so the judgment (and the
                // `bynk.resolve.fn_without_call` diagnostic for non-function
                // positions) now lives in the checker's ident rule. Silent
                // pass here keeps `unknown_name` from misfiring.
                cx.errors.refs.record(id.span, SymbolKind::Fn, &id.name);
            } else if cx.assets.contains_key(&id.name) {
                // #1883, slice 1a: an asset name is a value of type `Asset`;
                // the checker types it (`check_ident`).
            } else if cx.embeds.contains_key(&id.name) {
                // #2387, slice 1: an embed name is a `String` value; the
                // checker types it (`check_ident`).
            } else if find_ambiguous_variant_owners(&id.name, cx.types).len() > 1 {
                cx.errors.push(
                    CompileError::new(
                        "bynk.resolve.ambiguous_variant",
                        id.span,
                        format!(
                            "the variant name `{}` is declared on multiple sum types — qualify it as `TypeName.{}`",
                            id.name, id.name
                        ),
                    ),
                );
            } else {
                cx.errors.push(
                    CompileError::new(
                        "bynk.resolve.unknown_name",
                        id.span,
                        format!("unknown name `{}`", id.name),
                    )
                    .with_note(
                        "only parameters, `let` bindings, and functions declared \
                         in this commons are in scope",
                    ),
                );
            }
        }
        ExprKind::Call {
            name,
            type_args,
            args,
        } => {
            // #712: explicit type arguments (`identity[T](…)`) are type
            // references and must resolve — the checker's `check_generic_call`
            // otherwise dropped an unknown one silently. Validated here so
            // `fn`/method bodies are covered; the checker backstops handler
            // bodies (which never reach this walk).
            for ta in type_args {
                check_type_ref_resolves_in(ta, cx.types, cx.type_params, cx.errors);
            }
            match cx.fns.get(&name.name) {
                Some(decl) => {
                    cx.errors.refs.record(name.span, SymbolKind::Fn, &name.name);
                    if decl.params.len() != args.len() {
                        cx.errors.push(
                            CompileError::new(
                                "bynk.resolve.arity_mismatch",
                                name.span,
                                format!(
                                    "function `{}` expects {} argument(s), but {} were given",
                                    name.name,
                                    decl.params.len(),
                                    args.len()
                                ),
                            )
                            // Finding #46: `decl` is looked up in the
                            // combined cross-file symbol table, so its span
                            // may belong to a different file than this call
                            // — see the same note at the type-arity checks
                            // above.
                            .with_note("function declared here"),
                        );
                    }
                }
                None => {
                    // Maybe it's a variant constructor with a payload (e.g., `Placed(at, total)`).
                    let owners = find_ambiguous_variant_owners(&name.name, cx.types);
                    if http_variant(&name.name).is_some() {
                        // v0.9: predeclared HttpResult variant constructor.
                    } else if owners.len() == 1 {
                        // Single owner — treat as variant construction. Type
                        // checker validates arg count and types.
                    } else if owners.len() > 1 {
                        cx.errors.push(CompileError::new(
                            "bynk.resolve.ambiguous_variant",
                            name.span,
                            format!(
                                "the variant name `{}` is declared on multiple sum types — qualify it as `TypeName.{}(...)`",
                                name.name, name.name
                            ),
                        ));
                    } else if let Some(s) = cx.shadowed.get(&name.name) {
                        // #1925: no local function answers the call (one would
                        // have matched above), and the local declaration of
                        // another kind hides the import.
                        cx.errors
                            .push(shadowed_import_error(&name.name, name.span, s));
                    } else if cx.types.contains_key(&name.name) {
                        cx.errors.push(CompileError::new(
                            "bynk.resolve.type_as_function",
                            name.span,
                            format!(
                                "`{}` is a type, not a function — use `{}.of(value)` or `{} {{ ... }}` instead",
                                name.name, name.name, name.name
                            ),
                        ));
                    } else if name_in_scope(&name.name, cx.params, &cx.scopes) {
                        // v0.20a: an in-scope value being called may be a
                        // legal value application if its type is a function
                        // type. The resolver has no type information, so the
                        // judgment (and `bynk.resolve.param_as_function` for
                        // non-function-typed values) lives in the checker's
                        // call dispatch. Silent pass.
                    } else {
                        cx.errors.push(
                            CompileError::new(
                                "bynk.resolve.unknown_function",
                                name.span,
                                format!("unknown function `{}`", name.name),
                            )
                            .with_note("only functions declared in this commons are callable"),
                        );
                    }
                }
            }
            for a in args {
                check_expr_references(a, cx);
            }
        }
        // #1654: the right operand of `&&`/`implies` is evaluated only when the
        // left holds, so the left's `is` bindings are in scope there.
        ExprKind::BinOp(BinOp::And | BinOp::Implies, lhs, rhs) => {
            check_expr_references(lhs, cx);
            let mut extra: HashMap<String, ()> = HashMap::new();
            collect_is_binding_names(lhs, true, &mut extra);
            cx.scopes.push(extra);
            check_expr_references(rhs, cx);
            cx.scopes.pop();
        }
        ExprKind::BinOp(_, lhs, rhs) => {
            check_expr_references(lhs, cx);
            check_expr_references(rhs, cx);
        }
        ExprKind::UnaryOp(_, e) => check_expr_references(e, cx),
        ExprKind::Paren(e) => check_expr_references(e, cx),
        ExprKind::Block(b) => check_block_references(b, cx),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            check_expr_references(cond, cx);
            // `is`-pattern bindings the condition proves flow into the
            // then-branch's scope (v0.2 §3.9), and those its *falsity* proves
            // into the else-branch's (#1654: `if !(o is Some(v)) { … } else
            // { v }`).
            let mut then_extra: HashMap<String, ()> = HashMap::new();
            collect_is_binding_names(cond, true, &mut then_extra);
            cx.scopes.push(then_extra);
            check_block_references(then_block, cx);
            cx.scopes.pop();
            let mut else_extra: HashMap<String, ()> = HashMap::new();
            collect_is_binding_names(cond, false, &mut else_extra);
            cx.scopes.push(else_extra);
            check_block_references(else_block, cx);
            cx.scopes.pop();
        }
        ExprKind::Ok(inner) | ExprKind::Err(inner) | ExprKind::Question(inner) => {
            check_expr_references(inner, cx);
        }
        ExprKind::Some(inner) => {
            check_expr_references(inner, cx);
        }
        ExprKind::RecordConstruction { type_name, fields } => {
            match cx.types.get(&type_name.name) {
                Some(decl) => {
                    cx.errors
                        .refs
                        .record(type_name.span, SymbolKind::Type, &type_name.name);
                    match &decl.body {
                        TypeBody::Record(r) => {
                            // Field-set validation (missing / unknown / duplicate
                            // / shorthand-in-scope) is shared with the checker's
                            // `check_record_construction` so the two cannot
                            // re-diverge (#711). The value recursion below stays
                            // here — it is the resolver's reference walk.
                            check_record_field_set(
                                type_name,
                                fields,
                                r,
                                expr.span,
                                |n| name_in_scope(n, cx.params, &cx.scopes),
                                cx.errors.errs,
                            );
                            for f in fields {
                                if let Some(v) = &f.value {
                                    check_expr_references(v, cx);
                                }
                            }
                        }
                        TypeBody::Opaque { .. } => {
                            cx.errors.push(
                            CompileError::new(
                                "bynk.resolve.opaque_record_construction",
                                type_name.span,
                                format!(
                                    "opaque type `{}` cannot be constructed with record-literal syntax",
                                    type_name.name
                                ),
                            )
                            // Finding #46: cross-file table lookup — see resolver.rs:1029.
                            .with_note("type declared here")
                            .with_note(
                                "construct opaque values via `T.of(value)` (validated) or `T.unsafe(value)` (inside the defining commons)",
                            ),
                        );
                        }
                        _ => {
                            cx.errors.push(
                            CompileError::new(
                                "bynk.resolve.not_a_record_type",
                                type_name.span,
                                format!(
                                    "`{}` is not a record type — only record types can be constructed with `{{ ... }}`",
                                    type_name.name
                                ),
                            )
                            // Finding #46: cross-file table lookup — see resolver.rs:1029.
                            .with_note("type declared here"),
                        );
                        }
                    }
                }
                None => cx.errors.push(unknown_type_error(type_name)),
            }
        }
        ExprKind::FieldAccess { receiver, field } => {
            // v0.9: `HttpResult.Variant` qualified nullary variant.
            if let ExprKind::Ident(id) = &receiver.kind
                && !name_in_scope(&id.name, cx.params, &cx.scopes)
                && id.name == "HttpResult"
            {
                if http_variant(&field.name).is_none() {
                    cx.errors.push(CompileError::new(
                        "bynk.resolve.unknown_static_member",
                        field.span,
                        format!("`HttpResult` has no variant named `{}`", field.name),
                    ));
                }
                return;
            }
            // #2341 (L4): `ErrorStatus.Forbidden` — a `RawError` status, qualified.
            // The checker admits it only as `RawError`'s first argument.
            if let ExprKind::Ident(id) = &receiver.kind
                && !name_in_scope(&id.name, cx.params, &cx.scopes)
                && id.name == "ErrorStatus"
                && !cx.types.contains_key(&id.name)
            {
                if bynk_syntax::ast::error_status(&field.name).is_none() {
                    cx.errors.push(CompileError::new(
                        "bynk.resolve.unknown_static_member",
                        field.span,
                        format!(
                            "`ErrorStatus` has no status named `{}` — the error statuses are the 4xx and 5xx `HttpResult` variants",
                            field.name
                        ),
                    ));
                }
                return;
            }
            // `TypeName.Variant` — qualified nullary variant reference.
            // #1927: not for a local asset that hides an imported type of its
            // name: `logo.url` reads the asset, as the emitted module does.
            if let ExprKind::Ident(id) = &receiver.kind
                && !name_in_scope(&id.name, cx.params, &cx.scopes)
                && !matches!(cx.shadowed.get(&id.name), Some(s) if s.imported == "type" && s.local == "asset")
                && let Some(decl) = cx.types.get(&id.name)
            {
                cx.errors.refs.record(id.span, SymbolKind::Type, &id.name);
                let known_variant = match &decl.body {
                    TypeBody::Sum(s) => s.variants.iter().any(|v| v.name.name == field.name),
                    _ => false,
                };
                if !known_variant {
                    cx.errors.push(
                        CompileError::new(
                            "bynk.resolve.unknown_static_member",
                            field.span,
                            format!(
                                "type `{}` has no static method or variant named `{}`",
                                id.name, field.name
                            ),
                        )
                        // Finding #46: cross-file table lookup — see resolver.rs:1029.
                        .with_note("type declared here"),
                    );
                }
            } else {
                check_expr_references(receiver, cx);
            }
        }
        ExprKind::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            // v0.9: `HttpResult.Variant(args)` — qualified HttpResult constructor.
            if let ExprKind::Ident(id) = &receiver.kind
                && !name_in_scope(&id.name, cx.params, &cx.scopes)
                && id.name == "HttpResult"
            {
                if http_variant(&method.name).is_none() {
                    cx.errors.push(CompileError::new(
                        "bynk.resolve.unknown_static_member",
                        method.span,
                        format!("`HttpResult` has no variant named `{}`", method.name),
                    ));
                }
                for a in args {
                    check_expr_references(a, cx);
                }
                return;
            }
            // v0.20b: `List.empty()` / `Map.empty()` — qualified statics on
            // the built-in collection types (no user declaration to resolve
            // against; the checker owns their typing). v0.22a adds the
            // numeric parse statics, `Int.parse(…)` / `Float.parse(…)`.
            if let ExprKind::Ident(id) = &receiver.kind
                && !name_in_scope(&id.name, cx.params, &cx.scopes)
                && matches!(
                    id.name.as_str(),
                    "List"
                        | "Map"
                        | "Set"
                        | "Int"
                        | "Float"
                        | "Json"
                        | "Duration"
                        | "Instant"
                        | "Stream"
                        | "Bytes"
                        | "RequestHeaders"
                        | "TokenClaims"
                )
                && !cx.types.contains_key(&id.name)
            {
                let allowed: &[&str] = match id.name.as_str() {
                    "List" | "Map" => &["empty"],
                    // K6a (#2596): `Set.empty()` and `Set.of(xs)`.
                    "Set" => &["empty", "of"],
                    "Json" => &["encode", "decode"],
                    // v0.86 (ADR 0112): `Duration.millis(n)`.
                    "Duration" => &["millis"],
                    // v0.90 (ADR 0114): `Instant.fromEpochMillis(n)`; K3
                    // (#2592): `Instant.fromIso(s)`.
                    "Instant" => &["fromEpochMillis", "fromIso"],
                    // v0.100: `Stream.of(xs)`.
                    "Stream" => &["of"],
                    // v0.110 (ADR 0142): `Bytes.fromUtf8(s)`/`fromBase64(s)`/`empty()`;
                    // #2016: `fromHex(s)`/`fromBase64Url(s)`.
                    "Bytes" => &[
                        "fromUtf8",
                        "fromBase64",
                        "fromHex",
                        "fromBase64Url",
                        "empty",
                    ],
                    // #2341 (L4): the test-side constructors.
                    "RequestHeaders" => &["of", "empty"],
                    // #2380 (L5): the test-side constructor.
                    "TokenClaims" => &["of"],
                    _ => &["parse"],
                };
                let only = allowed.join("`/`");
                if !allowed.contains(&method.name.as_str()) {
                    cx.errors.push(CompileError::new(
                        "bynk.resolve.unknown_static_member",
                        method.span,
                        format!(
                            "the built-in `{}` type has no static method named `{}` — the statics are `{only}`",
                            id.name, method.name
                        ),
                    ));
                }
                for a in args {
                    check_expr_references(a, cx);
                }
                return;
            }
            // If the receiver is a bare ident of a declared type (and not a
            // local binding), this is a static call: `T.method(args)`.
            // Validate the type/method/variant resolution here. Otherwise
            // recurse into the receiver as a value expression.
            if let ExprKind::Ident(id) = &receiver.kind
                && !name_in_scope(&id.name, cx.params, &cx.scopes)
                && let Some(decl) = cx.types.get(&id.name)
            {
                cx.errors.refs.record(id.span, SymbolKind::Type, &id.name);
                let table = cx.methods.get(&id.name).cloned().unwrap_or_default();
                let is_static_method = table.statics.contains_key(&method.name);
                let is_of_constructor = method.name == "of"
                    && matches!(
                        decl.body,
                        TypeBody::Refined { .. } | TypeBody::Opaque { .. }
                    );
                let is_unsafe_constructor =
                    method.name == "unsafe" && matches!(decl.body, TypeBody::Opaque { .. });
                let is_variant = match &decl.body {
                    TypeBody::Sum(s) => s.variants.iter().any(|v| v.name.name == method.name),
                    _ => false,
                };
                if !(is_static_method || is_of_constructor || is_unsafe_constructor || is_variant) {
                    cx.errors.push(
                        CompileError::new(
                            "bynk.resolve.unknown_static_member",
                            method.span,
                            format!(
                                "type `{}` has no static method or variant named `{}`",
                                id.name, method.name
                            ),
                        )
                        // Finding #46: cross-file table lookup — see resolver.rs:1029.
                        .with_note("type declared here"),
                    );
                }
            } else {
                check_expr_references(receiver, cx);
            }
            for a in args {
                check_expr_references(a, cx);
            }
        }
        ExprKind::Match { discriminant, arms } => {
            check_expr_references(discriminant, cx);
            for arm in arms {
                // Pattern bindings introduce names in the arm body. The
                // type checker validates the pattern against the discriminant
                // type. Resolver pushes a scope with those binding names so
                // body references resolve.
                let mut arm_scope = HashMap::new();
                collect_pattern_bindings(&arm.pattern, &mut arm_scope);
                cx.scopes.push(arm_scope);
                // #1848: the guard, in the arm's pattern scope and before the
                // body, as the checker types it (ADR 0169). Its `is` bindings
                // stay in the guard; they do not flow into the body.
                if let Some(guard) = &arm.guard {
                    check_expr_references(guard, cx);
                }
                match &arm.body {
                    MatchBody::Expr(e) => check_expr_references(e, cx),
                    MatchBody::Block(b) => check_block_references(b, cx),
                }
                cx.scopes.pop();
            }
        }
        ExprKind::Is { value, pattern } => {
            check_expr_references(value, cx);
            // `is` pattern bindings flow through to the truthy branch of
            // an enclosing context; binding scope is handled by the type
            // checker. Resolver doesn't introduce anything here.
            let _ = pattern;
        }
    }
}

/// The names introduced by the `is` tests `expr` proves matched when it
/// evaluates to `when_true` (v0.2 §3.9). Which tests those are is the shared
/// rule in [`crate::narrowing`] (#1654), the one the checker and emitter use.
fn collect_is_binding_names(expr: &Expr, when_true: bool, into: &mut HashMap<String, ()>) {
    for test in crate::narrowing::matched_is_tests(expr, when_true) {
        if let ExprKind::Is { pattern, .. } = &test.kind {
            collect_is_pattern_binding_names(pattern, into);
        }
    }
}

/// The depth-1 names an `is` pattern introduces — a `Variant`'s own flat
/// bindings (`is` supports only flat, depth-1 name bindings, ADR 0169 keeps
/// nesting/guards match-only, matching `gather_pattern_bindings`), or — #474
/// — for an or-pattern, the first alternative's (Rule 2 guarantees every
/// alternative gives a shared name the same type, so any one alternative's
/// names are representative of them all).
fn collect_is_pattern_binding_names(pattern: &Pattern, into: &mut HashMap<String, ()>) {
    match pattern {
        Pattern::Variant { bindings, .. } => {
            for b in bindings {
                if let Pattern::Binding(name) = b.pattern() {
                    into.insert(name.name.clone(), ());
                }
            }
        }
        Pattern::Or(alts, _) => {
            if let Some(first) = alts.first() {
                collect_is_pattern_binding_names(first, into);
            }
        }
        _ => {}
    }
}

/// Walk a pattern collecting the names it would bind, recursively through
/// nested payload patterns (ADR 0169) — `Some(Ok(x))` binds `x`.
fn collect_pattern_bindings(pattern: &Pattern, into: &mut HashMap<String, ()>) {
    for id in pattern.bound_names() {
        into.insert(id.name.clone(), ());
    }
}

/// Find the unique sum type that owns a given variant name. Returns None
/// if no type owns it; ignores cases of multiple owners (those are
/// reported via `find_ambiguous_variant_owners`).
fn find_unique_variant_owner<'a>(
    name: &str,
    types: &'a HashMap<String, Arc<TypeDecl>>,
) -> Option<&'a TypeDecl> {
    let owners = find_ambiguous_variant_owners(name, types);
    if owners.len() == 1 {
        Some(owners[0])
    } else {
        None
    }
}

fn find_ambiguous_variant_owners<'a>(
    name: &str,
    types: &'a HashMap<String, Arc<TypeDecl>>,
) -> Vec<&'a TypeDecl> {
    let mut out = Vec::new();
    for t in types.values() {
        if let TypeBody::Sum(s) = &t.body
            && s.variants.iter().any(|v| v.name.name == name)
        {
            out.push(t.as_ref());
        }
    }
    out
}

/// #1848: a match-arm guard is resolved like every other expression, in its
/// arm's pattern scope.
#[cfg(test)]
mod guard_resolution_tests {
    use crate::testkit::analyse;

    #[test]
    fn an_unknown_variant_in_a_guard_is_reported_by_the_resolver() {
        analyse(&[(
            "d.bynk",
            "commons d\n\ntype T = enum { A, B }\n\nfn g(n: Int, t: T) -> Int {\n  match n {\n    x if t == T.Zzz => 1\n    _ => 0\n  }\n}\n",
        )])
        .assert_reports("bynk.resolve.unknown_static_member");
    }

    /// True before #1848 too (the checker records it). Pinned so resolving
    /// guards cannot drop or double the reference.
    #[test]
    fn a_function_named_only_in_a_guard_is_a_recorded_reference() {
        let a = analyse(&[(
            "d.bynk",
            "commons d\n\nfn big(n: Int) -> Bool { n > 9 }\n\nfn g(n: Int) -> Int {\n  match n {\n    x if big(x) => 1\n    _ => 0\n  }\n}\n",
        )]);
        a.assert_clean();
        let key = a.resolves_to("d.bynk", "big(x)");
        assert_eq!((key.unit.as_str(), key.name.as_str()), ("d", "big"));
    }

    #[test]
    fn a_guard_reads_its_arm_s_pattern_bindings() {
        analyse(&[(
            "d.bynk",
            "commons d\n\nfn g(o: Option[Int]) -> Int {\n  match o {\n    Some(v) if v > 0 => v\n    _ => 0\n  }\n}\n",
        )])
        .assert_clean();
    }
}
