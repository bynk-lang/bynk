//! #855: the wire-contract IR — the shape a Bynk type takes crossing a
//! context boundary, derived once from the AST + type table so both the
//! emitter's codec generation and the editor's "wire contract" peek render
//! the *same* derivation instead of two hand-synchronised ones.
//!
//! **The seam:** *`bynk-check` says what the boundary is. `bynk-emit` says
//! how that reads as TypeScript.*
//!
//! Crossing this module boundary is legal for `BaseType`, `PredKind`,
//! `TypeRef`, `Expr`, `TypeDecl` (all `bynk-syntax`) — the vocabulary a
//! boundary type is built from. It is **not** legal for any TS-token string,
//! `pred_condition_and_message` (`bynk-emit/src/emitter.rs`, stays put),
//! `lower_field_default_wire` (`bynk-emit/src/emitter/serialisation.rs`,
//! stays put — a default's *wire literal* is a rendering, not a boundary
//! fact), or the `Qual` type-name→TS-namespace-prefix map (`bynk-emit`'s
//! `serialisation.rs`; see the [`Provenance`] doc for why it cannot move
//! here). `bynk-emit` renders this IR into TypeScript; it does not re-derive
//! it.
//!
//! **Derived from AST + type table, not `checker::Ty`.** `Ty::Named { name,
//! kind, args }` carries no refinement predicates, so a `Ty`-based
//! derivation would still need this same type-table lookup for `PredKind`s —
//! `Ty` would buy only generic substitution, which the moved walks below
//! already implement directly over `TypeRef`. `contract.rs` (this crate)
//! also derives straight from the AST + type table and must not depend on
//! checker output being available; this module keeps the same shape of
//! dependency.
//!
//! **This IR is *not* `contract.rs`'s canonical form**, and the two must
//! never be unified — they disagree on purpose, on every axis that matters:
//!
//! | | `contract.rs` (hash) | `wire.rs` (this module) |
//! |---|---|---|
//! | predicates | sorted, deduped | **declaration order**, not deduped |
//! | record fields | sorted by name | declaration order (emitted key order) |
//! | sum variants | sorted by name | declaration order (`switch` arm order) |
//! | opaque predicate | **elided** — unobservable to the consumer | **present** — the owner still re-validates |
//!
//! `contract.rs:248` documents the sorting as a *precondition* for hash
//! correctness — hashing predicates in source order "would make two contexts
//! that agree perfectly fail closed against each other." The hash is a
//! **type identity** (order-insensitive by design); this module's shapes are
//! an **emission order** (order-sensitive by necessity, since a JS `switch`
//! and an inlined `if` chain both have a literal source order a reader can
//! see). Merging them would create the exact spurious-409 failure the hash
//! exists to prevent. What the two genuinely share — and what a cross-check
//! test elsewhere asserts — is *boundary-type reachability*, not shape.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use bynk_syntax::ast::*;

/// I4 (#1970): the declaration shapes this module's public API is
/// parameterised by ([`collect_codec_closure`] walks `TypeRef` roots over a
/// `TypeDecl` table, whose `body` is a `TypeBody`), re-exported so a renderer
/// over that API (`bynk-emit`'s TypeScript client) names them from here — the
/// P6.27 / P6.49 precedent `project_model`'s `TypeDecl` re-export follows.
pub use bynk_syntax::ast::{TypeBody, TypeRef};

// ---------------------------------------------------------------------
// Core vocabulary
// ---------------------------------------------------------------------

/// The JSON value shape a [`BaseType`] occupies on the wire. Replaces
/// `bynk-emit`'s `ts_base_for_serialisation` *classification* — the TS-token
/// spelling of each kind (`"number"`, `"string"`, …) stays in `bynk-emit`,
/// since a TS token is exactly what this seam excludes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonKind {
    Number,
    String,
    Boolean,
    Object,
    Array,
    Null,
}

/// The JSON kind a base type wires as. Same mapping as the `ts_base_for_serialisation`
/// it replaces: `Int`/`Float`/`Duration`/`Instant` → `Number` (all four erase to a TS
/// `number`), `String`/`Bytes` → `String` (a `Bytes` wires as a base64 string, ADR
/// 0142 D5), `Bool` → `Boolean`.
pub fn json_kind_of(b: BaseType) -> JsonKind {
    match b {
        BaseType::Int | BaseType::Float | BaseType::Duration | BaseType::Instant => {
            JsonKind::Number
        }
        BaseType::String | BaseType::Bytes => JsonKind::String,
        BaseType::Bool => JsonKind::Boolean,
    }
}

/// An extra structural guard a base-typed wire value must pass beyond its
/// `typeof`: an `Int`/`Instant` must be whole, a `Float` must be finite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseGuard {
    Integral,
    Finite,
}

/// What a structural-mismatch error reports as `expected` — the vocabulary a
/// renderer needs to explain *why* a wire value was rejected, independent of
/// the TS spelling of the check that rejected it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expected {
    Json(JsonKind),
    Integer,
    FiniteNumber,
    Base64String,
    SumVariantKind,
}

/// Where a boundary type's declaration lives relative to the module doing
/// the crossing.
///
/// Carries the owner's *qualified unit name*, never a TS namespace prefix —
/// the IR picks the re-validation *strategy* ([`Revalidation`]); `bynk-emit`
/// alone knows how to spell that owner as an `import type * as <ns>` alias
/// (`bynk-emit/src/emitter/serialisation.rs`'s `Qual`/`qual_prefix`, built
/// per-emission from build-mode facts this crate does not have). Unifying
/// the two would drag `BuildTarget` into the checker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provenance {
    /// The emitting module's own declaration: re-validation routes through
    /// the type's own constructor.
    Owned,
    /// A type declared by another unit and reached through it — `owner_unit`
    /// is that unit's qualified name (`commerce.payment`, never a TS
    /// namespace spelling).
    Consumed { owner_unit: String },
}

/// #661 Decisions C/D plus the owner path, stated once: how a boundary
/// scalar's refinement is re-checked on the way in, given who declared it
/// and where the check is happening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revalidation {
    /// Owner's own module: run the type's own `.of`.
    ViaConstructor,
    /// Consumed + transparent (Decision D): re-check each predicate inline,
    /// since the consumer knows the shape by declaration even without an
    /// importable constructor.
    Inline,
    /// Consumed + opaque (Decision C): the predicate is the owner's secret —
    /// cast structurally after the base check; skew is caught by the
    /// contract hash instead.
    StructuralOnly,
    /// `Bytes`: decoded, not cast — the one base type whose wire value is
    /// not a direct cast of its erased representation. Applies regardless of
    /// provenance, mirroring `emit_refined`'s early return to the dedicated
    /// `Bytes` codec before the owned/consumed split is even considered.
    Base64Decode,
}

/// A named boundary type's declared shape, refinement-independent of who is
/// asking. `owner`-relative facts ([`Revalidation`]) live one level down, in
/// [`WireScalar`] — the shape itself does not vary with provenance, only how
/// hard the receiver re-checks it.
#[derive(Debug, Clone)]
pub struct WireType {
    pub name: String,
    /// The codec-name suffix this type resolves to (`Order`, or a
    /// monomorphised generic's `Paginated_User`). Equal to `name` for every
    /// non-generic declaration `wire_type` produces; kept as its own field
    /// because a future generic-instantiation `WireType` (Phase 2+) needs
    /// the two to diverge the same way `serialisation.rs`'s `fn_suffix` /
    /// `ts_type` pair already does.
    pub codec_suffix: String,
    pub provenance: Provenance,
    pub body: WireBody,
}

#[derive(Debug, Clone)]
pub enum WireBody {
    Scalar(WireScalar),
    Record { fields: Vec<WireField> },
    Sum(WireSum),
}

/// A refined- or opaque-base-type boundary scalar.
#[derive(Debug, Clone)]
pub struct WireScalar {
    pub base: BaseType,
    pub json: JsonKind,
    /// `true` for `opaque BaseType`, `false` for a transparent refined type.
    pub opaque: bool,
    /// **Declaration order**, not sorted, not deduped — this is the
    /// highest-risk drift trap between this IR and `contract.rs`'s canonical
    /// form (see the module doc's comparison table). `Inline` revalidation
    /// emits one check per predicate in exactly this order.
    pub predicates: Vec<PredKind>,
    /// The base-type guards this scalar's `Inline` revalidation applies,
    /// *before* the declared predicates. Mirrors
    /// `emit_inline_refinement_checks` exactly: `Int` guards `Integral`,
    /// `Float` guards `Finite` — deliberately **not** `Instant`, which is
    /// guarded at record-*field* position (`emit_field_deserialise`'s
    /// `TypeRef::Base` arm) but not here. That asymmetry already exists in
    /// the emitter this IR was extracted from; it is preserved verbatim
    /// rather than "fixed" by this move, since fixing it would be a
    /// behaviour change this phase must not make. (See `field_base_guards`
    /// for the field-position set, which does include `Instant`.)
    pub base_guards: Vec<BaseGuard>,
    pub revalidation: Revalidation,
}

/// One field of a boundary record, in **declaration order** (the emitted
/// JSON key order).
#[derive(Debug, Clone)]
pub struct WireField {
    pub name: String,
    pub shape: WireRef,
    /// The JSON path segment this field contributes to a validation error's
    /// `path` (`` `${path}.<segment>` ``) — equal to `name` today; kept
    /// distinct because a future field-rename annotation would move the wire
    /// key without moving the path segment a hover/error should still name.
    pub path_segment: String,
    /// The field's default initialiser, if any, carried as raw AST — its
    /// *wire-JSON literal* rendering is `lower_field_default_wire`
    /// (`bynk-emit`), which stays in `bynk-emit` per the module doc: a
    /// default's lowered literal is a rendering, not a boundary fact.
    pub default: Option<(Expr, TypeRef)>,
    /// #1969: the field's inline refinement (`qty: Int where Positive`), in
    /// declaration order, checked on decode after the field's own codec. Empty
    /// for an unrefined field and for every sum-variant payload field (the
    /// grammar gives those no `where`).
    pub predicates: Vec<PredKind>,
}

/// #1969: the JSON key a sum value's variant name travels under (`{"kind":
/// "Circle", …}`). Part of the published wire format, version 1
/// ([`crate::wire_format`]); the in-memory discriminant is `tag`.
pub const WIRE_DISCRIMINANT: &str = "kind";

/// A boundary sum type. The wire and in-memory discriminants are carried
/// side by side because the codec's whole job is translating between them —
/// `memory_discriminant` is the softest part of this seam (a host
/// representation choice, not a wire fact); moving it back into `bynk-emit`
/// alone is a one-field change if a reviewer objects.
#[derive(Debug, Clone)]
pub struct WireSum {
    /// Always [`WIRE_DISCRIMINANT`].
    pub wire_discriminant: &'static str,
    pub memory_discriminant: &'static str,
    pub variants: Vec<WireVariant>,
}

/// One variant of a [`WireSum`], in **declaration order** (the emitted
/// `switch` arm order).
#[derive(Debug, Clone)]
pub struct WireVariant {
    pub name: String,
    pub payload: Vec<WireField>,
}

/// Why a [`WireRef`] carries no generated codec — the runtime-owned error
/// family, which has no `TypeDecl` to derive a shape from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UncheckedReason {
    /// An `Effect[T]` reached in a field/element position (never a top-level
    /// handler return, which strips it before it gets here).
    Effect,
    ValidationError,
    JsonError,
    HttpResult,
    QueueResult,
    /// #1883, slice 1a: an `Asset` (rejected at boundaries by the checker).
    Asset,
}

/// The resolved shape of one `TypeRef` occurrence — a field's type, a sum
/// variant payload's type, a generic instantiation's argument. Mirrors the
/// exhaustive dispatch `bynk-emit`'s `emit_field_deserialise` performs today,
/// one level removed from any TS string.
#[derive(Debug, Clone)]
pub enum WireRef {
    Base {
        base: BaseType,
        json: JsonKind,
        guards: Vec<BaseGuard>,
        expected: Expected,
    },
    /// `Bytes`: base64 string on the wire, decoded (never cast) into a
    /// `Uint8Array`-shaped value.
    Bytes,
    /// A named boundary type — resolve further via the model's own
    /// `WireType` table.
    Named { name: String },
    /// A generic instantiation — `Result[A, B]`, `Option[A]`, `List[A]`,
    /// `Map[K, V]`, or a generic record/sum application (`Paginated[User]`).
    /// `key` is the same codec-suffix string in every case
    /// (`Result_Int_String`, `Paginated_User`, …) — resolve further via the
    /// model's `instantiations` list, keyed by [`WireInst::ts_name`].
    Inst { key: String },
    /// `()` — no wire content; the wire slot is `null`, the value `undefined`.
    Unit,
    /// The runtime-owned error family (plus a stray field-position `Effect`)
    /// — cast through unchecked, since there is no `TypeDecl` to derive a
    /// shape from. See [`UncheckedReason`].
    Unchecked { reason: UncheckedReason },
}

/// The set of `TypeRef`s a boundary walk resolved and their re-derived
/// structural shapes — the single source of truth both the emitter's codec
/// generation and the wire-contract peek render from.
#[derive(Debug, Clone)]
pub struct WireModel {
    pub types: Vec<WireType>,
    pub instantiations: Vec<WireInst>,
    /// The generic-record names (v0.174 #592) that are transitively
    /// self-referential and therefore have no finite monomorphised codec set
    /// — rejected at the boundary before emit, carried here as the same
    /// membership set the codec walks use to short-circuit.
    pub recursive: BTreeSet<String>,
}

// ---------------------------------------------------------------------
// Moved verbatim from `bynk-emit/src/emitter/serialisation.rs` (pure,
// AST-only walks — no TS string ever touches these). `pub(crate)` in the
// original became `pub` here; everything else is unchanged apart from the
// renames the plan calls for (`inner_ts_name` → `codec_suffix`, `app_ts_name`
// → `inst_codec_suffix`).
//
// `GenericInst` moved as `WireInst` but **keeps its original variant names**
// (`ResultInst`/`OptionInst`/…, not the plan's bare `Result`/`Option`/…):
// `bynk-emit/src/emitter/serialisation.rs`'s `emit_generic_helpers_qualified`
// — a codec-*emission* function, out of scope for this phase — pattern-matches
// those variants directly, and this phase must not touch codec-emission call
// sites.
//
// #855 (Phase 2 step 9, decided): the plan allowed renaming to the bare
// spelling here since this step touches `emit_generic_helpers_qualified`
// anyway — but the RecordInst/SumInst arms it touches already consume the
// IR's `WireField`/`WireSum` shapes (steps 7/8's byte-identical rewrite of
// `emit_record_codec`/`emit_sum_codec` covers them), and the other four arms
// (`ResultInst`/`OptionInst`/`ListInst`/`MapInst`) build their TS inline —
// they were never going to move to a `Result`/`Option`/`List`/`Map` spelling
// of *this* enum either way. A pure rename would touch six match arms here
// and this module's doc/tests for zero behavioural or architectural gain, so
// the `*Inst`-suffixed names are kept. Revisit only if a future consumer
// (e.g. the Phase 3+ peek) finds the bare spelling actually reads better at
// its own call sites.
// ---------------------------------------------------------------------

/// Compute the set of type names (transitively reachable) that need
/// serialise/deserialise helpers for this context: any type used in the
/// argument or return position of a service handler exposed by this
/// context, walked through record fields, sum payloads, and the generic
/// type parameters of Result/Option/Effect.
pub fn collect_boundary_types(
    types: &HashMap<String, Arc<TypeDecl>>,
    services: &HashMap<String, ServiceDecl>,
    // v0.96 (ADR 0124): rehydration is a trust boundary — an agent's persisted
    // `store`-field types are validated on load, so they need their deserialisers
    // emitted. Register every store field's kind-argument types (the element /
    // key / value types of `Cell`/`Map`/`Set`/`Cache`/`Log`).
    agents: &HashMap<String, AgentDecl>,
    // #1678: further boundary positions to root the walk at. On the `workers`
    // target an agent call crosses a Durable Object `fetch`, so the emitter
    // passes every agent handler's parameter and return types here.
    extra_roots: &[TypeRef],
) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let recursive_set = recursive_generic_names(types);
    let recursive = &recursive_set;
    for r in extra_roots {
        collect_type_names(r, &mut stack, types, recursive);
    }

    let mut svc_names: Vec<&String> = services.keys().collect();
    svc_names.sort();
    for name in svc_names {
        let service = &services[name];
        for h in &service.handlers {
            for p in &h.params {
                collect_type_names(&p.type_ref, &mut stack, types, recursive);
            }
            collect_type_names(&h.return_type, &mut stack, types, recursive);
        }
    }

    let mut agent_names: Vec<&String> = agents.keys().collect();
    agent_names.sort();
    for name in agent_names {
        for f in &agents[name].store_fields {
            for arg in &f.kind.args {
                collect_type_names(arg, &mut stack, types, recursive);
            }
        }
        // State-migrations S2 (#2224): a `migrate` step decodes stored bytes
        // through its old record type, so that type needs a deserialiser too.
        for m in &agents[name].migrations {
            if let Some(old) = &m.old {
                collect_type_names(&old.type_ref, &mut stack, types, recursive);
            }
        }
    }

    while let Some(name) = stack.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        out.push(name.clone());
        let Some(decl) = types.get(&name) else {
            continue;
        };
        match &decl.body {
            TypeBody::Record(r) => {
                for f in &r.fields {
                    collect_type_names(&f.type_ref, &mut stack, types, recursive);
                }
            }
            TypeBody::Sum(s) => {
                for v in &s.variants {
                    for p in &v.payload {
                        collect_type_names(&p.type_ref, &mut stack, types, recursive);
                    }
                }
            }
            TypeBody::Refined { .. } | TypeBody::Opaque { .. } => {}
        }
    }

    out.sort();
    out
}

/// v0.174 (#592): the set of generic-record names that are *recursive* — they
/// transitively contain themselves, so they have no finite monomorphised codec
/// (rejected at the boundary by the checker before emit). Precomputed once per
/// collector so the per-`App` guard in the codec walks is an O(1) membership test
/// rather than a fresh graph reachability walk at every occurrence.
fn recursive_generic_names(
    types: &HashMap<String, Arc<TypeDecl>>,
) -> std::collections::HashSet<String> {
    types
        .iter()
        .filter(|(_, d)| !d.type_params.is_empty())
        .map(|(n, _)| n.clone())
        .filter(|n| generic_record_is_recursive(n, types))
        .collect()
}

fn collect_type_names(
    t: &TypeRef,
    stack: &mut Vec<String>,
    types: &HashMap<String, Arc<TypeDecl>>,
    recursive: &std::collections::HashSet<String>,
) {
    match t {
        TypeRef::Named(id) => stack.push(id.name.clone()),
        // Query/Stream/Connection types carry no boundary-collectable user
        // types (non-boundary).
        TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..) => {}
        // v0.174 (#592): a generic-record instantiation is boundary-serialisable
        // through its monomorphised codec (`serialise_Paginated_User`). The
        // *named* helpers that codec calls come from its concrete field types —
        // the type arguments (`User`) and any non-parameter named field types
        // (`Envelope[T] = { meta: Metadata, … }`) — so walk the substituted
        // fields. A *recursive* generic record has no finite codec set and is
        // rejected at the boundary before emit; the guard here is defence in
        // depth so this walk can never fail to terminate.
        TypeRef::App { name, args, .. } => {
            if recursive.contains(&name.name) {
                return;
            }
            // #593: a generic-sum instantiation's codec (`serialise_ApiResult_User`)
            // likewise calls the named helpers of its *substituted variant
            // payloads* (`serialise_User` for `Loaded(value: T)` at `T = User`),
            // so walk those the same way records walk their fields.
            if let Some(fields) = record_inst_fields(&name.name, args, types) {
                for (_, ft) in &fields {
                    collect_type_names(ft, stack, types, recursive);
                }
            } else if let Some(variants) = sum_inst_variants(&name.name, args, types) {
                for (_, payload) in &variants {
                    for (_, ft) in payload {
                        collect_type_names(ft, stack, types, recursive);
                    }
                }
            }
        }
        // v0.20a: function types carry no user-named types to collect and are
        // rejected at boundaries anyway.
        TypeRef::Fn(..) => {}
        TypeRef::Result(a, b, _) => {
            collect_type_names(a, stack, types, recursive);
            collect_type_names(b, stack, types, recursive);
        }
        TypeRef::Option(a, _) => collect_type_names(a, stack, types, recursive),
        TypeRef::Effect(a, _) => collect_type_names(a, stack, types, recursive),
        TypeRef::HttpResult(a, _) => collect_type_names(a, stack, types, recursive),
        // v0.20b: collections serialise element-/entry-wise; their inner
        // named types need helpers.
        TypeRef::List(a, _) => collect_type_names(a, stack, types, recursive),
        TypeRef::Map(k, v, _) => {
            collect_type_names(k, stack, types, recursive);
            collect_type_names(v, stack, types, recursive);
        }
        TypeRef::Base(_, _)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::ValidationError(_)
        | TypeRef::JsonError(_)
        | TypeRef::Unit(_) => {}
    }
}

/// v0.174 (#592): substitute a generic record's declared field type — replacing
/// each type-parameter name with the concrete argument type-ref — so a
/// per-instantiation codec sees fully concrete field types.
/// `Paginated[User]`'s `items: List[T]` becomes `items: List[User]`.
fn subst_type_ref(t: &TypeRef, subst: &HashMap<String, TypeRef>) -> TypeRef {
    match t {
        TypeRef::Named(id) => match subst.get(&id.name) {
            Some(replacement) => replacement.clone(),
            None => t.clone(),
        },
        TypeRef::App { name, args, span } => TypeRef::App {
            name: name.clone(),
            args: args.iter().map(|a| subst_type_ref(a, subst)).collect(),
            span: *span,
        },
        TypeRef::Result(a, b, s) => TypeRef::Result(
            Box::new(subst_type_ref(a, subst)),
            Box::new(subst_type_ref(b, subst)),
            *s,
        ),
        TypeRef::Option(a, s) => TypeRef::Option(Box::new(subst_type_ref(a, subst)), *s),
        TypeRef::Effect(a, s) => TypeRef::Effect(Box::new(subst_type_ref(a, subst)), *s),
        TypeRef::HttpResult(a, s) => TypeRef::HttpResult(Box::new(subst_type_ref(a, subst)), *s),
        TypeRef::List(a, s) => TypeRef::List(Box::new(subst_type_ref(a, subst)), *s),
        TypeRef::Map(k, v, s) => TypeRef::Map(
            Box::new(subst_type_ref(k, subst)),
            Box::new(subst_type_ref(v, subst)),
            *s,
        ),
        TypeRef::Query(a, s) => TypeRef::Query(Box::new(subst_type_ref(a, subst)), *s),
        TypeRef::Stream(a, s) => TypeRef::Stream(Box::new(subst_type_ref(a, subst)), *s),
        TypeRef::Connection(a, s) => TypeRef::Connection(Box::new(subst_type_ref(a, subst)), *s),
        TypeRef::History(a, s) => TypeRef::History(Box::new(subst_type_ref(a, subst)), *s),
        TypeRef::Fn(ps, r, s) => TypeRef::Fn(
            ps.iter().map(|p| subst_type_ref(p, subst)).collect(),
            Box::new(subst_type_ref(r, subst)),
            *s,
        ),
        TypeRef::Base(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::ValidationError(_)
        | TypeRef::JsonError(_)
        | TypeRef::Unit(_) => t.clone(),
    }
}

/// #1846: `t` with each type `renames` names renamed, wherever it is named:
/// a bare name, and the base of a generic application (`Envelope[Int]`),
/// which `subst_type_ref` leaves alone because a type parameter is never
/// applied. `bynk-emit` renames a consumed context's unexported types with it.
pub fn rename_type_ref(t: &TypeRef, renames: &HashMap<String, String>) -> TypeRef {
    match t {
        TypeRef::Named(id) => match renames.get(&id.name) {
            Some(to) => TypeRef::Named(Ident {
                name: to.clone(),
                span: id.span,
            }),
            None => t.clone(),
        },
        TypeRef::App { name, args, span } => TypeRef::App {
            name: match renames.get(&name.name) {
                Some(to) => Ident {
                    name: to.clone(),
                    span: name.span,
                },
                None => name.clone(),
            },
            args: args.iter().map(|a| rename_type_ref(a, renames)).collect(),
            span: *span,
        },
        TypeRef::Result(a, b, s) => TypeRef::Result(
            Box::new(rename_type_ref(a, renames)),
            Box::new(rename_type_ref(b, renames)),
            *s,
        ),
        TypeRef::Option(a, s) => TypeRef::Option(Box::new(rename_type_ref(a, renames)), *s),
        TypeRef::Effect(a, s) => TypeRef::Effect(Box::new(rename_type_ref(a, renames)), *s),
        TypeRef::HttpResult(a, s) => TypeRef::HttpResult(Box::new(rename_type_ref(a, renames)), *s),
        TypeRef::List(a, s) => TypeRef::List(Box::new(rename_type_ref(a, renames)), *s),
        TypeRef::Map(k, v, s) => TypeRef::Map(
            Box::new(rename_type_ref(k, renames)),
            Box::new(rename_type_ref(v, renames)),
            *s,
        ),
        TypeRef::Query(a, s) => TypeRef::Query(Box::new(rename_type_ref(a, renames)), *s),
        TypeRef::Stream(a, s) => TypeRef::Stream(Box::new(rename_type_ref(a, renames)), *s),
        TypeRef::Connection(a, s) => TypeRef::Connection(Box::new(rename_type_ref(a, renames)), *s),
        TypeRef::History(a, s) => TypeRef::History(Box::new(rename_type_ref(a, renames)), *s),
        TypeRef::Fn(ps, r, s) => TypeRef::Fn(
            ps.iter().map(|p| rename_type_ref(p, renames)).collect(),
            Box::new(rename_type_ref(r, renames)),
            *s,
        ),
        TypeRef::Base(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::ValidationError(_)
        | TypeRef::JsonError(_)
        | TypeRef::Unit(_) => t.clone(),
    }
}

/// v0.174 (#592): the concrete `(field-name, field-type)` list for a generic
/// record instantiation `Name[args…]` — the declared fields with every type
/// parameter substituted by the matching argument. Returns `None` if `name` is
/// not a declared generic record or the arity does not match (both guaranteed
/// impossible by the checker, so this is purely defensive).
pub fn record_inst_fields(
    name: &str,
    args: &[TypeRef],
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Option<Vec<(String, TypeRef)>> {
    let decl = types.get(name)?;
    let TypeBody::Record(r) = &decl.body else {
        return None;
    };
    if decl.type_params.len() != args.len() {
        return None;
    }
    let subst: HashMap<String, TypeRef> = decl
        .type_params
        .iter()
        .map(|p| p.name.name.clone())
        .zip(args.iter().cloned())
        .collect();
    Some(
        r.fields
            .iter()
            .map(|f| (f.name.name.clone(), subst_type_ref(&f.type_ref, &subst)))
            .collect(),
    )
}

/// #593: the concrete `(variant-name, [(field-name, field-type)])` list for a
/// generic sum instantiation `Name[args…]` — the declared variants with every
/// type parameter substituted by the matching argument. The sum analogue of
/// [`record_inst_fields`]; `None` (defensively) if `name` is not a declared
/// generic sum or the arity does not match.
#[allow(clippy::type_complexity)]
pub fn sum_inst_variants(
    name: &str,
    args: &[TypeRef],
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Option<Vec<(String, Vec<(String, TypeRef)>)>> {
    let decl = types.get(name)?;
    let TypeBody::Sum(s) = &decl.body else {
        return None;
    };
    if decl.type_params.len() != args.len() {
        return None;
    }
    let subst: HashMap<String, TypeRef> = decl
        .type_params
        .iter()
        .map(|p| p.name.name.clone())
        .zip(args.iter().cloned())
        .collect();
    Some(
        s.variants
            .iter()
            .map(|v| {
                (
                    v.name.name.clone(),
                    v.payload
                        .iter()
                        .map(|f| (f.name.name.clone(), subst_type_ref(&f.type_ref, &subst)))
                        .collect(),
                )
            })
            .collect(),
    )
}

/// v0.174 (#592): the monomorphised codec suffix for a generic-record
/// instantiation — `Paginated[User]` → `Paginated_User`,
/// `Pair[User, String]` → `Pair_User_String`. #593: shared with generic sums.
/// Renamed from `app_ts_name` in the move from `bynk-emit`.
pub fn inst_codec_suffix(name: &str, args: &[TypeRef]) -> String {
    let mut s = name.to_string();
    for a in args {
        s.push('_');
        s.push_str(&codec_suffix(a));
    }
    s
}

/// The codec-name suffix a `TypeRef` resolves to — `Int`, `Order`,
/// `Result_Int_String`, `Paginated_User`. Renamed from `inner_ts_name` in the
/// move from `bynk-emit`; used both to key a [`WireInst`] and, unqualified,
/// as the bare codec function suffix a same-module call reaches.
pub fn codec_suffix(t: &TypeRef) -> String {
    match t {
        TypeRef::Base(b, _) => b.name().to_string(),
        // v0.20a: function types are confined to non-boundary positions
        // (`bynk.types.function_at_boundary`), so the serialisation machinery
        // can never legally see one.
        TypeRef::Fn(..)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..) => {
            unreachable!("function/query/stream types are rejected at boundaries")
        }
        // v0.174 (#592): the codec suffix for a generic-record instantiation —
        // `Paginated[User]` → `Paginated_User`.
        TypeRef::App { name, args, .. } => inst_codec_suffix(&name.name, args),
        TypeRef::Named(id) => id.name.clone(),
        TypeRef::Result(a, b, _) => format!("Result_{}_{}", codec_suffix(a), codec_suffix(b)),
        TypeRef::Option(a, _) => format!("Option_{}", codec_suffix(a)),
        TypeRef::Effect(a, _) => format!("Effect_{}", codec_suffix(a)),
        TypeRef::HttpResult(a, _) => format!("HttpResult_{}", codec_suffix(a)),
        TypeRef::List(a, _) => format!("List_{}", codec_suffix(a)),
        TypeRef::Map(k, v, _) => format!("Map_{}_{}", codec_suffix(k), codec_suffix(v)),
        TypeRef::QueueResult(_) => "QueueResult".to_string(),
        TypeRef::Asset(_) => "Asset".to_string(),
        TypeRef::RequestHeaders(_) => "RequestHeaders".to_string(),
        TypeRef::BoundaryRejection(_) => "BoundaryRejection".to_string(),
        TypeRef::ValidationError(_) => "ValidationError".to_string(),
        TypeRef::JsonError(_) => "JsonError".to_string(),
        TypeRef::Unit(_) => "Unit".to_string(),
    }
}

/// v0.22b: the codec closure for a set of `Json.encode`/`Json.decode[T]`
/// target type-refs — the named types needing per-type helpers (transitively
/// through record fields and sum payloads) plus the generic instantiations
/// needing specialised helpers. The same closure logic as the boundary
/// collectors, rooted at expressions instead of service signatures.
pub fn collect_codec_closure(
    roots: &[TypeRef],
    types: &HashMap<String, Arc<TypeDecl>>,
) -> (Vec<String>, Vec<WireInst>) {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut names: Vec<String> = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let recursive_set = recursive_generic_names(types);
    let recursive = &recursive_set;
    for r in roots {
        collect_type_names(r, &mut stack, types, recursive);
    }
    while let Some(name) = stack.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        names.push(name.clone());
        let Some(decl) = types.get(&name) else {
            continue;
        };
        match &decl.body {
            TypeBody::Record(r) => {
                for f in &r.fields {
                    collect_type_names(&f.type_ref, &mut stack, types, recursive);
                }
            }
            TypeBody::Sum(s) => {
                for v in &s.variants {
                    for p in &v.payload {
                        collect_type_names(&p.type_ref, &mut stack, types, recursive);
                    }
                }
            }
            TypeBody::Refined { .. } | TypeBody::Opaque { .. } => {}
        }
    }
    names.sort();

    let mut insts: Vec<WireInst> = Vec::new();
    let mut inst_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for r in roots {
        walk_generic_inst(r, &mut insts, &mut inst_seen, types, recursive);
    }
    for name in &names {
        let Some(decl) = types.get(name) else {
            continue;
        };
        match &decl.body {
            TypeBody::Record(r) => {
                for f in &r.fields {
                    walk_generic_inst(&f.type_ref, &mut insts, &mut inst_seen, types, recursive);
                }
            }
            TypeBody::Sum(s) => {
                for v in &s.variants {
                    for p in &v.payload {
                        walk_generic_inst(
                            &p.type_ref,
                            &mut insts,
                            &mut inst_seen,
                            types,
                            recursive,
                        );
                    }
                }
            }
            TypeBody::Refined { .. } | TypeBody::Opaque { .. } => {}
        }
    }
    (names, insts)
}

/// Collect the set of `Result<A, B>` / `Option<A>` instantiations used in
/// boundary positions so the emitter can synthesise the specialised
/// helpers. v0.18: an instantiation may also appear in the *fields* of a
/// boundary record or sum payload (e.g. the bynk surface's
/// `Request.contentType: Option[String]`) — the per-type serialisers
/// delegate to the specialised generic helpers, so walk those too.
pub fn collect_generic_instantiations(
    services: &HashMap<String, ServiceDecl>,
    // v0.96 (ADR 0124): an agent's `store`-field element types are validated on
    // rehydration, so a `Cell[Option[Int]]` / `Log[List[T]]` needs its specialised
    // generic helper emitted just like a boundary signature does.
    agents: &HashMap<String, AgentDecl>,
    boundary_type_names: &[String],
    types: &HashMap<String, Arc<TypeDecl>>,
    // #1678: further boundary positions, as for `collect_boundary_types`. Walked
    // after the services and agent stores, so existing emission order is kept.
    extra_roots: &[TypeRef],
) -> Vec<WireInst> {
    let mut out: Vec<WireInst> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let recursive_set = recursive_generic_names(types);
    let recursive = &recursive_set;
    // Iterate services in name order: `HashMap::values()` order varies per
    // process, and the *emission order* of the specialised helpers follows
    // first-encounter order here. Surfaced by the first fixture with
    // multiple same-file services carrying different instantiations (v0.23
    // #35 CI); latent since v0.8.
    let mut svc_names: Vec<&String> = services.keys().collect();
    svc_names.sort();
    for name in svc_names {
        let s = &services[name];
        for h in &s.handlers {
            for p in &h.params {
                walk_generic_inst(&p.type_ref, &mut out, &mut seen, types, recursive);
            }
            walk_generic_inst(&h.return_type, &mut out, &mut seen, types, recursive);
        }
    }
    let mut agent_names: Vec<&String> = agents.keys().collect();
    agent_names.sort();
    for name in agent_names {
        for f in &agents[name].store_fields {
            for arg in &f.kind.args {
                walk_generic_inst(arg, &mut out, &mut seen, types, recursive);
            }
        }
        for m in &agents[name].migrations {
            if let Some(old) = &m.old {
                walk_generic_inst(&old.type_ref, &mut out, &mut seen, types, recursive);
            }
        }
    }
    for r in extra_roots {
        walk_generic_inst(r, &mut out, &mut seen, types, recursive);
    }
    for name in boundary_type_names {
        let Some(decl) = types.get(name) else {
            continue;
        };
        // v0.174 (#592): never walk a *generic* declaration's own fields — they
        // are the declared, unsubstituted body (`Paginated[T] = { items: List[T]
        // }`), so walking `List[T]` would emit a bogus `serialise_List_T` over the
        // unbound type variable `T`. The instantiations a generic record needs
        // come from its *use* sites (`Paginated[User]`, a `TypeRef::App`), which
        // `walk_generic_inst` expands with concrete arguments. This mirrors the
        // `emit_helpers_for_owner` skip that keeps a bare `serialise_Paginated`
        // from being emitted.
        if !decl.type_params.is_empty() {
            continue;
        }
        match &decl.body {
            TypeBody::Record(r) => {
                for f in &r.fields {
                    walk_generic_inst(&f.type_ref, &mut out, &mut seen, types, recursive);
                }
            }
            TypeBody::Sum(s) => {
                for v in &s.variants {
                    for p in &v.payload {
                        walk_generic_inst(&p.type_ref, &mut out, &mut seen, types, recursive);
                    }
                }
            }
            TypeBody::Refined { .. } | TypeBody::Opaque { .. } => {}
        }
    }
    out
}

/// A boundary occurrence of a generic instantiation needing its own
/// specialised codec — `Result<A, B>`, `Option<A>`, `List<A>`, `Map<K, V>`,
/// or a generic user record/sum applied to concrete arguments
/// (`Paginated[User]`, `ApiResult[User]`).
///
/// Moved from `bynk-emit/src/emitter/serialisation.rs`'s `GenericInst` as
/// part of the #855 IR extraction. Variant names are kept exactly as they
/// were (`ResultInst`, not the plan's bare `Result`, …): `bynk-emit`'s
/// `emit_generic_helpers_qualified` pattern-matches these variants directly.
/// Phase 2 step 9 ("Generic helpers") revisited the plan's bare
/// `Result`/`Option`/`List`/`Map`/`Record`/`Sum` spelling and **kept** the
/// `*Inst` suffix — see the "Moved verbatim" comment block above this
/// module's walk functions for the reasoning (a pure rename of six match
/// arms with no behavioural or architectural payoff).
#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
pub enum WireInst {
    ResultInst {
        ok: TypeRef,
        err: TypeRef,
    },
    OptionInst {
        inner: TypeRef,
    },
    /// v0.20b: a `List[T]` boundary instantiation — element-wise wire format.
    ListInst {
        elem: TypeRef,
    },
    /// v0.20b: a `Map[K, V]` boundary instantiation — entries-array wire
    /// format (`[[k, v], …]`), insertion-ordered.
    MapInst {
        key: TypeRef,
        val: TypeRef,
    },
    /// v0.174 (#592): a generic user-record instantiation `Name[args…]` — a
    /// monomorphised per-instantiation record codec (`serialise_Paginated_User`)
    /// specialised to the concrete arguments (ADR 0183 Decision C's follow-on).
    RecordInst {
        name: String,
        args: Vec<TypeRef>,
    },
    /// #593: a generic user-sum instantiation `Name[args…]` — a monomorphised
    /// per-instantiation discriminated-union codec (`serialise_ApiResult_User`),
    /// the sum analogue of [`WireInst::RecordInst`].
    SumInst {
        name: String,
        args: Vec<TypeRef>,
    },
}

impl WireInst {
    pub fn ts_name(&self) -> String {
        match self {
            WireInst::ResultInst { ok, err } => {
                format!("Result_{}_{}", codec_suffix(ok), codec_suffix(err))
            }
            WireInst::OptionInst { inner } => {
                format!("Option_{}", codec_suffix(inner))
            }
            WireInst::ListInst { elem } => format!("List_{}", codec_suffix(elem)),
            WireInst::MapInst { key, val } => {
                format!("Map_{}_{}", codec_suffix(key), codec_suffix(val))
            }
            WireInst::RecordInst { name, args } => inst_codec_suffix(name, args),
            WireInst::SumInst { name, args } => inst_codec_suffix(name, args),
        }
    }
}

fn walk_generic_inst(
    t: &TypeRef,
    out: &mut Vec<WireInst>,
    seen: &mut std::collections::HashSet<String>,
    types: &HashMap<String, Arc<TypeDecl>>,
    recursive: &std::collections::HashSet<String>,
) {
    match t {
        // v0.174 (#592): a generic-record instantiation needs a monomorphised
        // codec, and so do the generic instantiations reachable through its
        // concrete field types (`Paginated[User]` → `List[User]` →
        // `serialise_List_User`, `Envelope[Box[User]]` → `Box[User]` →
        // `serialise_Box_User`). Substitute the fields and walk them. A recursive
        // generic record (no finite codec set) is rejected at the boundary before
        // emit; the guard here is defence in depth so this walk always terminates
        // (the `seen` dedup alone cannot bound *polymorphic* recursion, whose
        // instantiations each carry a distinct name).
        TypeRef::App { name, args, .. } => {
            if recursive.contains(&name.name) {
                return;
            }
            // #593: an `App` names a generic record OR a generic sum; dispatch on
            // the declaration's body so the right monomorphised codec is emitted,
            // and walk the reachable instantiations through its concrete member
            // types (a sum's variant payloads, a record's fields).
            let is_sum = matches!(
                types.get(&name.name).map(|d| &d.body),
                Some(TypeBody::Sum(_))
            );
            let inst = if is_sum {
                WireInst::SumInst {
                    name: name.name.clone(),
                    args: args.clone(),
                }
            } else {
                WireInst::RecordInst {
                    name: name.name.clone(),
                    args: args.clone(),
                }
            };
            let key = inst.ts_name();
            if !seen.insert(key) {
                return;
            }
            out.push(inst);
            for a in args {
                walk_generic_inst(a, out, seen, types, recursive);
            }
            if is_sum {
                if let Some(variants) = sum_inst_variants(&name.name, args, types) {
                    for (_, payload) in &variants {
                        for (_, ft) in payload {
                            walk_generic_inst(ft, out, seen, types, recursive);
                        }
                    }
                }
            } else if let Some(fields) = record_inst_fields(&name.name, args, types) {
                for (_, ft) in &fields {
                    walk_generic_inst(ft, out, seen, types, recursive);
                }
            }
        }
        TypeRef::Result(a, b, _) => {
            let inst = WireInst::ResultInst {
                ok: (**a).clone(),
                err: (**b).clone(),
            };
            let key = inst.ts_name();
            if seen.insert(key) {
                out.push(inst);
            }
            walk_generic_inst(a, out, seen, types, recursive);
            walk_generic_inst(b, out, seen, types, recursive);
        }
        TypeRef::Option(a, _) => {
            let inst = WireInst::OptionInst {
                inner: (**a).clone(),
            };
            let key = inst.ts_name();
            if seen.insert(key) {
                out.push(inst);
            }
            walk_generic_inst(a, out, seen, types, recursive);
        }
        TypeRef::Effect(a, _) => walk_generic_inst(a, out, seen, types, recursive),
        TypeRef::HttpResult(a, _) => walk_generic_inst(a, out, seen, types, recursive),
        TypeRef::List(a, _) => {
            let inst = WireInst::ListInst {
                elem: (**a).clone(),
            };
            let key = inst.ts_name();
            if seen.insert(key) {
                out.push(inst);
            }
            walk_generic_inst(a, out, seen, types, recursive);
        }
        TypeRef::Map(k, v, _) => {
            let inst = WireInst::MapInst {
                key: (**k).clone(),
                val: (**v).clone(),
            };
            let key = inst.ts_name();
            if seen.insert(key) {
                out.push(inst);
            }
            walk_generic_inst(k, out, seen, types, recursive);
            walk_generic_inst(v, out, seen, types, recursive);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------
// New: resolving a `TypeRef` / `TypeDecl` into the IR above. Not yet called
// by any codec path — `bynk-emit` still derives its own shape inline
// (Phase 2 switches it over). Exercised only by this module's own tests
// until then.
// ---------------------------------------------------------------------

/// The base-type guards a bare **field-position** value (a record field, a
/// sum-variant payload field, a collection element) is checked against,
/// mirroring `emit_field_deserialise`'s `TypeRef::Base` arm: `Int`/`Duration`/
/// `Instant` must be whole, `Float` must be finite. #1969: `Duration` joined the
/// first group; a top-level `Duration` was already guarded, a field was not. Contrast [`WireScalar::base_guards`]
/// (a **named-type** consumed-inline revalidation), which guards `Int`/`Float`
/// only — the two positions are not the same check in the code this was
/// extracted from, and that asymmetry is preserved rather than merged.
pub fn field_base_guards(b: BaseType) -> Vec<BaseGuard> {
    match b {
        BaseType::Int | BaseType::Duration | BaseType::Instant => vec![BaseGuard::Integral],
        BaseType::Float => vec![BaseGuard::Finite],
        _ => Vec::new(),
    }
}

/// The base-type guards a named type's `Inline` (consumed, transparent)
/// revalidation applies, mirroring `emit_inline_refinement_checks` exactly:
/// `Int` → `Integral`, `Float` → `Finite`. See [`WireScalar::base_guards`]'s
/// doc for why `Instant` is deliberately absent here despite being guarded
/// at field position.
fn scalar_base_guards(b: BaseType) -> Vec<BaseGuard> {
    match b {
        BaseType::Int => vec![BaseGuard::Integral],
        BaseType::Float => vec![BaseGuard::Finite],
        _ => Vec::new(),
    }
}

/// Resolve one `TypeRef` occurrence (a field's type, a variant payload
/// field's type, a generic argument) into its [`WireRef`] shape — the same
/// dispatch `emit_field_deserialise` performs today, one level removed from
/// any TS string.
///
/// `types` is threaded through for signature symmetry with the walks above
/// and forward compatibility with a future disambiguation need; this
/// resolution is single-level (unlike `collect_type_names`'s transitive
/// walk) and does not currently consult it.
///
/// **This encodes the *deserialise*-side dispatch only** (mirroring
/// `emit_field_deserialise`'s arms one-for-one, including its `Effect` /
/// `HttpResult` arm, which it folds into the same unchecked cast as
/// `ValidationError`/`JsonError`/`QueueResult`). The emitter does not
/// actually agree with itself on those two shapes across its other three
/// dispatches: `serialise_field_expr_via` *recurses* through `Effect` rather
/// than casting it unchecked, `deserialise_expr_via` also recurses through
/// `Effect`, and `codec_suffix` gives both `Effect` and `HttpResult` their
/// own composed names (`Effect_<inner>`, `HttpResult_<inner>`) rather than
/// treating them as opaque. A future serialise-side or codec-suffix-side
/// `WireRef` derivation must **not** assume this function's `Unchecked`
/// answer already covers it — that would silently change `Effect`-at-field-
/// position from "recurse into the inner type" to "cast unchecked" the
/// moment Phase 2 routes the serialise path through this same resolver.
/// Reconciling (or deliberately keeping two resolvers) is a Phase 2 decision,
/// not implied by this function's existence.
pub fn wire_ref(t: &TypeRef, _types: &HashMap<String, Arc<TypeDecl>>) -> WireRef {
    match t {
        // v0.20a: function types are confined to non-boundary positions
        // (`bynk.types.function_at_boundary`), so the serialisation machinery
        // can never legally see one — mirrors `emit_field_deserialise`'s
        // `unreachable!` on this same arm.
        TypeRef::Fn(..)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..) => {
            unreachable!(
                "function/query/stream/connection/history types are rejected at boundaries"
            )
        }
        // v0.110 (ADR 0142 D5): a bare `Bytes` field is a base64 JSON string,
        // decoded (not cast) into a `Uint8Array`.
        TypeRef::Base(BaseType::Bytes, _) => WireRef::Bytes,
        TypeRef::Base(b, _) => {
            let json = json_kind_of(*b);
            WireRef::Base {
                base: *b,
                json,
                guards: field_base_guards(*b),
                expected: Expected::Json(json),
            }
        }
        TypeRef::Named(id) => WireRef::Named {
            name: id.name.clone(),
        },
        // Every generic instantiation — App/Result/Option/List/Map — resolves
        // to the same codec-suffix key; `codec_suffix` already builds exactly
        // that string for each of these shapes.
        TypeRef::App { .. }
        | TypeRef::Result(..)
        | TypeRef::Option(..)
        | TypeRef::List(..)
        | TypeRef::Map(..) => WireRef::Inst {
            key: codec_suffix(t),
        },
        TypeRef::Effect(..) => WireRef::Unchecked {
            reason: UncheckedReason::Effect,
        },
        TypeRef::HttpResult(..) => WireRef::Unchecked {
            reason: UncheckedReason::HttpResult,
        },
        TypeRef::ValidationError(_) => WireRef::Unchecked {
            reason: UncheckedReason::ValidationError,
        },
        TypeRef::JsonError(_) => WireRef::Unchecked {
            reason: UncheckedReason::JsonError,
        },
        TypeRef::QueueResult(_) => WireRef::Unchecked {
            reason: UncheckedReason::QueueResult,
        },
        // #1883, slice 1a: never reached through a well-checked program — an
        // `Asset` at a boundary is `bynk.types.asset_at_boundary` — but the
        // model still needs a total answer.
        TypeRef::Asset(_) | TypeRef::RequestHeaders(_) | TypeRef::BoundaryRejection(_) => {
            WireRef::Unchecked {
                reason: UncheckedReason::Asset,
            }
        }
        TypeRef::Unit(_) => WireRef::Unit,
    }
}

fn wire_fields(fields: &[RecordField], types: &HashMap<String, Arc<TypeDecl>>) -> Vec<WireField> {
    fields
        .iter()
        .map(|f| WireField {
            name: f.name.name.clone(),
            shape: wire_ref(&f.type_ref, types),
            path_segment: f.name.name.clone(),
            default: f.init.as_ref().map(|e| (e.clone(), f.type_ref.clone())),
            predicates: field_predicates(f),
        })
        .collect()
}

/// #1969: a record field's inline refinement predicates, in declaration order
/// (`qty: Int where Positive` → `[Positive]`). The decoder checks them after
/// the field's own codec, so a refined field reads the same from the wire as a
/// field of a named refined type.
pub fn field_predicates(f: &RecordField) -> Vec<PredKind> {
    f.refinement
        .as_ref()
        .map(|r| r.predicates.iter().map(|p| p.kind.clone()).collect())
        .unwrap_or_default()
}

fn wire_sum(body: &SumBody, types: &HashMap<String, Arc<TypeDecl>>) -> WireSum {
    WireSum {
        wire_discriminant: WIRE_DISCRIMINANT,
        memory_discriminant: "tag",
        variants: body
            .variants
            .iter()
            .map(|v| WireVariant {
                name: v.name.name.clone(),
                payload: v
                    .payload
                    .iter()
                    .map(|f| WireField {
                        name: f.name.name.clone(),
                        shape: wire_ref(&f.type_ref, types),
                        path_segment: f.name.name.clone(),
                        // Sum-variant payload fields carry no default in the
                        // surface grammar (`VariantField` has no `init`).
                        default: None,
                        predicates: Vec::new(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// Build one [`WireScalar`], choosing [`Revalidation`] per #661 Decisions
/// C/D plus the owner path: `Owned` → `ViaConstructor`; consumed + opaque →
/// `StructuralOnly`; consumed + transparent → `Inline`; a `Bytes` base →
/// `Base64Decode` regardless of provenance, mirroring `emit_refined`'s early
/// return to the dedicated `Bytes` codec before the owned/consumed split is
/// even considered.
fn wire_scalar(
    base: BaseType,
    opaque: bool,
    refinement: Option<&Refinement>,
    prov: &Provenance,
) -> WireScalar {
    let json = json_kind_of(base);
    let predicates: Vec<PredKind> = refinement
        .map(|r| r.predicates.iter().map(|p| p.kind.clone()).collect())
        .unwrap_or_default();
    let revalidation = if base == BaseType::Bytes {
        Revalidation::Base64Decode
    } else {
        match prov {
            Provenance::Owned => Revalidation::ViaConstructor,
            Provenance::Consumed { .. } if opaque => Revalidation::StructuralOnly,
            Provenance::Consumed { .. } => Revalidation::Inline,
        }
    };
    WireScalar {
        base,
        json,
        opaque,
        predicates,
        base_guards: scalar_base_guards(base),
        revalidation,
    }
}

/// Build a [`WireType`] from a `TypeDecl`, using the same base/predicate/
/// opaque logic currently inline in `bynk-emit`'s `emit_refined` and
/// `emit_bytes_named_codec` — without any TS string emission. Returns `None`
/// for a **generic** declaration (`decl.type_params` non-empty): a generic
/// record/sum has no single bare `WireType` of its own, only per-instantiation
/// shapes (`WireInst`), mirroring the skip in `emit_helpers_for_owner_qualified`.
pub fn wire_type(
    name: &str,
    decl: &TypeDecl,
    types: &HashMap<String, Arc<TypeDecl>>,
    prov: Provenance,
) -> Option<WireType> {
    if !decl.type_params.is_empty() {
        return None;
    }
    let body = match &decl.body {
        TypeBody::Refined {
            base, refinement, ..
        } => WireBody::Scalar(wire_scalar(*base, false, refinement.as_ref(), &prov)),
        TypeBody::Opaque {
            base, refinement, ..
        } => WireBody::Scalar(wire_scalar(*base, true, refinement.as_ref(), &prov)),
        TypeBody::Record(r) => WireBody::Record {
            fields: wire_fields(&r.fields, types),
        },
        TypeBody::Sum(s) => WireBody::Sum(wire_sum(s, types)),
    };
    Some(WireType {
        name: name.to_string(),
        codec_suffix: name.to_string(),
        provenance: prov,
        body,
    })
}

/// Build the full [`WireModel`] for a boundary: every non-generic named type
/// in `type_names` resolved through `types`, plus the generic instantiations
/// the caller already collected (`collect_generic_instantiations` /
/// `collect_codec_closure`). `provenance` is supplied by the caller —
/// `bynk-emit` knows the build target and the consumed set; this crate must
/// not (see the [`Provenance`] doc).
pub fn boundary_model(
    type_names: &[String],
    types: &HashMap<String, Arc<TypeDecl>>,
    instantiations: Vec<WireInst>,
    provenance: impl Fn(&str) -> Provenance,
) -> WireModel {
    let recursive: BTreeSet<String> = recursive_generic_names(types).into_iter().collect();
    let mut wtypes = Vec::with_capacity(type_names.len());
    for name in type_names {
        let Some(decl) = types.get(name) else {
            continue;
        };
        if let Some(wt) = wire_type(name, decl, types, provenance(name)) {
            wtypes.push(wt);
        }
    }
    WireModel {
        types: wtypes,
        instantiations,
        recursive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types_of(src: &str) -> HashMap<String, Arc<TypeDecl>> {
        let tokens = bynk_syntax::lexer::tokenize(src).expect("lex");
        let commons = bynk_syntax::parser::parse(&tokens, src).expect("parse");
        commons
            .items
            .iter()
            .filter_map(|i| match i {
                CommonsItem::Type(t) => Some((t.name.name.clone(), Arc::new(t.clone()))),
                _ => None,
            })
            .collect()
    }

    /// Parse a `context` unit and split it into its `types` and `services`
    /// tables, mirroring `bynk-emit`'s own boundary-collection call sites.
    fn context_of(src: &str) -> (HashMap<String, Arc<TypeDecl>>, HashMap<String, ServiceDecl>) {
        let tokens = bynk_syntax::lexer::tokenize(src).expect("lex");
        let unit = bynk_syntax::parser::parse_unit(&tokens, src).expect("parse");
        let SourceUnit::Context(ctx) = unit else {
            panic!("expected a context unit");
        };
        let mut types = HashMap::new();
        let mut services = HashMap::new();
        for item in &ctx.items {
            match item {
                CommonsItem::Type(t) => {
                    types.insert(t.name.name.clone(), Arc::new(t.clone()));
                }
                CommonsItem::Service(s) => {
                    services.insert(s.name.name.clone(), s.clone());
                }
                _ => {}
            }
        }
        (types, services)
    }

    #[test]
    fn json_kind_of_matches_the_ts_base_for_serialisation_mapping() {
        assert_eq!(json_kind_of(BaseType::Int), JsonKind::Number);
        assert_eq!(json_kind_of(BaseType::Float), JsonKind::Number);
        assert_eq!(json_kind_of(BaseType::Duration), JsonKind::Number);
        assert_eq!(json_kind_of(BaseType::Instant), JsonKind::Number);
        assert_eq!(json_kind_of(BaseType::String), JsonKind::String);
        // v0.110 (ADR 0142 D5): a `Bytes` wires as a base64 JSON string.
        assert_eq!(json_kind_of(BaseType::Bytes), JsonKind::String);
        assert_eq!(json_kind_of(BaseType::Bool), JsonKind::Boolean);
    }

    #[test]
    fn collect_boundary_types_walks_params_return_and_record_fields() {
        let src = r#"
context test

type ClientId = String where NonEmpty
type Rate = { count: Int, limit: Int }

service Api {
  on call(client: ClientId) -> Effect[Rate] {
    client
  }
}
"#;
        let (types, services) = context_of(src);
        let agents: HashMap<String, AgentDecl> = HashMap::new();
        let names = collect_boundary_types(&types, &services, &agents, &[]);
        assert_eq!(names, vec!["ClientId".to_string(), "Rate".to_string()]);
    }

    #[test]
    fn collect_boundary_types_ignores_types_unreachable_from_any_handler() {
        let src = r#"
context test

type Used = String where NonEmpty
type Unused = { n: Int }

service Api {
  on call(x: Used) -> Effect[Used] {
    x
  }
}
"#;
        let (types, services) = context_of(src);
        let agents: HashMap<String, AgentDecl> = HashMap::new();
        let names = collect_boundary_types(&types, &services, &agents, &[]);
        assert_eq!(names, vec!["Used".to_string()]);
    }

    /// The highest-risk drift trap the plan flags: `contract.rs` sorts
    /// predicates (a precondition for hash correctness), but this IR must
    /// carry **declaration order** — `Inline` revalidation emits one `if`
    /// per predicate in source order. `NonEmpty` before `MaxLength` sorts
    /// (alphabetically, as `contract.rs`'s `canon_refinement` would — folding
    /// `NonEmpty` to `MinLength(1)` there does not change this: `MaxLength` <
    /// `MinLength` alphabetically, same relative order as `MaxLength` <
    /// `NonEmpty` before the fold) to `MaxLength` first, so this fixture's
    /// declaration order visibly differs from the sorted order — a positive
    /// assertion that the trap stays closed.
    #[test]
    fn wire_scalar_predicates_preserve_declaration_order_not_sorted() {
        let types = types_of("commons x\n\ntype Id = String where NonEmpty && MaxLength(20)\n");
        let decl = &types["Id"];
        let wt = wire_type("Id", decl, &types, Provenance::Owned).expect("scalar wire type");
        let WireBody::Scalar(scalar) = &wt.body else {
            panic!("expected a scalar body, got {:?}", wt.body);
        };
        assert_eq!(scalar.predicates.len(), 2);
        assert!(
            matches!(scalar.predicates[0], PredKind::NonEmpty),
            "predicates[0] must be the declared-first NonEmpty, got {:?}",
            scalar.predicates[0]
        );
        assert!(
            matches!(scalar.predicates[1], PredKind::MaxLength(20)),
            "predicates[1] must be the declared-second MaxLength(20), got {:?}",
            scalar.predicates[1]
        );
        // The sorted (contract.rs-style) order would put MaxLength first —
        // assert this IR does not match that order.
        assert!(
            !matches!(scalar.predicates[0], PredKind::MaxLength(_)),
            "predicates must be in declaration order, not sorted"
        );
    }

    #[test]
    fn wire_type_selects_revalidation_by_provenance_and_opacity() {
        let types = types_of(
            "commons x\n\ntype Refined = String where NonEmpty\ntype Opaque = opaque String where NonEmpty\ntype Raw = Bytes\n",
        );

        let refined = wire_type("Refined", &types["Refined"], &types, Provenance::Owned)
            .expect("owned scalar");
        let WireBody::Scalar(s) = &refined.body else {
            panic!("expected scalar")
        };
        assert_eq!(s.revalidation, Revalidation::ViaConstructor);

        let consumed = Provenance::Consumed {
            owner_unit: "other".to_string(),
        };
        let refined_consumed =
            wire_type("Refined", &types["Refined"], &types, consumed.clone()).expect("consumed");
        let WireBody::Scalar(s) = &refined_consumed.body else {
            panic!("expected scalar")
        };
        assert_eq!(s.revalidation, Revalidation::Inline);

        let opaque_consumed =
            wire_type("Opaque", &types["Opaque"], &types, consumed.clone()).expect("consumed");
        let WireBody::Scalar(s) = &opaque_consumed.body else {
            panic!("expected scalar")
        };
        assert_eq!(s.revalidation, Revalidation::StructuralOnly);

        // Bytes: Base64Decode regardless of provenance.
        let raw_owned =
            wire_type("Raw", &types["Raw"], &types, Provenance::Owned).expect("owned bytes");
        let WireBody::Scalar(s) = &raw_owned.body else {
            panic!("expected scalar")
        };
        assert_eq!(s.revalidation, Revalidation::Base64Decode);
        let raw_consumed = wire_type("Raw", &types["Raw"], &types, consumed).expect("consumed");
        let WireBody::Scalar(s) = &raw_consumed.body else {
            panic!("expected scalar")
        };
        assert_eq!(s.revalidation, Revalidation::Base64Decode);
    }

    #[test]
    fn wire_type_returns_none_for_a_generic_declaration() {
        let types = types_of("commons x\n\ntype Page[T] = { items: List[T] }\n");
        assert!(wire_type("Page", &types["Page"], &types, Provenance::Owned).is_none());
    }

    /// Part 1.4's cross-check: for an `on call` handler, `wire.rs`'s codec
    /// walk (`collect_boundary_types`, restricted to that one handler) and
    /// `contract.rs`'s canonical hash form (`service_normal_form`, over the
    /// same handler projected as a `CrossContextService`) must reach the
    /// exact same *set* of named boundary types — even though the module
    /// doc's comparison table says the two disagree on purpose about
    /// *ordering* (declaration order vs sorted) and about whether an opaque
    /// predicate is shown. This test asserts REACHABILITY only: the set of
    /// type names each derivation's walk visits from the same root, not any
    /// string equality or order between the two renderings.
    ///
    /// The type names in the fixture are chosen so none is a substring of
    /// another (`ClientId`, `Address`, `Profile`, `Unrelated`) —
    /// `contract.rs`'s `canon_type`/`canon_named_in` are private to that
    /// module, so rather than duplicating their traversal here, this test
    /// recovers the set of types `service_normal_form`'s rendered string
    /// reaches by substring containment against every declared type name.
    /// With non-overlapping names that containment test is unambiguous.
    ///
    /// `Unrelated` is declared but reachable from no handler, so the
    /// equality assertion below is not vacuously true merely because this
    /// fixture happens to make every *other* declared type reachable — a
    /// regression that made `collect_boundary_types` return every declared
    /// name instead of the reachable closure would leak `Unrelated` into
    /// `boundary_names`, and this test would catch it.
    #[test]
    fn boundary_reachability_agrees_with_contract_normal_form() {
        let src = r#"
context test

type ClientId = String where NonEmpty
type Address = { street: String, city: String }
type Profile = { id: ClientId, home: Address }

// Declared, but reachable from no handler below (see the doc comment on
// this test for why).
type Unrelated = { n: Int }

service Api {
  on call(client: ClientId) -> Effect[Profile] {
    Profile { id: client, home: Address { street: "x", city: "y" } }
  }
}
"#;
        let (types, services) = context_of(src);
        let service = &services["Api"];
        let handler = &service.handlers[0];

        // Restrict `collect_boundary_types`'s walk to exactly this handler —
        // the same synthetic-single-handler-service narrowing
        // `bynk-ide`'s `wire_contract_for_service` performs, so a service
        // with more than one handler could not leak an unrelated handler's
        // types into either side of this comparison.
        let mut narrowed = service.clone();
        narrowed.handlers = vec![handler.clone()];
        let narrowed_services: HashMap<String, ServiceDecl> =
            HashMap::from([("Api".to_string(), narrowed)]);
        let agents: HashMap<String, AgentDecl> = HashMap::new();
        let boundary_names: std::collections::HashSet<String> =
            collect_boundary_types(&types, &narrowed_services, &agents, &[])
                .into_iter()
                .collect();

        // The same handler, projected as a `CrossContextService` exactly as
        // `bynk-emit`'s `own_contract_hashes` / `bynk-ide`'s
        // `wire_contract::contract_form` project it, canonicalised through
        // the same type table.
        let svc = crate::resolver::CrossContextService {
            name: "Api".to_string(),
            params: handler
                .params
                .iter()
                .map(|p| (p.name.name.clone(), p.type_ref.clone()))
                .collect(),
            return_type: handler.return_type.clone(),
            span: handler.span,
            foreign_callers: Vec::new(),
        };
        let normal_form = crate::contract::service_normal_form(&svc, &types);

        let contract_reachable: std::collections::HashSet<String> = types
            .keys()
            .filter(|name| normal_form.contains(name.as_str()))
            .cloned()
            .collect();

        assert_eq!(
            boundary_names, contract_reachable,
            "wire::collect_boundary_types and contract::service_normal_form must \
             agree on *which* named types a handler's contract reaches, even though \
             they render/order that set completely differently (see this module's \
             doc comparison table):\n\
             wire.rs reached:     {boundary_names:?}\n\
             contract.rs reached: {contract_reachable:?}\n\
             normal form:         {normal_form:?}"
        );
        assert!(
            !boundary_names.contains("Unrelated") && !contract_reachable.contains("Unrelated"),
            "`Unrelated` is declared but reachable from no handler — both derivations \
             must exclude it, not merely agree on every declared type by coincidence:\n\
             wire.rs reached: {boundary_names:?}\n\
             contract.rs reached: {contract_reachable:?}"
        );
    }

    /// `wire_ref` is the plan's flagged highest-risk function ("its two
    /// functions must stay in exact agreement about which `WireRef` arm a
    /// `TypeRef` resolves to"). Pin the arm each shape lands on before Phase 2
    /// starts consuming it.
    #[test]
    fn wire_ref_dispatches_every_type_ref_shape_to_the_expected_arm() {
        let types: HashMap<String, Arc<TypeDecl>> = HashMap::new();
        let sp = || bynk_syntax::span::Span::new(0, 0);
        let named = |n: &str| {
            TypeRef::Named(Ident {
                name: n.to_string(),
                span: sp(),
            })
        };
        let base = |b: BaseType| TypeRef::Base(b, sp());

        assert!(matches!(
            wire_ref(&base(BaseType::Int), &types),
            WireRef::Base {
                base: BaseType::Int,
                json: JsonKind::Number,
                ..
            }
        ));
        assert!(matches!(
            wire_ref(&base(BaseType::Bytes), &types),
            WireRef::Bytes
        ));
        assert!(
            matches!(wire_ref(&named("ClientId"), &types), WireRef::Named { name } if name == "ClientId")
        );
        assert!(matches!(
            wire_ref(
                &TypeRef::Result(Box::new(base(BaseType::Int)), Box::new(base(BaseType::String)), sp()),
                &types
            ),
            WireRef::Inst { key } if key == "Result_Int_String"
        ));
        assert!(matches!(
            wire_ref(&TypeRef::Option(Box::new(base(BaseType::Int)), sp()), &types),
            WireRef::Inst { key } if key == "Option_Int"
        ));
        assert!(matches!(
            wire_ref(&TypeRef::List(Box::new(base(BaseType::Int)), sp()), &types),
            WireRef::Inst { key } if key == "List_Int"
        ));
        assert!(matches!(
            wire_ref(
                &TypeRef::Map(Box::new(base(BaseType::String)), Box::new(base(BaseType::Int)), sp()),
                &types
            ),
            WireRef::Inst { key } if key == "Map_String_Int"
        ));
        assert!(matches!(
            wire_ref(
                &TypeRef::App {
                    name: Ident { name: "Paginated".to_string(), span: sp() },
                    args: vec![named("User")],
                    span: sp(),
                },
                &types
            ),
            WireRef::Inst { key } if key == "Paginated_User"
        ));
        assert!(matches!(
            wire_ref(&TypeRef::Unit(sp()), &types),
            WireRef::Unit
        ));
        assert!(matches!(
            wire_ref(
                &TypeRef::Effect(Box::new(base(BaseType::Int)), sp()),
                &types
            ),
            WireRef::Unchecked {
                reason: UncheckedReason::Effect
            }
        ));
        assert!(matches!(
            wire_ref(
                &TypeRef::HttpResult(Box::new(base(BaseType::Int)), sp()),
                &types
            ),
            WireRef::Unchecked {
                reason: UncheckedReason::HttpResult
            }
        ));
        assert!(matches!(
            wire_ref(&TypeRef::ValidationError(sp()), &types),
            WireRef::Unchecked {
                reason: UncheckedReason::ValidationError
            }
        ));
        assert!(matches!(
            wire_ref(&TypeRef::JsonError(sp()), &types),
            WireRef::Unchecked {
                reason: UncheckedReason::JsonError
            }
        ));
        assert!(matches!(
            wire_ref(&TypeRef::QueueResult(sp()), &types),
            WireRef::Unchecked {
                reason: UncheckedReason::QueueResult
            }
        ));
    }
}
