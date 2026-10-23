//! `bynk-ir`: the resolved *declaration-level* vocabulary `bynk-emit` reads
//! through `bynk-lower`'s AST-analysis helpers, instead of re-deriving the
//! same facts from `bynk_syntax::ast` a second time.
//!
//! Every type here is a value some `bynk-lower` helper returns from a
//! syntax-tree node plus the checker's own `TypedCommons`, and some
//! `bynk-emit` site consumes — a service's protocol ([`ProtocolIr`]), a
//! handler's kind ([`IrHandlerKind`]) and route cache ([`CacheIr`]), a
//! `type` declaration's structure ([`TypeShape`]), an agent store field's
//! storage shape ([`StoreFieldIr`]/[`StoreKindIr`]), a
//! capability op's or attached method's signature ([`OpSig`]/[`FnSig`]), a
//! `given` capability reference ([`CapRefIr`]), an actor's authentication
//! seam ([`ActorSeamIr`]), an events subscription's pattern and shape
//! ([`EventPatternIr`]/[`EventPatternValueIr`]/[`EventSubscriberShape`]),
//! and the literal values a pattern can carry ([`ConstVal`]). Alongside
//! them: four AST-walk helpers `bynk-emit` reads ([`block_uses_emit`],
//! [`walk_block_exprs`], [`walk_exprs`], [`match_needs_if_chain`] — they
//! live here rather than in the emitter because they were written to be
//! shared with `bynk-lower`, and `bynk-emit` is the consumer that remains).
//!
//! **Every `pub` item here has a reader outside both `bynk-ir` and
//! `bynk-lower`**, and the gated `unconsumed_ir_items` probe
//! (`cargo xtask greenfield-status`) fails CI if one ever does not — the
//! two crates do not get to vouch for each other, since a `bynk-ir` type
//! constructed only by a `bynk-lower` helper nobody calls is exactly the
//! shape phase 6 shipped.
//!
//! **What this crate is not, and was.** Phase 6 of
//! `design/bynk-compiler-trajectory.md` (`the-ir.md`, #1137) built here a
//! full typed expression IR — `IrExpr`/`IrExprKind`/`IrStmt`, patterns and
//! match compilation, `IrItem` with every declaration variant, `IrHandler`,
//! `CommitShape` — and `bynk-lower` built the `&CheckedProgram → Ir` pass
//! that constructed it. The emitter never consumed any of it: its own
//! string-emitting lowerer kept reading the AST, and the one production
//! route into the IR constructors lowered every events-service handler body
//! and discarded the result. The follow-on track (the IR cutover,
//! #1542) priced finishing that cutover, found it was a second code
//! generator rather than a retype, and re-settled on deletion: Slices D0–D2
//! removed the detour, the constructors and those 23 types. The refusal is
//! recorded with a trigger in `design/bynk-greenfield-compiler.md` Part 15.1
//! (Slice D3). `design/archive/retired-tracks.md` has both tracks' closing
//! summaries.
//!
//! **Identity fields are adapted, not literal** (Decision B of P6.1,
//! extending ADR 0333's own precedent): the reference's
//! `DefId`/`FieldId`/`VariantId` arena does not exist in this codebase, so
//! every such slot is whatever cheap resolved handle the checker already
//! has — a [`TyId`] for a type, a `String` for a name with no arena of its
//! own.
//!
//! **Two AST types are embedded on purpose** (ADR 0366, P6.41):
//! [`TypeShape::Refined`] carries `bynk_syntax::ast::BaseType` and
//! `Refinement` directly, because the refinement predicate is rendered from
//! its syntax and an IR mirror would be a field-for-field copy with no
//! consumer of its own. The `ast_importers` probe's own doc comment
//! (`xtask/src/greenfield_status.rs`) records that this embedding is
//! invisible to it by construction.

use std::collections::HashMap;

use bynk_check::checker::TyId;
use bynk_syntax::ast::{
    BaseType, Block, Expr, ExprId, MatchArm, Pattern, Refinement, expr_children, statement_exprs,
};

/// A literal value an event-subscription pattern can match on
/// ([`EventPatternValueIr::Const`]). Adapted from the
/// reference's own `ConstVal` (`Int Float Str Bool Unit Bytes`, Part 6.2's
/// comment): `DurationMillis` replaces `Bytes` because Bynk has a real
/// `<int>.<unit>` duration *literal* (`ExprKind::DurationLit`) `Const` must
/// cover, while `Bytes` has no literal AST form at all in this language —
/// every `Bytes` value comes from a static-constructor *call*
/// (`Bytes.fromUtf8`/`fromBase64`/`empty()`, `Callee::Intrinsic` territory,
/// not a literal).
#[derive(Debug, Clone, PartialEq)]
pub enum ConstVal {
    Int(i64),
    Float(f64),
    DurationMillis(i64),
    Str(String),
    Bool(bool),
    Unit,
}

/// P6.14's real `CapRefIr` ([DECISION A], #1174, review of #1186) — one
/// `bynk_syntax::ast::CapRef` entry of a provider's own `given` clause,
/// under this module's usual "no arena, bare name" substitution:
/// `context: Option<QualifiedName>` flattens to `Option<String>` via
/// `QualifiedName::joined()` (the same `.`-joined form
/// `resolve_consume_prefix` — `bynk-emit/src/project.rs` — already
/// resolves against `consumes`/aliases), and `name: Ident` flattens to
/// `String`, mirroring every other bare-name identity field in this module.
/// Deliberately **not** resolved further here: which context a `Some`
/// prefix actually names is whole-project `consumes`/alias data, a phase
/// boundary this crate sits below — this type only preserves what `CapRef`
/// itself carries, unresolved.
#[derive(Debug, Clone)]
pub struct CapRefIr {
    pub context: Option<String>,
    pub name: String,
}

/// A capability op's resolved signature (P6.12, [DECISION A], #1173) —
/// `bynk_lower::lower_capability_ops_ir`'s per-op return value, read by
/// `emit_capability` (`bynk-emit/src/emitter/emit.rs`). The reference's own
/// sketch named `ops: Vec<OpSig>` (`bynk-greenfield-compiler.md:1134`)
/// without defining the type; this is that shape, adapted from
/// `bynk_syntax::ast::CapabilityOp` — a signature only, no body — under
/// this crate's "no arena" substitutions: `params: Vec<(String, TyId)>`,
/// and `type_params: Vec<String>` mirroring the checker's own
/// already-resolved `CapabilityOpInfo::type_params`
/// (`bynk-check/src/checker.rs`) — a bare rigid-variable name, not a
/// `TypeParam` AST node, since nothing here re-derives bounds a capability
/// op's own `[T, …]` list never carries in the first place (#926).
/// `bynk_lower::lower_op_sig_ir` resolves `params`/`return_ty` in the scope
/// `type_params` names, mirroring `context_checks::build_capability_op_info`'s
/// own `vars` treatment (`bynk-check/src/context_checks.rs`) so a generic
/// op's own `T` survives as `Ty::Var("T")` rather than collapsing to
/// `Ty::Unit`. On a genuinely unresolvable name a `params`/`return_ty` entry
/// *is* `Ty::Unit`, deliberately — see `bynk_lower::lower_op_sig_ir`'s own doc
/// comment for why that mirrors the checker's own fallback rather than
/// panicking.
#[derive(Debug, Clone)]
pub struct OpSig {
    pub name: String,
    /// The op's own type parameters (#926) — empty for a non-generic op.
    /// Scoped to the op itself, not the capability: `CapabilityDecl` carries
    /// no `type_params` of its own (`bynk-syntax/src/ast.rs:556-562`), so
    /// this is never merged with anything above it, unlike an attached
    /// method, whose receiver type's own params are in scope too.
    pub type_params: Vec<String>,
    pub params: Vec<(String, TyId)>,
    pub return_ty: TyId,
}

/// A `fn`'s own resolved signature, with no `body` and no `receiver` —
/// the [`OpSig`]-shaped value a *foreign* unit's attached method needs when
/// only its signature will ever be rendered (`emit_attached_methods`'
/// delegating forward at `bynk_emit::emitter::emit::emit_forwarded_methods`),
/// never its body (P6.18). `bynk_lower::lower_attached_fn_sig_ir_from_types`
/// resolves `params`/`return_ty` in the scope the method's own `[T, …]` list
/// names (mirroring `OpSig`'s identical `type_params` treatment) — a
/// genuinely unresolvable name degrades to `Ty::Unit`, deliberately, the
/// same non-panicking posture `OpSig` already established: nothing
/// checker-side actually validates an attached method's own
/// `params`/`return_type` against the *importing* context's own visible
/// types (only the declaring commons' own checking does), so a resolve miss
/// here is an expected, not exceptional, state.
#[derive(Debug, Clone)]
pub struct FnSig {
    pub name: String,
    pub has_self: bool,
    pub params: Vec<(String, TyId)>,
    pub return_ty: TyId,
}

/// P6.11's real `ProtocolIr` ([DECISION A], #1171) — one variant per
/// `bynk_syntax::ast::ServiceProtocol` variant, `bynk_lower::lower_protocol_ir`'s
/// own return value. The reference's own sketch specifies only two of the
/// six: `Events { event, pattern, schema_dispatch }`
/// (`bynk-greenfield-compiler.md:1881`) and `WebSocket { in_ty, out_ty }`
/// (`:1959`) — field names taken verbatim from those two rows. `Call`/
/// `Http`/`Cron` carry no payload, not because one was dropped: the actual
/// per-trigger binding (a route, a schedule) lives on each *handler*
/// (`HandlerKind::Http { method, path }`/`Cron { expr }`), already
/// reachable through [`IrHandlerKind`] — `ServiceProtocol`'s own doc
/// comment says this in as many words ("the endpoint lives on each
/// handler"), which is why the reference never spells these three out
/// either. E2 (`:1737`) constrains the *set*, not the shape: "a closed
/// nominal set … grows one variant per real trigger" — the AST's own closed
/// `ServiceProtocol` already satisfies that exactly, so this type is total
/// over what a certified program's own service can declare, the same claim
/// [`StoreKindIr`]'s own doc makes about `Queue` being gated pre-`certify`.
#[derive(Debug, Clone)]
pub enum ProtocolIr {
    Call,
    Http,
    Cron,
    /// `from queue("name")`.
    Queue {
        name: String,
    },
    /// `from websocket(in: …, out: …)` — the two frame types, resolved
    /// (each through the `Ty::Unit` fallback `bynk_lower::lower_protocol_ir`
    /// documents, since the checker itself does not reject an unresolvable
    /// frame type).
    WebSocket {
        in_ty: TyId,
        out_ty: TyId,
    },
    /// `from Events(E)` — the subscribed event type, resolved, plus the
    /// two independent, optional filters a subscription may carry.
    Events {
        event: TyId,
        pattern: Option<EventPatternIr>,
        /// P6.40 (design/tracks/the-ir.md §6a): flattened to `Option<i64>` —
        /// `SchemaVersionPattern` has exactly one variant (`Literal(i64)`),
        /// so once a real consumer needed to match on it
        /// (`emitter/emit.rs`'s `via schema(N)` guard prologue), mirroring
        /// the single-payload `i64` directly was simpler than introducing a
        /// one-variant IR-native enum purely to re-wrap it. A future range
        /// pattern (`via schema(2..)`) widens this field's own shape when it
        /// lands, not before. The `SchemaDispatch` wrapper itself stays
        /// dropped — it carried only this `pattern` plus a parse-only
        /// `span`.
        schema_dispatch: Option<i64>,
    },
}

/// #1226/#1187 slice 6: the two facts a service's own event-subscriber
/// *shape* needs, captured at that unit's own check time (its
/// `CheckedProgram` does not survive past `check_unit_files`'s per-file loop)
/// so a *different* unit's own composition root can later decide whether its
/// subscriber to this service wants the event envelope forwarded, without
/// re-walking this unit's raw `UnitTable`. Pure syntax, zero `TyId`
/// dependency. Produced by `bynk_lower::lower_event_subscriber_shapes_ir`, sized
/// like #1187's own `unit_callees` (#1202) accumulator.
#[derive(Debug, Clone, Copy, Default)]
pub struct EventSubscriberShape {
    pub two_param_handler: bool,
    pub schema_dispatch: bool,
}

/// The payload of `ProtocolIr::Events`'s own `pattern` — a `from
/// Events(E { field: value, .. })` structural filter, [DECISION C] (#1171).
/// **Not** `bynk_syntax::ast::EventPattern` reused verbatim, unlike
/// `SchemaVersionPattern`: that type carries `rest_span` (a parse artefact
/// for the grammar-required trailing `..`, giving a later reader nothing to
/// act on) and its own `EventPatternValue::Variant` is an *unresolved,
/// optionally-qualified* name pair — exactly the shape this module's whole
/// posture rejects everywhere else.
#[derive(Debug, Clone)]
pub struct EventPatternIr {
    /// `(field name, matched value)`, in source order — no dedicated
    /// `EventPatternFieldIr` struct: a two-part fact with no further
    /// structure, the same plain-tuple shape `TypeShape::Sum::embeds` uses.
    pub fields: Vec<(String, EventPatternValueIr)>,
}

/// One [`EventPatternIr`] field's own matched value.
#[derive(Debug, Clone)]
pub enum EventPatternValueIr {
    /// Reuses [`ConstVal`] for the closed `Int`/`Str`/`Bool` literal set —
    /// `Pattern::Literal`'s own closed set, and the only reason `ConstVal`
    /// is still a type of its own.
    Const(ConstVal),
    /// A nullary sum-variant tag, resolved and unqualified — a bare
    /// `tag: String`. The AST's own optional qualifying `type_name` is
    /// dropped, not lost: the sole consumer
    /// (`bynk_emit::emitter::lower::event_pattern_guard_ir`, #1187's slice 5)
    /// already destructures down to the bare tag alone — the qualification
    /// is disambiguation for the *checker*, resolved against the field's
    /// declared sum type before this point.
    Variant { tag: String },
}

/// A GET handler's own `@cache(maxAge:, scope:)` freshness policy (#1228),
/// interpreted — raw `{name, value: Expr}` annotation pairs mean nothing on
/// their own, so the seconds and the scope are resolved here once.
/// Handler-scoped, not service-scoped: `bynk_lower::lower_route_cache_ir` is
/// a standalone per-route reader, wired directly into
/// `emitter/workers_entry.rs`'s own route construction — the same live,
/// standalone-consumer shape `lower_protocol_ir`/`lower_handler_given_ir`/
/// `lower_actor_seam_ir` already established. (A service-level policy
/// struct existed beside it until Slice D2 of #1542; it had no consumer.)
#[derive(Debug, Clone)]
pub struct CacheIr {
    /// `maxAge` in whole seconds (the `Cache-Control: max-age`).
    pub max_age_secs: i64,
    /// `"public"` or `"private"` — defaults to `"private"` so a *shared*
    /// cache never stores unless the author opts into `public`. Bare
    /// `&'static str`, not an enum: mirrors `emitter/workers_entry.rs`'s
    /// own former `CachePolicy::scope` shape verbatim (the two literal
    /// values this route's own generated `Cache-Control` header ever
    /// spells), not a new representation invented here — `CachePolicy`
    /// itself is gone, superseded by this struct (#1228).
    pub scope: &'static str,
    /// `immutable: true` (caching raw responses) — appends the RFC 8246
    /// `immutable` directive to the `Cache-Control`. Defaults to `false`.
    pub immutable: bool,
}

/// A declared type's own resolved structure (Part 6.6, #1161) —
/// `bynk_lower::lower_type_shape_ir`'s return value. Covers the AST's
/// four `TypeBody` variants (`Refined`/`Record`/`Sum`/`Opaque`) with the
/// reference's own three ([DECISION A]): `Opaque` unifies into `Refined`
/// via its own `opaque: bool` field, mirroring `emitter/emit.rs`'s own
/// `RefinedShape { base, refinement, is_opaque }` — the shipped emitter's
/// own precedent for exactly this unification (`emit_type`,
/// `emitter/emit.rs:19`).
#[derive(Debug, Clone)]
pub enum TypeShape {
    /// Every field the record declares, in declaration order ([DECISION B]
    /// extended: a field's own inline `refinement` is dropped — a
    /// construction-time constraint the checker already enforces, not part
    /// of the emitted shape. Scoped claim: no reader on the record-*type*
    /// emission path (`emit_record_type`, `emitter/emit.rs:234-263`, reads
    /// `type_ref` alone). There *is* one `.refinement` reader in the
    /// emitter overall — `emitter/emit.rs:2781`, agent-state zero-value
    /// construction — but that is `StoreFieldIr` territory (P6.7), out of
    /// this variant's scope; do not read this comment as licence to drop
    /// `refinement` from a future store field too.
    Record { fields: Vec<(String, TyId)> },
    /// Every variant the sum declares, each with its own payload field
    /// list, plus any `embeds` clauses ([DECISION C], #1161) — each a
    /// `(source type, target variant tag)` pair, a two-part fact with no
    /// further structure and so a plain tuple, not a dedicated struct.
    Sum {
        variants: Vec<(String, Vec<(String, TyId)>)>,
        embeds: Vec<(TyId, String)>,
    },
    /// `type X = base where refinement` (`Refined`) or `type X = unsafe
    /// base ...` (`Opaque`, `opaque: true`). `refinement` is `Option`, not
    /// the reference's own bare `RefinementId` ([DECISION B]) — a bare
    /// `type X = Int` (no `where` clause) is legal and carries none.
    Refined {
        base: BaseType,
        refinement: Option<Refinement>,
        opaque: bool,
    },
}

/// An agent `store` field's storage shape (`design/bynk-greenfield-compiler.md`
/// §6.6, R6.14, #1163) — `bynk_lower::lower_store_field_shape_ir`'s return
/// value, read by `emit_agent`'s state section. Mirrors
/// `checker::StoreField`'s own five-kind dispatch
/// (`bynk-check/src/checker.rs`) in shape, but is persistent IR data, not
/// that checking pass's own ephemeral, per-agent scratch value — the two are
/// deliberately not unified. Shape only: a `Cell` field's initialiser
/// expression is rendered by the emitter from the AST (the `init` slot that
/// once carried it as an `IrExpr` went with the expression IR in Slice D2 of
/// #1542).
#[derive(Debug, Clone)]
pub struct StoreFieldIr {
    /// The field's own declared name ([DECISION A]: `String`, sourced
    /// directly from `StoreField.name.name` — this module's own "no arena
    /// exists in this codebase" substitution, applied to the reference's
    /// own `FieldId` arena slot).
    pub field: String,
    pub kind: StoreKindIr,
    /// `@indexed(by: …)` sibling-table keys, in the annotation's own
    /// `by:`-argument order — one entry per *distinct* `by:` argument
    /// ([DECISION C]), no sort ([DECISION E]). Deduplicated: the checker
    /// validates each `by:` argument independently with no duplicate check
    /// (`validate_indexed_keys`), so `@indexed(by: k, by: k)` certifies —
    /// `bynk_lower::lower_store_field_shape_ir` guards against it, mirroring the
    /// shipped emitter's own `store_map_indexes` dedup. Empty for every kind
    /// but `Map`, the only kind `@indexed` attaches to (`ANNOTATIONS`'s own
    /// registry, `bynk-check/src/context_checks.rs`). Each key is the
    /// indexed value-field's own name, a bare `String` ([DECISION C],
    /// #1163): the sibling table's own emitted shape
    /// (`Record<string, string[]>`) is fixed by the *map's own key type*,
    /// not the indexed field's, so the indexed field's resolved type is not
    /// needed downstream — the same "no further structure" plain-value
    /// shape `TypeShape::Sum::embeds` uses.
    pub indexed: Vec<String>,
}

/// P6.7's real `StoreKindIr` (Part 6.6, R6.14, #1163) — five variants, one
/// per functional storage kind (`Cell`/`Map`/`Set`/`Cache`/`Log`). `Queue`
/// is not a variant here: `bynk.store.kind_unsupported` gates it before
/// `certify` (R3.10), so this type is total over what a certified program's
/// own store fields can actually contain, not a subset some later slice
/// needs to extend. [DECISION B]: `Duration` substitutes to `i64`
/// milliseconds throughout — the same substitution [`ConstVal::DurationMillis`]
/// and `checker::StoreField::Cache`'s own already-resolved TTL already made.
#[derive(Debug, Clone)]
pub enum StoreKindIr {
    /// `Cell[T]` — element type.
    Cell(TyId),
    /// `Map[K, V]` — key, value.
    Map(TyId, TyId),
    /// `Set[T]` — element type.
    Set(TyId),
    /// `Cache[K, V] @ttl(...)` — key, value, TTL in milliseconds.
    Cache(TyId, TyId, i64),
    /// `Log[T] [@retain(...)]` — element type, optional retain millis.
    Log(TyId, Option<i64>),
}

/// #1187's slice 3: a handler's resolved actor-verification seam, wrapping
/// `bynk-check`'s own five already-resolved seam structs
/// (`bynk-check/src/actors.rs`) by value — confirmed none carry any
/// `bynk_syntax::ast`/`TypeRef`/`Expr`: every field is `String`/`bool`/
/// `i64`/`Option`/`Vec` (or, for `BearerSeam::authorization`,
/// `ClaimPredicate`, itself a plain recursive `String`/`Box` enum). Built by
/// `bynk_lower::lower_actor_seam_ir`, which tries the five resolvers in the one
/// priority order that actually matters — `sum_members_for` first, since
/// it's the only resolver whose result can otherwise collide with
/// `bearer_seam_for`'s (a sum's own first peer can itself be Bearer-schemed;
/// `bearer_seam_for` has no `by.is_sum()` guard of its own to prevent that).
/// The other three pairs are mutually exclusive by construction — each
/// single-actor resolver requires the primary actor's own `auth` scheme to
/// match one specific `Scheme` variant, a closed set — so their relative
/// order here is a no-op, not a second load-bearing decision.
///
/// No `Signature` variant, deliberately: neither call site this slice
/// converts (`emit_service`'s `deps`-identity-binder chain, `emit.rs`;
/// `emit_worker_compose`'s HTTP-dispatch match, `workers.rs`) ever consults
/// `signature_seam_for` as part of this priority chain — Signature is a
/// separate, request-verification-only concept there (see
/// `workers_entry.rs`'s own `HttpRoute.signature` field), not one this
/// enum's callers need. Adding an unreachable variant this slice's own
/// `lower_actor_seam_ir` never constructs would be exactly the kind of
/// premature surface `bynk-design-notes.md`'s own conventions ask this
/// codebase to avoid.
#[derive(Debug, Clone)]
pub enum ActorSeamIr {
    /// No `by` clause resolves to any of the four seams below (`Visitor`/
    /// `None`-schemed, or no `by` clause at all).
    None,
    /// `by who: A | B` — an ordered sum of peer actors, first-wins.
    Sum(Vec<bynk_check::actors::SumMember>),
    /// I3 (#1971): an `on call` handler's sum, `by who: Caller | Legacy`.
    /// The call is already authenticated; the sum names which verified
    /// caller it was (`bynk_check::actors::call_admission_for`).
    CallSum(Vec<bynk_check::actors::CallSumMember>),
    Bearer(bynk_check::actors::BearerSeam),
    Oidc(bynk_check::actors::OidcSeam),
    /// A cross-context `on call … by c: Caller` handler's own binder name —
    /// `caller_binder_for`'s return type is already the bare `Option<String>`
    /// the other four resolvers reduce a whole struct down to one field for.
    Caller(String),
}

/// P6.24a: an IR-native mirror of [`bynk_syntax::ast::HandlerKind`] — a
/// field-for-field copy, not a re-export. Every field (`HttpMethod`, a
/// route `path: String`, a cron `expr: String`) is already fully resolved
/// at parse time; nothing here ever needed `TyId`/`CheckedProgram`, so
/// `bynk_lower::lower_handler_kind_ir` is a pure, unconditional conversion —
/// unlike almost everything else in this module, it carries no ADR 0334
/// totality story because it can never fail to resolve.
///
/// Exists so `emitter.rs`'s several purely-structural handler-kind scans
/// ("is this an HTTP handler", "which cron expression") have somewhere
/// IR-native to route through instead of matching `bynk_syntax::ast`
/// directly (#1184's review; R6.16, handler-invocation
/// origin-independence). With 70-odd references across `bynk-emit` it is
/// this crate's most-consumed type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IrHandlerKind {
    Call,
    Http { method: IrHttpMethod, path: String },
    Cron { expr: String },
    Message,
    Open,
    Close,
    Event,
}

/// [`IrHandlerKind::Http`]'s own method field — a field-for-field mirror of
/// [`bynk_syntax::ast::HttpMethod`], same reasoning as [`IrHandlerKind`]
/// itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrHttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl IrHttpMethod {
    /// P6.51 (design/tracks/the-ir.md §6b): field-for-field mirror of
    /// [`bynk_syntax::ast::HttpMethod::as_str`].
    pub fn as_str(self) -> &'static str {
        match self {
            IrHttpMethod::Get => "GET",
            IrHttpMethod::Post => "POST",
            IrHttpMethod::Put => "PUT",
            IrHttpMethod::Patch => "PATCH",
            IrHttpMethod::Delete => "DELETE",
        }
    }

    /// P6.57 (design/tracks/the-ir.md §6b): field-for-field mirror of
    /// [`bynk_syntax::ast::HttpMethod::from_ident`].
    pub fn from_ident(s: &str) -> Option<IrHttpMethod> {
        match s {
            "GET" => Some(IrHttpMethod::Get),
            "POST" => Some(IrHttpMethod::Post),
            "PUT" => Some(IrHttpMethod::Put),
            "PATCH" => Some(IrHttpMethod::Patch),
            "DELETE" => Some(IrHttpMethod::Delete),
            _ => None,
        }
    }
}

/// Events track, slice 0 (spine #936): does this block contain a real
/// `Events.emit[...]` call anywhere — including nested branches, match arms,
/// lambdas, and any other expression position (a `Paren`, an `Ok`/`Err`
/// wrapper, a `Call`/`RecordConstruction` argument, a `BinOp` operand, …)?
/// Gates release-at-commit buffer threading (`deps.__events`) so a handler
/// that never emits keeps byte-identical output, mirroring `block_uses_send`'s
/// gate on `deps.__exec`.
///
/// Driven off the exhaustive `walk_block_exprs`/`walk_exprs` visitor rather
/// than a hand-rolled `ExprKind` match — a bespoke match here previously
/// covered only `MethodCall`/`Block`/`If`/`Match`/`Lambda` and silently
/// disagreed with `lower_expr_into` (which recurses into every expression
/// position), so `do (Events.emit[E](event))` — one added paren — compiled
/// clean but emitted a body that referenced an undeclared `__events` local
/// (`tsc`-only failure, no bynk diagnostic). Riding the walker means this
/// can't drift from the lowering again: a new `ExprKind` variant fails to
/// compile here until `walk_exprs` itself is taught to visit it.
///
/// #1187's slice 6 plumbing (review of #1202): reads the checker's own
/// already-resolved `Callee::Capability{cap:"Events",op:"emit"}` for each
/// visited call site instead of a bare-`Ident("Events")`-receiver name
/// match. Was deliberately syntactic before this — this function's own
/// prior doc comment named the locally-shadowed-`Events` false positive an
/// "accepted approximation," matching `block_uses_send`'s own precedent —
/// but that approximation stopped being harmless once `crate::project::
/// unit_table_uses_emit` (the project-wide compose-gating twin this
/// function's own callers must agree with) became precise first: the two
/// disagreeing on exactly the shadowed case produces a real `tsc` type
/// error (a `deps.__eventsDispatch` call site with nothing supplying it),
/// not just an unused interface field. `block_uses_send` needs no matching
/// fix — a `~>` send is a real `Statement::Send` AST variant, not a method
/// call that could be shadowed, so it was never approximate to begin with.
pub fn block_uses_emit(b: &Block, callees: &HashMap<ExprId, bynk_check::checker::Callee>) -> bool {
    let mut found = false;
    walk_block_exprs(b, &mut |e| {
        if !found
            && matches!(
                callees.get(&e.id),
                Some(bynk_check::checker::Callee::Capability { cap, op })
                    if cap == "Events" && op == "emit"
            )
        {
            found = true;
        }
    });
    found
}

/// #1960: whether a body reaches the Worker `env` on the Workers target: it
/// calls a consumed context's service (through its Service Binding) or
/// constructs a local agent (through its Durable Object namespace).
pub fn block_reaches_worker_env(
    b: &Block,
    callees: &HashMap<ExprId, bynk_check::checker::Callee>,
) -> bool {
    let mut found = false;
    walk_block_exprs(b, &mut |e| {
        if !found
            && matches!(
                callees.get(&e.id),
                Some(
                    bynk_check::checker::Callee::Cross { .. }
                        | bynk_check::checker::Callee::AgentInit(_)
                )
            )
        {
            found = true;
        }
    });
    found
}

pub fn walk_block_exprs(b: &Block, f: &mut impl FnMut(&Expr)) {
    let mut exprs = Vec::new();
    for s in &b.statements {
        statement_exprs(s, &mut exprs);
    }
    exprs.push(&b.tail);
    for e in exprs {
        walk_exprs(e, f);
    }
}

/// v0.22b: pre-order expression visitor — visits `e`, then every
/// sub-expression, including statements and tails of nested blocks. Driven by
/// `ast::expr_children`, the exhaustive total child iterator, rather than a
/// hand-matched recursion duplicating it — a new `ExprKind` variant fails to
/// compile in `expr_children` until it is taught to visit it, instead of
/// silently under-visiting here.
pub fn walk_exprs(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    for child in expr_children(e) {
        walk_exprs(child, f);
    }
}

/// A match needs the if/else-if lowering (ADR 0169) when any arm carries a guard
/// or a refutable nested payload pattern — a JS `switch` on `.tag` can express
/// neither. Flat, unguarded matches keep the `switch` (zero churn to existing
/// output).
///
/// Consumed only by `bynk-emit`'s string emitter today (`emitter/lower.rs`'s
/// two match-lowering sites). It became `pub` and moved here at P6.5/P7.12
/// so the IR lowering pass could reuse the identical predicate to record its
/// own match form and the two could never silently disagree; that second
/// consumer went with Slices D1/D2 of #1542. It stays in `bynk-ir` rather
/// than moving back because it is a pure predicate over the AST with no
/// emitter state, the same footing as its three sibling walk helpers above,
/// and moving it would be churn with no reader gained.
pub fn match_needs_if_chain(arms: &[MatchArm]) -> bool {
    arms.iter().any(|a| {
        a.guard.is_some()
            || pattern_has_nested_test(&a.pattern)
            || matches!(a.pattern, Pattern::Refined { .. })
    })
}

/// True when `pat` carries a payload sub-pattern that is itself refutable (a
/// nested variant/literal) — i.e. it cannot be tested by a single `.tag` switch.
fn pattern_has_nested_test(pat: &Pattern) -> bool {
    match pat {
        Pattern::Variant { bindings, .. } => bindings.iter().any(|b| {
            let sp = b.pattern();
            !sp.is_irrefutable() || pattern_has_nested_test(sp)
        }),
        // #472: a refined pattern is never a top-level irrefutable/nested
        // payload today (the parser admits only `_` as `inner`, and only at a
        // match arm's top level), but keep this exhaustive and correct for
        // when nesting is admitted. (An `Or`'s alternatives can never
        // themselves be `Refined` — #472/#474's merged parser design only
        // ever wraps a *whole*, already-folded `|`-chain in `Refined`, never
        // the other way around — so this arm only matters nested under a
        // payload, e.g. a hypothetical `Some(_ where P)`.)
        Pattern::Refined { inner, .. } => !inner.is_irrefutable() || pattern_has_nested_test(inner),
        // #474: a bindingless, non-nested or-pattern (`1 | 2 | 3`,
        // `Pending | Cancelled(_, _)`) stays on the flat switch — it lowers to
        // fall-through `case` labels sharing one body. One with bindings or a
        // nested refutable payload needs the if-chain (a `switch` can't bind
        // different alternatives' fields to the same name).
        Pattern::Or(alts, _) => alts
            .iter()
            .any(|p| !p.bound_names().is_empty() || pattern_has_nested_test(p)),
        _ => false,
    }
}
