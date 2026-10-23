use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use crate::checker::CapabilityInfo;
use crate::index::{IndexBuilder, ProjectIndex, RefSink, SiteRef, SymbolKind};
use crate::project_model::UnitInfo;
use crate::resolver::{self, MethodTable as ResolverMethodTable};
use bynk_project::{ParsedFile, UnitKind};
use bynk_syntax::ast::{
    ActorDecl, AgentDecl, AssetDecl, BaseType, Block, CapRef, CapabilityDecl, CommonsItem,
    EventDecl, ExportKind, Expr, ExprId, ExprKind, FnDecl, FnName, HandlerKind, Ident, Param,
    ProviderDecl, ServiceDecl, ServiceProtocol, Trivia, TypeBody, TypeDecl, TypeRef, Visibility,
};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

/// v0.25 (ADR 0053): walk every parsed file's top-level declarations into
/// the def table (synthetic first-party units and test files excluded —
/// neither declares user-editable symbols), then qualify and attach the
/// recorded edges. Methods register as owners only (attribution), not as
/// symbols — they are deferred along with fields and op names.
pub fn assemble_index(
    parsed: &[ParsedFile],
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    refs: RefSink,
) -> ProjectIndex {
    let mut builder = IndexBuilder::default();
    let mut uses = unit_uses.clone();
    uses.extend(refs.extra_uses);
    builder.set_uses(uses);
    builder.set_consumes(unit_consumes.clone());
    for pf in parsed {
        if matches!(pf.kind(), UnitKind::Test | UnitKind::Integration) {
            continue;
        }
        let unit = pf.unit().name().joined();
        // v0.28 (ADR 0057): synthetic first-party units stay out of
        // `symbols` (their defs point at files not on disk — the v0.25
        // rule), but their declarations register for the second
        // qualification pass so references to them colour as tokens.
        if pf.is_synthetic() {
            for item in pf.items() {
                let (kind, name, modifiers) = match item {
                    CommonsItem::Type(t) => (
                        SymbolKind::Type,
                        &t.name.name,
                        symbol_modifiers(&unit, Some(t)),
                    ),
                    // Events track, slice 0 (spine #936): an `event` indexes
                    // as an ordinary `Type` symbol — it *is* one, a record,
                    // registered via `EventDecl::as_type_decl`. A dedicated
                    // `SymbolKind::Event` (its own hover/completion icon) is
                    // a follow-on, not a slice-0 blocker.
                    CommonsItem::Event(e) => (
                        SymbolKind::Type,
                        &e.name.name,
                        symbol_modifiers(&unit, None),
                    ),
                    CommonsItem::Fn(f) => match &f.name {
                        FnName::Free(id) => {
                            (SymbolKind::Fn, &id.name, symbol_modifiers(&unit, None))
                        }
                        FnName::Method { .. } => continue,
                    },
                    CommonsItem::Capability(c) => (
                        SymbolKind::Capability,
                        &c.name.name,
                        symbol_modifiers(&unit, None),
                    ),
                    CommonsItem::Service(s) => (
                        SymbolKind::Service,
                        &s.name.name,
                        symbol_modifiers(&unit, None),
                    ),
                    CommonsItem::Agent(a) => (
                        SymbolKind::Agent,
                        &a.name.name,
                        symbol_modifiers(&unit, None),
                    ),
                    CommonsItem::Provider(p) => (
                        SymbolKind::Provider,
                        &p.provider_name.name,
                        symbol_modifiers(&unit, None),
                    ),
                    CommonsItem::Actor(a) => (
                        SymbolKind::Actor,
                        &a.name.name,
                        symbol_modifiers(&unit, None),
                    ),
                    CommonsItem::Messages(m) => {
                        (SymbolKind::Messages, &m.tag, symbol_modifiers(&unit, None))
                    }
                    // #1883: an asset indexes under its own kind, keyed by
                    // name in the declaring unit.
                    CommonsItem::Asset(a) => (
                        SymbolKind::Asset,
                        &a.name.name,
                        symbol_modifiers(&unit, None),
                    ),
                };
                builder.add_first_party_def(&unit, kind, name, modifiers);
            }
            continue;
        }
        let site = |id: &Ident| SiteRef {
            path: pf.identity_path(),
            span: id.span,
        };
        for item in pf.items() {
            match item {
                CommonsItem::Type(t) => {
                    builder.add_def(
                        &unit,
                        SymbolKind::Type,
                        &t.name.name,
                        site(&t.name),
                        symbol_modifiers(&unit, Some(t)),
                    );
                    // v0.129 (#259): record a refined/opaque type's builtin base
                    // for the refinement-family codelens. A plain alias
                    // (`type Age = Int`) counts — it parses as `Refined { …, base }`
                    // with no `where`, still declared over the base.
                    if let TypeBody::Refined { base, .. } | TypeBody::Opaque { base, .. } = &t.body
                    {
                        builder.add_refinement(&unit, &t.name.name, *base);
                    }
                    // v0.36 (ADR 0069, slice 2): record fields are first-class
                    // symbols keyed by the compound `"Type.field"` name.
                    if let TypeBody::Record(r) = &t.body {
                        for field in &r.fields {
                            builder.add_def(
                                &unit,
                                SymbolKind::Field,
                                &format!("{}.{}", t.name.name, field.name.name),
                                site(&field.name),
                                symbol_modifiers(&unit, None),
                            );
                        }
                    }
                }
                // Events track, slice 0 (spine #936): an `event` indexes
                // exactly like a `Type` whose body is a record — same
                // `SymbolKind::Type`/`SymbolKind::Field` reuse as the
                // synthetic-unit arm above, so hover/go-to-def/rename work
                // on an event and its fields without a new symbol kind.
                CommonsItem::Event(e) => {
                    builder.add_def(
                        &unit,
                        SymbolKind::Type,
                        &e.name.name,
                        site(&e.name),
                        symbol_modifiers(&unit, None),
                    );
                    for field in &e.body.fields {
                        builder.add_def(
                            &unit,
                            SymbolKind::Field,
                            &format!("{}.{}", e.name.name, field.name.name),
                            site(&field.name),
                            symbol_modifiers(&unit, None),
                        );
                    }
                }
                CommonsItem::Fn(f) => match &f.name {
                    FnName::Free(id) => {
                        builder.add_def(
                            &unit,
                            SymbolKind::Fn,
                            &id.name,
                            site(id),
                            symbol_modifiers(&unit, None),
                        );
                    }
                    FnName::Method { .. } => {
                        // v0.36 (ADR 0069): a method is a first-class symbol
                        // keyed by the compound `"Type.method"` name, and (as
                        // before) an attribution owner for call-hierarchy.
                        builder.add_owner(&unit, &f.name.display(), &pf.identity_path());
                        builder.add_def(
                            &unit,
                            SymbolKind::Method,
                            &f.name.display(),
                            site(f.name.ident()),
                            symbol_modifiers(&unit, None),
                        );
                    }
                },
                CommonsItem::Capability(c) => {
                    builder.add_def(
                        &unit,
                        SymbolKind::Capability,
                        &c.name.name,
                        site(&c.name),
                        symbol_modifiers(&unit, None),
                    );
                    // v0.36 (ADR 0069, slice 2): capability operations are
                    // first-class symbols keyed by the compound `"Cap.op"` name.
                    for op in &c.ops {
                        builder.add_def(
                            &unit,
                            SymbolKind::CapabilityOp,
                            &format!("{}.{}", c.name.name, op.name.name),
                            site(&op.name),
                            symbol_modifiers(&unit, None),
                        );
                    }
                }
                CommonsItem::Service(s) => {
                    builder.add_def(
                        &unit,
                        SymbolKind::Service,
                        &s.name.name,
                        site(&s.name),
                        symbol_modifiers(&unit, None),
                    );
                }
                CommonsItem::Agent(a) => {
                    builder.add_def(
                        &unit,
                        SymbolKind::Agent,
                        &a.name.name,
                        site(&a.name),
                        symbol_modifiers(&unit, None),
                    );
                    // #304: an agent handler is a first-class symbol keyed by
                    // the compound `"Agent.handler"` name, mirroring the
                    // v0.36 (ADR 0069) method/field/op convention. Service
                    // handlers have no per-handler name (`method_name` is
                    // always `None`), so this is naturally agent-only.
                    for h in &a.handlers {
                        if let Some(name) = &h.method_name {
                            builder.add_def(
                                &unit,
                                SymbolKind::Handler,
                                &format!("{}.{}", a.name.name, name.name),
                                site(name),
                                symbol_modifiers(&unit, None),
                            );
                        }
                    }
                }
                CommonsItem::Provider(p) => {
                    builder.add_def(
                        &unit,
                        SymbolKind::Provider,
                        &p.provider_name.name,
                        site(&p.provider_name),
                        symbol_modifiers(&unit, None),
                    );
                }
                CommonsItem::Actor(a) => {
                    builder.add_def(
                        &unit,
                        SymbolKind::Actor,
                        &a.name.name,
                        site(&a.name),
                        symbol_modifiers(&unit, None),
                    );
                }
                CommonsItem::Messages(m) => {
                    // The tag is a string literal, not an `Ident`, so build the
                    // `SiteRef` from its span directly rather than via `site`.
                    builder.add_def(
                        &unit,
                        SymbolKind::Messages,
                        &m.tag,
                        SiteRef {
                            path: pf.identity_path(),
                            span: m.tag_span,
                        },
                        symbol_modifiers(&unit, None),
                    );
                }
                // #1883: an asset's declaration site, for hover, go-to-
                // definition, references, rename and semantic tokens. A bare
                // use in a unit that `uses` this one resolves here through the
                // qualification pass, as a function's does. A leaf def: an
                // asset encloses no call site, so it registers no owner (one
                // would clash with a same-named service's — review of #1924).
                CommonsItem::Asset(a) => {
                    builder.add_leaf_def(
                        &unit,
                        SymbolKind::Asset,
                        &a.name.name,
                        site(&a.name),
                        symbol_modifiers(&unit, None),
                    );
                }
            }
        }
    }
    builder.build(refs.edges)
}

/// v0.28 (ADR 0057): a symbol's semantic-token modifiers from its
/// declaration. `refined` only when a refinement is present — `type X = Int`
/// is `Refined { refinement: None }`, a plain alias, and carries neither;
/// `opaque` is orthogonal (an `opaque … where` type carries both).
/// `platform_native` when the declaring unit is a platform adapter.
fn symbol_modifiers(unit: &str, type_decl: Option<&TypeDecl>) -> crate::index::SymbolModifiers {
    let (refined, opaque) = match type_decl.map(|t| &t.body) {
        Some(TypeBody::Refined { refinement, .. }) => (refinement.is_some(), false),
        Some(TypeBody::Opaque { refinement, .. }) => (refinement.is_some(), true),
        _ => (false, false),
    };
    crate::index::SymbolModifiers {
        refined,
        opaque,
        platform_native: crate::firstparty::platform_of(unit).is_some(),
    }
}

/// Combined symbol tables for a single logical commons or context.
#[derive(Clone, Default)]
pub struct UnitTable {
    pub kind: Option<UnitKind>,
    pub types: HashMap<String, Arc<TypeDecl>>,
    pub fns: HashMap<String, Arc<FnDecl>>,
    pub methods: HashMap<String, ResolverMethodTable>,
    /// Per-context capabilities (v0.5). Empty for commons.
    pub capabilities: HashMap<String, CapabilityDecl>,
    /// Per-context providers (v0.5). One provider per capability in v0.5.
    /// Key: capability name. Value: provider declaration.
    pub providers: HashMap<String, ProviderDecl>,
    /// Per-context services (v0.5). Empty for commons.
    pub services: HashMap<String, ServiceDecl>,
    /// Per-context agents (v0.5). Empty for commons.
    pub agents: HashMap<String, AgentDecl>,
    /// State-migrations S3 (#2248): the context's `removed agent`
    /// declarations, across its files.
    pub removed_agents: Vec<bynk_syntax::ast::RemovedAgentDecl>,
    /// v0.45: actors — boundary contracts consumed by handler `by` clauses.
    pub actors: HashMap<String, ActorDecl>,
    /// v0.15: capability names this context offers to consumers via
    /// `exports capability { … }`. Empty for commons.
    pub exported_capabilities: std::collections::HashSet<String>,
    /// Events track, slice 0 (spine #936): `event` declarations. Each also
    /// registers into `types` (via `EventDecl::as_type_decl`) so ordinary
    /// type-reference/exports/consumes/construction machinery treats it like
    /// any other record type; this table is the separate "is `name`
    /// specifically an event" answer — owner-only emission and the
    /// `from Events(E)`/`Events.emit[E]` "must name a declared event, not
    /// just any type" checks key off it. Empty for commons/adapters
    /// (`bynk.event.outside_context` rejects it there).
    pub events: HashMap<String, EventDecl>,
    /// #1883, slice 1a: `asset` declarations, by name — values of the
    /// built-in type `Asset`, local to this unit. A context or commons only
    /// (`bynk.asset.outside_context_or_commons`); a unit's visible assets
    /// add those of the commons it `uses` (`assets::compose_unit_assets`).
    pub assets: HashMap<String, AssetDecl>,
    /// #291: the capabilities this unit flattens in with `consumes U { Cap }`,
    /// each mapped to the unit providing it — `phase_resolve_consumes`'s
    /// per-unit answer, recorded by [`record_flattened_caps`] so a test body's
    /// privileged view sees the target's platform seams.
    pub flattened_caps: HashMap<String, String>,
    /// S4a (#1966): the derived capabilities (`capability Sessions = Kv`)
    /// this context declares, by name, once resolved to their base by
    /// [`crate::project_model::phase_resolve_derived`]. Each is also in
    /// [`UnitTable::capabilities`], with its base's operations, so `given`,
    /// calls and test views treat it as an ordinary capability.
    pub derived: HashMap<String, DerivedInstance>,
}

/// S4a (#1966): a resolved derived capability: a distinct instance of a
/// platform capability, provided by that platform's own provider bound to this
/// instance's Worker binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedInstance {
    /// The unit providing the base capability (`bynk.cloudflare`).
    pub base_unit: String,
    /// The base capability (`Kv`).
    pub base_cap: String,
    /// The instance's Worker binding name (`KV_SESSIONS`).
    pub binding: String,
    /// Its creation options, each with its literal value, as written
    /// (`[("worker", Str("legacy-billing"))]`, interop I5 #1972; `[("limit",
    /// Int(100)), ("period", Seconds(60))]`, P3 #2342).
    pub options: Vec<(String, OptionValue)>,
    /// The `capability … = …` declaration.
    pub span: bynk_syntax::span::Span,
}

/// The value of one instance option (standard-library P3, #2342).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionValue {
    /// A string literal (`@worker("legacy-billing")`).
    Str(String),
    /// A positive `Int` literal (`@limit(100)`).
    Int(i64),
    /// A `Duration` literal, in whole seconds (`@period(60.seconds)`).
    Seconds(i64),
}

impl DerivedInstance {
    /// The value of string option `name`, if the instance carries it.
    pub fn option(&self, name: &str) -> Option<&str> {
        self.options
            .iter()
            .find(|(o, _)| o == name)
            .and_then(|(_, v)| match v {
                OptionValue::Str(s) => Some(s.as_str()),
                _ => None,
            })
    }

    /// The value of numeric option `name` (an `Int`, or a `Duration` in
    /// seconds), if the instance carries it.
    pub fn number(&self, name: &str) -> Option<i64> {
        self.options
            .iter()
            .find(|(o, _)| o == name)
            .and_then(|(_, v)| match v {
                OptionValue::Int(n) | OptionValue::Seconds(n) => Some(*n),
                OptionValue::Str(_) => None,
            })
    }

    /// Whether this is a `RateLimit` instance (P3, #2342).
    pub fn is_ratelimit(&self) -> bool {
        self.base_unit == crate::firstparty::CLOUDFLARE_UNIT && self.base_cap == "RateLimit"
    }

    /// A `RateLimit` instance's `(limit, period in seconds)`, or `None` for
    /// any other kind.
    pub fn rate_limit(&self) -> Option<(i64, i64)> {
        if !self.is_ratelimit() {
            return None;
        }
        Some((
            self.number(crate::firstparty::RATELIMIT_LIMIT_OPTION)?,
            self.number(crate::firstparty::RATELIMIT_PERIOD_OPTION)?,
        ))
    }

    /// The jurisdiction an `R2` instance's bucket is pinned to (P5, #2323), or
    /// `None` for an unpinned one or any other kind.
    pub fn jurisdiction(&self) -> Option<&str> {
        self.is_r2()
            .then(|| self.option(crate::firstparty::R2_JURISDICTION_OPTION))
            .flatten()
    }

    /// Whether this is an `R2` instance.
    pub fn is_r2(&self) -> bool {
        self.base_unit == crate::firstparty::CLOUDFLARE_UNIT && self.base_cap == "R2"
    }

    /// The Worker a `Service` instance reaches (interop I5, #1972), or `None`
    /// for any other kind.
    pub fn worker(&self) -> Option<&str> {
        (self.base_unit == crate::firstparty::CLOUDFLARE_UNIT && self.base_cap == "Service")
            .then(|| self.option(crate::firstparty::SERVICE_WORKER_OPTION))
            .flatten()
    }
}

/// #291: record each unit's flattened capabilities (`consumes U { Cap }`, from
/// `phase_resolve_consumes`) on its table.
pub fn record_flattened_caps(
    unit_tables: &mut HashMap<String, UnitTable>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
) {
    for (name, flattened) in unit_flattened {
        if let Some(t) = unit_tables.get_mut(name) {
            t.flattened_caps = flattened.clone();
        }
    }
}

/// #696: each table-construction diagnostic is attributed to the project-relative
/// `identity_path` of the file whose item produced it. Every error-producing loop
/// below iterates `for &i in indices`, so it shadows a local `errors` vec and
/// drains it into `out`, tagged with `parsed[i].identity_path()`, at the end of each
/// file's pass — leaving the many inner `errors.push(…)` sites untouched.
pub fn build_unit_table(
    _name: &str,
    kind: UnitKind,
    indices: &[usize],
    parsed: &[ParsedFile],
    out: &mut Vec<(PathBuf, CompileError)>,
) -> UnitTable {
    let mut table = UnitTable {
        kind: Some(kind),
        ..UnitTable::default()
    };
    for &i in indices {
        let mut errors: Vec<CompileError> = Vec::new();
        table
            .removed_agents
            .extend(parsed[i].removed_agents().iter().cloned());
        for item in parsed[i].items() {
            // Events track, slice 0 (spine #936): an `event` registers into
            // `types` exactly like a `type` (via `EventDecl::as_type_decl`,
            // so name-conflict detection against ordinary types is a single
            // check regardless of declaration order within the file) and
            // additionally into `events`, the separate "is this specifically
            // an event" table.
            if let CommonsItem::Event(e) = item
                && kind != UnitKind::Context
            {
                errors.push(CompileError::new(
                    "bynk.event.outside_context",
                    e.span,
                    "`event` declarations are only allowed inside a context",
                ));
                continue;
            }
            let as_type: Option<(&Ident, TypeDecl, bool)> = match item {
                CommonsItem::Type(t) => Some((&t.name, t.clone(), false)),
                CommonsItem::Event(e) => Some((&e.name, e.as_type_decl(), true)),
                _ => None,
            };
            if let Some((name, decl, is_event)) = as_type {
                if let Some(prev) = table.types.get(&name.name) {
                    errors.push(
                        CompileError::new(
                            "bynk.resolve.duplicate_type",
                            name.span,
                            format!("type `{}` is already declared", name.name),
                        )
                        .with_label(prev.name.span, "previously declared here"),
                    );
                } else {
                    table.methods.entry(name.name.clone()).or_default();
                    if is_event {
                        let CommonsItem::Event(e) = item else {
                            unreachable!("is_event only set for CommonsItem::Event")
                        };
                        table.events.insert(name.name.clone(), e.clone());
                    }
                    table.types.insert(name.name.clone(), Arc::new(decl));
                }
            }
        }
        out.extend(errors.into_iter().map(|e| (parsed[i].identity_path(), e)));
    }
    // v0.15: collect the names a context exports as capabilities.
    // v0.17: adapters export capabilities too.
    for &i in indices {
        {
            for clause in parsed[i].exports() {
                if matches!(clause.kind, ExportKind::Capability) {
                    for n in clause.names.iter().map(|e| &e.name) {
                        table.exported_capabilities.insert(n.name.clone());
                    }
                }
            }
        }
    }
    // v0.5: collect capabilities, providers, services, agents.
    for &i in indices {
        let mut errors: Vec<CompileError> = Vec::new();
        for item in parsed[i].items() {
            match item {
                CommonsItem::Capability(c) => {
                    if kind != UnitKind::Context && kind != UnitKind::Adapter {
                        errors.push(CompileError::new(
                            "bynk.capability.outside_context",
                            c.span,
                            "`capability` declarations are only allowed inside a context or adapter",
                        ));
                        continue;
                    }
                    if let Some(prev) = table.capabilities.get(&c.name.name) {
                        errors.push(
                            CompileError::new(
                                "bynk.resolve.duplicate_capability",
                                c.name.span,
                                format!("capability `{}` is already declared", c.name.name),
                            )
                            .with_label(prev.name.span, "previously declared here"),
                        );
                    } else {
                        table.capabilities.insert(c.name.name.clone(), c.clone());
                    }
                }
                CommonsItem::Provider(p) => {
                    match kind {
                        UnitKind::Context => {
                            // v0.17: a bodiless (external) provider is only legal
                            // inside an adapter.
                            if p.external {
                                errors.push(CompileError::new(
                                    "bynk.context.external_provider",
                                    p.span,
                                    "an external (bodiless) provider is only allowed inside an `adapter` — a context provider must have a Bynk body",
                                ));
                                continue;
                            }
                        }
                        UnitKind::Adapter => {
                            // v0.17: an adapter provider must be external — its
                            // implementation comes from the binding.
                            if !p.external {
                                errors.push(CompileError::new(
                                    "bynk.adapter.provider_has_body",
                                    p.span,
                                    "a provider inside an `adapter` must be external (no body) — its implementation is supplied by the binding",
                                ));
                                continue;
                            }
                        }
                        // The parser rejects `provides` anywhere else
                        // (`bynk.provider.outside_context`) before a symbol
                        // table is built (#1662).
                        _ => continue,
                    }
                    if let Some(prev) = table.providers.get(&p.capability.name) {
                        errors.push(
                            CompileError::new(
                                "bynk.resolve.duplicate_provider",
                                p.span,
                                format!(
                                    "capability `{}` already has a provider in this context",
                                    p.capability.name
                                ),
                            )
                            .with_label(prev.span, "previously provided here"),
                        );
                    } else {
                        table.providers.insert(p.capability.name.clone(), p.clone());
                    }
                }
                CommonsItem::Service(s) => {
                    if kind == UnitKind::Adapter {
                        errors.push(CompileError::new(
                            "bynk.adapter.disallowed_item",
                            s.span,
                            "an `adapter` may not declare a `service` — adapters contain only capabilities, boundary types, external providers, and helpers",
                        ));
                        continue;
                    }
                    // An adapter's is rejected just above
                    // (`bynk.adapter.disallowed_item`), and the parser rejects
                    // a `service` in a commons (`bynk.service.outside_context`)
                    // before a symbol table is built, so only a context reaches
                    // here (#1662).
                    if kind != UnitKind::Context {
                        continue;
                    }
                    if let Some(prev) = table.services.get(&s.name.name) {
                        errors.push(
                            CompileError::new(
                                "bynk.resolve.duplicate_service",
                                s.name.span,
                                format!("service `{}` is already declared", s.name.name),
                            )
                            .with_label(prev.name.span, "previously declared here"),
                        );
                    } else {
                        table.services.insert(s.name.name.clone(), s.clone());
                    }
                }
                CommonsItem::Agent(a) => {
                    if kind == UnitKind::Adapter {
                        errors.push(CompileError::new(
                            "bynk.adapter.disallowed_item",
                            a.span,
                            "an `adapter` may not declare an `agent` — adapters contain only capabilities, boundary types, external providers, and helpers",
                        ));
                        continue;
                    }
                    // An adapter's is rejected just above
                    // (`bynk.adapter.disallowed_item`), and the parser rejects
                    // an `agent` in a commons (`bynk.agent.outside_context`)
                    // before a symbol table is built, so only a context reaches
                    // here (#1662).
                    if kind != UnitKind::Context {
                        continue;
                    }
                    if let Some(prev) = table.agents.get(&a.name.name) {
                        errors.push(
                            CompileError::new(
                                "bynk.resolve.duplicate_agent",
                                a.name.span,
                                format!("agent `{}` is already declared", a.name.name),
                            )
                            .with_label(prev.name.span, "previously declared here"),
                        );
                    } else {
                        table.agents.insert(a.name.name.clone(), a.clone());
                    }
                }
                CommonsItem::Actor(a) => {
                    if kind == UnitKind::Adapter {
                        errors.push(CompileError::new(
                            "bynk.adapter.disallowed_item",
                            a.span,
                            "an `adapter` may not declare an `actor` — adapters contain only capabilities, boundary types, external providers, and helpers",
                        ));
                        continue;
                    }
                    if let Some(prev) = table.actors.get(&a.name.name) {
                        errors.push(
                            CompileError::new(
                                "bynk.resolve.duplicate_actor",
                                a.name.span,
                                format!("actor `{}` is already declared", a.name.name),
                            )
                            .with_label(prev.name.span, "previously declared here"),
                        );
                    } else {
                        table.actors.insert(a.name.name.clone(), a.clone());
                    }
                }
                _ => {}
            }
        }
        out.extend(errors.into_iter().map(|e| (parsed[i].identity_path(), e)));
    }
    for &i in indices {
        let mut errors: Vec<CompileError> = Vec::new();
        for item in parsed[i].items() {
            let CommonsItem::Fn(f) = item else { continue };
            match &f.name {
                FnName::Free(id) => {
                    if let Some(prev) = table.fns.get(&id.name) {
                        errors.push(
                            CompileError::new(
                                "bynk.resolve.duplicate_fn",
                                id.span,
                                format!("function `{}` is already declared", id.name),
                            )
                            .with_label(prev.name.ident().span, "previously declared here"),
                        );
                    } else if let Some(prev) = table.types.get(&id.name) {
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
                        table.fns.insert(id.name.clone(), Arc::new(f.clone()));
                    }
                }
                FnName::Method {
                    type_name,
                    method_name,
                } => {
                    if !table.types.contains_key(&type_name.name) {
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
                                "methods can only be declared on types defined in the same commons or context (across all of its files)",
                            ),
                        );
                        continue;
                    }
                    let mt = table.methods.entry(type_name.name.clone()).or_default();
                    let bucket = if f.has_self {
                        &mut mt.instance
                    } else {
                        &mut mt.statics
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
            }
        }
        out.extend(errors.into_iter().map(|e| (parsed[i].identity_path(), e)));
    }
    // #1883, slice 1a: `asset` declarations. After the function pass, so an
    // asset that shares a function's or type's name is reported whichever
    // was declared first: an asset is a value, in the same namespace as a
    // free function, and a type name in expression position is an error.
    for &i in indices {
        let mut errors: Vec<CompileError> = Vec::new();
        for item in parsed[i].items() {
            let CommonsItem::Asset(a) = item else {
                continue;
            };
            if !matches!(kind, UnitKind::Context | UnitKind::Commons) {
                errors.push(
                    CompileError::new(
                        "bynk.asset.outside_context_or_commons",
                        a.span,
                        "`asset` declarations are only allowed inside a context or a commons",
                    )
                    .with_note(
                        "an asset ships with the Worker of each context that declares it or `uses` a commons that does",
                    ),
                );
                continue;
            }
            errors.extend(crate::assets::at_path_error(a));
            if let Some(err) = crate::assets::path_errors(a) {
                errors.push(err);
                // Still registered below: the declaration is otherwise
                // well-formed, so its uses should not also report
                // `unknown_name`.
            }
            let key = &a.name.name;
            match crate::assets::name_error(
                a,
                table.assets.get(key).map(|p| &p.name),
                table.fns.get(key).map(|f| f.name.ident()),
                table.types.get(key).map(|t| &t.name),
            ) {
                Some(err) => errors.push(err),
                None => {
                    table.assets.insert(key.clone(), a.clone());
                }
            }
        }
        out.extend(errors.into_iter().map(|e| (parsed[i].identity_path(), e)));
    }
    // #1925: a service, agent or provider shares one namespace with the
    // unit's functions, assets, types and events (and with each other) — they
    // are all one scope of the emitted TypeScript module, so a shared name
    // would be declared twice. After every other pass, so each table is
    // complete whatever the file and declaration order; reported at the
    // service, agent or provider, the later kinds in this order. An external
    // (bodiless) provider emits no declaration, so it takes no name.
    let mut providers_seen: HashMap<&str, Span> = HashMap::new();
    for &i in indices {
        let mut errors: Vec<CompileError> = Vec::new();
        for item in parsed[i].items() {
            // Only the declaration registered: a duplicate has already
            // reported `duplicate_service` / `duplicate_agent` /
            // `duplicate_provider`.
            let (what, name) = match item {
                CommonsItem::Service(s)
                    if table.services.get(&s.name.name).map(|p| p.name.span)
                        == Some(s.name.span) =>
                {
                    (ValueKind::Service, &s.name)
                }
                CommonsItem::Agent(a)
                    if table.agents.get(&a.name.name).map(|p| p.name.span) == Some(a.name.span) =>
                {
                    (ValueKind::Agent, &a.name)
                }
                CommonsItem::Provider(p)
                    if !p.external
                        && table.providers.get(&p.capability.name).map(|q| q.span)
                            == Some(p.span) =>
                {
                    (ValueKind::Provider, &p.provider_name)
                }
                _ => continue,
            };
            // Provider against provider only. A provider meets a service or
            // agent through their tables whichever comes first, so the pair
            // is reported once, at the provider.
            let earlier_provider = (what == ValueKind::Provider)
                .then(|| providers_seen.get(name.name.as_str()).copied())
                .flatten();
            if let Some(err) = value_namespace_error(&table, what, name, earlier_provider) {
                errors.push(err);
            }
            if what == ValueKind::Provider {
                providers_seen.entry(&name.name).or_insert(name.span);
            }
        }
        out.extend(errors.into_iter().map(|e| (parsed[i].identity_path(), e)));
    }
    // message-bundles slice 1 (#859): a commons declaring at least one
    // `messages` block also gets a synthetic `render(tag: LocaleTag, msg:
    // Message) -> String` in its own local function table — not just emitted
    // TS. Without this, a Bynk-source `render(...)` call has no local
    // declaration to resolve to and silently falls through to `bynk.locale`'s
    // *imported* `render` (same signature, wrong — bundle-free — behaviour):
    // resolution would "type-check" while quietly calling the wrong function.
    // Registering it here, in the same local `table.fns` a real `CommonsItem::Fn`
    // would populate, makes ordinary lexical precedence (local beats
    // `uses`-imported, `compose_unit_symbols`) and call-site type-checking
    // (`fns.get(name)`, never touching `.body`) work with no changes anywhere
    // else. The body is a placeholder — nothing ever type-checks it, since
    // body-checking walks `commons.items` (real AST items) directly, and this
    // entry is never added there.
    if kind == UnitKind::Commons
        && let Some(m) = indices.iter().find_map(|&i| {
            parsed[i].items().iter().find_map(|item| match item {
                CommonsItem::Messages(m) => Some(m),
                _ => None,
            })
        })
    {
        if let Some(prev) = table.fns.get("render") {
            out.push((
                parsed[indices[0]].identity_path(),
                CompileError::new(
                    "bynk.resolve.duplicate_fn",
                    m.span,
                    "function `render` is already declared",
                )
                .with_label(prev.name.ident().span, "previously declared here")
                .with_note(
                    "a `messages` block in this commons implicitly declares its own \
                     `render(tag, msg) -> String` — name it something else",
                ),
            ));
        } else {
            table
                .fns
                .insert("render".to_string(), Arc::new(synthetic_render_fn()));
        }
    }
    table
}

/// #1925: what `name` is in `table`'s own value namespace — its kind, as a
/// diagnostic names it, and its declaration's name span — or `None` when the
/// unit declares no value of that name. The predicate both compositions use
/// to let a local declaration of any kind shadow a `uses`-imported function or
/// asset (`project_model::compose_unit_symbols`,
/// `assets::compose_unit_assets`), as the emitted module does.
pub fn local_value_kind(table: &UnitTable, name: &str) -> Option<(&'static str, Span)> {
    if let Some(f) = table.fns.get(name) {
        Some(("function", f.name.ident().span))
    } else if let Some(a) = table.assets.get(name) {
        Some(("asset", a.name.span))
    } else if let Some(e) = table.events.get(name) {
        Some(("event", e.name.span))
    } else if let Some(t) = table.types.get(name) {
        Some(("type", t.name.span))
    } else if let Some(s) = table.services.get(name) {
        Some(("service", s.name.span))
    } else if let Some(a) = table.agents.get(name) {
        Some(("agent", a.name.span))
    } else {
        table
            .providers
            .values()
            .find(|p| !p.external && p.provider_name.name == name)
            .map(|p| ("provider", p.provider_name.span))
    }
}

/// #1925: the kinds [`value_namespace_error`] checks, in the order the
/// conflict pass meets them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueKind {
    Service,
    Agent,
    Provider,
}

/// #1925: `bynk.resolve.name_conflict` for a service, agent or provider
/// (`what`) whose `name` the unit also gives a function, an asset, a type or
/// event, or a kind `what` follows in the pass: an agent conflicts with a
/// service, a provider with a service, an agent or `earlier_provider`. `None`
/// when the name is free.
fn value_namespace_error(
    table: &UnitTable,
    what: ValueKind,
    name: &Ident,
    earlier_provider: Option<Span>,
) -> Option<CompileError> {
    let key = &name.name;
    let services = || table.services.get(key).map(|s| s.name.span);
    let agents = || table.agents.get(key).map(|a| a.name.span);
    let (other, prev) = if let Some(f) = table.fns.get(key) {
        ("function", f.name.ident().span)
    } else if let Some(a) = table.assets.get(key) {
        ("asset", a.name.span)
    } else if let Some(e) = table.events.get(key) {
        ("event", e.name.span)
    } else if let Some(t) = table.types.get(key) {
        ("type", t.name.span)
    } else if let Some(span) = (what != ValueKind::Service).then(services).flatten() {
        ("service", span)
    } else if let Some(span) = (what == ValueKind::Provider).then(agents).flatten() {
        ("agent", span)
    } else if let Some(span) = earlier_provider {
        ("provider", span)
    } else {
        return None;
    };
    let what = match what {
        ValueKind::Service => "service",
        ValueKind::Agent => "agent",
        ValueKind::Provider => "provider",
    };
    let other_article = if matches!(other, "asset" | "event" | "agent") {
        "an"
    } else {
        "a"
    };
    Some(
        CompileError::new(
            "bynk.resolve.name_conflict",
            name.span,
            format!("{what} `{key}` conflicts with {other_article} {other} of the same name"),
        )
        .with_label(prev, format!("{other} declared here"))
        .with_note(
            "a unit's services, agents, providers, functions, assets, types and events share one namespace",
        ),
    )
}

/// The synthetic `FnDecl` [`build_unit_table`] registers for a messages-bearing
/// commons. Its body is never checked (see the call site's comment) — it
/// exists only so `Param`/`TypeRef`/`FnDecl` construction has somewhere to put
/// a syntactically valid placeholder.
fn synthetic_render_fn() -> FnDecl {
    let span = Span::default();
    FnDecl {
        type_params: Vec::new(),
        name: FnName::Free(Ident {
            name: "render".to_string(),
            span,
        }),
        params: vec![
            Param {
                name: Ident {
                    name: "tag".to_string(),
                    span,
                },
                type_ref: TypeRef::Named(Ident {
                    name: "LocaleTag".to_string(),
                    span,
                }),
                span,
            },
            Param {
                name: Ident {
                    name: "msg".to_string(),
                    span,
                },
                type_ref: TypeRef::Named(Ident {
                    name: "Message".to_string(),
                    span,
                }),
                span,
            },
        ],
        return_type: TypeRef::Base(BaseType::String, span),
        requires: Vec::new(),
        ensures: Vec::new(),
        body: Block {
            statements: Vec::new(),
            tail: Box::new(Expr {
                id: ExprId::SYNTHETIC,
                kind: ExprKind::StrLit(String::new()),
                span,
            }),
            span,
            tail_leading_comments: Vec::new(),
            implicit_tail: false,
        },
        has_self: false,
        documentation: None,
        span,
        trivia: Trivia::default(),
    }
}

/// #1824: whether `f` is the synthetic `render` [`build_unit_table`] registers
/// for a messages-bearing commons. It stands in for `bynk.locale`'s `render`,
/// and its `LocaleTag`/`Message` mean `bynk.locale`'s, whatever the commons
/// itself declares under those names: the emitted bundle names the stdlib
/// types (#1697).
pub fn is_synthetic_render(f: &FnDecl) -> bool {
    matches!(&f.name, FnName::Free(n) if n.name == "render")
        && f.span == Span::default()
        && f.body.tail.id == ExprId::SYNTHETIC
}

/// For each name declared in the unit (type, fn, method, agent), record which
/// source file declared it. Used by the emitter to render relative imports.
#[derive(Clone)]
pub struct FileDeclIndex {
    pub types: HashMap<String, PathBuf>,
    pub fns: HashMap<String, PathBuf>,
    pub methods: HashMap<String, HashMap<String, PathBuf>>,
    /// #1820: an agent's declaring file. A sibling file that constructs it
    /// calls its factory, `__make<Agent>`, which that file exports.
    pub agents: HashMap<String, PathBuf>,
    /// #1820: a capability's declaring file. A sibling file names the
    /// capability's interface in a handler's `deps` and a provider's
    /// `implements`, so it imports the type from there.
    pub capabilities: HashMap<String, PathBuf>,
}

/// **Tree-relative, deliberately.** This is an *emit* structure, not an index:
/// `record_name_ref` compares these paths against `ctx.source_path`
/// (`emitter.rs`), which is the file's `include`-root-relative path. Keying it
/// by `identity_path` (ADR 0198) makes `path != &ctx.source_path` always true
/// for a split project, so a name declared in the *same* file is emitted as a
/// sibling import of itself — the module then cannot load, and a workers
/// runtime test hangs rather than fails. See ADR 0201 (E).
pub fn build_file_decl_index(indices: &[usize], parsed: &[ParsedFile]) -> FileDeclIndex {
    let mut idx = FileDeclIndex {
        types: HashMap::new(),
        fns: HashMap::new(),
        methods: HashMap::new(),
        agents: HashMap::new(),
        capabilities: HashMap::new(),
    };
    for &i in indices {
        let path = parsed[i].source_path();
        for item in parsed[i].items() {
            match item {
                CommonsItem::Type(t) => {
                    idx.types
                        .entry(t.name.name.clone())
                        .or_insert_with(|| path.clone());
                }
                // Events track, slice 0 (spine #936): an `event` name shares
                // the `types` file index — it registers into the same
                // `types` symbol table as an ordinary `type` everywhere else
                // in this module.
                CommonsItem::Event(e) => {
                    idx.types
                        .entry(e.name.name.clone())
                        .or_insert_with(|| path.clone());
                }
                CommonsItem::Fn(f) => match &f.name {
                    FnName::Free(id) => {
                        idx.fns
                            .entry(id.name.clone())
                            .or_insert_with(|| path.clone());
                    }
                    FnName::Method {
                        type_name,
                        method_name,
                    } => {
                        idx.methods
                            .entry(type_name.name.clone())
                            .or_default()
                            .entry(method_name.name.clone())
                            .or_insert_with(|| path.clone());
                    }
                },
                CommonsItem::Agent(a) => {
                    idx.agents
                        .entry(a.name.name.clone())
                        .or_insert_with(|| path.clone());
                }
                CommonsItem::Capability(c) => {
                    idx.capabilities
                        .entry(c.name.name.clone())
                        .or_insert_with(|| path.clone());
                }
                CommonsItem::Provider(_)
                | CommonsItem::Service(_)
                | CommonsItem::Actor(_)
                // `messages` bundles aren't cross-file-imported by name in
                // slice 1 (no multi-file bundle merge yet).
                | CommonsItem::Messages(_) => {}
                // #1883, slice 1b: an asset is an exported constant of its
                // file, in the function namespace, so a sibling file or a
                // `uses`-ing unit imports it the way it imports a function.
                CommonsItem::Asset(a) => {
                    idx.fns
                        .entry(a.name.name.clone())
                        .or_insert_with(|| path.clone());
                }
            }
        }
    }
    idx
}

/// #696: returns the `parsed` index of the owning file alongside the `uses`
/// clause span, so the caller can attribute the diagnostic to that file.
pub fn uses_span_of(
    parsed: &[ParsedFile],
    indices: &[usize],
    target: &str,
) -> Option<(usize, Span)> {
    for &i in indices {
        for u in parsed[i].uses() {
            if u.target.joined() == target {
                return Some((i, u.span));
            }
        }
    }
    None
}

/// Build the [`resolver::CrossContextInfo`] for a given consuming context.
/// Used by both the resolver/checker (per-file processing) and the emitter
/// (composition root + boundary casts).
pub fn build_cross_context_info(
    name: &str,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_tables: &HashMap<String, UnitTable>,
) -> resolver::CrossContextInfo {
    let consumed_contexts: Vec<String> = unit_consumes.get(name).cloned().unwrap_or_default();
    let aliases: HashMap<String, String> =
        unit_consumes_aliases.get(name).cloned().unwrap_or_default();
    let mut consumed_services: HashMap<String, HashMap<String, resolver::CrossContextService>> =
        HashMap::new();
    let mut consumed_types: HashMap<String, HashMap<String, Arc<TypeDecl>>> = HashMap::new();
    let mut consumed_capabilities: HashMap<
        String,
        HashMap<String, resolver::CrossContextCapability>,
    > = HashMap::new();
    // Events track, slice 0 (spine #936): each consumed context's own event
    // names, so a subscriber's `from Events(E)` can be checked against a
    // foreign owner too — mirrors `discover_event_subscribers` below, which
    // already resolves ownership this same way for wiring.
    let mut consumed_event_names: HashMap<String, std::collections::HashSet<String>> =
        HashMap::new();
    let mut consumed_type_owners: HashMap<String, HashMap<String, String>> = HashMap::new();
    for t in &consumed_contexts {
        let (other_types_combined, owners) =
            contract_types_for(t, unit_tables, unit_uses, unit_consumes);
        consumed_types.insert(t.clone(), other_types_combined);
        if !owners.is_empty() {
            consumed_type_owners.insert(t.clone(), owners);
        }
        let Some(other_table) = unit_tables.get(t) else {
            continue;
        };
        consumed_event_names.insert(t.clone(), other_table.events.keys().cloned().collect());
        let mut svcs: HashMap<String, resolver::CrossContextService> = HashMap::new();
        for (sname, sdecl) in &other_table.services {
            if let Some(mut svc) = resolver::cross_context_service_for(sname, sdecl) {
                svc.foreign_callers = sdecl
                    .handlers
                    .iter()
                    .find_map(|h| crate::actors::call_admission_for(h, &other_table.actors))
                    .map(|a| a.foreign.into_iter().map(|f| f.actor).collect())
                    .unwrap_or_default();
                svcs.insert(sname.clone(), svc);
            }
        }
        consumed_services.insert(t.clone(), svcs);

        // v0.15: gather the consumed context's exported capabilities, each
        // paired with the provider that implements it.
        let mut caps: HashMap<String, resolver::CrossContextCapability> = HashMap::new();
        for cap_name in &other_table.exported_capabilities {
            let Some(decl) = other_table.capabilities.get(cap_name) else {
                continue;
            };
            let Some(provider) = other_table.providers.get(cap_name) else {
                continue;
            };
            let ops = decl
                .ops
                .iter()
                .map(|op| resolver::CrossContextCapabilityOp {
                    name: op.name.name.clone(),
                    type_params: op.type_params.iter().map(|p| p.name.name.clone()).collect(),
                    params: op
                        .params
                        .iter()
                        .map(|p| (p.name.name.clone(), p.type_ref.clone()))
                        .collect(),
                    return_type: op.return_type.clone(),
                })
                .collect();
            caps.insert(
                cap_name.clone(),
                resolver::CrossContextCapability {
                    name: cap_name.clone(),
                    ops,
                    provider_name: provider.provider_name.name.clone(),
                    provider_given: provider
                        .given
                        .iter()
                        .filter(|c| !c.is_cross_context())
                        .map(|c| c.key().to_string())
                        .collect(),
                    span: decl.span,
                },
            );
        }
        consumed_capabilities.insert(t.clone(), caps);
    }
    resolver::CrossContextInfo {
        self_context: Some(name.to_string()),
        consumed_contexts,
        aliases,
        consumed_services,
        consumed_types,
        // Set by the emitter (#1932).
        consumed_type_renames: HashMap::new(),
        consumed_capabilities,
        // Set by the caller from the unit's `consumes U { … }` clauses.
        flattened_caps: unit_tables
            .get(name)
            .map(|t| t.flattened_caps.clone())
            .unwrap_or_default(),
        consumed_event_names,
        consumed_type_owners,
    }
}

/// Events track, slice 0 (spine #936): project-wide "who subscribes to what"
/// — for every `service ... from Events(E) { on event ... }` anywhere in the
/// project, resolve `E` to its *owning* context (the one whose `events` table
/// actually declares it — bare-name resolution, since `TypeRef` has no dotted
/// form) and group subscribers under `(owning_context, event_type_name)`. No
/// prior art to reuse: cross-context wiring elsewhere is driven by an
/// explicit author-written `consumes` clause, resolved eagerly in
/// `phase_resolve_consumes`; this is the first wiring driven by an *implicit*
/// relationship (a bare event-type name shared between a publisher's `event`
/// declaration and a subscriber's `from Events(E)` header) — the same
/// ownership resolution [`build_cross_context_info`]'s own `consumed_event_names`
/// performs above, for the same reason (#936, events track slice 0).
pub fn discover_event_subscribers(
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
) -> BTreeMap<(String, String), Vec<(String, String)>> {
    let mut out: BTreeMap<(String, String), Vec<(String, String)>> = BTreeMap::new();
    for (ctx_name, table) in unit_tables {
        for (svc_name, svc) in &table.services {
            let ServiceProtocol::Events { event_type, .. } = &svc.protocol else {
                continue;
            };
            let TypeRef::Named(id) = event_type else {
                continue;
            };
            let name = &id.name;
            let owner = if table.events.contains_key(name) {
                Some(ctx_name.clone())
            } else {
                unit_consumes.get(ctx_name).and_then(|consumed| {
                    consumed
                        .iter()
                        .find(|c| {
                            unit_tables
                                .get(c.as_str())
                                .is_some_and(|t| t.events.contains_key(name))
                        })
                        .cloned()
                })
            };
            if let Some(owner) = owner {
                out.entry((owner, name.clone()))
                    .or_default()
                    .push((ctx_name.clone(), svc_name.clone()));
            }
        }
    }
    // `unit_tables`/`table.services` are `HashMap`s, so the pushes above race
    // across builds — a multi-subscriber event's dispatch order (and so the
    // emitted `__eventsDispatch` closure's `await sub1; await sub2;`
    // sequence) would otherwise vary build to build with no source change.
    for subs in out.values_mut() {
        subs.sort();
    }
    out
}

/// P6.x (#1137): one context's own cron expressions (`on cron "expr"`) and
/// queue names (`from queue("name")`), sorted and deduped — the two `wrangler.toml`
/// binding lists a context's own compose entry needs. `v0.44`: one queue binding
/// per service, on the `from queue(...)` header.
pub fn cron_and_queue_triggers(table: &UnitTable) -> (Vec<String>, Vec<String>) {
    let mut crons: Vec<String> = Vec::new();
    let mut queues: Vec<String> = Vec::new();
    for service in table.services.values() {
        for handler in &service.handlers {
            if let HandlerKind::Cron { expr } = &handler.kind {
                crons.push(expr.clone());
            }
        }
        if let ServiceProtocol::Queue { name } = &service.protocol {
            queues.push(name.clone());
        }
    }
    crons.sort();
    crons.dedup();
    queues.sort();
    queues.dedup();
    (crons, queues)
}

/// v0.15: validate one `given` capability reference. A bare reference must name
/// a capability declared in this context; a cross-context reference (`given
/// B.Cap`) must name a capability the consumed context exports. Returns the
/// local [`CapabilityInfo`] to add to the in-scope map for bare references;
/// cross-context references return `None` (their calls are type-checked via
/// `consumed_capabilities` at the call site) but are still validated here.
/// v0.25: record a clause-position capability reference (`provides Cap`,
/// bare `given Cap`), qualifying a flattened bare name to its providing
/// unit. The span is the name segment only.
pub fn record_capability_clause_ref(
    name: &Ident,
    cross_context: &resolver::CrossContextInfo,
    refs: &mut RefSink,
) {
    record_capability_clause_ref_inner(name, cross_context, refs, false);
}

/// v0.35 (ADR 0068): the `Cap` of a `provides Cap = Provider` clause — a
/// capability reference *and* an implementation edge (the ambient owner is the
/// provider). Flagged so assembly can tell it apart from the provider's own
/// `given` deps, which are capability refs owned by the same provider.
pub fn record_provides_clause_ref(
    name: &Ident,
    cross_context: &resolver::CrossContextInfo,
    refs: &mut RefSink,
) {
    record_capability_clause_ref_inner(name, cross_context, refs, true);
}

fn record_capability_clause_ref_inner(
    name: &Ident,
    cross_context: &resolver::CrossContextInfo,
    refs: &mut RefSink,
    provides: bool,
) {
    let unit = cross_context.flattened_caps.get(&name.name);
    if provides {
        refs.record_provides(name.span, &name.name, unit.map(String::as_str));
    } else if let Some(unit) = unit {
        refs.record_in_unit(name.span, SymbolKind::Capability, &name.name, unit);
    } else {
        refs.record(name.span, SymbolKind::Capability, &name.name);
    }
}

pub fn resolve_given_cap_ref(
    cap_ref: &CapRef,
    capability_info_map: &HashMap<String, CapabilityInfo>,
    cross_context: &resolver::CrossContextInfo,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
) -> Option<CapabilityInfo> {
    let Some(prefix) = cap_ref.prefix() else {
        // Local capability.
        match capability_info_map.get(cap_ref.key()) {
            Some(info) => {
                record_capability_clause_ref(&cap_ref.name, cross_context, refs);
                // Interop I5 (#1972): a kind every use of which needs an
                // option (a `Service` names its Worker) is reached only
                // through a named instance. Still resolved, so the handler's
                // calls are not refused a second time as undeclared.
                if let Some(unit) = cross_context.flattened_caps.get(cap_ref.key())
                    && let Some(kind) = crate::firstparty::instantiable(unit, cap_ref.key())
                    && !kind.bare
                {
                    let example = crate::project_model::instance_example(kind);
                    errors.push(
                        CompileError::new(
                            "bynk.capability.instance_only",
                            cap_ref.span,
                            format!(
                                "`{unit}.{}` is used only through a named instance",
                                cap_ref.key()
                            ),
                        )
                        .with_note(format!("declare one and use it instead: `{example}`")),
                    );
                }
                return Some(info.clone());
            }
            None => {
                errors.push(CompileError::new(
                    "bynk.given.unknown_capability",
                    cap_ref.span,
                    format!(
                        "capability `{}` is not declared in this context",
                        cap_ref.key()
                    ),
                ));
                return None;
            }
        }
    };
    // Cross-context capability (`given B.Cap` / `given Alias.Cap`).
    let Some(ctx_name) = cross_context.resolve_prefix(&prefix) else {
        errors.push(
            CompileError::new(
                "bynk.resolve.unconsumed_context",
                cap_ref.span,
                format!(
                    "`given {}.{}` refers to a context that this context does not `consumes`",
                    prefix,
                    cap_ref.key()
                ),
            )
            .with_note(
                "add a `consumes` clause for the providing context (optionally with an alias) at the top of this context",
            ),
        );
        return None;
    };
    let exports_it = cross_context
        .consumed_capabilities
        .get(&ctx_name)
        .is_some_and(|m| m.contains_key(cap_ref.key()));
    if exports_it {
        // v0.25: dotted `given B.Cap` — the name segment, in the consumed
        // unit's namespace.
        refs.record_in_unit(
            cap_ref.name.span,
            SymbolKind::Capability,
            cap_ref.key(),
            &ctx_name,
        );
    }
    if !exports_it {
        errors.push(
            CompileError::new(
                "bynk.given.cross_context_unknown_capability",
                cap_ref.span,
                format!(
                    "context `{}` does not export a capability named `{}`",
                    ctx_name,
                    cap_ref.key()
                ),
            )
            .with_note(
                "the providing context must list the capability in an `exports capability { … }` clause",
            ),
        );
    }
    None
}

/// Build the combined type table for `unit`: its own types merged with the
/// types of every commons it `uses`. Used by cross-context resolution so we
/// can resolve a consumed context's service signatures against that context's
/// own view of types (v0.6 §4.5).
/// v0.177 (#643): the callee's own type namespace — its local declarations plus
/// the commons types it `uses`.
///
/// Shared deliberately. The **caller** reaches this table through
/// `consumed_types[callee]` and the **callee** builds it for itself; both must
/// canonicalise the callee's contract from the *same* table or their hashes
/// diverge and every call 409s. Routing both through one function makes that
/// agreement structural rather than a thing to keep in step by hand.
/// #2073: the type table a context's service signatures resolve in, as a
/// caller and the context itself both see it: [`combined_types_for`] (its own
/// types and its `uses` commons'), plus the types of the contexts it consumes,
/// which a signature may name (`on call(o: Order)` in `shop.ledger`, `Order`
/// from `shop.order`). Its own types win a name clash. The second map names
/// the declaring context of each type that came from a consumed context.
///
/// A borrowed type's body resolves in its declaring context's view, not this
/// one's: `shop.order`'s `Order { region: Region }` names `shop.order`'s
/// `Region` even when `shop.ledger` declares a `Region` of its own. Where the
/// two would differ, the reference is renamed to a key qualified by the
/// declaring context (`shop_order__Region`) under which that context's
/// declaration is added, so canonicalising (the contract hash) and walking
/// the table both see the right shape. A name two consumed contexts both
/// declare is added bare for neither; a signature cannot name it bare
/// anyway (an exported clash is `bynk.consumes.name_conflict`), and a body
/// that reaches it does so through its own context's qualified key.
///
/// Both sides of a cross-context call build the contract hash from this one
/// table, so they agree by construction.
pub fn contract_types_for(
    unit: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
) -> (HashMap<String, Arc<TypeDecl>>, HashMap<String, String>) {
    let mut walk = ContractWalk::default();
    let (table, owners, _) =
        contract_types_inner(unit, unit_tables, unit_uses, unit_consumes, &mut walk);
    (table, owners)
}

/// #2087: the name a user reads for a type `contract_types_for` may have
/// filed under a qualified key. A key is the declaring context's namespace,
/// `__`, and the declaration's own name (`shop_order__Code`); the declaration
/// carries the key as its name too, so every path that prints a type name to a
/// user goes through this, and reads `Code`. Any other name is returned as is:
/// a key's prefix is a lowercase namespace and its last segment a capitalised
/// type name, which a declared type name never is.
pub fn display_type_name(name: &str) -> &str {
    match name.rsplit_once("__") {
        Some((prefix, last))
            if prefix.starts_with(|c: char| c.is_ascii_lowercase())
                && prefix
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                && last.starts_with(|c: char| c.is_ascii_uppercase()) =>
        {
            last
        }
        _ => name,
    }
}

/// #2087: `text` (a contract's normal form, say) with every qualified key in
/// it shown by its declaration's own name ([`display_type_name`]).
pub fn display_type_names_in(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        out.push_str(display_type_name(word));
        word.clear();
    };
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// A context's contract table and the declaring context of each borrowed type.
type ContractView = (HashMap<String, Arc<TypeDecl>>, HashMap<String, String>);

/// #2073 review: one `contract_types_for` walk. `visiting` breaks a `consumes`
/// cycle; `memo` holds each context's view once it is computed, so a layered
/// `consumes` DAG costs one walk per context, not one per path. A view that
/// met a cycle depends on where the walk entered it, so it is not memoised.
#[derive(Default)]
struct ContractWalk {
    visiting: std::collections::HashSet<String>,
    memo: HashMap<String, ContractView>,
}

/// The view, its owners, and whether computing it met a `consumes` cycle.
fn contract_types_inner(
    unit: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    walk: &mut ContractWalk,
) -> (
    HashMap<String, Arc<TypeDecl>>,
    HashMap<String, String>,
    bool,
) {
    if let Some((table, owners)) = walk.memo.get(unit) {
        return (table.clone(), owners.clone(), false);
    }
    let mut out = combined_types_for(unit, unit_tables, unit_uses);
    let mut owners: HashMap<String, String> = HashMap::new();
    if !walk.visiting.insert(unit.to_string()) {
        // A `consumes` cycle: stop at the unit's own view.
        return (out, owners, true);
    }
    let mut consumed: Vec<&String> = unit_consumes
        .get(unit)
        .into_iter()
        .flatten()
        .filter(|u| {
            unit_tables
                .get(*u)
                .is_some_and(|t| t.kind == Some(UnitKind::Context))
        })
        .collect();
    consumed.sort();
    consumed.dedup();
    // Each consumed context's own view, in which its types' bodies resolve.
    let mut cyclic = false;
    let views: HashMap<String, ContractView> = consumed
        .iter()
        .map(|u| {
            let (table, owners, met) =
                contract_types_inner(u, unit_tables, unit_uses, unit_consumes, walk);
            cyclic |= met;
            ((*u).clone(), (table, owners))
        })
        .collect();
    // The context that declares `r` as `owner`'s view resolves it: `owner`
    // itself, a context `owner` consumes, or none (a commons type, whose
    // codecs and namespace are the commons').
    let declarer = |owner: &str, r: &str, target: &Arc<TypeDecl>| -> Option<String> {
        if unit_tables
            .get(owner)
            .and_then(|t| t.types.get(r))
            .is_some_and(|d| Arc::ptr_eq(d, target) || d.span == target.span)
        {
            return Some(owner.to_string());
        }
        views.get(owner).and_then(|(_, o)| o.get(r).cloned())
    };
    let mut by_name: std::collections::BTreeMap<&String, Vec<&String>> = Default::default();
    for u in &consumed {
        for n in unit_tables[*u].types.keys() {
            if !out.contains_key(n) {
                by_name.entry(n).or_default().push(u);
            }
        }
    }
    let mut queue: Vec<(String, String)> = Vec::new();
    for (n, us) in by_name {
        if let [u] = us.as_slice() {
            out.insert(n.clone(), unit_tables[*u].types[n].clone());
            owners.insert(n.clone(), (*u).clone());
            queue.push((n.clone(), (*u).clone()));
        }
    }
    queue.sort();
    let same = |a: &Arc<TypeDecl>, b: &Arc<TypeDecl>| Arc::ptr_eq(a, b) || a.span == b.span;
    // `via`: the consumed context whose view the key came from, in which its
    // body's references resolve (the declarer, for `owners`, may be a context
    // `via` itself consumes).
    while let Some((key, via)) = queue.pop() {
        let owner = via;
        let Some((view, _)) = views.get(&owner) else {
            continue;
        };
        let decl = out[&key].clone();
        let mut subst: HashMap<String, String> = HashMap::new();
        for r in crate::project_model::type_decl_ref_names(&decl) {
            let Some(target) = view.get(&r) else {
                continue;
            };
            match out.get(&r) {
                Some(here) if same(here, target) => {}
                None => {
                    out.insert(r.clone(), target.clone());
                    if let Some(d) = declarer(&owner, &r, target) {
                        owners.insert(r.clone(), d);
                    }
                    queue.push((r, owner.clone()));
                }
                Some(_) => {
                    let d = declarer(&owner, &r, target);
                    let qualified = format!(
                        "{}__{r}",
                        d.as_deref().unwrap_or(owner.as_str()).replace('.', "_")
                    );
                    if !out.contains_key(&qualified) {
                        // #2087: the declaration takes its key as its name, so a
                        // type resolved through the key (`Ty::Named`'s name is
                        // the declaration's) looks up this declaration, not the
                        // unit's same-named one.
                        let mut renamed = (**target).clone();
                        renamed.name.name = qualified.clone();
                        out.insert(qualified.clone(), Arc::new(renamed));
                        if let Some(d) = d {
                            owners.insert(qualified.clone(), d);
                        }
                        queue.push((qualified.clone(), owner.clone()));
                    }
                    subst.insert(r, qualified);
                }
            }
        }
        if !subst.is_empty() {
            out.insert(key, Arc::new(rename_decl_refs(&decl, &subst)));
        }
    }
    walk.visiting.remove(unit);
    if !cyclic {
        walk.memo
            .insert(unit.to_string(), (out.clone(), owners.clone()));
    }
    (out, owners, cyclic)
}

/// #2073: `decl` with every type it names renamed by `subst` (its own name
/// kept).
fn rename_decl_refs(decl: &TypeDecl, subst: &HashMap<String, String>) -> TypeDecl {
    let mut out = decl.clone();
    match &mut out.body {
        TypeBody::Record(r) => {
            for f in &mut r.fields {
                rename_type_ref(&mut f.type_ref, subst);
            }
        }
        TypeBody::Sum(sum) => {
            for v in &mut sum.variants {
                for f in &mut v.payload {
                    rename_type_ref(&mut f.type_ref, subst);
                }
            }
        }
        TypeBody::Refined { .. } | TypeBody::Opaque { .. } => {}
    }
    out
}

/// #2073: rename every user type `r` names, in place. Exhaustive over
/// `TypeRef`, like [`crate::project_model::type_ref_names`].
fn rename_type_ref(r: &mut TypeRef, subst: &HashMap<String, String>) {
    match r {
        TypeRef::Named(id) => {
            if let Some(to) = subst.get(&id.name) {
                id.name = to.clone();
            }
        }
        TypeRef::App { name, args, .. } => {
            if let Some(to) = subst.get(&name.name) {
                name.name = to.clone();
            }
            for a in args {
                rename_type_ref(a, subst);
            }
        }
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            rename_type_ref(a, subst);
            rename_type_ref(b, subst);
        }
        TypeRef::Option(t, _)
        | TypeRef::Effect(t, _)
        | TypeRef::HttpResult(t, _)
        | TypeRef::List(t, _)
        | TypeRef::Query(t, _)
        | TypeRef::Stream(t, _)
        | TypeRef::Connection(t, _)
        | TypeRef::History(t, _) => rename_type_ref(t, subst),
        TypeRef::Fn(params, ret, _) => {
            for p in params {
                rename_type_ref(p, subst);
            }
            rename_type_ref(ret, subst);
        }
        TypeRef::Base(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::ValidationError(_)
        | TypeRef::JsonError(_)
        | TypeRef::Unit(_) => {}
    }
}

pub fn combined_types_for(
    unit: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
) -> HashMap<String, Arc<TypeDecl>> {
    let mut out: HashMap<String, Arc<TypeDecl>> = HashMap::new();
    if let Some(table) = unit_tables.get(unit) {
        for (n, d) in &table.types {
            out.insert(n.clone(), d.clone());
        }
    }
    if let Some(targets) = unit_uses.get(unit) {
        for t in targets {
            if let Some(used) = unit_tables.get(t) {
                for (n, d) in &used.types {
                    out.entry(n.clone()).or_insert_with(|| d.clone());
                }
            }
        }
    }
    out
}

/// P6.18: a `uses`-imported type's own combined visible types (one level,
/// matching [`combined_types_for`]'s identical shape) — the narrow resolution
/// scope a signature-lowering pass needs for that unit's own attached
/// methods. Reimplemented against [`UnitInfo`] rather than calling
/// `combined_types_for` directly: that function takes the flat, project-wide
/// `unit_tables`/`unit_uses` maps a per-unit emission prologue doesn't thread
/// this deep (only the coarser, already-merged `unit_info` reaches there) —
/// rebuilding those two maps from `unit_info` on every call would be needless
/// O(units) cloning for a per-unit prologue already called once per emitted
/// unit.
pub fn combined_types_for_unit_info(
    unit: &str,
    unit_info: &BTreeMap<String, UnitInfo>,
) -> HashMap<String, Arc<TypeDecl>> {
    let mut out: HashMap<String, Arc<TypeDecl>> = HashMap::new();
    let Some(info) = unit_info.get(unit) else {
        return out;
    };
    for (n, d) in &info.table.types {
        out.insert(n.clone(), d.clone());
    }
    for t in &info.uses {
        if let Some(used) = unit_info.get(t) {
            for (n, d) in &used.table.types {
                out.entry(n.clone()).or_insert_with(|| d.clone());
            }
        }
    }
    out
}

/// Locale capability track, slice 2 (#882): the message bundle a context's
/// `Locale.current()` negotiates against, auto-detected from the context's
/// *direct* `uses` (one level, not transitive — see [`combined_types_for`]
/// just above, the precedent for this rule). `None`/`One`/`Many` drive three
/// different behaviours: unchanged fixed-default `Locale`, real negotiation
/// wiring, or (when the context also consumes `Locale`)
/// `bynk.messages.multiple_message_bundles` — see `check_locale_bundle_ambiguity`
/// (`bynk-emit/src/project/validate.rs`) and the per-Worker composition loop
/// (`bynk-emit/src/project.rs`).
// `pub`, not `pub(crate)`: `MessageBundleInfo` appears in `emit_worker_compose`'s
// public signature (`bynk-emit/src/emitter/workers.rs`), which must expose
// types at least as visible as itself (matching `UnitTable`'s own `pub`).
pub enum ContextMessageBundle {
    /// No directly-`uses`d commons declares a `messages` block.
    None,
    /// Exactly one — the negotiable case.
    One(MessageBundleInfo),
    /// Two or more (each commons's own qualified name, for the diagnostic).
    Many(Vec<String>),
}

pub struct MessageBundleInfo {
    /// The commons's qualified unit name (e.g. `"app.msgs"`).
    pub commons: String,
    /// Project-relative path of the file carrying the `@reference` block —
    /// the import target for `__messagesLocales`/`__messagesReferenceLocale`.
    /// (A bundle genuinely split across multiple files, per the track doc's
    /// own §4.1, is not correctly merged by `emit_messages_bundle` today —
    /// each file emits independently, `bynk-emit/src/project.rs`'s per-file
    /// `emit_items` loop — this detection mirrors that same file-scoped
    /// reality rather than a wider, currently-unimplemented merge.)
    pub source_path: PathBuf,
}

/// Walks `ctx`'s own direct `uses` list for commons declaring a `messages`
/// bundle with exactly one `@reference` block (a bundle missing or
/// duplicating its own reference is already diagnosed by
/// `check_messages_bundles` — this function simply doesn't count it as
/// "found", rather than compounding an already-reported error).
pub fn detect_context_message_bundle(
    ctx: &str,
    unit_uses: &HashMap<String, Vec<String>>,
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    parsed: &[ParsedFile],
) -> ContextMessageBundle {
    let mut found: Vec<MessageBundleInfo> = Vec::new();
    for target in unit_uses.get(ctx).into_iter().flatten() {
        if kinds.get(target) != Some(&UnitKind::Commons) {
            continue;
        }
        let Some(indices) = groups.get(target) else {
            continue;
        };
        for &i in indices {
            let has_reference = parsed[i].items().iter().any(|item| {
                matches!(item, CommonsItem::Messages(m) if m.annotations.iter().any(|a| a.name.name == "reference"))
            });
            if has_reference {
                found.push(MessageBundleInfo {
                    commons: target.clone(),
                    source_path: parsed[i].source_path(),
                });
                break;
            }
        }
    }
    match found.len() {
        0 => ContextMessageBundle::None,
        1 => ContextMessageBundle::One(found.pop().expect("len == 1")),
        _ => ContextMessageBundle::Many(found.into_iter().map(|b| b.commons).collect()),
    }
}

#[cfg(test)]
mod detect_context_message_bundle_tests {
    use super::*;
    use bynk_syntax::ast::{
        Annotation, Commons, CommonsForm, Context, MessagesDecl, QualifiedName, SourceUnit,
        UsesDecl,
    };

    fn ident(name: &str) -> Ident {
        Ident {
            name: name.to_string(),
            span: Span::default(),
        }
    }

    fn qualified(name: &str) -> QualifiedName {
        QualifiedName {
            parts: name.split('.').map(ident).collect(),
            span: Span::default(),
        }
    }

    /// A commons `ParsedFile` declaring one `messages <tag>` block, its
    /// `@reference` annotation present or not. `source_path` is derived from
    /// `name` so each test bundle gets a distinct, recognisable import
    /// target — real content doesn't matter, only that a path exists.
    fn commons_with_messages(name: &str, tag: &str, is_reference: bool) -> ParsedFile {
        let annotations = if is_reference {
            vec![Annotation {
                name: ident("reference"),
                args: Vec::new(),
                span: Span::default(),
            }]
        } else {
            Vec::new()
        };
        let messages = MessagesDecl {
            tag: tag.to_string(),
            tag_span: Span::default(),
            annotations,
            entries: Vec::new(),
            documentation: None,
            span: Span::default(),
            trivia: Trivia::default(),
        };
        ParsedFile::new(
            PathBuf::from(format!("{}.bynk", name.replace('.', "/"))),
            PathBuf::from(format!("src/{}.bynk", name.replace('.', "/"))),
            None,
            String::new(),
            SourceUnit::Commons(Commons {
                name: qualified(name),
                items: vec![CommonsItem::Messages(messages)],
                uses: Vec::new(),
                documentation: None,
                form: CommonsForm::Brace,
                span: Span::default(),
                trivia: Trivia::default(),
                trailing_comments: Vec::new(),
            }),
            UnitKind::Commons,
            false,
        )
    }

    /// A minimal context `ParsedFile` with no items of its own — only its
    /// `uses` list matters for this function.
    fn context_using(name: &str, targets: &[&str]) -> ParsedFile {
        ParsedFile::new(
            PathBuf::from(format!("{}.bynk", name.replace('.', "/"))),
            PathBuf::from(format!("src/{}.bynk", name.replace('.', "/"))),
            None,
            String::new(),
            SourceUnit::Context(Context {
                removed_agents: Vec::new(),
                name: qualified(name),
                uses: targets
                    .iter()
                    .map(|t| UsesDecl {
                        target: qualified(t),
                        span: Span::default(),
                        trivia: Trivia::default(),
                    })
                    .collect(),
                consumes: Vec::new(),
                exports: Vec::new(),
                items: Vec::new(),
                documentation: None,
                form: CommonsForm::Brace,
                span: Span::default(),
                trivia: Trivia::default(),
                trailing_comments: Vec::new(),
            }),
            UnitKind::Context,
            false,
        )
    }

    /// The four tables `detect_context_message_bundle`'s real callers
    /// already build — bundled here so [`scenario`] doesn't need a
    /// clippy-unfriendly four-tuple return type.
    struct Scenario {
        parsed: Vec<ParsedFile>,
        groups: BTreeMap<String, Vec<usize>>,
        kinds: BTreeMap<String, UnitKind>,
        unit_uses: HashMap<String, Vec<String>>,
    }

    /// Assembles a [`Scenario`] from a context plus its bundle files.
    fn scenario(ctx_name: &str, ctx_uses: &[&str], bundles: Vec<(&str, ParsedFile)>) -> Scenario {
        let mut parsed = vec![context_using(ctx_name, ctx_uses)];
        let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut kinds: BTreeMap<String, UnitKind> = BTreeMap::new();
        groups.insert(ctx_name.to_string(), vec![0]);
        kinds.insert(ctx_name.to_string(), UnitKind::Context);
        for (name, pf) in bundles {
            let idx = parsed.len();
            parsed.push(pf);
            groups.entry(name.to_string()).or_default().push(idx);
            kinds.insert(name.to_string(), UnitKind::Commons);
        }
        let mut unit_uses: HashMap<String, Vec<String>> = HashMap::new();
        unit_uses.insert(
            ctx_name.to_string(),
            ctx_uses.iter().map(|s| s.to_string()).collect(),
        );
        Scenario {
            parsed,
            groups,
            kinds,
            unit_uses,
        }
    }

    #[test]
    fn zero_bundles_when_uses_reaches_no_messages_commons() {
        let Scenario {
            parsed,
            groups,
            kinds,
            unit_uses,
        } = scenario("app.web", &["app.other"], vec![]);
        assert!(matches!(
            detect_context_message_bundle("app.web", &unit_uses, &groups, &kinds, &parsed),
            ContextMessageBundle::None
        ));
    }

    #[test]
    fn zero_bundles_when_uses_is_empty() {
        let Scenario {
            parsed,
            groups,
            kinds,
            unit_uses,
        } = scenario("app.web", &[], vec![]);
        assert!(matches!(
            detect_context_message_bundle("app.web", &unit_uses, &groups, &kinds, &parsed),
            ContextMessageBundle::None
        ));
    }

    #[test]
    fn one_bundle_is_found_by_its_reference_block() {
        let bundle = commons_with_messages("app.msgs", "en", true);
        let Scenario {
            parsed,
            groups,
            kinds,
            unit_uses,
        } = scenario("app.web", &["app.msgs"], vec![("app.msgs", bundle)]);
        let found = detect_context_message_bundle("app.web", &unit_uses, &groups, &kinds, &parsed);
        let ContextMessageBundle::One(info) = found else {
            panic!("expected exactly one bundle");
        };
        assert_eq!(info.commons, "app.msgs");
        assert_eq!(info.source_path, PathBuf::from("app/msgs.bynk"));
    }

    #[test]
    fn a_bundle_missing_its_reference_block_is_not_counted() {
        // Not a `@reference` block — already diagnosed elsewhere
        // (`bynk.messages.missing_reference`); this function simply doesn't
        // count it, rather than compounding an already-reported error.
        let bundle = commons_with_messages("app.msgs", "en", false);
        let Scenario {
            parsed,
            groups,
            kinds,
            unit_uses,
        } = scenario("app.web", &["app.msgs"], vec![("app.msgs", bundle)]);
        assert!(matches!(
            detect_context_message_bundle("app.web", &unit_uses, &groups, &kinds, &parsed),
            ContextMessageBundle::None
        ));
    }

    #[test]
    fn two_bundles_report_both_commons_names() {
        let a = commons_with_messages("app.msgs_a", "en", true);
        let b = commons_with_messages("app.msgs_b", "en", true);
        let Scenario {
            parsed,
            groups,
            kinds,
            unit_uses,
        } = scenario(
            "app.web",
            &["app.msgs_a", "app.msgs_b"],
            vec![("app.msgs_a", a), ("app.msgs_b", b)],
        );
        let found = detect_context_message_bundle("app.web", &unit_uses, &groups, &kinds, &parsed);
        let ContextMessageBundle::Many(names) = found else {
            panic!("expected two bundles");
        };
        let mut names = names;
        names.sort();
        assert_eq!(
            names,
            vec!["app.msgs_a".to_string(), "app.msgs_b".to_string()]
        );
    }

    #[test]
    fn a_commons_reached_only_transitively_is_not_counted() {
        // `app.web` uses `app.mid`, which itself uses the bundle — `uses` is
        // one level, not transitive (message-bundles' own established rule),
        // so this must still report `None`.
        let bundle = commons_with_messages("app.msgs", "en", true);
        let mut mid = context_using("app.mid", &["app.msgs"]);
        // `app.mid` needs to be a commons for this scenario to be legal, but
        // `context_using` builds a Context — for this narrow test only the
        // `uses` *resolution* (does app.web's own direct list reach the
        // bundle) matters, and app.web's own list never names `app.msgs`
        // directly, so the unit kind of the intermediate is irrelevant.
        mid.set_kind(UnitKind::Commons);
        let Scenario {
            mut parsed,
            mut groups,
            mut kinds,
            unit_uses,
        } = scenario("app.web", &["app.mid"], vec![("app.msgs", bundle)]);
        let mid_idx = parsed.len();
        parsed.push(mid);
        groups.insert("app.mid".to_string(), vec![mid_idx]);
        kinds.insert("app.mid".to_string(), UnitKind::Commons);
        assert!(matches!(
            detect_context_message_bundle("app.web", &unit_uses, &groups, &kinds, &parsed),
            ContextMessageBundle::None
        ));
    }
}

/// #696: returns the `parsed` index of the owning file alongside the `consumes`
/// clause span, so the caller can attribute the diagnostic to that file.
pub fn consumes_span_of(
    parsed: &[ParsedFile],
    indices: &[usize],
    target: &str,
) -> Option<(usize, Span)> {
    for &i in indices {
        for c in parsed[i].consumes() {
            if c.target.joined() == target {
                return Some((i, c.span));
            }
        }
    }
    None
}

/// #696: returns the `parsed` index of the owning file alongside the alias span,
/// so the caller can attribute the diagnostic to that file.
pub fn parsed_alias_span(
    parsed: &[ParsedFile],
    indices: &[usize],
    alias: &str,
) -> Option<(usize, Span)> {
    for &i in indices {
        for c in parsed[i].consumes() {
            if let Some(a) = &c.alias
                && a.name == alias
            {
                return Some((i, a.span));
            }
        }
    }
    None
}

/// A type imported into a context via `consumes`. Carries enough metadata for
/// the checker and emitter to enforce / express visibility.
#[derive(Debug, Clone)]
pub struct ConsumedType {
    pub owning_context: String,
    pub visibility: Visibility,
}

/// #1925: every pair of top-level declarations sharing one name, in one
/// context, against the rule that a unit's services, agents, providers,
/// functions, assets, types and events share one namespace — the scope of the
/// emitted TypeScript module. Capabilities and actors keep their own.
#[cfg(test)]
mod value_namespace_tests {
    use super::*;
    use crate::testkit::analyse;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Kind {
        Fn,
        Type,
        Event,
        Asset,
        Service,
        Agent,
        Provider,
        Capability,
        Actor,
    }

    const ALL: [Kind; 9] = [
        Kind::Fn,
        Kind::Type,
        Kind::Event,
        Kind::Asset,
        Kind::Service,
        Kind::Agent,
        Kind::Provider,
        Kind::Capability,
        Kind::Actor,
    ];

    /// The kind a declaration names, and so the side of the rule it takes.
    /// Exhaustive on purpose: a new declaration kind does not compile here
    /// until it is placed in or out of the value namespace (and in [`ALL`]).
    fn kind_of(item: &CommonsItem) -> Option<Kind> {
        match item {
            CommonsItem::Fn(_) => Some(Kind::Fn),
            CommonsItem::Type(_) => Some(Kind::Type),
            CommonsItem::Event(_) => Some(Kind::Event),
            CommonsItem::Asset(_) => Some(Kind::Asset),
            CommonsItem::Service(_) => Some(Kind::Service),
            CommonsItem::Agent(_) => Some(Kind::Agent),
            CommonsItem::Provider(_) => Some(Kind::Provider),
            CommonsItem::Capability(_) => Some(Kind::Capability),
            CommonsItem::Actor(_) => Some(Kind::Actor),
            // Keyed by locale tag, in a commons only: it names nothing.
            CommonsItem::Messages(_) => None,
        }
    }

    fn shares_the_value_namespace(k: Kind) -> bool {
        !matches!(k, Kind::Capability | Kind::Actor)
    }

    /// A declaration of `kind` named `n`. A provider brings the capability it
    /// provides, under a name no other snippet uses.
    fn decl(kind: Kind, n: &str) -> String {
        match kind {
            Kind::Fn => format!("fn {n}() -> Int {{\n  1\n}}\n"),
            Kind::Type => format!("type {n} = {{ n: Int }}\n"),
            Kind::Event => format!("event {n} = {{ n: Int }}\n"),
            Kind::Asset => format!("asset {n} = \"assets/x.json\"\n"),
            Kind::Service => {
                format!(
                    "service {n} {{\n  on call() -> Effect[Int] {{\n    Effect.pure(1)\n  }}\n}}\n"
                )
            }
            Kind::Agent => format!(
                "agent {n} {{\n  key id: String\n\n  store hits: Cell[Int]\n\n  on call ping() -> Effect[Int] {{\n    1\n  }}\n}}\n"
            ),
            Kind::Provider => format!(
                "capability ProbeCap {{\n  fn go() -> Effect[Int]\n}}\n\nprovides ProbeCap = {n} {{\n  fn go() -> Effect[Int] {{\n    Effect.pure(1)\n  }}\n}}\n"
            ),
            Kind::Capability => format!("capability {n} {{\n  fn go() -> Effect[Int]\n}}\n"),
            Kind::Actor => {
                format!("actor {n} {{ auth = Bearer(secret = \"S\"), identity = ProbeId }}\n")
            }
        }
    }

    const HEADER: &str = "context app\n\ntype ProbeId = opaque String\n\n";

    #[test]
    fn each_snippet_declares_its_kind() {
        for k in ALL {
            let src = format!("{HEADER}{}", decl(k, "Thing"));
            let tokens = bynk_syntax::lexer::tokenize(&src).expect("lexes");
            let unit = bynk_syntax::parser::parse_unit(&tokens, &src).expect("parses");
            let bynk_syntax::ast::SourceUnit::Context(c) = unit else {
                panic!("a context")
            };
            assert!(
                c.items.iter().any(|i| kind_of(i) == Some(k)),
                "the {k:?} snippet declares no {k:?}"
            );
        }
    }

    #[test]
    fn every_pair_of_kinds_against_the_value_namespace() {
        for (x, &a) in ALL.iter().enumerate() {
            for &b in &ALL[x + 1..] {
                // Both declaration orders, under both casings.
                for (first, second) in [(a, b), (b, a)] {
                    for n in ["thing", "Thing"] {
                        let src = format!("{HEADER}{}\n{}", decl(first, n), decl(second, n));
                        let r = analyse(&[("app/main.bynk", &src), ("assets/x.json", "{}")]);
                        let cats = r.error_categories();
                        if shares_the_value_namespace(a) && shares_the_value_namespace(b) {
                            let want = if matches!((a, b), (Kind::Type, Kind::Event)) {
                                "bynk.resolve.duplicate_type"
                            } else {
                                "bynk.resolve.name_conflict"
                            };
                            assert_eq!(cats, vec![want], "{first:?} then {second:?} `{n}`");
                        } else {
                            assert!(
                                cats.is_empty(),
                                "{first:?} then {second:?} `{n}` should check clean: {cats:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// A context split across files is one namespace: the conflict is found
    /// whichever file declares which, and reported in the service's file.
    #[test]
    fn a_split_context_is_one_namespace() {
        for (svc_file, asset_file) in [("a.bynk", "z.bynk"), ("z.bynk", "a.bynk")] {
            let svc = format!("context app\n\n{}", decl(Kind::Service, "api"));
            let asset = format!("context app\n\n{}", decl(Kind::Asset, "api"));
            let svc_path = format!("app/{svc_file}");
            let asset_path = format!("app/{asset_file}");
            let r = analyse(&[
                (svc_path.as_str(), svc.as_str()),
                (asset_path.as_str(), asset.as_str()),
                ("assets/x.json", "{}"),
            ]);
            assert_eq!(r.error_categories(), vec!["bynk.resolve.name_conflict"]);
            let d = &r.analysis.errors[0];
            assert!(
                d.source_path
                    .as_deref()
                    .is_some_and(|p| p.ends_with(svc_file)),
                "reported in the service's file: {:?}",
                d.source_path
            );
        }
    }

    /// Two bodied providers of different capabilities under one name each
    /// emit a class of that name.
    #[test]
    fn two_providers_under_one_name_conflict() {
        let src = "context app\n\ncapability A {\n  fn go() -> Effect[Int]\n}\n\ncapability B {\n  fn go() -> Effect[Int]\n}\n\nprovides A = Impl {\n  fn go() -> Effect[Int] {\n    Effect.pure(1)\n  }\n}\n\nprovides B = Impl {\n  fn go() -> Effect[Int] {\n    Effect.pure(2)\n  }\n}\n";
        let r = analyse(&[("app/main.bynk", src)]);
        assert_eq!(r.error_categories(), vec!["bynk.resolve.name_conflict"]);
    }
}

/// #2073 review: a borrowed type's body resolves in its declaring context's
/// view, so the contract table hashes the shape the wire carries.
#[cfg(test)]
mod contract_types_tests {
    use crate::testkit::analyse;

    const ORDER: &str = "context shop.order\n\nexports transparent { Order }\n\ntype Region = enum { Domestic, International }\n\ntype Order = { id: String, region: Region }\n";
    const POST: &str = "context shop.post\n\nexports transparent { Parcel }\n\ntype Region = enum { North, South }\n\ntype Parcel = { ref: String, region: Region }\n";
    const LEDGER: &str = "context shop.ledger\n\nconsumes shop.order\nconsumes shop.post\n\ntype Region = enum { Here, There }\n\nservice orderId {\n  on call(o: Order) -> Effect[String] {\n    o.id\n  }\n}\n\nservice parcelRef {\n  on call(p: Parcel) -> Effect[String] {\n    p.ref\n  }\n}\n";

    fn normal_form(service: &str) -> String {
        let a = analyse(&[
            ("shop/order.bynk", ORDER),
            ("shop/post.bynk", POST),
            ("shop/ledger.bynk", LEDGER),
        ]);
        a.assert_clean();
        let info = &a.analysis.boundary_info["shop.ledger"];
        let svc = crate::resolver::cross_context_service_for(service, &info.services[service])
            .expect("an `on call` service");
        crate::contract::service_normal_form(&svc, &info.types)
    }

    /// #2087: a qualified key reads as its declaration's own name wherever a
    /// type is shown to a user. `Ty::display` is the renderer behind every
    /// checker diagnostic that names a type.
    #[test]
    fn a_qualified_key_displays_as_its_declarations_name() {
        use super::{display_type_name, display_type_names_in};
        assert_eq!(display_type_name("shop_order__Code"), "Code");
        assert_eq!(display_type_name("shop_ledger__shop_order__Code"), "Code");
        assert_eq!(display_type_name("Code"), "Code");
        assert_eq!(display_type_name("My__Thing"), "My__Thing");
        assert_eq!(display_type_name("shop_order__lower"), "shop_order__lower");
        assert_eq!(
            display_type_names_in("orderId(o: Order{code: shop_order__Code(String)})"),
            "orderId(o: Order{code: Code(String)})"
        );
        let tys = crate::checker::Types::default();
        let key = tys.intern(crate::checker::Ty::Named {
            name: "shop_order__Code".to_string(),
            kind: crate::checker::NamedKind::Opaque(bynk_syntax::ast::BaseType::String),
            args: Vec::new(),
        });
        assert_eq!(tys.display(key), "Code");
        let listed = tys.intern(crate::checker::Ty::List(key));
        assert_eq!(tys.display(listed), "List[Code]");
    }

    #[test]
    fn a_borrowed_record_names_its_own_contexts_field_type() {
        let order = normal_form("orderId");
        assert!(
            order.contains("Domestic") && !order.contains("Here") && !order.contains("North"),
            "{order}"
        );
        let parcel = normal_form("parcelRef");
        assert!(
            parcel.contains("North") && !parcel.contains("Here") && !parcel.contains("Domestic"),
            "{parcel}"
        );
    }
}
