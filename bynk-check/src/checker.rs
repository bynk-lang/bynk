//! Type checker and refinement validator (spec §§5–6, v0.1 §4.2, v0.2 §4.2).
//!
//! Operates on a [`ResolvedCommons`]. Walks declarations, validates each
//! refinement against the spec's predicate-base compatibility and combination
//! rules, then type-checks every function and method body.
//!
//! v0.2 extensions:
//! - Record types (compatibility, field access, construction).
//! - Sum types and variant construction (qualified and unqualified).
//! - Methods (instance and static) with UFCS-style call resolution.
//! - Pattern matching with exhaustiveness checking.
//! - The `is` operator with binding flow into truthy contexts.
//! - The built-in generic `Option[T]`.

use std::collections::{BTreeSet, HashMap, HashSet};
#[cfg(debug_assertions)]
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::builtin_names::map_query;
use crate::builtin_names::methods::*;
use crate::builtin_names::types::*;
use crate::hints::HintSink;
use crate::index::{RefSink, SymbolKind};
use crate::locals::LocalsSink;
use crate::requirements::{
    Materialize, Requirement, RequirementSink, RequirementSource, StoreKind,
};
use crate::resolver::{MethodTable, ResolvedCommons};
use bynk_syntax::ast::*;
use bynk_syntax::error::{Applicability, CompileError};
use bynk_syntax::span::Span;

/// P6.27 (design/tracks/the-ir.md §6a): re-exported so a checker consumer keying
/// off `expr_types`/`Callee` (both `HashMap<ExprId, _>`, Q2's own settled totality
/// story) can name `ExprId` through `bynk-check` alone, without also depending on
/// `bynk_syntax::ast` directly just to spell this one identity type — the same
/// public-dependency-already-exists shape as `Ty`/`TyId` below.
pub use bynk_syntax::ast::ExprId;

mod calls;
mod d1;
pub(crate) mod equality;
pub mod ordering;
use equality::EqScope;
mod expressions;
mod kernels;
mod linearity;
mod par;
mod refinements;
pub(crate) use expressions::empty_range_error;
pub use refinements::split_regex_flags;
mod regex_ambiguity;
mod usefulness;

use calls::*;
use expressions::*;
use kernels::*;
use refinements::*;

pub use calls::{check_const_decl, check_event_field_default, check_state_initialiser};
pub use d1::row_bool_fields;
pub(crate) use d1::{is_sql_statement_type, statement_at_boundary};
pub use refinements::{locale_tag_accepts, locale_tag_pattern, zero_value_ts};

// ==== Type representation ====

/// T3.6b (R4.1): the intern table `TyId` is minted from. Owned per
/// `check_record` invocation (design settled in the identity-and-totality
/// track doc §9 before this slice started): created fresh at `check_record`'s
/// entry, threaded through `Ctx`, carried out on `TypedCommons`/`RecordCheck`
/// alongside `expr_types`, and forwarded across the `bynk-check`→`bynk-emit`
/// boundary on `CheckedProgram` (T3.7a/T3.7b already built that seam).
/// Confirmed safe by checking how cross-unit type references actually flow:
/// `compose_unit_symbols` merges `TypeDecl` (immutable AST declarations)
/// across units, never an already-interned `Ty`/`TyId` — every unit
/// re-interns its own `Ty` graph from shared declarations, so `TyId`s are
/// never compared across two different `check_record` invocations.
///
/// **Why [`intern`](Self::intern) takes `&self`, not `&mut self`.** The table
/// is reached from `Ctx`, whose other fields (`expr_types`, `errors`, the
/// sinks) are themselves `&mut` and are routinely live across an interning
/// call — `ctx.tys.intern(…)` inside a loop over `ctx.scopes` is the common
/// shape, not the exception. A `&mut Types` would make the borrow checker,
/// not the type system, the thing every one of the ~200 minting sites is
/// written around. Interior mutability keeps `&'a Types` `Copy`, so a
/// function that needs the table just reads `ctx.tys` once and is done.
///
/// **Why a `Mutex` and `Arc`, not a `RefCell` and `Rc`.** The compiler itself
/// is single-threaded, so a cell would do for `bynk-check` and `bynk-emit` —
/// but the table rides out on `TypedCommons`/`ProjectAnalysis` into
/// `bynk-lsp`, whose `tower-lsp` handlers are `async` and therefore require
/// `Send`. A non-atomic refcount is exactly what `Send` forbids, so the
/// choice is made by the consumer, not by the compiler's own threading. The
/// lock is uncontended in every current caller.
pub struct Types {
    inner: Mutex<TypesInner>,
    /// Which table this is, so [`Types::get`] can reject a foreign `TyId`
    /// whose index happens to be in range — see [`TyId`]'s own note.
    #[cfg(debug_assertions)]
    tag: u32,
}

/// Hands each [`Types`] a distinct [`Types::tag`]. Wrapping is not a
/// correctness problem: it would take 2^32 tables in one process for two to
/// collide, and the guard is a debug-build aid, not a soundness argument.
#[cfg(debug_assertions)]
static NEXT_TABLE_TAG: AtomicU32 = AtomicU32::new(0);

impl Default for Types {
    fn default() -> Self {
        Self {
            inner: Mutex::default(),
            #[cfg(debug_assertions)]
            tag: NEXT_TABLE_TAG.fetch_add(1, Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Default)]
struct TypesInner {
    /// `TyId(i)` resolves to `table[i]`. `Arc` so [`Types::get`] hands back a
    /// handle by refcount bump rather than cloning the node, and so the same
    /// allocation backs both `table` and `index` without storing it twice.
    table: Vec<Arc<Ty>>,
    index: HashMap<Arc<Ty>, TyId>,
}

impl Types {
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern `ty`, returning its `TyId`. The same `Ty` value (by `Eq`)
    /// always yields the same `TyId` — the property `ty_hash_eq_ord_tests`
    /// (T3.6b's own settling-review prerequisite) pins directly. Dedup is by
    /// the *shallow* `Ty`, which is sound precisely because every recursive
    /// field is already a `TyId`: two structurally-equal types have equal
    /// children ids by induction, so they hash and compare equal here.
    pub fn intern(&self, ty: Ty) -> TyId {
        let mut inner = self.lock();
        if let Some(&id) = inner.index.get(&ty) {
            return id;
        }
        let node = Arc::new(ty);
        let id = TyId {
            idx: inner.table.len() as u32,
            #[cfg(debug_assertions)]
            tag: self.tag,
        };
        inner.table.push(Arc::clone(&node));
        inner.index.insert(node, id);
        id
    }

    /// The node `id` was interned from.
    ///
    /// Panics on a `TyId` minted by a *different* table. That is the one new
    /// failure mode interning introduces, and it is a wiring bug in the
    /// compiler, never something a Bynk program can provoke — so it fails
    /// loudly and by name rather than as a bare index-out-of-bounds. It was
    /// worth the message: this fired twice while T3.6b was being built, both
    /// times a synthesised `TypedCommons` that had been given a table of its
    /// own while its `expr_types` was filled in from another.
    ///
    /// Both of those were the *shorter*-table shape, where a bounds check
    /// alone catches it. The dangerous shape is the other one: a foreign id
    /// that happens to be in range resolves to an unrelated `Ty` and the
    /// caller mis-diagnoses or mis-emits in silence. So in debug builds the
    /// check is identity, not length — [`TyId`] carries its table's tag and
    /// this compares it. Release builds keep the bounds check only, which is
    /// what indexing would have cost anyway.
    pub fn get(&self, id: TyId) -> Arc<Ty> {
        #[cfg(debug_assertions)]
        assert!(
            id.tag == self.tag,
            "bynk internal error (T3.6b, R4.1): {id:?} resolved against a table it was not \
             interned into (this is table {}). A `TyId` is only meaningful in its own `Types` — \
             check that whatever produced this id and whatever is reading it share one table",
            self.tag
        );
        let inner = self.lock();
        match inner.table.get(id.idx as usize) {
            Some(node) => Arc::clone(node),
            None => panic!(
                "bynk internal error (T3.6b, R4.1): {id:?} resolved against a table it was not \
                 interned into (this table holds {}). A `TyId` is only meaningful in its own \
                 `Types` — check that whatever produced this id and whatever is reading it share \
                 one table",
                inner.table.len()
            ),
        }
    }

    /// [`Ty::display`] for an already-interned type.
    pub fn display(&self, id: TyId) -> String {
        self.get(id).display(self)
    }

    /// Number of distinct types interned so far. Exposed for the interner's
    /// own tests (dedup is observable only as "the table did not grow").
    pub fn len(&self) -> usize {
        self.lock().table.len()
    }

    /// The lock, recovered from poisoning. `intern` never panics while
    /// holding it (it only pushes to a `Vec` and a `HashMap`), so a poisoned
    /// lock can only mean an unrelated panic unwound past a live guard —
    /// where the table is still structurally sound.
    fn lock(&self) -> std::sync::MutexGuard<'_, TypesInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::fmt::Debug for Types {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Types")
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

/// T3.6b (R4.1/R4.2): a `Ty`'s identity above the intern table — `Copy`,
/// `Hash`, `Ord`, cheap to pass and compare. Resolved back to a `Ty` only
/// via the [`Types`] table it was interned into (see that type's own doc).
///
/// In debug builds it also carries the tag of the table it came from, so
/// [`Types::get`] can make good on its "interned into another table" promise
/// for a foreign id whose index is merely *in range* — the case a bounds
/// check cannot see, and the one that would otherwise resolve to an
/// unrelated `Ty` in silence. `idx` is declared first so the derived `Ord`
/// still orders by insertion within a table, exactly as it does in release.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TyId {
    idx: u32,
    #[cfg(debug_assertions)]
    tag: u32,
}

impl std::fmt::Debug for TyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[cfg(debug_assertions)]
        return write!(f, "TyId({} of table {})", self.idx, self.tag);
        #[cfg(not(debug_assertions))]
        return write!(f, "TyId({})", self.idx);
    }
}

impl TyId {
    /// The interned node, for the (many) sites that need to look at the
    /// type's shape. Sugar for [`Types::get`], so a `TyId` reads like the
    /// `&Ty` it replaced.
    pub fn get(self, tys: &Types) -> Arc<Ty> {
        tys.get(self)
    }

    /// [`Ty::display`] for this id — the form nearly every diagnostic uses.
    pub fn display(self, tys: &Types) -> String {
        tys.display(self)
    }

    /// True if this is [`Ty::Error`] — the type T3.3b records for an
    /// expression whose typing failed. An editor reading `expr_types` treats
    /// it as "no type" and shows nothing, rather than displaying it — its
    /// display string is a diagnostic placeholder, not a Bynk type.
    pub fn is_error(self, tys: &Types) -> bool {
        matches!(*tys.get(self), Ty::Error)
    }

    /// True if this type is `Effect[_]` (v0.5).
    pub fn is_effect(self, tys: &Types) -> bool {
        tys.get(self).is_effect()
    }

    /// v0.102: true if this type belongs to the closed `Held` kind.
    pub fn is_held(self, tys: &Types) -> bool {
        tys.get(self).is_held()
    }

    /// The underlying base type, if this type widens to one.
    pub fn base(self, tys: &Types) -> Option<BaseType> {
        tys.get(self).base()
    }
}

/// A resolved type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Ty {
    /// R4.3 measurement probe (not shipped): a real Error variant.
    Error,
    /// A base type (`Int`, `String`, `Bool`).
    Base(BaseType),
    /// A user-declared named type. `kind` records the declaration's shape
    /// for compatibility / dispatch decisions. `args` holds the applied type
    /// arguments of a generic type (`Paginated[String]` → `args = [String]`);
    /// it is empty for a non-generic type (v0.157, ADR 0183). Substitution,
    /// unification, and display recurse into `args`.
    Named {
        name: String,
        kind: NamedKind,
        args: Vec<TyId>,
    },
    /// `Result[T, E]`.
    Result(TyId, TyId),
    /// `Option[T]`.
    Option(TyId),
    /// `Effect[T]` (v0.5).
    Effect(TyId),
    /// `HttpResult[T]` (v0.9).
    HttpResult(TyId),
    /// `QueueResult` — the built-in queue verdict sum (v0.44). Non-generic.
    QueueResult,
    /// `HttpOutcome[T]` — what a `system`-tier case observes when it drives an
    /// http address past the handler's types (#1934): a `Wire(…)` argument, `by
    /// Nobody`, or a method the path declares no handler for. `Rejected(
    /// BoundaryRejection)` when the router or the auth seam refused the request
    /// before the handler ran, `Handled(HttpResult[T])` when it ran. Produced
    /// only by a test-service address; it cannot be written in source.
    HttpOutcome(TyId),
    /// `BoundaryRejection` — why the boundary refused a request before the
    /// handler (#1934), the payload of `HttpOutcome.Rejected`. Non-generic.
    BoundaryRejection,
    /// `Asset` — a static file declared by `asset name = "path"` (#1883,
    /// slice 1a, [DECISION H]). Non-generic; its one field is `.url : String`.
    Asset,
    /// `RequestHeaders` — an HTTP handler's read-only view of the request's
    /// headers (#2341, decision A). Non-generic and opaque, with no wire form.
    RequestHeaders,
    /// `TokenClaims` (#2380, L5) — an `Oidc` actor's verified token claims,
    /// read through its binder's `.claims`.
    TokenClaims,
    /// `List[T]` — built-in immutable list (v0.20b).
    List(TyId),
    /// `Map[K, V]` — built-in immutable map (v0.20b). The key type is
    /// confined to value-keyable types at TypeRef resolution.
    Map(TyId, TyId),
    /// `Set[T]` — built-in immutable set (K6a, #2596). The element type is
    /// confined to the `Map`-key rule at TypeRef resolution.
    Set(TyId),
    /// `Query[T]` — a lazy, by-reference description of a read over agent-local
    /// storage (v0.91, ADR 0115). The inner type is the element a terminal
    /// yields. Built by the lazy combinator vocabulary over a `store` field,
    /// executed by a terminal (`-> Effect[…]`). Non-storable, non-boundary, and
    /// not value-comparable — like `Effect`/`Fn` (ADRs 0031/0030).
    Query(TyId),
    /// `Stream[T]` — a lazy, pull-shaped sequence of values produced over time
    /// (v0.100, real-time track slice 0). The inner type is the element a
    /// terminal yields. Built from a runtime source (`Stream.of` at v1),
    /// transformed by lazy builders (`map`/`take`), drained by a terminal
    /// (`collect -> Effect[List[T]]`). Non-storable, non-boundary, and not
    /// value-comparable — like `Query`/`Effect`/`Fn` (ADRs 0031/0030).
    Stream(TyId),
    /// `Connection[F]` — a held WebSocket connection (v0.102, real-time track
    /// slice 2). `F` is the server→client frame type. The one concrete instance
    /// of the closed `Held` kind (`is_held`). Governed by the linearity
    /// discipline (§2.9): single-owner, mandatory disposal. Non-serialisable,
    /// non-boundary, non-comparable; storable only in `Cell[Option[Connection]]`
    /// / `Map[K, Connection]`.
    Connection(TyId),
    /// `ValidationError` — built-in error type.
    ValidationError,
    /// `JsonError` — built-in JSON-decode error type (v0.22b). A uniform
    /// record: `kind`/`path`/`message`, all `String`.
    JsonError,
    /// `()` — the unit type (v0.5).
    Unit,
    /// v0.45: a verified actor binding (`by name: Actor`). The inner type is
    /// the actor's identity, read as `name.identity`. A boundary-minted, sealed
    /// value — only ever `.identity`-accessed, never constructed or passed.
    Actor(TyId),
    /// v0.52: a resolved multi-actor binding (`by who: A | B`) — an ordered sum
    /// of peer actors. Each member is `(actor name, identity ty)`; the body
    /// `match`es on the resolved actor, each non-unit member binding its
    /// identity directly. Like `Actor`, a sealed boundary value — only ever
    /// matched, never constructed or passed.
    ActorSum(Vec<(String, TyId)>),
    /// `A -> B` — a function type (v0.20a). Effectful iff `ret` is
    /// `Effect[_]` (the structural rule); no separate flag, so there is a
    /// single source of truth.
    Fn { params: Vec<TyId>, ret: TyId },
    /// A function type parameter (v0.20a). Two lives: *rigid* while checking
    /// a generic function's own body (name-equality in `compatible`), and
    /// *flexible* during call-site instantiation, where it is matched by
    /// `unify` and fully eliminated by `substitute` before any `compatible`
    /// runs against argument types. Vars never escape call checking into the
    /// caller's expression types.
    Var(String),
}

/// The shape of a named type — what its declaration looks like.
///
/// `Refined` widens to its base type when used in arithmetic, comparisons,
/// and other operations on the base. `Opaque` does NOT widen — its identity
/// is nominal and the base type is hidden outside the defining commons.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NamedKind {
    /// Refined-base type: widens to the recorded base.
    Refined(BaseType),
    /// Record type.
    Record,
    /// Sum type.
    Sum,
    /// Opaque base type. The base is hidden; identity is purely nominal.
    /// The recorded base is used by the type checker (for `.raw`, `.of`,
    /// `.unsafe`) and by the emitter, but not for compatibility widening.
    Opaque(BaseType),
}

impl Ty {
    /// Display name for diagnostics. Takes the table the type was interned
    /// into (T3.6b): every recursive field is a `TyId` now, so rendering a
    /// nested type is a table read rather than a pointer chase.
    pub fn display(&self, types: &Types) -> String {
        match self {
            // R4.3: a resolution failure already has its own diagnostic at the
            // site that produced this; this string exists only so a *second*
            // diagnostic that happens to mention the type (e.g. a mismatch one
            // level up) reads as "type error" rather than a blank or `unknown`.
            Ty::Error => "<type error>".to_string(),
            Ty::Base(b) => b.name().to_string(),
            // #2087: a qualified key reads as the declaration's own name.
            Ty::Named { name, args, .. } if args.is_empty() => {
                crate::symbols::display_type_name(name).to_string()
            }
            Ty::Named { name, args, .. } => format!(
                "{}[{}]",
                crate::symbols::display_type_name(name),
                args.iter()
                    .map(|a| types.display(*a))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Ty::Result(t, e) => {
                format!("Result[{}, {}]", types.display(*t), types.display(*e))
            }
            Ty::Option(t) => format!("Option[{}]", types.display(*t)),
            Ty::Effect(t) => format!("Effect[{}]", types.display(*t)),
            Ty::HttpResult(t) => format!("HttpResult[{}]", types.display(*t)),
            Ty::QueueResult => "QueueResult".to_string(),
            Ty::HttpOutcome(t) => format!("HttpOutcome[{}]", types.display(*t)),
            Ty::BoundaryRejection => "BoundaryRejection".to_string(),
            Ty::Asset => "Asset".to_string(),
            Ty::RequestHeaders => "RequestHeaders".to_string(),
            Ty::TokenClaims => "TokenClaims".to_string(),
            Ty::List(t) => format!("List[{}]", types.display(*t)),
            Ty::Set(t) => format!("Set[{}]", types.display(*t)),
            Ty::Map(k, v) => format!("Map[{}, {}]", types.display(*k), types.display(*v)),
            Ty::Query(t) => format!("Query[{}]", types.display(*t)),
            Ty::Stream(t) => format!("Stream[{}]", types.display(*t)),
            Ty::Connection(t) => format!("Connection[{}]", types.display(*t)),
            Ty::ValidationError => "ValidationError".to_string(),
            Ty::JsonError => "JsonError".to_string(),
            Ty::Unit => "()".to_string(),
            Ty::Actor(id) => format!("actor[{}]", types.display(*id)),
            Ty::ActorSum(members) => members
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>()
                .join(" | "),
            Ty::Fn { params, ret } => {
                let params = match params.len() {
                    0 => "()".to_string(),
                    // A single Fn-typed param needs parens to stay readable
                    // under right-associativity.
                    1 if !matches!(&*types.get(params[0]), Ty::Fn { .. }) => {
                        types.display(params[0])
                    }
                    _ => format!(
                        "({})",
                        params
                            .iter()
                            .map(|p| types.display(*p))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                };
                format!("{params} -> {}", types.display(*ret))
            }
            Ty::Var(name) => name.clone(),
        }
    }

    /// True if this type is `Effect[_]`.
    pub fn is_effect(&self) -> bool {
        matches!(self, Ty::Effect(_))
    }

    /// v0.102: true if this type belongs to the closed `Held` kind — a
    /// runtime-managed resource governed by the linearity discipline (§2.9).
    /// The one instance at v1 is `Connection[F]`; the single extension point
    /// for future held types (file handles, DB connections).
    pub fn is_held(&self) -> bool {
        matches!(self, Ty::Connection(_))
    }

    /// v0.102: for a `Held` type, the held element it wraps (the frame type of a
    /// `Connection[F]`). Used by the storage-admission rules to look through an
    /// `Option[Connection]` value.
    pub fn held_inner(&self) -> Option<TyId> {
        match self {
            Ty::Connection(t) => Some(*t),
            _ => None,
        }
    }

    /// The underlying base type, if this type widens to a base type.
    /// Opaque types deliberately do NOT widen — that's the whole point of
    /// the opacity — so `Ty::Named { kind: Opaque(_), .. }` returns None.
    pub fn base(&self) -> Option<BaseType> {
        match self {
            Ty::Base(b) => Some(*b),
            Ty::Named {
                kind: NamedKind::Refined(b),
                ..
            } => Some(*b),
            _ => None,
        }
    }
}

/// P6.0 (design/tracks/the-ir.md §6, #1139): a resolved classification of a
/// call-shaped expression, recorded once by the checker's own dispatch
/// (`checker::calls`) rather than re-derived by each later consumer —
/// closing R6.10's duplicated-classification gap between `bynk-check` and
/// `bynk-emit`'s `lower_method_call`/`lower_call`.
///
/// Adapted to the identity handles this checker already has (Decision A,
/// ADR 0333, `the-ir-callee-in-bynk-check`) rather than the reference
/// document's `DefId`/`LocalId`/`VariantId`/`OpId` arena — none of which
/// exists here, since the `Resolve` phase that would mint them was never
/// built (`project-model.md` §3.4 deferred it to phase 8).
/// `Arc<FnDecl>`/`Arc<TypeDecl>` are already-cheap resolved handles
/// (`ResolvedCommons::fns`/`types`); every other variant's identity is a
/// name, exactly as the checker already keys capabilities, store fields,
/// units, and agents.
///
/// Recorded at each dispatch decision as soon as it is known — including on
/// an error sub-branch (an arity mismatch, an undeclared capability) — since
/// the *kind* of call is fixed by dispatch, not by whether it went on to
/// type-check cleanly.
#[derive(Debug, Clone)]
pub enum Callee {
    /// A free function call. The `Vec<TyId>` is the call's comparator witnesses
    /// (constrained-generics slice 1): the type argument of each `Ord`-bounded
    /// type parameter of the callee, in [`ordering::witness_params`] order.
    /// Empty for a callee with no `Ord` bound, which is almost every call.
    Fn(Arc<FnDecl>, Vec<TyId>),
    /// A generic function with `Ord` bounds named as a value (`xs.map(max)`),
    /// instantiated at the reference: its witnesses, as for [`Callee::Fn`].
    FnValue(Arc<FnDecl>, Vec<TyId>),
    /// Applying a function-typed local or parameter (`f(x)` where `f` is in
    /// scope, not declared). No stable id exists for a local beyond its
    /// name — the reference's `LocalId` presumes the same `Resolve` phase
    /// Decision A declines to build here.
    Value(String),
    /// Sum-variant construction, bare (`Some(x)`) or qualified
    /// (`Opt.Some(x)`).
    Ctor { sum: Arc<TypeDecl>, tag: String },
    /// `T.of(value)` — the refined/opaque runtime constructor.
    Refine(Arc<TypeDecl>),
    /// `T.unsafe(value)` — the opaque constructor, defining-unit only.
    Unsafe(Arc<TypeDecl>),
    /// A user-declared static method (`Type.method(...)`).
    Static(Arc<FnDecl>),
    /// A user-declared instance method (UFCS), generic or not. The `Vec<TyId>`
    /// is the call's comparator witnesses, as for [`Callee::Fn`].
    Method(Arc<FnDecl>, Vec<TyId>),
    /// A built-in method on a value — the collection/query/stream/
    /// connection/numeric/duration/instant/bytes/string/option/result/
    /// effect kernels, including the refined-receiver fallback (ADR 0168).
    /// `recv` is the receiver's own checked type; no `KernelOp` enum exists
    /// yet in this crate (R6.11), so the operation is named, not typed.
    Kernel { recv: TyId, op: String },
    /// A built-in static constructor with no declaring type — `List.empty`,
    /// `Map.empty`, `Int.parse`/`Float.parse`, `Duration.millis`,
    /// `Instant.fromEpochMillis`, `Bytes.fromUtf8`/`fromBase64`/`empty`,
    /// `Json.decode`/`encode`, `Stream.of`.
    Intrinsic { ns: &'static str, op: String },
    /// A same-context capability operation call (`Cap.op(...)`).
    Capability { cap: String, op: String },
    /// A cross-context capability operation call (`B.Cap.op(...)` /
    /// `Alias.Cap.op(...)`).
    CrossCap {
        unit: String,
        cap: String,
        op: String,
    },
    /// A cross-context service call (`B.service(...)` / `Alias.service(...)`).
    Cross { unit: String, service: String },
    /// `AgentName(key)` — agent instance construction. No slot exists for
    /// this in the reference's own `Callee` taxonomy (Part 6.5 only names
    /// handler dispatch); added here since this slice covers every call
    /// shape `check_call` dispatches, not only the ones the reference
    /// anticipated.
    AgentInit(String),
    /// `agent.handler(args)` — agent handler dispatch.
    Agent { agent: String, handler: String },
    /// A test-body service address (`svc.call`/`svc.<VERB>("/path", …)`/
    /// `svc.schedule(...)`/`svc.message(...)`). `check_test_service_address`
    /// always returns `None` by design (the runner recovers the outcome
    /// type at runtime) — this classification exists purely for a later
    /// consumer (e.g. go-to-definition on the address), not typing.
    TestService { service: String, address: String },
    /// #1883, slice 3: `svc.GET(asset)` in a test body — an asset fetch,
    /// answered by the asset tier rather than a handler. Its own variant
    /// (review of #1922) so no route address can be mistaken for it.
    TestAssetFetch { service: String },
    /// An effectful `<field>.<op>(…)` storage operation on a `store`
    /// `Map`/`Set`/`Cache`/`Log`/`Cell` field — R6.5's own named target
    /// (P6.2, #1143): a mutation detector keyed on this variant, not a
    /// receiver's bare name, cannot miss a mutation reached through a
    /// non-`Ident` receiver or false-negative on a shadowed local, the
    /// defect class `block_writes_state`'s `mutating_op` still carries.
    /// `field` is the store field's own name (no `FieldId` arena exists —
    /// same adaptation `check_store_*_op`'s own lookups already use). Note
    /// this is recorded *outside* `calls.rs`'s six functions — the
    /// store-field ladder lives directly in `checker.rs`'s own `type_of`,
    /// never reaching any of them — extending P6.0's own recording surface
    /// past the boundary its "Done when" deliberately drew.
    Store { field: String, op: String },
    /// A query builder/terminal call that *lifts* a bare `store` `Map`/`Log`
    /// field into a lazy `Query[V]` (`is_query_op`'s own gate,
    /// `checker.rs`'s `type_of`) — R6.12's own named target (P6.2, #1143).
    /// `field` names the store field being lifted — without it, a chain
    /// rooted at this call (`orders.filter(p).count()`) would carry no
    /// identity for `orders` anywhere in the classification, the same
    /// information loss R6.5 exists to close on the write side. `role` is
    /// read back from the checker's own typing decision for this exact call
    /// (`Ty::Query(_)` result ⇒ `Builder`, anything else ⇒ `Terminal`), not
    /// a second name-list classifier alongside `is_query_op`'s. A *chained*
    /// builder/terminal call on an already-`Query`-typed receiver
    /// (`.filter(p).count()`'s own `.count()`) is not this variant — it
    /// reaches `check_method_call`'s ordinary kernel dispatch and is
    /// `Callee::Kernel` already (P6.0); `Query` here exists only because the
    /// lift call's own outer expression never passes through any of
    /// `calls.rs`'s six functions to get one.
    Query {
        field: String,
        op: String,
        role: QueryRole,
    },
}

/// Whether a [`Callee::Query`] call returns another `Query[T]` (chainable)
/// or executes and returns `Effect[T]` — R6.12: "the builder/terminal split
/// is a field on the callee, not a name list." Primarily read back from
/// `check_query_kernel_method`'s own return type at the recording site
/// (`query_role`, below) — falling back to `is_query_builder_name` only
/// when the call didn't type at all (an arity mismatch, or a type error
/// deeper inside the call — `map`'s own lambda body, say — both return
/// `None` too, not just an arity failure), so a best-effort reader of an
/// uncertified/erroring unit still gets the right role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryRole {
    Builder,
    Terminal,
}

/// The fallback `Callee::Query::role` classifier for when
/// `check_query_kernel_method`'s own return type doesn't settle it (see
/// `QueryRole`'s doc comment). Kept in sync by hand with
/// `check_query_kernel_method`'s own match arms
/// (`bynk-check/src/checker/kernels.rs:861-1118`) — the same "kept in sync
/// by hand" risk R6.11 already names for `kernel_methods.rs`'s own
/// registries, not a new class of drift this slice introduces.
fn is_query_builder_name(name: &str) -> bool {
    matches!(
        name,
        "map"
            | "filter"
            | "flatMap"
            | "sortBy"
            | "take"
            | "skip"
            | "distinct"
            | "distinctBy"
            | "joinOn"
            | "leftJoin"
            | "join"
            | "groupBy"
    )
}

/// P6.2 (#1143): `Callee::Query`'s `role` for a call whose op name is `op`
/// and whose checked result (from `check_query_kernel_method`/
/// `check_store_log_op`) is `result`.
fn query_role(result: Option<TyId>, op: &str, tys: &Types) -> QueryRole {
    match result.map(|t| tys.get(t)).as_deref() {
        Some(Ty::Query(_)) => QueryRole::Builder,
        Some(_) => QueryRole::Terminal,
        None if is_query_builder_name(op) => QueryRole::Builder,
        None => QueryRole::Terminal,
    }
}

/// Output of type checking.
pub struct TypedCommons {
    pub commons: Commons,
    pub types: HashMap<String, Arc<TypeDecl>>,
    pub fns: HashMap<String, Arc<FnDecl>>,
    pub methods: HashMap<String, MethodTable>,
    /// T3.4 (R2.4/R2.5): keyed by [`ExprId`] — a node's identity, not its
    /// position. The value carries its own `span` alongside `ty`, so
    /// LSP-facing consumers that need "type at this cursor offset" (a
    /// position-shaped question, asked at the editor boundary, not the
    /// checker's own identity) can still answer it without a second map.
    pub expr_types: HashMap<ExprId, TypedExpr>,
    /// P6.0 (#1139): the call-shaped expressions this unit's checker
    /// dispatched, classified once here rather than re-derived by
    /// `bynk-emit`'s lowering (P6.2) or any other later consumer. Mirrors
    /// `expr_types` exactly — same key, same "recorded during checking, read
    /// afterward" shape.
    pub callees: HashMap<ExprId, Callee>,
    /// v0.89 (ADR 0117): non-failing warnings produced while checking this unit
    /// — surfaced but not gating. Empty unless a warning-category diagnostic
    /// (e.g. `bynk.given.unused_capability`) fired on an otherwise-clean check.
    pub warnings: Vec<CompileError>,
    /// T3.6b (R4.1): the intern table every `TyId` on this unit — in
    /// `expr_types`, in a `Ty` node's own recursive fields — was minted from.
    /// Named `ty_intern` rather than `types` only because `types` above is
    /// already this struct's *declaration* table (`TypeDecl` by name); the two
    /// are unrelated. `Rc` so [`RecordCheck`] can hand the same table out
    /// beside its `result` whichever way the check went.
    pub ty_intern: Arc<Types>,
    /// #1170: a service handler's own resolved `by <binder>: <Actor>` actor
    /// binding — `handler_actor_binding`'s own return value
    /// (`context_checks.rs`), persisted here rather than discarded once
    /// `check_service_decls`'s own per-handler loop moves on, the same
    /// "recorded during checking, read afterward" shape `callees` (above)
    /// already established. Keyed by the handler's own `span`: a `Handler`
    /// has no arena identity of its own (no `DefId`/`ExprId` — it is a
    /// declaration, not an expression), and `Span` is already this
    /// codebase's established "no arena" substitute for exactly this kind
    /// of identity (`Copy`/`Eq`/`Hash`, already used as a diagnostic anchor
    /// throughout `context_checks.rs`). No entry for a handler
    /// `handler_actor_binding` itself resolves to `None` for: a
    /// binder-less `by <Actor>` clause, or no `by` clause at all —
    /// including every agent handler, which cannot carry one
    /// (`bynk.actor.by_on_agent`). Read back through [`Self::actor_binding`];
    /// its one IR-side consumer (P6.11, #1171, the service-handler
    /// actor-binder constructor in `bynk-lower`) was deleted by Slice D1 of
    /// #1542, so as of that slice the only readers are this crate's own
    /// `context_checks` tests — the table is kept because the binding is a
    /// checked fact about the handler, not because of who reads it.
    ///
    /// **Unit-wide, not per-file** (review of #1170, unlike `callees`/
    /// `expr_types`, which are genuinely per-file — keyed by `ExprId`s this
    /// file's own checking pass minted): `check_service_decls` walks
    /// `table.services`, the whole unit's own `UnitTable`, not just this
    /// file's declarations, so every file of a multi-file `context` ends up
    /// with the *entire unit's* bindings in its own `TypedCommons`. Harmless
    /// for a by-span lookup (a span is only ever looked up in the file that
    /// actually owns it), but a future consumer that *iterates* this map
    /// rather than looking up one known `span` would see sibling files'
    /// handlers too — worth knowing before writing that consumer, not
    /// discovering it by surprise.
    pub actor_bindings: HashMap<Span, (String, TyId)>,
    /// #1883, slice 1a: the assets visible in this unit, carried from
    /// [`ResolvedCommons::assets`] so a per-handler check built from this
    /// program (`context_checks`) still sees them.
    pub assets: HashMap<String, AssetDecl>,
    /// #2387, slice 1: the embeds visible in this unit, carried from
    /// [`ResolvedCommons::embeds`] for the same per-handler checks.
    pub embeds: HashMap<String, EmbedDecl>,
    /// #2617: the constants visible in this unit, carried from
    /// [`ResolvedCommons::consts`] for the same per-handler checks and for
    /// emission.
    pub consts: HashMap<String, ConstDecl>,
    /// #1925: the imports this unit's own declarations shadow, carried from
    /// [`ResolvedCommons::shadowed_imports`] for the same per-handler checks.
    pub shadowed_imports: HashMap<String, crate::resolver::ShadowedImport>,
    /// S3 (#1963): the extensible first-party enums in scope, carried from
    /// [`ResolvedCommons::extensible_enums`] for the same per-handler checks.
    pub extensible_enums: HashSet<String>,
    /// S3 (#1963): carried from [`ResolvedCommons::stability`] for the same
    /// per-handler checks.
    pub stability: crate::stability::UnitStability,
}

impl TypedCommons {
    /// A `TypedCommons` with no declarations — a pure-data test fixture, so a
    /// consuming crate's own test exercising only the `TypedCommons`-shaped
    /// part of some function doesn't need to hand-construct a
    /// `Commons`/`QualifiedName` itself (`bynk_syntax::ast` types, invisible
    /// to `bynk-check` but not to a consumer whose own probe tracks that
    /// dependency — P6.38, `design/tracks/the-ir.md` §6a).
    pub fn empty() -> Self {
        TypedCommons {
            commons: Commons {
                name: QualifiedName {
                    parts: Vec::new(),
                    span: Default::default(),
                },
                items: Vec::new(),
                uses: Vec::new(),
                documentation: None,
                form: CommonsForm::Fragment,
                span: Default::default(),
                trivia: Default::default(),
                trailing_comments: Vec::new(),
            },
            types: HashMap::new(),
            fns: HashMap::new(),
            methods: HashMap::new(),
            expr_types: HashMap::new(),
            callees: HashMap::new(),
            warnings: Vec::new(),
            ty_intern: Arc::new(Types::new()),
            actor_bindings: HashMap::new(),
            assets: HashMap::new(),
            embeds: HashMap::new(),
            consts: HashMap::new(),
            shadowed_imports: HashMap::new(),
            extensible_enums: HashSet::new(),
            stability: Default::default(),
        }
    }

    /// T3.6b (R4.1): this unit's intern table — what every `TyId` reachable
    /// from `expr_types` resolves against.
    /// Returns the `Rc` handle rather than a bare `&Types` so a caller that
    /// needs to *share* the table (the project path, which checks many units
    /// into one `ExprTypeSink`) can clone it; `&Arc<Types>` deref-coerces to
    /// `&Types` everywhere a plain borrow is wanted.
    pub fn tys(&self) -> &Arc<Types> {
        &self.ty_intern
    }

    /// The interned node an expression was typed to, resolved in one step.
    /// The reader-side shape `bynk-emit`/the LSP want: they ask "what shape is
    /// this expression?", never "which id is it?". `Rc` so the resolve is a
    /// refcount bump, and `.as_deref()` gives back the `&Ty` these call sites
    /// read before T3.6b.
    pub fn expr_ty(&self, id: ExprId) -> Option<Arc<Ty>> {
        self.expr_types.get(&id).map(|te| self.ty_intern.get(te.ty))
    }

    /// P6.0 (#1139): the resolved [`Callee`] classification for a
    /// call-shaped expression, if this unit's checker dispatched one at
    /// `id`. Mirrors [`Self::expr_ty`]'s shape.
    pub fn callee(&self, id: ExprId) -> Option<&Callee> {
        self.callees.get(&id)
    }

    /// #1170: a service handler's own resolved actor binding, if
    /// `handler_actor_binding` (`context_checks.rs`) resolved one for the
    /// handler at `span`. Mirrors [`Self::callee`]'s shape — the single
    /// documented read point for `actor_bindings`, kept symmetric with
    /// `expr_ty`/`callee` rather than leaving every future consumer to
    /// reach into the `HashMap` directly. Its IR-side reader (P6.11, #1171)
    /// went with Slice D1 of #1542; see `actor_bindings`'s own doc comment.
    pub fn actor_binding(&self, span: Span) -> Option<&(String, TyId)> {
        self.actor_bindings.get(&span)
    }
}

/// T3.4: an `expr_types` entry — the checked type, plus the span of the node
/// it was computed for. `Deref`-free by design (`.ty`/`.span`, not `.0`/`.1`)
/// so call sites read the same as they did against a bare `Ty` before this.
///
/// T3.6b (R4.1/R4.2): `ty` is a `TyId`, not a `Ty` — the whole entry is
/// `Copy`-cheap, and resolving it needs the unit's `ty_intern` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypedExpr {
    pub span: Span,
    pub ty: TyId,
}

/// The outcome of [`check_record`]: the typed model (`Err` if the file had any
/// error) and, on the error path, the program as checked so far
/// (`typed_despite_errors`), whose `expr_types` are the best-effort types the
/// checker computed. Analyse mode surfaces those for `.`-member completion and
/// signature help even on a broken buffer (ADR 0094).
pub struct RecordCheck {
    pub result: Result<TypedCommons, Vec<CompileError>>,
    /// T3.6b (R4.1): the table every `TyId` in `result` or
    /// `typed_despite_errors` resolves against — the same `Rc` either
    /// `TypedCommons::ty_intern` carries.
    pub ty_intern: Arc<Types>,
    /// #1663 (Decision A): on the `Err` path, the program as checked so far —
    /// every declaration was still checked, so a later stage (context
    /// declarations, handler bodies) can go on checking the *other*
    /// declarations. Never certifiable: the errors in `result` still fail
    /// the unit. `None` on the `Ok` path.
    pub typed_despite_errors: Option<TypedCommons>,
}

/// T3.7 (R3.10): the gate between analysis and emission, as a type rather
/// than a control-flow decision — constructible only by [`certify`], so no
/// unchecked or error-carrying `TypedCommons` can reach the emitter by
/// construction (previously enforced only by every caller happening to check
/// a `Result` first). `certify` rejects on any error-severity diagnostic;
/// T3.3a's `Ty::Error` is what a diagnosed checker failure records into
/// `expr_types`, so in practice a `Ty::Error` never reaches a `CheckedProgram`
/// either — R4.3's "rejected by certify" already holds today via the same
/// diagnostic-severity gate `certify` makes structural.
///
/// Scoped to the single-file compile path for now (`bynk-emit`'s
/// `compile_with_warnings`). The project/batch path's per-unit `emit_project`
/// call happens *before* that unit's build-wide gate is finally decided
/// (cross-unit validation can still fail the whole build afterward), so
/// wrapping it in `CheckedProgram` at today's call site would misrepresent an
/// unfinished decision as a certified one — that path needs its own slice,
/// not forced into this one.
pub struct CheckedProgram(TypedCommons);

impl CheckedProgram {
    /// The certified program. No accessor exists that goes the other
    /// direction — a `TypedCommons` is never recoverable-then-rewrapped
    /// without going through `certify` again.
    pub fn program(&self) -> &TypedCommons {
        &self.0
    }
}

/// The single place "may we emit?" is asked (R3.10). Rejects — returning
/// every diagnostic, not just the error-severity ones, matching
/// `check_record`'s own error-path convention — if `diagnostics` contains an
/// error-severity entry; otherwise wraps `program` as certified.
pub fn certify(
    program: TypedCommons,
    diagnostics: Vec<CompileError>,
) -> Result<CheckedProgram, Vec<CompileError>> {
    let (hard_errors, warnings) = bynk_syntax::partition_by_severity(diagnostics);
    if hard_errors.is_empty() {
        Ok(CheckedProgram(program))
    } else {
        let mut all = hard_errors;
        all.extend(warnings);
        Err(all)
    }
}

// ==== Entry points ====

/// #2607 (Decisions B and I): validate each default a record type's fields
/// declare (`field: T = expr`). It must be a static value of the field's type,
/// the rule an event field's default follows, and it must have a wire form,
/// since a body that lacks the key decodes to it. In a generic record, a
/// default must hold for every instantiation: a field whose type is a bare
/// type parameter has none.
#[allow(clippy::too_many_arguments)]
fn check_record_field_defaults(
    t: &TypeDecl,
    input: &ResolvedCommons,
    tys: &Types,
    expr_types: &mut HashMap<ExprId, TypedExpr>,
    callees: &mut HashMap<ExprId, Callee>,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
) {
    let TypeBody::Record(r) = &t.body else {
        return;
    };
    for field in &r.fields {
        let Some(init) = &field.init else {
            continue;
        };
        if let TypeRef::Named(id) = &field.type_ref
            && t.type_params.iter().any(|p| p.name.name == id.name)
        {
            errors.push(
                CompileError::new(
                    "bynk.types.bad_field_default",
                    init.span,
                    format!(
                        "field `{}` has the type parameter `{}`, so it cannot have a default",
                        field.name.name, id.name
                    ),
                )
                .with_note(
                    "no single value has every type; a default must hold for every \
                     instantiation of the record, as `None`, `[]` or `Map.empty()` do",
                ),
            );
            continue;
        }
        let before = errors.len();
        check_event_field_default(
            init,
            &field.type_ref,
            "record field default",
            input,
            tys,
            expr_types,
            callees,
            errors,
            refs,
            hints,
            locals,
        );
        if errors.len() > before {
            continue;
        }
        if let Err(reason) =
            crate::wire_default::lower_field_default_wire(init, &field.type_ref, &input.types)
        {
            // A default that reaches a type parameter (`Option[T] = Some(1)`)
            // holds for one instantiation at most.
            if let crate::wire_default::DefaultReason::UnresolvedType(name) = &reason
                && let Some(p) = t.type_params.iter().find(|p| &p.name.name == name)
            {
                errors.push(
                    CompileError::new(
                        "bynk.types.bad_field_default",
                        init.span,
                        format!(
                            "field `{}`'s default does not hold for every `{}`",
                            field.name.name, p.name.name
                        ),
                    )
                    .with_note(
                        "a generic record's default must hold for every instantiation, as \
                         `None`, `[]` or `Map.empty()` do",
                    ),
                );
                continue;
            }
            errors.push(
                CompileError::new(
                    "bynk.types.bad_field_default",
                    init.span,
                    format!(
                        "field `{}`'s default cannot be represented on the wire: {reason}",
                        field.name.name
                    ),
                )
                .with_note(
                    "a body without this field decodes to its default, so the default needs a \
                     wire form: a literal, a variant, `Some`/`None`/`Ok`/`Err`, a record, a \
                     list literal, or `Map.empty()`/`Set.empty()`",
                ),
            );
        }
    }
}

/// #1688: clear the cache of generic functions' compared type parameters.
/// A project check calls this once before its per-unit loop; a single-file
/// [`check`] calls it itself.
pub fn reset_compared_cache() {
    equality::reset_compared_cache();
}

pub fn check(input: ResolvedCommons) -> Result<TypedCommons, Vec<CompileError>> {
    reset_compared_cache();
    check_record(
        input,
        &mut RefSink::new(),
        &mut HintSink::new(),
        &mut LocalsSink::new(),
        &mut RequirementSink::new(),
    )
    .result
}

/// [`check`], recording binding edges into `refs` at the checker's
/// resolution sites (v0.25). A fresh sink records nothing.
pub fn check_record(
    input: ResolvedCommons,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
    requirements: &mut RequirementSink,
) -> RecordCheck {
    check_record_in(
        input,
        &Arc::new(Types::new()),
        refs,
        hints,
        locals,
        requirements,
    )
}

/// [`check_record`] against a **caller-supplied** intern table (T3.6b, R4.1).
///
/// The per-invocation table `check_record` mints is the right default: one
/// unit, one table, ids that never escape it. A *project* check is the case
/// that needs more — it runs `check_record` once per unit but funnels every
/// unit's `expr_types` into one `ExprTypeSink`, so a `TyId` recorded there
/// would be ambiguous if each unit interned into a table of its own. Sharing
/// one table across the whole analysis makes those ids mean one thing, and is
/// strictly safer than the per-unit case the track doc argued for: ids are
/// still only ever compared against ids from the same table.
pub fn check_record_in(
    input: ResolvedCommons,
    ty_intern: &Arc<Types>,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
    requirements: &mut RequirementSink,
) -> RecordCheck {
    let ty_intern = Arc::clone(ty_intern);
    let mut errors = Vec::new();
    let mut expr_types: HashMap<ExprId, TypedExpr> = HashMap::new();
    let mut callees: HashMap<ExprId, Callee> = HashMap::new();
    // 1. Validate each type declaration. State-migrations S2 (#2224): a record
    // naming a `migrate` step's old shape may declare field defaults — they
    // stand in for an initialiser a field had at that version (Decision C).
    let migrate_old_types: HashSet<&str> = input
        .commons
        .items
        .iter()
        .filter_map(|i| match i {
            CommonsItem::Agent(a) => Some(a),
            _ => None,
        })
        .flat_map(|a| a.migrations.iter())
        .filter_map(|m| match m.old.as_ref().map(|o| &o.type_ref) {
            Some(TypeRef::Named(id)) => Some(id.name.as_str()),
            _ => None,
        })
        .collect();
    for item in &input.commons.items {
        if let CommonsItem::Type(t) = item {
            check_type_decl(t, &input.types, &ty_intern, &mut errors);
            // #2607: a record field's default. A `migrate` step's old shape
            // is validated by its agent's own check, against the types the
            // agent sees, so it is skipped here rather than reported twice.
            if !migrate_old_types.contains(t.name.name.as_str()) {
                check_record_field_defaults(
                    t,
                    &input,
                    &ty_intern,
                    &mut expr_types,
                    &mut callees,
                    &mut errors,
                    refs,
                    hints,
                    locals,
                );
            }
        }
    }

    // 1b. #2617: check each constant declared in this file — its name, its
    // initialiser's static-ness and type — and the cycles through it.
    let local_consts: Vec<&ConstDecl> = input
        .commons
        .items
        .iter()
        .filter_map(|i| match i {
            CommonsItem::Const(c) => Some(c),
            _ => None,
        })
        .collect();
    for c in &local_consts {
        check_const_decl(
            c,
            &input,
            &ty_intern,
            &mut expr_types,
            &mut callees,
            &mut errors,
            refs,
            hints,
            locals,
        );
    }
    // A cycle can only run through this unit's own constants (`uses` is
    // acyclic), so the walk follows only them and treats an imported
    // constant as a leaf. Following an imported constant's initialiser here
    // would resolve its names in this unit's scope, where a local constant of
    // the same name would be taken for the one it actually names.
    let own_consts: HashMap<String, ConstDecl> = local_consts
        .iter()
        .map(|c| (c.name.name.clone(), (*c).clone()))
        .collect();
    errors.extend(crate::consts::cycle_errors(&local_consts, &own_consts));

    // 2. Type-check each function and method body.
    for item in &input.commons.items {
        if let CommonsItem::Fn(f) = item {
            refs.set_owner(f.name.display());
            check_fn(
                f,
                &input,
                &mut expr_types,
                &mut callees,
                &mut errors,
                refs,
                hints,
                locals,
                requirements,
                &ty_intern,
            );
            refs.clear_owner();
        }
    }
    // #1688: record this unit's generics' compared type parameters, in this
    // unit's environment, for the units that import them.
    equality::scan_local_generics(&input, &ty_intern);

    // v0.89 (ADR 0117): split diagnostics by severity. A unit with no
    // error-severity diagnostic *checks* — its warnings ride on `TypedCommons`,
    // surfaced but non-gating. Only error-severity diagnostics fail the check;
    // on that path the warnings are appended so a failed build still renders
    // them.
    // Finding #28 (debug-only): `Span` is `expr_types`'s key, but nothing
    // enforces that no two AST nodes needing a type share one — bug #844 and
    // the else-less-`if` synthesis both did, silently, before either was
    // caught. Walk every checked function/method body with the total child
    // iterator `ast::expr_children` and assert no two nodes recorded here
    // collide; a release build doesn't pay for the walk. Scoped per item,
    // not across the whole commons: a multi-file commons's merged item list
    // legitimately re-walks the same function more than once (its own,
    // separate redundancy, outside this finding's scope), and both known
    // collisions (#844, the else-less-`if` synthesis) are contained within a
    // single function/handler body regardless.
    #[cfg(debug_assertions)]
    for item in &input.commons.items {
        if let CommonsItem::Fn(f) = item {
            let mut seen: HashSet<ExprId> = HashSet::new();
            assert_expr_types_disjoint_in_block(&f.body, &expr_types, &mut seen);
        }
    }

    let (hard_errors, warnings) = bynk_syntax::partition_by_severity(errors);
    if hard_errors.is_empty() {
        RecordCheck {
            result: Ok(TypedCommons {
                commons: input.commons,
                types: input.types,
                fns: input.fns,
                methods: input.methods,
                expr_types,
                callees,
                warnings,
                ty_intern: Arc::clone(&ty_intern),
                actor_bindings: HashMap::new(),
                assets: input.assets,
                embeds: input.embeds,
                consts: input.consts,
                shadowed_imports: input.shadowed_imports,
                extensible_enums: input.extensible_enums,
                stability: input.stability,
            }),
            ty_intern,
            typed_despite_errors: None,
        }
    } else {
        // Keep the best-effort types the checker already computed; Analyse mode
        // surfaces them for `.`-member completion on a broken buffer (ADR 0094).
        let mut all = hard_errors;
        all.extend(warnings);
        RecordCheck {
            result: Err(all),
            typed_despite_errors: Some(TypedCommons {
                commons: input.commons,
                types: input.types,
                fns: input.fns,
                methods: input.methods,
                expr_types,
                callees,
                warnings: Vec::new(),
                ty_intern: Arc::clone(&ty_intern),
                actor_bindings: HashMap::new(),
                assets: input.assets,
                embeds: input.embeds,
                consts: input.consts,
                shadowed_imports: input.shadowed_imports,
                extensible_enums: input.extensible_enums,
                stability: input.stability,
            }),
            ty_intern,
        }
    }
}

/// Finding #28 (debug-only), T3.4: the block-level half of the `expr_types`
/// identity-uniqueness walk — visits every statement expression and the tail.
/// `ExprId` uniqueness is guaranteed by construction (`Parser::alloc_expr_id`
/// is the sole allocation point), so this can no longer catch a *parser*
/// collision the way it caught #844 on `Span`; it stays as the loud check
/// that a synthetic node (`ExprId::SYNTHETIC`) never reaches the checker's
/// own `expr_types` — the one way two entries could still collide.
#[cfg(debug_assertions)]
fn assert_expr_types_disjoint_in_block(
    block: &Block,
    expr_types: &HashMap<ExprId, TypedExpr>,
    seen: &mut HashSet<ExprId>,
) {
    let mut roots: Vec<&Expr> = Vec::new();
    for s in &block.statements {
        bynk_syntax::ast::statement_exprs(s, &mut roots);
    }
    roots.push(&block.tail);
    for e in roots {
        assert_expr_types_disjoint(e, expr_types, seen);
    }
}

/// Finding #28 (debug-only), T3.4: recurses over an expression with the
/// total child iterator `ast::expr_children`, asserting no two nodes
/// recorded into `expr_types` share an [`ExprId`] — a collision means one
/// node's recorded type silently clobbered another's (bug #844's class of
/// bug, before `ExprId` made position-derived collisions structurally
/// impossible for parser-allocated nodes).
#[cfg(debug_assertions)]
fn assert_expr_types_disjoint(
    e: &Expr,
    expr_types: &HashMap<ExprId, TypedExpr>,
    seen: &mut HashSet<ExprId>,
) {
    assert!(
        e.id != ExprId::SYNTHETIC || !expr_types.contains_key(&e.id),
        "bynk internal error (finding #28): a synthetic node (ExprId::SYNTHETIC) reached the \
         checker's own `expr_types` at {:?} — synthetic nodes are built after checking and must \
         never be inserted here",
        e.span
    );
    if expr_types.contains_key(&e.id) {
        assert!(
            seen.insert(e.id),
            "bynk internal error (finding #28): two typed AST nodes share id {:?} (span {:?}) in \
             `expr_types` — one node's recorded type silently clobbered another's",
            e.id,
            e.span
        );
    }
    for child in bynk_syntax::ast::expr_children(e) {
        assert_expr_types_disjoint(child, expr_types, seen);
    }
}

/// #522: the six output sinks a handler-body check writes into. One struct at
/// each call site instead of six positional `&mut` arguments.
pub struct CheckSinks<'a> {
    /// T3.6b (R4.1): the intern table the `TyId`s written into `expr_types`
    /// (and carried on [`HandlerBodyCheck`]) resolve against. Belongs with the
    /// sinks rather than the signature: it is the thing a body check *writes*
    /// types into, and a caller holding a `TypedCommons` passes its
    /// `ty_intern` straight through.
    pub tys: &'a Types,
    pub expr_types: &'a mut HashMap<ExprId, TypedExpr>,
    pub errors: &'a mut Vec<CompileError>,
    pub refs: &'a mut RefSink,
    pub hints: &'a mut HintSink,
    pub locals: &'a mut LocalsSink,
    pub requirements: &'a mut RequirementSink,
    /// P6.0 (#1139): the `Callee` classification sink — see [`Callee`].
    pub callees: &'a mut HashMap<ExprId, Callee>,
}

/// #522: everything [`check_handler_body`] needs to know about the handler —
/// signature, capability scope, agent state, and held bindings. Replaces what
/// was 17 positional parameters (of a 24-parameter signature); [`Self::new`]
/// fills the agent/actor/store extras with empties, so a simple site
/// (provider op, test body) sets only the fields it actually uses.
pub struct HandlerBodyCheck<'a> {
    pub body: &'a Block,
    pub return_type: &'a TypeRef,
    pub params: &'a [Param],
    /// The capabilities the body may call (the handler's resolved `given`).
    pub capabilities: HashMap<String, CapabilityInfo>,
    /// Every declared capability, for "declared but not given" diagnostics.
    pub declared_capabilities: HashMap<String, CapabilityInfo>,
    pub given: &'a [CapRef],
    pub given_anchor: Option<Span>,
    pub report_unused: bool,
    /// An agent handler's synthetic state-record type, when one is in scope.
    pub agent_state_ty: Option<TyId>,
    pub agent_self_scope: Option<HashMap<String, TyId>>,
    /// v0.45/v0.52: the `by <binder>: <Actor(s)>` binding — the binder name and
    /// its fully-formed sealed type: `Ty::Actor(identity)` for a single actor
    /// (so `binder.identity` type-checks), or `Ty::ActorSum(members)` for a sum
    /// (so the body `match`es on it). `None` for handlers without a `by` binder.
    pub actor_binding: Option<(String, TyId)>,
    /// The agent's `store` fields, by name (finding #36 — see [`StoreField`]),
    /// so the `:=` write form, `<field>.<op>(…)`, and the map query accessors
    /// can resolve their target. Empty for service/test bodies and `state {
    /// }` agents.
    pub store_fields: HashMap<String, StoreField>,
    /// v0.106 (slice 3b-iii): held params that are **borrowed**, not owned —
    /// the firing `connection` of a `from websocket` `on message`/`on close`.
    /// Borrowed bindings admit non-consuming ops (`send`) but carry no disposal
    /// obligation. Empty for every other handler (including `on open`, whose
    /// connection is owned).
    pub borrowed_held: HashSet<String>,
    /// #2380 (L5): the binder name when the `by` actor is an `Oidc` one.
    pub oidc_binder: Option<String>,
    /// #2683: the body is a service handler's, whose `deps` carry the
    /// execution context `e.within(d)` needs.
    pub service_handler: bool,
}

impl<'a> HandlerBodyCheck<'a> {
    /// A check of `body` against `return_type` with everything optional empty:
    /// no capabilities, no agent state, no actor binding, no store fields.
    pub fn new(
        body: &'a Block,
        return_type: &'a TypeRef,
        params: &'a [Param],
        given: &'a [CapRef],
    ) -> Self {
        Self {
            body,
            return_type,
            params,
            capabilities: HashMap::new(),
            declared_capabilities: HashMap::new(),
            given,
            given_anchor: None,
            report_unused: false,
            agent_state_ty: None,
            agent_self_scope: None,
            actor_binding: None,
            store_fields: HashMap::new(),
            borrowed_held: HashSet::new(),
            oidc_binder: None,
            service_handler: false,
        }
    }
}

/// Check a single handler body (used for service and agent handlers).
pub fn check_handler_body(
    input: &ResolvedCommons,
    check: HandlerBodyCheck<'_>,
    sinks: CheckSinks<'_>,
) {
    let HandlerBodyCheck {
        body,
        return_type,
        params,
        capabilities,
        declared_capabilities,
        given,
        given_anchor,
        report_unused,
        agent_state_ty,
        agent_self_scope,
        actor_binding,
        store_fields,
        borrowed_held,
        oidc_binder,
        service_handler: in_service_handler,
    } = check;
    let CheckSinks {
        tys,
        expr_types,
        errors,
        refs,
        hints,
        locals,
        requirements,
        callees,
    } = sinks;
    let return_ty_span = return_type.span();
    let Some(return_ty) = resolve_type_ref(return_type, &input.types, tys) else {
        return;
    };
    let no_vars = HashSet::new();
    record_type_refs(return_type, &input.types, &no_vars, refs);
    // Build the parameter scope.
    let mut param_scope: HashMap<String, TyId> = HashMap::new();
    for p in params {
        if let Some(t) = resolve_type_ref(&p.type_ref, &input.types, tys) {
            record_type_refs(&p.type_ref, &input.types, &no_vars, refs);
            // v0.31: a handler/op parameter is in scope over the whole body.
            if p.name.name != "_" {
                locals.record(
                    p.name.name.clone(),
                    p.name.span,
                    crate::locals::LocalKind::Param,
                    t.display(tys),
                    body.span,
                );
            }
            param_scope.insert(p.name.name.clone(), t);
        }
    }
    if let Some((binder, binder_ty)) = actor_binding {
        if binder != "_" {
            locals.record(
                binder.clone(),
                body.span,
                crate::locals::LocalKind::Param,
                "actor".to_string(),
                body.span,
            );
        }
        param_scope.insert(binder, binder_ty);
    }
    if let Some(self_scope) = agent_self_scope {
        param_scope.extend(self_scope);
    }
    let effectful = return_ty.is_effect(tys);
    let given_entries: Vec<(String, Span)> = given
        .iter()
        .map(|c| (c.key().to_string(), c.span))
        .collect();
    let given_remaining: HashSet<String> = given_entries.iter().map(|(k, _)| k.clone()).collect();
    let mut ctx = Ctx {
        input,
        tys,
        expr_types,
        errors,
        refs,
        hints,
        locals,
        requirements,
        callees,
        scopes: vec![param_scope],
        is_binding_cache: HashMap::new(),
        admitted_literals: HashMap::new(),
        pattern_binding_types: HashMap::new(),
        let_else_removals: HashMap::new(),
        return_ty,
        return_ty_span,
        effectful,
        agent_state_ty,
        commit_seen: false,
        caps: CapabilityCtx {
            capabilities,
            declared_capabilities,
            given_remaining,
            given_used: HashSet::new(),
            given_entries: given_entries.clone(),
            given_anchor,
        },
        in_test_body: false,
        oidc_binder,
        test_services: HashMap::new(),
        test_actors: HashMap::new(),
        type_vars: HashSet::new(),
        eq: EqScope::default(),
        last_witness: None,
        store_fields,
        in_agent_handler: agent_state_ty.is_some(),
        in_service_handler,
    };
    // Check the body and validate it matches the return type.
    let Some(body_ty) = type_of_block(body, Some(return_ty), &mut ctx) else {
        return;
    };
    // v0.102 (§3 step 11): the held-resource linearity pass, now that
    // `expr_types` is fully populated by the body walk above.
    linearity::check(
        body,
        params,
        &input.types,
        ctx.expr_types,
        &ctx.pattern_binding_types,
        &borrowed_held,
        ctx.errors,
        tys,
    );
    // Finding #28 (debug-only), extended: `check_record`'s per-function walk
    // (43abc242) never reaches a handler body — `check_handler_body` is
    // `bynk-emit`'s own entry point for service/agent handlers, called
    // directly from `validate.rs`, not from `check_record`'s
    // `CommonsItem::Fn` loop. A fresh `seen` set per call, matching the
    // per-item (not per-commons) granularity 43abc242 chose, so a
    // multi-file commons re-checking the same handler doesn't false-positive.
    #[cfg(debug_assertions)]
    {
        let mut seen: HashSet<ExprId> = HashSet::new();
        assert_expr_types_disjoint_in_block(body, ctx.expr_types, &mut seen);
    }
    if !compatible(body_ty, return_ty, tys) {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.return_mismatch",
                body.tail.span,
                format!(
                    "handler body has type `{}`, but the declared return type is `{}`",
                    body_ty.display(tys),
                    return_ty.display(tys)
                ),
            )
            .with_label(return_ty_span, "declared return type"),
        );
    }
    // Bidirectional `given` check.
    // 1) Every used capability is declared. (Handled in capability-call site.)
    // 2) Every declared capability is used — anything left in given_remaining
    //    minus given_used is unused. Emit as a warning-category error so the
    //    test harness can match it. Entries are walked in declaration order
    //    (deduplicated by key) so diagnostics and their fixes are stable.
    let mut reported: HashSet<&str> = HashSet::new();
    for (i, (c, _)) in given_entries.iter().enumerate() {
        if !report_unused {
            break;
        }
        if ctx.caps.given_used.contains(c) || !reported.insert(c) {
            continue;
        }
        ctx.errors.push(
            CompileError::new(
                "bynk.given.unused_capability",
                return_ty_span,
                format!("capability `{c}` is declared in `given` but never used in the body"),
            )
            // Finding #49: the CLI now renders `.with_suggestion` below, so
            // this note carries only the alternative fix the suggestion
            // doesn't (removing the capability from `given`).
            .with_note("alternatively, use the capability in the handler body")
            // v0.26 (ADR 0054): the removal is list-aware — only `report_unused`
            // sites are handlers, where the clause follows the return type, so
            // `return_ty_span` anchors the only-entry case.
            .with_suggestion(
                format!("remove `{c}` from the `given` clause"),
                vec![(
                    given_removal_span(&given_entries, i, return_ty_span),
                    String::new(),
                )],
                Applicability::MachineApplicable,
            ),
        );
    }
}

/// Type-check a bare body against `return_ty` in `scope`, with `caps`
/// available as both in-scope and declared capabilities and (if non-empty)
/// `test_services`/`test_actors` in scope for a test-case body's `svc.call`/
/// `by <Actor>(...)` resolution (§32/#33: the one shape every hand-rolled
/// `Ctx` outside this crate needed, letting `Ctx` itself stay `pub(crate)`).
/// `where_pred`, if present, is checked first against `Bool` (a property's
/// optional `for all ... where` filter — `bynk.property.where_not_bool` on
/// mismatch), sharing `ctx` with the main body so both populate the same
/// `expr_types`/`errors` sinks. Unlike [`check_handler_body`], this skips
/// the linearity pass, the return-type-mismatch diagnostic, and the
/// unused-`given` diagnostic — nothing outside this crate that built its
/// own `Ctx` ran those either, and adding them here would be a behaviour
/// change, not a refactor.
#[allow(clippy::too_many_arguments)]
pub fn check_body(
    input: &ResolvedCommons,
    body: &Block,
    return_ty: TyId,
    return_ty_span: Span,
    scope: HashMap<String, TyId>,
    caps: CapabilityCtx,
    test_services: HashMap<String, TestServiceSig>,
    test_actors: HashMap<String, bynk_syntax::ast::ActorDecl>,
    where_pred: Option<&Expr>,
    sinks: CheckSinks<'_>,
) -> Option<TyId> {
    let CheckSinks {
        tys,
        expr_types,
        errors,
        refs,
        hints,
        locals,
        requirements,
        callees,
    } = sinks;
    let mut ctx = Ctx {
        input,
        tys,
        expr_types,
        errors,
        refs,
        hints,
        locals,
        requirements,
        callees,
        scopes: vec![scope],
        is_binding_cache: HashMap::new(),
        admitted_literals: HashMap::new(),
        pattern_binding_types: HashMap::new(),
        let_else_removals: HashMap::new(),
        return_ty,
        return_ty_span,
        effectful: return_ty.is_effect(tys),
        agent_state_ty: None,
        commit_seen: false,
        caps,
        in_test_body: true,
        oidc_binder: None,
        test_services,
        test_actors,
        type_vars: HashSet::new(),
        eq: EqScope::default(),
        last_witness: None,
        store_fields: HashMap::new(),
        in_agent_handler: false,
        in_service_handler: false,
    };
    if let Some(w) = where_pred {
        let bool_ty = tys.intern(Ty::Base(BaseType::Bool));
        if let Some(actual) = type_of(w, Some(bool_ty), &mut ctx)
            && actual.base(tys) != Some(BaseType::Bool)
        {
            ctx.errors.push(CompileError::new(
                "bynk.property.where_not_bool",
                w.span,
                format!(
                    "a `for all ... where` filter has type `{}`, but a `Bool` is required",
                    actual.display(tys)
                ),
            ));
        }
    }
    let result = type_of_block(body, Some(return_ty), &mut ctx);
    // Finding #28 (debug-only), extended: see the identical note in
    // `check_handler_body` — `check_body`'s test-case/property callers bypass
    // `check_record`'s walk the same way handler bodies do.
    #[cfg(debug_assertions)]
    {
        let mut seen: HashSet<ExprId> = HashSet::new();
        assert_expr_types_disjoint_in_block(body, ctx.expr_types, &mut seen);
    }
    result
}

/// Check an agent's invariant declarations (v0.80 §14). Each predicate is a pure
/// `Bool`-typed expression over the agent's state fields (referenced by bare
/// name), plus `implies`/`is`. The pass enforces:
///
/// - `bynk.invariant.duplicate_name` — two invariants share a name.
/// - `bynk.invariant.cross_agent_reference` — a predicate names another agent
///   (§14 closes that door; sagas/scenarios are the cross-agent tools).
/// - `bynk.invariant.impure_predicate` — a predicate uses an effectful or
///   test-only construct (Effect, `?` propagation, `expect`, `Val`).
/// - `bynk.invariant.not_bool` — the predicate does not type to `Bool`.
///
/// Store `Cell` fields are placed in scope as the predicate's locals; invariants
/// read fields directly by bare name, mirroring the design-notes worked examples.
#[allow(clippy::too_many_arguments)]
pub fn check_invariants(
    invariants: &[Invariant],
    // A `store`-bearing agent's invariants reference its `Cell` fields by bare
    // name (a pure read of the staged value), so they form the predicate scope.
    store_cells: &HashMap<String, TyId>,
    agent_name: &str,
    input: &ResolvedCommons,
    tys: &Types,
    expr_types: &mut HashMap<ExprId, TypedExpr>,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
    requirements: &mut RequirementSink,
    callees: &mut HashMap<ExprId, Callee>,
) {
    // Duplicate-name check across the agent's invariants.
    let mut seen: HashMap<&str, ()> = HashMap::new();
    for inv in invariants {
        if seen.insert(inv.name.name.as_str(), ()).is_some() {
            errors.push(
                CompileError::new(
                    "bynk.invariant.duplicate_name",
                    inv.name.span,
                    format!(
                        "agent `{agent_name}` declares more than one invariant named `{}`",
                        inv.name.name
                    ),
                )
                .with_note("give each invariant a distinct name"),
            );
        }
    }

    // Build the predicate scope once: each `store` `Cell` is in scope by bare
    // name (a `Cell` reads as its element type).
    let mut field_scope: HashMap<String, TyId> = HashMap::new();
    for (name, ty) in store_cells {
        field_scope.insert(name.clone(), *ty);
    }

    for inv in invariants {
        // Reject cross-agent references and impure/effectful constructs before
        // type-checking, so the bespoke diagnostics win over any cascade.
        if let Some(span) = predicate_cross_agent_ref(&inv.predicate, input) {
            errors.push(
                CompileError::new(
                    "bynk.invariant.cross_agent_reference",
                    span,
                    format!(
                        "invariant `{}` references another agent; invariants constrain a \
                         single agent's reachable states",
                        inv.name.name
                    ),
                )
                .with_note(
                    "a property that genuinely spans agents belongs in a saga or a scenario, \
                     not an invariant — see §14",
                ),
            );
            continue;
        }
        if let Some(span) = predicate_impure_construct(&inv.predicate) {
            errors.push(
                CompileError::new(
                    "bynk.invariant.impure_predicate",
                    span,
                    format!(
                        "invariant `{}` uses an effectful or test-only construct; invariant \
                         predicates must be pure",
                        inv.name.name
                    ),
                )
                .with_note(
                    "an invariant predicate may read state fields and call pure value methods, \
                     but not perform effects",
                ),
            );
            continue;
        }

        let bool_ty = tys.intern(Ty::Base(BaseType::Bool));
        let mut ctx = Ctx {
            input,
            tys,
            expr_types,
            errors,
            refs,
            hints,
            locals,
            requirements,
            callees,
            scopes: vec![field_scope.clone()],
            is_binding_cache: HashMap::new(),
            admitted_literals: HashMap::new(),
            pattern_binding_types: HashMap::new(),
            let_else_removals: HashMap::new(),
            return_ty: bool_ty,
            return_ty_span: inv.predicate.span,
            // A predicate is a pure expression — effectful operations (capability
            // calls, `<-`) are not permitted and are rejected as type errors.
            effectful: false,
            agent_state_ty: None,
            commit_seen: false,
            caps: CapabilityCtx {
                capabilities: HashMap::new(),
                declared_capabilities: HashMap::new(),
                given_remaining: HashSet::new(),
                given_used: HashSet::new(),
                given_entries: Vec::new(),
                given_anchor: None,
            },
            in_test_body: false,
            oidc_binder: None,
            test_services: HashMap::new(),
            test_actors: HashMap::new(),
            type_vars: HashSet::new(),
            eq: EqScope::default(),
            last_witness: None,
            store_fields: HashMap::new(),
            in_agent_handler: false,
            in_service_handler: false,
        };
        let pred_ty = type_of(&inv.predicate, Some(bool_ty), &mut ctx);
        if let Some(t) = pred_ty
            && t.base(tys) != Some(BaseType::Bool)
        {
            ctx.errors.push(
                CompileError::new(
                    "bynk.invariant.not_bool",
                    inv.predicate.span,
                    format!(
                        "invariant `{}` predicate has type `{}`, but an invariant must be `Bool`",
                        inv.name.name,
                        t.display(tys)
                    ),
                )
                .with_note("an invariant predicate is a `Bool`-valued property of the state"),
            );
        }
    }
}

/// Check a function's contract clauses (v0.115 §, testing track slice 3). A
/// contract is the invariant predicate attached to a function (ADR 0144 — one
/// predicate surface): each `requires`/`ensures` is a pure `Bool`-typed
/// expression, `requires` over the parameters and `ensures` over the parameters
/// plus `result` (the return value; the awaited element for an `Effect`). The
/// pass enforces, mirroring [`check_invariants`]:
///
/// - `bynk.contract.duplicate_name` — two clauses (across `requires`/`ensures`)
///   share a name; the name rides the failure report and dedup.
/// - `bynk.contract.result_in_requires` — a precondition references `result`
///   (the return value is not yet bound on entry).
/// - `bynk.contract.impure_predicate` — a clause uses an effectful or test-only
///   construct (Effect, `?` propagation, `expect`, `Val`).
/// - `bynk.contract.not_bool` — a clause does not type to `Bool`.
///
/// Distinct from ADR 0127's capability `@requires` annotation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_contracts(
    requires: &[Contract],
    ensures: &[Contract],
    // The function's parameters in scope by bare name (plus `self` for a
    // method), the shared predicate scope for both clause kinds.
    param_scope: &HashMap<String, TyId>,
    // The declared return type, awaited for an `Effect` — the type of `result`
    // inside an `ensures` predicate.
    result_ty: TyId,
    // True when a parameter is literally named `result`; then `result` in a
    // `requires` is that parameter, not the (unbound) return value.
    has_result_param: bool,
    fn_label: &str,
    input: &ResolvedCommons,
    expr_types: &mut HashMap<ExprId, TypedExpr>,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
    requirements: &mut RequirementSink,
    callees: &mut HashMap<ExprId, Callee>,
    type_vars: &HashSet<String>,
    eq: &EqScope,
    tys: &Types,
) {
    // Duplicate-name check across *all* clauses — the name is the dedup key for
    // the failure report and the redundant-test flag, so it is unique per fn.
    let mut seen: HashMap<&str, ()> = HashMap::new();
    for c in requires.iter().chain(ensures.iter()) {
        if seen.insert(c.name.name.as_str(), ()).is_some() {
            errors.push(
                CompileError::new(
                    "bynk.contract.duplicate_name",
                    c.name.span,
                    format!(
                        "{fn_label} declares more than one contract clause named `{}`",
                        c.name.name
                    ),
                )
                .with_note("give each `requires`/`ensures` clause a distinct name"),
            );
        }
    }

    // Type-check one clause predicate in the given scope, emitting the shared
    // impurity / non-`Bool` diagnostics.
    let check_clause = |c: &Contract,
                        scope: HashMap<String, TyId>,
                        expr_types: &mut HashMap<ExprId, TypedExpr>,
                        errors: &mut Vec<CompileError>,
                        refs: &mut RefSink,
                        hints: &mut HintSink,
                        locals: &mut LocalsSink,
                        requirements: &mut RequirementSink,
                        callees: &mut HashMap<ExprId, Callee>| {
        if let Some(span) = predicate_impure_construct(&c.predicate) {
            errors.push(
                CompileError::new(
                    "bynk.contract.impure_predicate",
                    span,
                    format!(
                        "contract clause `{}` uses an effectful or test-only construct; a \
                             contract predicate must be pure",
                        c.name.name
                    ),
                )
                .with_note(
                    "a contract predicate may read the parameters (and `result`) and call \
                         pure value methods, but not perform effects",
                ),
            );
            return;
        }
        let bool_ty = tys.intern(Ty::Base(BaseType::Bool));
        let mut ctx = Ctx {
            input,
            tys,
            expr_types,
            errors,
            refs,
            hints,
            locals,
            requirements,
            callees,
            scopes: vec![scope],
            is_binding_cache: HashMap::new(),
            admitted_literals: HashMap::new(),
            pattern_binding_types: HashMap::new(),
            let_else_removals: HashMap::new(),
            return_ty: bool_ty,
            return_ty_span: c.predicate.span,
            effectful: false,
            agent_state_ty: None,
            commit_seen: false,
            caps: CapabilityCtx {
                capabilities: HashMap::new(),
                declared_capabilities: HashMap::new(),
                given_remaining: HashSet::new(),
                given_used: HashSet::new(),
                given_entries: Vec::new(),
                given_anchor: None,
            },
            in_test_body: false,
            oidc_binder: None,
            test_services: HashMap::new(),
            test_actors: HashMap::new(),
            type_vars: type_vars.clone(),
            eq: eq.clone(),
            last_witness: None,
            store_fields: HashMap::new(),
            in_agent_handler: false,
            in_service_handler: false,
        };
        let pred_ty = type_of(&c.predicate, Some(bool_ty), &mut ctx);
        if let Some(t) = pred_ty
            && t.base(tys) != Some(BaseType::Bool)
        {
            ctx.errors.push(
                CompileError::new(
                    "bynk.contract.not_bool",
                    c.predicate.span,
                    format!(
                        "contract clause `{}` predicate has type `{}`, but a contract clause \
                             must be `Bool`",
                        c.name.name,
                        t.display(tys)
                    ),
                )
                .with_note("a contract predicate is a `Bool`-valued claim over the arguments"),
            );
        }
    };

    for c in requires {
        // `result` is the *return value* — not in scope on entry. A `requires`
        // that names it is a scope error with a bespoke diagnostic (unless a
        // parameter is literally named `result`, in which case it is that param).
        if !has_result_param && let Some(span) = predicate_references_result(&c.predicate) {
            errors.push(
                CompileError::new(
                    "bynk.contract.result_in_requires",
                    span,
                    format!(
                        "precondition `{}` references `result`, but the return value is not bound \
                         until the function returns",
                        c.name.name
                    ),
                )
                .with_note("`result` is only in scope inside an `ensures` clause"),
            );
            continue;
        }
        check_clause(
            c,
            param_scope.clone(),
            expr_types,
            errors,
            refs,
            hints,
            locals,
            requirements,
            callees,
        );
    }

    for c in ensures {
        // `ensures` scope = parameters + `result` (the return value; awaited for
        // an `Effect`). A parameter named `result` is shadowed by the binding.
        let mut scope = param_scope.clone();
        scope.insert("result".to_string(), result_ty);
        check_clause(
            c,
            scope,
            expr_types,
            errors,
            refs,
            hints,
            locals,
            requirements,
            callees,
        );
    }
}

/// Check an agent's step invariants (v0.116 §, testing track slice 4). A
/// `transition` is the invariant predicate widened to the *step* (ADR 0144 — one
/// predicate surface): a pure `Bool` predicate over the `old`/`new` state pair,
/// each bound to the agent's synthetic state record (`state_ty`), so `old.status`
/// / `new.status` resolve like any record field. The pass enforces, mirroring
/// [`check_invariants`]:
///
/// - `bynk.transition.duplicate_name` — two transitions share a name (the name
///   rides the `InvariantViolation` failure report).
/// - `bynk.transition.impure_predicate` — a predicate uses an effectful or
///   test-only construct.
/// - `bynk.transition.no_step_reference` — a predicate references neither `old`
///   nor `new`; it is a snapshot claim misfiled as a step (use `invariant`).
/// - `bynk.transition.not_bool` — a predicate does not type to `Bool`.
///
/// Placement is enforced structurally by the grammar (a `transition` is an
/// agent-body-only declaration), so there is no "transition on a non-agent"
/// diagnostic to raise here.
#[allow(clippy::too_many_arguments)]
pub fn check_transitions(
    transitions: &[Transition],
    // The agent's synthetic state record type — both `old` and `new` are bound to
    // it, so `old.field` / `new.field` read as the field's element type.
    state_ty: TyId,
    agent_name: &str,
    // Resolved commons carrying the synthetic `<Agent>State` record so field
    // access on `old`/`new` resolves.
    input: &ResolvedCommons,
    expr_types: &mut HashMap<ExprId, TypedExpr>,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
    requirements: &mut RequirementSink,
    callees: &mut HashMap<ExprId, Callee>,
    tys: &Types,
) {
    // Duplicate-name check across the agent's transitions.
    let mut seen: HashMap<&str, ()> = HashMap::new();
    for tr in transitions {
        if seen.insert(tr.name.name.as_str(), ()).is_some() {
            errors.push(
                CompileError::new(
                    "bynk.transition.duplicate_name",
                    tr.name.span,
                    format!(
                        "agent `{agent_name}` declares more than one transition named `{}`",
                        tr.name.name
                    ),
                )
                .with_note("give each transition a distinct name"),
            );
        }
    }

    // Both `old` and `new` are in scope as the state record.
    let mut scope: HashMap<String, TyId> = HashMap::new();
    scope.insert("old".to_string(), state_ty);
    scope.insert("new".to_string(), state_ty);

    for tr in transitions {
        // Reject cross-agent references and impure constructs before type-checking,
        // so the bespoke diagnostics win over any cascade.
        if let Some(span) = predicate_cross_agent_ref(&tr.predicate, input) {
            errors.push(
                CompileError::new(
                    "bynk.transition.cross_agent_reference",
                    span,
                    format!(
                        "transition `{}` references another agent; a step invariant \
                         constrains a single agent's own state move",
                        tr.name.name
                    ),
                )
                .with_note(
                    "a property that genuinely spans agents belongs in a saga or a scenario, \
                     not a transition",
                ),
            );
            continue;
        }
        if let Some(span) = predicate_impure_construct(&tr.predicate) {
            errors.push(
                CompileError::new(
                    "bynk.transition.impure_predicate",
                    span,
                    format!(
                        "transition `{}` uses an effectful or test-only construct; a step \
                         invariant predicate must be pure",
                        tr.name.name
                    ),
                )
                .with_note(
                    "a transition predicate may read the `old`/`new` state and call pure value \
                     methods, but not perform effects",
                ),
            );
            continue;
        }
        // A transition that mentions neither `old` nor `new` is not a step claim —
        // it is a snapshot invariant misfiled. Flag it conservatively.
        if predicate_references_old_or_new(&tr.predicate).is_none() {
            errors.push(
                CompileError::new(
                    "bynk.transition.no_step_reference",
                    tr.predicate.span,
                    format!(
                        "transition `{}` references neither `old` nor `new`, so it constrains a \
                         single state, not a step",
                        tr.name.name
                    ),
                )
                .with_note(
                    "a claim about one committed state is an `invariant`, not a `transition`",
                ),
            );
            continue;
        }

        let bool_ty = tys.intern(Ty::Base(BaseType::Bool));
        let mut ctx = Ctx {
            input,
            tys,
            expr_types,
            errors,
            refs,
            hints,
            locals,
            requirements,
            callees,
            scopes: vec![scope.clone()],
            is_binding_cache: HashMap::new(),
            admitted_literals: HashMap::new(),
            pattern_binding_types: HashMap::new(),
            let_else_removals: HashMap::new(),
            return_ty: bool_ty,
            return_ty_span: tr.predicate.span,
            effectful: false,
            agent_state_ty: None,
            commit_seen: false,
            caps: CapabilityCtx {
                capabilities: HashMap::new(),
                declared_capabilities: HashMap::new(),
                given_remaining: HashSet::new(),
                given_used: HashSet::new(),
                given_entries: Vec::new(),
                given_anchor: None,
            },
            in_test_body: false,
            oidc_binder: None,
            test_services: HashMap::new(),
            test_actors: HashMap::new(),
            type_vars: HashSet::new(),
            eq: EqScope::default(),
            last_witness: None,
            store_fields: HashMap::new(),
            in_agent_handler: false,
            in_service_handler: false,
        };
        let pred_ty = type_of(&tr.predicate, Some(bool_ty), &mut ctx);
        if let Some(t) = pred_ty
            && t.base(tys) != Some(BaseType::Bool)
        {
            ctx.errors.push(
                CompileError::new(
                    "bynk.transition.not_bool",
                    tr.predicate.span,
                    format!(
                        "transition `{}` predicate has type `{}`, but a transition must be `Bool`",
                        tr.name.name,
                        t.display(tys)
                    ),
                )
                .with_note("a transition predicate is a `Bool`-valued property of the state move"),
            );
        }
    }
}

/// If the predicate references `old` or `new` (a bare identifier) anywhere,
/// return the span of the first such reference. Used to flag a `transition` that
/// makes no step claim.
fn predicate_references_old_or_new(e: &Expr) -> Option<Span> {
    match &e.kind {
        ExprKind::Ident(id) if id.name == "old" || id.name == "new" => Some(id.span),
        _ => bynk_syntax::ast::expr_children(e)
            .into_iter()
            .find_map(predicate_references_old_or_new),
    }
}

/// If the predicate references `result` (a bare identifier) anywhere, return the
/// span of the first such reference. Used to reject `result` in a `requires`.
fn predicate_references_result(e: &Expr) -> Option<Span> {
    match &e.kind {
        ExprKind::Ident(id) if id.name == "result" => Some(id.span),
        _ => bynk_syntax::ast::expr_children(e)
            .into_iter()
            .find_map(predicate_references_result),
    }
}

/// If the predicate references another agent (by bare name, call, or qualified
/// constructor), return the span of the first such reference. Used by the
/// invariant well-formedness pass to forbid cross-agent predicates.
fn predicate_cross_agent_ref(e: &Expr, input: &ResolvedCommons) -> Option<Span> {
    let is_agent = |name: &str| input.agents.contains_key(name);
    match &e.kind {
        ExprKind::Ident(id) if is_agent(&id.name) => Some(id.span),
        ExprKind::Call { name, .. } if is_agent(&name.name) => Some(name.span),
        ExprKind::RecordConstruction { type_name, .. } if is_agent(&type_name.name) => {
            Some(type_name.span)
        }
        _ => bynk_syntax::ast::expr_children(e)
            .into_iter()
            .find_map(|c| predicate_cross_agent_ref(c, input)),
    }
}

/// State-migrations S2 (#2224, track §3.3): check each `migrate` step's body
/// on its own terms, with no lock: the old parameter names a record type, the
/// body is pure and key-free, each `:=` targets a `Cell` or `Map` field with a
/// value of that field's type, and no field is assigned twice (record-literal
/// semantics). What the lock alone can check (the old shape against the
/// recorded version, and which changes the step covers) is
/// `schema_registry::reconcile_agents`'s.
///
/// A bare current field reads its *carried* value, so the scope is the agent's
/// `Cell` fields (as for invariants) plus `old` and each `let` in order.
#[allow(clippy::too_many_arguments)]
pub fn check_migrations(
    agent: &AgentDecl,
    store_cells: &HashMap<String, TyId>,
    types: &HashMap<String, Arc<TypeDecl>>,
    input: &ResolvedCommons,
    tys: &Types,
    expr_types: &mut HashMap<ExprId, TypedExpr>,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
    requirements: &mut RequirementSink,
    callees: &mut HashMap<ExprId, Callee>,
) {
    let key = agent.key_name.name.as_str();
    let mut seen_from: HashMap<i64, ()> = HashMap::new();
    for step in &agent.migrations {
        if seen_from.insert(step.from, ()).is_some() {
            errors.push(
                CompileError::new(
                    "bynk.agent.migrate_duplicate",
                    step.from_span,
                    format!(
                        "agent `{}` declares more than one `migrate from {}` step",
                        agent.name.name, step.from
                    ),
                )
                .with_note("merge them into one step: a stored version runs exactly one step"),
            );
        }
        let mut scope: HashMap<String, TyId> = store_cells.clone();
        if let Some(old) = &step.old {
            let is_record = match &old.type_ref {
                TypeRef::Named(id) => types
                    .get(&id.name)
                    .is_some_and(|d| matches!(d.body, TypeBody::Record(_))),
                _ => false,
            };
            if !is_record {
                errors.push(
                    CompileError::new(
                        "bynk.agent.migrate_shape_mismatch",
                        old.name.span.merge(old.type_ref.span()),
                        format!(
                            "`migrate from {}`'s `{}` must name a record type declaring the \
                             version-{} fields the step reads",
                            step.from, old.name.name, step.from
                        ),
                    )
                    .with_note(
                        "declare the old shape as a type, e.g. `type CartV1 = { owner: String }`",
                    ),
                );
            } else if let Some(ty) = resolve_type_ref(&old.type_ref, types, tys) {
                record_type_refs(&old.type_ref, types, &HashSet::new(), refs);
                scope.insert(old.name.name.clone(), ty);
            }
        }
        // The value type a `:=` to each assignable field takes.
        let assignable: HashMap<&str, Option<TyId>> = agent
            .store_fields
            .iter()
            .filter_map(|f| {
                let value = match (f.kind.head.name.as_str(), f.kind.args.as_slice()) {
                    ("Cell", [t]) => resolve_type_ref(t, types, tys),
                    ("Map", [k, v]) => resolve_type_ref(
                        &TypeRef::Map(Box::new(k.clone()), Box::new(v.clone()), f.kind.span),
                        types,
                        tys,
                    ),
                    _ => return None,
                };
                Some((f.name.name.as_str(), value))
            })
            .collect();
        let mut assigned: HashMap<String, ()> = HashMap::new();
        let mut ctx = Ctx {
            input,
            tys,
            expr_types: &mut *expr_types,
            errors: &mut *errors,
            refs: &mut *refs,
            hints: &mut *hints,
            locals: &mut *locals,
            requirements: &mut *requirements,
            callees: &mut *callees,
            scopes: vec![scope],
            is_binding_cache: HashMap::new(),
            admitted_literals: HashMap::new(),
            pattern_binding_types: HashMap::new(),
            let_else_removals: HashMap::new(),
            return_ty: tys.intern(Ty::Unit),
            return_ty_span: step.span,
            // A step is pure: no capability calls, no `<-` (track §3.3).
            effectful: false,
            agent_state_ty: None,
            commit_seen: false,
            caps: CapabilityCtx {
                capabilities: HashMap::new(),
                declared_capabilities: HashMap::new(),
                given_remaining: HashSet::new(),
                given_used: HashSet::new(),
                given_entries: Vec::new(),
                given_anchor: None,
            },
            in_test_body: false,
            oidc_binder: None,
            test_services: HashMap::new(),
            test_actors: HashMap::new(),
            type_vars: HashSet::new(),
            eq: EqScope::default(),
            last_witness: None,
            store_fields: HashMap::new(),
            in_agent_handler: false,
            in_service_handler: false,
        };
        for item in &step.items {
            let value = match item {
                MigrateItem::Let(l) => Some(&l.value),
                MigrateItem::Assign(a) => Some(&a.value),
                MigrateItem::Drop(_) => None,
            };
            // A `let` rejected below still binds its name (to its annotation,
            // or the error type), so later items that use it report nothing
            // further (#2243 review).
            let bind_rejected_let = |ctx: &mut Ctx| {
                if let MigrateItem::Let(l) = item {
                    let ty = l
                        .type_annot
                        .as_ref()
                        .and_then(|t| resolve_type_ref(t, types, tys))
                        .unwrap_or_else(|| tys.intern(Ty::Error));
                    if let Some(top) = ctx.scopes.last_mut() {
                        top.insert(l.name.name.clone(), ty);
                    }
                }
            };
            if let Some(v) = value {
                if let Some(span) = predicate_impure_construct(v) {
                    ctx.errors.push(
                        CompileError::new(
                            "bynk.agent.migrate_impure",
                            span,
                            format!(
                                "`migrate from {}` uses an effectful or test-only construct; a \
                                 migration step must be pure",
                                step.from
                            ),
                        )
                        .with_note("a step may read `old` and the carried fields and call pure value methods"),
                    );
                    bind_rejected_let(&mut ctx);
                    continue;
                }
                if expr_reads_ident(v, key) && !ctx.scopes.iter().any(|s| s.contains_key(key)) {
                    ctx.errors.push(
                        CompileError::new(
                            "bynk.agent.migrate_impure",
                            v.span,
                            format!(
                                "`migrate from {}` reads the agent's key `{key}`; a migration \
                                 step is key-free",
                                step.from
                            ),
                        )
                        .with_note(
                            "a migration sweep wakes an instance by its object ID, which does not \
                             recover the key (track §3.6)",
                        ),
                    );
                    bind_rejected_let(&mut ctx);
                    continue;
                }
            }
            match item {
                MigrateItem::Let(l) => {
                    let annot = l
                        .type_annot
                        .as_ref()
                        .and_then(|t| resolve_type_ref(t, types, tys));
                    let ty = type_of(&l.value, annot, &mut ctx);
                    if let Some(ty) = annot.or(ty)
                        && let Some(top) = ctx.scopes.last_mut()
                    {
                        top.insert(l.name.name.clone(), ty);
                    }
                }
                MigrateItem::Assign(a) => {
                    let target = a.target.name.as_str();
                    if assigned.insert(target.to_string(), ()).is_some() {
                        ctx.errors.push(
                            CompileError::new(
                                "bynk.agent.migrate_duplicate_assignment",
                                a.target.span,
                                format!(
                                    "`migrate from {}` assigns `{target}` more than once",
                                    step.from
                                ),
                            )
                            .with_note(
                                "a step gives each field at most one value; a bare field reads \
                                 its carried value, never an earlier assignment",
                            ),
                        );
                    }
                    match assignable.get(target) {
                        Some(expected) => {
                            let _ = type_of(&a.value, *expected, &mut ctx);
                        }
                        None => {
                            let why = if agent.store_fields.iter().any(|f| f.name.name == target) {
                                "a `Set`, `Log` or `Cache` field has no value type to assign; \
                                 `drop` it to start it afresh"
                            } else {
                                "it is not one of the agent's `store` fields"
                            };
                            ctx.errors.push(CompileError::new(
                                "bynk.cell.invalid_target",
                                a.target.span,
                                format!(
                                    "`migrate from {}` cannot assign `{target}`: {why}",
                                    step.from
                                ),
                            ));
                        }
                    }
                }
                MigrateItem::Drop(_) => {}
            }
        }
    }
}

/// If the predicate contains an effectful or test-only construct, return its
/// span. Capability misuse (an effect operation in a pure context) is left to
/// the type checker; this catches the syntactically-impure surface.
pub(crate) fn predicate_impure_construct(e: &Expr) -> Option<Span> {
    match &e.kind {
        ExprKind::EffectPure(_)
        | ExprKind::Question(_)
        | ExprKind::Expect(_)
        | ExprKind::Val { .. }
        | ExprKind::Observation(_)
        | ExprKind::Faults(_)
        | ExprKind::Trace { .. } => Some(e.span),
        _ => bynk_syntax::ast::expr_children(e)
            .into_iter()
            .find_map(predicate_impure_construct),
    }
}

/// Whether `e` reads the identifier `name` anywhere — used by the `:=`
/// read-modify-write rule (a cell write whose RHS reads its own LHS).
pub(crate) fn expr_reads_ident(e: &Expr, name: &str) -> bool {
    match &e.kind {
        ExprKind::Ident(id) => id.name == name,
        _ => bynk_syntax::ast::expr_children(e)
            .into_iter()
            .any(|c| expr_reads_ident(c, name)),
    }
}

// ==== Checking context and capability metadata ====

/// v0.9.4: a compile-time-constant literal usable for static refinement
/// discharge during `T.of(...)` construction.
enum ConstLit {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Unit,
}

impl ConstLit {
    fn display(&self) -> String {
        match self {
            ConstLit::Int(n) => n.to_string(),
            ConstLit::Float(v) => v.to_string(),
            ConstLit::Str(s) => format!("{s:?}"),
            ConstLit::Bool(b) => b.to_string(),
            ConstLit::Unit => "()".to_string(),
        }
    }
}

/// Mutable per-function context.
/// Capability bookkeeping for the checker — the `given`-clause lifecycle and
/// capability dispatch, grouped out of the checker's working context
/// (v0.29.10). Empty (`Default`) for pure functions / non-context code.
#[derive(Default)]
pub struct CapabilityCtx {
    /// Capabilities in scope for the current handler, as a name → CapabilityInfo
    /// map. Empty for pure functions and non-context code.
    pub capabilities: HashMap<String, CapabilityInfo>,
    /// All capabilities declared in the surrounding context (for diagnostic
    /// purposes — used to detect `<Cap>.op(...)` calls where the capability is
    /// declared in the context but not listed in `given`).
    pub declared_capabilities: HashMap<String, CapabilityInfo>,
    /// Names of capabilities the user listed in `given`, but haven't yet
    /// observed used. After checking the body, anything left here is
    /// unused — a warning.
    pub given_remaining: HashSet<String>,
    /// Names of capabilities actually used in the body so far.
    pub given_used: HashSet<String>,
    /// v0.26 (ADR 0054): the `given` clause's entries in declaration order —
    /// (deps key, source span) — so the `given` quick-fixes can author
    /// list-aware edits at the diagnosis site. Empty where no `given` clause
    /// applies (fns, mock ops, state initialisers).
    pub given_entries: Vec<(String, Span)>,
    /// v0.26: where the add-capability fix synthesises an *absent* `given`
    /// clause — the handler's return type (the clause follows it). `None`
    /// where the clause lives elsewhere (a provider's `provides … given`
    /// line); the fix is then offered only when entries already exist.
    pub given_anchor: Option<Span>,
}

/// v0.178 (Slice 0, #662) / v0.182 (Slice A, #664): the shape a test body needs
/// to resolve a service invocation. Built by the project test pass from the
/// target unit's service declarations, so the checker can resolve the addressed
/// handler (`svc.call(...)` on an `on call` service, or — Slice A —
/// `svc.GET("/x")` / `svc.schedule("…")` / `svc.message(m)` on a `from http` /
/// `cron` / `queue` service) and check its arity, argument types, and principal.
#[derive(Debug, Clone)]
pub struct TestServiceSig {
    /// The service's protocol as an author-facing word (`"http"`, `"cron"`,
    /// `"queue"`, `"websocket"`), or `None` for a plain `service X { on call }`.
    pub protocol: Option<String>,
    /// Every handler the service declares, so the branch can resolve any address
    /// form. Slice 0 only reads the `on call` entry.
    pub handlers: Vec<TestHandler>,
    /// #2052: for a `from Events(E)` service, every event the project
    /// declares, so `svc.event(f)` with some other event `f` is reported as an
    /// event the service does not subscribe to. Empty for any other protocol.
    pub known_events: std::collections::BTreeSet<String>,
    /// #2052 review: for a `from Events(E)` service, the context that
    /// publishes `E` to it ([`crate::test_suites::event_publisher`]), which a
    /// default envelope names; `Err` lists the candidates when there is no
    /// unique one. `None` for any other protocol.
    pub event_publisher: Option<Result<String, Vec<String>>>,
}

/// One service handler, as a test body sees it (v0.178 / v0.182).
#[derive(Debug, Clone)]
pub struct TestHandler {
    pub kind: bynk_syntax::ast::HandlerKind,
    pub params: Vec<bynk_syntax::ast::Param>,
    /// The handler's declared `by <Actor>` clause, if any — the actor a call-site
    /// principal is checked against. `None` inherits the protocol default actor.
    pub by_clause: Option<bynk_syntax::ast::ByClause>,
    /// #1979, #1934: the handler's declared return type — what a case's
    /// call to it yields.
    pub return_type: bynk_syntax::ast::TypeRef,
    pub span: Span,
}

impl TestServiceSig {
    /// The `on call` handler, if the service declares one.
    pub fn call_handler(&self) -> Option<&TestHandler> {
        self.handlers
            .iter()
            .find(|h| matches!(h.kind, bynk_syntax::ast::HandlerKind::Call))
    }
}

/// One agent `store` field's kind and shape (finding #36) — the checker's
/// dispatch keys off this instead of five separate per-kind maps, so a new
/// storage kind is one new variant rather than a sixth map threaded through
/// every constructor and lookup site.
#[derive(Debug, Clone, Copy)]
pub enum StoreField {
    /// `store <name>: Cell[T]` — element type.
    Cell(TyId),
    /// `store <name>: Map[K, V]` — key, value.
    Map(TyId, TyId),
    /// `store <name>: Set[T]` — element type.
    Set(TyId),
    /// `store <name>: Cache[K, V] @ttl(...)` — key, value, TTL in milliseconds.
    Cache(TyId, TyId, i64),
    /// `store <name>: Log[T]` — element type.
    Log(TyId),
}

/// The checker's working context. `pub(crate)`: every caller outside this
/// crate goes through [`check_handler_body`] or [`check_body`] instead of
/// hand-building one — adding a field no longer needs auditing every
/// external construction site.
pub(crate) struct Ctx<'a> {
    pub input: &'a ResolvedCommons,
    /// T3.6b (R4.1): the unit's intern table. A shared `&` (the table is
    /// interior-mutable, see [`Types`]) so it stays `Copy` — a function that
    /// needs it reads `ctx.tys` once and is then free of the `ctx` borrow.
    /// Spelled `tys`, not `types`, because `input.types` next door is the
    /// unrelated `TypeDecl`-by-name declaration map.
    pub tys: &'a Types,
    pub expr_types: &'a mut HashMap<ExprId, TypedExpr>,
    pub errors: &'a mut Vec<CompileError>,
    /// v0.25 (ADR 0053): binding edges recorded at the checker's own
    /// resolution sites — capability/service dispatch, typed call dispatch,
    /// annotation resolution. Handler/test/provider bodies never pass
    /// through the resolver's reference walk, so the checker is their only
    /// recording point.
    pub refs: &'a mut RefSink,
    /// v0.27 (ADR 0056): inferred-type inlay hints recorded at the
    /// annotation-absent binding sites (`let` / `let <-` / lambda params)
    /// as the binding's final type is computed.
    pub hints: &'a mut HintSink,
    /// v0.31 (ADR 0064): local bindings recorded with their scope ranges at
    /// every binding site (`let`/`let <-`, params, match patterns), for the
    /// LSP's scope-at-offset query.
    pub locals: &'a mut LocalsSink,
    /// v0.99: the capability-requirement ledger — every capability-consuming
    /// site (direct call, store op), covered or not, recorded so the editor
    /// surfaces (the ghost `given` inlay hint, hover) can read it.
    pub requirements: &'a mut RequirementSink,
    /// P6.0 (#1139): the `Callee` classification sink — see [`Callee`].
    pub callees: &'a mut HashMap<ExprId, Callee>,
    /// Stack of in-scope name → type frames.
    pub scopes: Vec<HashMap<String, TyId>>,
    /// Memoised `is`-pattern bindings, keyed by the condition sub-expression's
    /// span. `collect_is_bindings` runs at every `&&`/`implies` node and, for a
    /// left-nested `&&` chain, would otherwise re-walk each lhs subtree once per
    /// enclosing node — O(N²) for an N-term chain. Because the collector is a
    /// pure read over `expr_types` (already populated by the time it runs) and
    /// spans are unique per body, caching each node's result collapses the walk
    /// to a single pass.
    pub is_binding_cache: HashMap<(ExprId, bool), Vec<(String, TyId)>>,
    /// #2039: literals inside a cross-context argument already admitted to
    /// the refined type at their position in the callee's parameter type,
    /// against the callee's declaration. `type_of` takes a literal's type
    /// from here before trying admission in the caller's own scope.
    pub admitted_literals: HashMap<ExprId, TyId>,
    /// T3.4: a pattern-bound name's resolved type, keyed by the binding
    /// `Ident`'s own span. Deliberately **not** `ExprId`-keyed and not
    /// folded into `expr_types` — a `Pattern::Binding` is not an `Expr` and
    /// giving `Ident` an id of its own would touch every identifier
    /// construction site in the workspace (field names, type names, params,
    /// …), not just the handful that bind. This is exactly the `PatId`
    /// reference draws as a *separate* identity from `ExprId` (Part 2) —
    /// out of this slice's scope on purpose, not overlooked.
    pub pattern_binding_types: HashMap<Span, TyId>,
    /// #2654: for each hidden `let … else` binder seen so far (by name), the
    /// source ranges to delete to drop its `else` — what the
    /// "remove the `else`" fix of an `else` that can never run applies.
    pub let_else_removals: HashMap<String, Vec<Span>>,
    pub return_ty: TyId,
    pub return_ty_span: Span,
    /// True if the enclosing function/handler returns `Effect[T]` (v0.5).
    /// Determines whether `<-` and capability calls are permitted.
    pub effectful: bool,
    /// If inside an agent handler, the agent's state type and the agent's
    /// name. Used to validate `commit` statements.
    pub agent_state_ty: Option<TyId>,
    /// True if a `commit` has been seen on the current control-flow path.
    /// Used to detect "two reachable commits".
    pub commit_seen: bool,
    /// Capability bookkeeping — the `given`-clause lifecycle + dispatch,
    /// grouped (v0.29.10). Empty for pure functions / non-context code.
    pub caps: CapabilityCtx,
    /// True when the body being checked is a test case body. Permits
    /// `expect` statements (v0.7; renamed from `assert` in v0.112).
    pub in_test_body: bool,
    /// #2380 (L5): the binder of an `Oidc` actor (`by who: Workload`), whose
    /// `.claims` reads the verified token's claims. `None` elsewhere.
    pub oidc_binder: Option<String>,
    /// The target unit's services, populated for test case bodies (v0.25).
    /// `svc.call(args)` in a test invokes the target's service; the checker
    /// resolves the service's `on call` handler here to check the call's
    /// arity and argument types, and records the binding edge so test-file
    /// references index. A service with no `on call` handler (a `from http`
    /// / `cron` / `queue` service) carries `None` for `call_handler`, which
    /// makes `svc.call(...)` a diagnostic rather than a silent runtime crash.
    pub test_services: HashMap<String, TestServiceSig>,
    /// v0.182 (Slice A, #664): the target unit's actor declarations, so a
    /// call-site `by <Actor>(<identity>)` can resolve the actor and type the
    /// identity value against its declared identity type. Prelude actors
    /// (`Visitor`, `Caller`, …) are resolved separately. Empty outside test
    /// bodies.
    pub test_actors: HashMap<String, bynk_syntax::ast::ActorDecl>,
    /// v0.20a: the enclosing function's type parameters (rigid vars), so
    /// nested explicit type arguments (`identity[A](x)` inside a generic
    /// body) resolve. Empty outside generic fn bodies.
    pub type_vars: HashSet<String>,
    /// Constrained generics slice 0: the `Eq` bounds declared in scope.
    pub eq: EqScope,
    /// Slice 1: the comparator witnesses of the generic call just checked, left
    /// by the generic-call path for the caller that holds the call's `ExprId` to
    /// take ([`Ctx::take_witnesses`]) and record on its [`Callee`].
    pub last_witness: Option<Vec<TyId>>,
    /// The agent's `store` fields, by name (finding #36: collapses the five
    /// former per-kind maps — `store_cells`/`store_maps`/`store_sets`/
    /// `store_caches`/`store_logs` — into one, since a field name can only
    /// ever be one kind). A `:=` write, a `<field>.<op>(…)` call, and the
    /// `.entries`/`.keys`/`.values` map accessors all resolve their target
    /// here, by receiver provenance. Empty outside `store`-bearing agent
    /// handlers.
    pub store_fields: HashMap<String, StoreField>,
    /// #2017: true while checking an agent handler body, lambdas included.
    /// Unlike [`agent_state_ty`](Self::agent_state_ty), a lambda frame does
    /// not clear it: a `~>` inside a closure still runs in the agent's
    /// handler, which carries no execution context, so the send gate must
    /// see through the frame swap.
    pub in_agent_handler: bool,
    /// #2683: true while checking a service handler body, lambdas included
    /// (a lambda frame does not clear it, as with
    /// [`in_agent_handler`](Self::in_agent_handler)). `e.within(d)` hands its
    /// abandoned effect to the execution context, which only a service
    /// handler's `deps` carry.
    pub in_service_handler: bool,
}

/// Per-capability info for checker dispatch within a handler body.
#[derive(Debug, Clone)]
pub struct CapabilityInfo {
    pub name: String,
    pub ops: Vec<CapabilityOpInfo>,
    /// S4a (#1966): for a derived capability (`capability Sessions = Kv`),
    /// its base as `(unit, capability)`. An operation call on the instance
    /// references the base's operation, where it is declared.
    pub derived_from: Option<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct CapabilityOpInfo {
    pub name: String,
    /// #926: the op's own type parameters (empty for a non-generic op).
    /// `params`/`return_ty` below are *pattern* types resolved with these in
    /// scope, so a declared `T` survives as `Ty::Var("T")` rather than
    /// collapsing to `Ty::Unit` — a call site substitutes a concrete `Ty` for
    /// each before checking arguments/return.
    pub type_params: Vec<String>,
    pub params: Vec<TyId>,
    /// The operation's parameter names, positionally aligned with `params`
    /// (v0.117). Needed for observation: the `with <pred>` scope binds them by
    /// name and `trace(Cap.op)` yields records with these fields.
    pub param_names: Vec<String>,
    pub return_ty: TyId,
}

/// The synthetic record type name for `trace(Cap.op)`'s call records (v0.117):
/// one record per capability operation, its fields the operation's parameters.
pub fn call_record_type_name(cap: &str, op: &str) -> String {
    format!("__{cap}_{op}_Call")
}

impl<'a> Ctx<'a> {
    pub fn lookup(&self, name: &str) -> Option<TyId> {
        for scope in self.scopes.iter().rev() {
            if let Some(t) = scope.get(name) {
                return Some(*t);
            }
        }
        None
    }

    /// Returns the type of an expression's "root identifier" — for `a.b.c`
    /// that's `a`; for a bare `a` it's `a`. Used to detect whether a chain's
    /// outermost name shadows an alias / consumed-context prefix.
    pub fn lookup_root_ident(&self, expr: &Expr) -> Option<TyId> {
        match &expr.kind {
            ExprKind::Ident(id) => self.lookup(&id.name),
            ExprKind::FieldAccess { receiver, .. } => self.lookup_root_ident(receiver),
            ExprKind::MethodCall { receiver, .. } => self.lookup_root_ident(receiver),
            _ => None,
        }
    }

    /// #1930: whether an expression's root ident names an asset in scope
    /// (`sheet` in `sheet.url.startsWith(…)`). An asset is a value, but not a
    /// local binding, so [`lookup_root_ident`](Self::lookup_root_ident) returns
    /// `None` for it, and its chain would otherwise read as a dotted context
    /// prefix (`sheet.url` as a context, `startsWith` as its service). Like
    /// [`root_ident_is_store_field`](Self::root_ident_is_store_field), it is
    /// asked only once the root has no local binding.
    pub fn root_ident_is_asset(&self, expr: &Expr) -> bool {
        match &expr.kind {
            // #2387: an embed is a value of the same kind (`shell.replace(…)`).
            ExprKind::Ident(id) => {
                self.input.assets.contains_key(&id.name) || self.input.embeds.contains_key(&id.name)
            }
            ExprKind::FieldAccess { receiver, .. } | ExprKind::MethodCall { receiver, .. } => {
                self.root_ident_is_asset(receiver)
            }
            _ => false,
        }
    }

    /// v0.158 (ADR 0184): whether an expression's root ident names an agent
    /// `store` field. A store field is not in the value scope (so
    /// [`lookup_root_ident`](Self::lookup_root_ident) returns `None` for it),
    /// yet `<map>.entries.…` / `<map>.values.…` chains root in one — this
    /// distinguishes them from an un-consumed cross-context prefix so the
    /// `map.entries` query accessor is not mistaken for a service call.
    pub fn root_ident_is_store_field(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Ident(id) => self.store_fields.contains_key(&id.name),
            ExprKind::FieldAccess { receiver, .. } | ExprKind::MethodCall { receiver, .. } => {
                self.root_ident_is_store_field(receiver)
            }
            _ => false,
        }
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    /// Push a frame already holding `scope`'s bindings.
    pub fn push_scope_with(&mut self, scope: HashMap<String, TyId>) {
        self.scopes.push(scope);
    }
    pub fn pop_scope(&mut self) {
        self.scopes.pop();
    }
    pub fn bind(&mut self, name: String, ty: TyId) {
        self.scopes.last_mut().unwrap().insert(name, ty);
    }
}

// ==== Type-system core (resolution, unification, compatibility, inference) ====

/// Build a `Ty` from a TypeDecl name reference.
pub fn type_from_decl(
    id: &Ident,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
) -> Option<TyId> {
    let decl = types.get(&id.name)?;
    Some(named_ty(decl, tys))
}

/// Build a `Ty::Named` for the given declaration with the given applied type
/// arguments (empty for a non-generic reference).
pub fn named_ty_with_args(decl: &TypeDecl, args: Vec<TyId>, tys: &Types) -> TyId {
    let kind = match &decl.body {
        TypeBody::Refined { base, .. } => NamedKind::Refined(*base),
        TypeBody::Record(_) => NamedKind::Record,
        TypeBody::Sum(_) => NamedKind::Sum,
        TypeBody::Opaque { base, .. } => NamedKind::Opaque(*base),
    };
    tys.intern(Ty::Named {
        name: decl.name.name.clone(),
        kind,
        args,
    })
}

/// Build a `Ty::Named` for the given declaration (no applied type arguments).
pub fn named_ty(decl: &TypeDecl, tys: &Types) -> TyId {
    named_ty_with_args(decl, Vec::new(), tys)
}

/// v0.158 (ADR 0184): the compiler-known `MapEntry[K, V]` record — the element
/// a `store Map[K, V]`'s `.entries` query yields. A nominal generic record
/// (`{ key: K, value: V }`), so it flows through `unify`/`compatible`/`display`
/// and the ADR 0183 non-boundary rule like any generic-record instantiation;
/// its fields are resolved by name in `check_field_access` (it has no
/// user-visible `TypeDecl`, like `JsonError`).
pub fn map_entry_ty(k: TyId, v: TyId, tys: &Types) -> TyId {
    tys.intern(Ty::Named {
        name: MAP_ENTRY.to_string(),
        kind: NamedKind::Record,
        args: vec![k, v],
    })
}

/// v0.157 (ADR 0183): the substitution mapping a generic record's declared
/// type parameters onto a concrete instantiation's arguments. Empty when the
/// type is non-generic or `args` is empty (an under-applied reference — the
/// resolver reports that separately).
pub fn type_param_subst(decl: &TypeDecl, args: &[TyId]) -> HashMap<String, TyId> {
    decl.type_params
        .iter()
        .map(|p| p.name.name.clone())
        .zip(args.iter().copied())
        .collect()
}

/// v0.157 (ADR 0183): the type of a generic record's field at a concrete
/// instantiation. The field's declared type is resolved with the declaration's
/// type parameters in scope as rigid vars, then those vars are replaced by the
/// instantiation's `args`. For a non-generic record this is a plain resolve.
pub fn instantiate_field_ty(
    decl: &TypeDecl,
    args: &[TyId],
    field_ref: &TypeRef,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
) -> Option<TyId> {
    if decl.type_params.is_empty() {
        return resolve_type_ref(field_ref, types, tys);
    }
    // Without a full argument set the substitution is partial and would leave a
    // rigid `Ty::Var` in the field type; an under-/over-applied reference is an
    // error the resolver reports, so field access yields no type here.
    if decl.type_params.len() != args.len() {
        return None;
    }
    let vars: HashSet<String> = decl
        .type_params
        .iter()
        .map(|p| p.name.name.clone())
        .collect();
    let field_ty = resolve_type_ref_in(field_ref, types, &vars, tys)?;
    Some(substitute(field_ty, &type_param_subst(decl, args), tys))
}

/// v0.20a: like [`resolve_type_ref`], with a set of in-scope **type
/// parameters**: a `Named` reference matching one resolves to [`Ty::Var`]
/// (checked before the type-table lookup — a type parameter shadows a
/// same-named declaration; the collision is diagnosed at the declaration).
pub fn resolve_type_ref_in(
    r: &TypeRef,
    types: &HashMap<String, Arc<TypeDecl>>,
    vars: &HashSet<String>,
    tys: &Types,
) -> Option<TyId> {
    let ty = match r {
        TypeRef::Named(id) if vars.contains(&id.name) => Ty::Var(id.name.clone()),
        TypeRef::Result(t, e, _) => Ty::Result(
            resolve_type_ref_in(t, types, vars, tys)?,
            resolve_type_ref_in(e, types, vars, tys)?,
        ),
        TypeRef::Option(t, _) => Ty::Option(resolve_type_ref_in(t, types, vars, tys)?),
        TypeRef::Effect(t, _) => Ty::Effect(resolve_type_ref_in(t, types, vars, tys)?),
        TypeRef::HttpResult(t, _) => Ty::HttpResult(resolve_type_ref_in(t, types, vars, tys)?),
        TypeRef::List(t, _) => Ty::List(resolve_type_ref_in(t, types, vars, tys)?),
        TypeRef::Set(t, _) => Ty::Set(resolve_type_ref_in(t, types, vars, tys)?),
        TypeRef::Query(t, _) => Ty::Query(resolve_type_ref_in(t, types, vars, tys)?),
        TypeRef::Stream(t, _) => Ty::Stream(resolve_type_ref_in(t, types, vars, tys)?),
        TypeRef::Connection(t, _) => Ty::Connection(resolve_type_ref_in(t, types, vars, tys)?),
        TypeRef::Map(k, v, _) => Ty::Map(
            resolve_type_ref_in(k, types, vars, tys)?,
            resolve_type_ref_in(v, types, vars, tys)?,
        ),
        TypeRef::Fn(params, ret, _) => {
            let params: Option<Vec<TyId>> = params
                .iter()
                .map(|p| resolve_type_ref_in(p, types, vars, tys))
                .collect();
            Ty::Fn {
                params: params?,
                ret: resolve_type_ref_in(ret, types, vars, tys)?,
            }
        }
        // v0.157 (ADR 0183): `Name[Arg, …]` — application of a user generic
        // type. Arguments resolve with the enclosing type parameters in scope;
        // existence/arity are validated in the resolver, so an unknown or
        // mis-applied name simply produces no type here.
        TypeRef::App { name, args, .. } => {
            let decl = types.get(&name.name)?;
            let args: Option<Vec<TyId>> = args
                .iter()
                .map(|a| resolve_type_ref_in(a, types, vars, tys))
                .collect();
            return Some(named_ty_with_args(decl, args?, tys));
        }
        _ => return resolve_type_ref(r, types, tys),
    };
    Some(tys.intern(ty))
}

/// v0.20a: substitute type variables in `t` per `subst`. Must be total when
/// instantiating a call (the uninferable check runs first); an unbound Var
/// passes through unchanged for partial substitution during inference.
pub(crate) fn substitute(t: TyId, subst: &HashMap<String, TyId>, tys: &Types) -> TyId {
    let node = tys.get(t);
    let substituted = match &*node {
        Ty::Var(n) => return subst.get(n).copied().unwrap_or(t),
        Ty::Result(a, b) => Ty::Result(substitute(*a, subst, tys), substitute(*b, subst, tys)),
        Ty::Option(a) => Ty::Option(substitute(*a, subst, tys)),
        Ty::Effect(a) => Ty::Effect(substitute(*a, subst, tys)),
        Ty::HttpResult(a) => Ty::HttpResult(substitute(*a, subst, tys)),
        Ty::HttpOutcome(a) => Ty::HttpOutcome(substitute(*a, subst, tys)),
        Ty::List(a) => Ty::List(substitute(*a, subst, tys)),
        Ty::Set(a) => Ty::Set(substitute(*a, subst, tys)),
        Ty::Query(a) => Ty::Query(substitute(*a, subst, tys)),
        Ty::Stream(a) => Ty::Stream(substitute(*a, subst, tys)),
        Ty::Connection(a) => Ty::Connection(substitute(*a, subst, tys)),
        Ty::Map(k, v) => Ty::Map(substitute(*k, subst, tys), substitute(*v, subst, tys)),
        Ty::Fn { params, ret } => Ty::Fn {
            params: params.iter().map(|p| substitute(*p, subst, tys)).collect(),
            ret: substitute(*ret, subst, tys),
        },
        // v0.157 (ADR 0183): a generic named type's arguments may carry vars —
        // substitution recurses into them (an under-applied bare reference has
        // empty `args`, so this is a no-op there).
        Ty::Named { name, kind, args } => Ty::Named {
            name: name.clone(),
            kind: kind.clone(),
            args: args.iter().map(|a| substitute(*a, subst, tys)).collect(),
        },
        // Leaves (no inner type to ground) and the sealed actor bindings
        // (boundary-minted, never Var-bearing). Enumerated — no `_` — so a
        // new `Ty` variant must state whether substitution recurses into it.
        // `Ty::Error` is a leaf by construction (R4.3): it never carries a
        // `Var` to ground. T3.6b: a leaf substitutes to itself, and its `TyId`
        // is already that value — return it rather than re-interning.
        Ty::Error
        | Ty::Base(_)
        | Ty::QueueResult
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::TokenClaims
        | Ty::ValidationError
        | Ty::JsonError
        | Ty::Unit
        | Ty::Actor(_)
        | Ty::ActorSum(_) => return t,
    };
    tys.intern(substituted)
}

/// v0.20a: does `t` still contain a type variable?
pub(crate) fn contains_var(t: TyId, tys: &Types) -> bool {
    match &*tys.get(t) {
        Ty::Var(_) => true,
        // R4.3: `Ty::Error` is a leaf; it never carries a `Var`.
        Ty::Error => false,
        Ty::Result(a, b) | Ty::Map(a, b) => contains_var(*a, tys) || contains_var(*b, tys),
        Ty::Option(a)
        | Ty::Effect(a)
        | Ty::HttpResult(a)
        | Ty::HttpOutcome(a)
        | Ty::List(a)
        | Ty::Set(a)
        | Ty::Query(a)
        | Ty::Stream(a)
        | Ty::Connection(a) => contains_var(*a, tys),
        Ty::Fn { params, ret } => {
            params.iter().any(|p| contains_var(*p, tys)) || contains_var(*ret, tys)
        }
        // v0.157 (ADR 0183): a generic named type's arguments may carry vars.
        Ty::Named { args, .. } => args.iter().any(|a| contains_var(*a, tys)),
        Ty::Base(_)
        | Ty::QueueResult
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::TokenClaims
        | Ty::ValidationError
        | Ty::JsonError
        | Ty::Unit
        | Ty::Actor(_)
        | Ty::ActorSum(_) => false,
    }
}

/// v0.20b: does `t` contain a type variable that is NOT one of the enclosing
/// function's rigid type parameters? Rigid vars are fully constrained inside
/// the body; only flexible (call-site instantiation) vars mean "still being
/// inferred".
fn contains_flexible_var(t: TyId, rigid: &HashSet<String>, tys: &Types) -> bool {
    match &*tys.get(t) {
        Ty::Var(n) => !rigid.contains(n),
        // R4.3: `Ty::Error` is a leaf; it never carries a `Var`.
        Ty::Error => false,
        Ty::Result(a, b) | Ty::Map(a, b) => {
            contains_flexible_var(*a, rigid, tys) || contains_flexible_var(*b, rigid, tys)
        }
        Ty::Option(a)
        | Ty::Effect(a)
        | Ty::HttpResult(a)
        | Ty::HttpOutcome(a)
        | Ty::List(a)
        | Ty::Set(a)
        | Ty::Query(a)
        | Ty::Stream(a)
        | Ty::Connection(a) => contains_flexible_var(*a, rigid, tys),
        Ty::Fn { params, ret } => {
            params.iter().any(|p| contains_flexible_var(*p, rigid, tys))
                || contains_flexible_var(*ret, rigid, tys)
        }
        // v0.157 (ADR 0183): a generic named type's arguments may carry vars.
        Ty::Named { args, .. } => args.iter().any(|a| contains_flexible_var(*a, rigid, tys)),
        Ty::Base(_)
        | Ty::QueueResult
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::TokenClaims
        | Ty::ValidationError
        | Ty::JsonError
        | Ty::Unit
        | Ty::Actor(_)
        | Ty::ActorSum(_) => false,
    }
}

/// v0.20a: argument-directed unification. Walks `pattern` (possibly
/// Var-bearing) against the ground `actual`; a Var binds on first sight and
/// must match its prior binding **exactly** afterwards (keep inference dumb
/// and predictable — the explicit `name[T](…)` form is the pressure valve).
/// Returns false on a conflict; structural mismatches are NOT reported here —
/// the post-substitution `compatible` check owns those diagnostics.
pub(crate) fn unify(
    pattern: TyId,
    actual: TyId,
    subst: &mut HashMap<String, TyId>,
    tys: &Types,
) -> bool {
    // T3.6b: bind the two nodes first — the `Rc`s must outlive the `match`
    // they are destructured by, and a `TyId` pair is not itself matchable.
    let (p_node, a_node) = (tys.get(pattern), tys.get(actual));
    match (&*p_node, &*a_node) {
        (Ty::Var(n), _) => match subst.get(n) {
            // T3.6b (R4.1): "matches its prior binding exactly" is now a
            // `TyId` comparison — one `u32` equality, where it used to be a
            // recursive structural walk. Interning is what makes the two
            // equivalent.
            Some(bound) => *bound == actual,
            None => {
                subst.insert(n.clone(), actual);
                true
            }
        },
        (Ty::Result(a1, b1), Ty::Result(a2, b2)) | (Ty::Map(a1, b1), Ty::Map(a2, b2)) => {
            unify(*a1, *a2, subst, tys) && unify(*b1, *b2, subst, tys)
        }
        (Ty::Option(a1), Ty::Option(a2))
        | (Ty::Effect(a1), Ty::Effect(a2))
        | (Ty::HttpResult(a1), Ty::HttpResult(a2))
        | (Ty::HttpOutcome(a1), Ty::HttpOutcome(a2))
        | (Ty::List(a1), Ty::List(a2))
        | (Ty::Set(a1), Ty::Set(a2))
        | (Ty::Query(a1), Ty::Query(a2))
        | (Ty::Stream(a1), Ty::Stream(a2))
        | (Ty::Connection(a1), Ty::Connection(a2)) => unify(*a1, *a2, subst, tys),
        (
            Ty::Fn {
                params: p1,
                ret: r1,
            },
            Ty::Fn {
                params: p2,
                ret: r2,
            },
        ) => {
            p1.len() == p2.len()
                && p1
                    .iter()
                    .zip(p2)
                    .all(|(a, b)| unify(*a, *b, subst, tys))
                && unify(*r1, *r2, subst, tys)
        }
        // v0.157 (ADR 0183): a generic named type binds vars through its
        // arguments — `Paginated[T]` against `Paginated[User]` binds `T=User`.
        (
            Ty::Named {
                name: n1, args: a1, ..
            },
            Ty::Named {
                name: n2, args: a2, ..
            },
        ) if n1 == n2 && a1.len() == a2.len() && !a1.is_empty() => {
            a1.iter().zip(a2).all(|(x, y)| unify(*x, *y, subst, tys))
        }
        // Ground-vs-ground: any pair is fine here; `compatible` owns the
        // real check after substitution. The left side is enumerated — no
        // `_` — so a new inner-type-bearing `Ty` variant must add its
        // recursion arm above instead of silently skipping unification.
        (
            Ty::Base(_)
            | Ty::Named { .. }
            | Ty::Result(..)
            | Ty::Option(_)
            | Ty::Effect(_)
            | Ty::HttpResult(_)
            | Ty::HttpOutcome(_)
            | Ty::QueueResult
            | Ty::BoundaryRejection
            | Ty::Asset
            | Ty::RequestHeaders
            | Ty::TokenClaims
            | Ty::List(_)
            | Ty::Set(_)
            | Ty::Map(..)
            | Ty::Query(_)
            | Ty::Stream(_)
            | Ty::Connection(_)
            | Ty::ValidationError
            | Ty::JsonError
            | Ty::Unit
            | Ty::Actor(_)
            | Ty::ActorSum(_)
            | Ty::Fn { .. }
            // R4.3: an already-diagnosed subtree unifies with anything —
            // the failure was reported once, at the site that produced
            // `Ty::Error`; unification is not where a second one belongs.
            | Ty::Error,
            _,
        ) => true,
    }
}

/// v0.25 (ADR 0053): record a binding edge for every `Named` reference
/// inside a type-ref that resolved. Called alongside the `resolve_type_ref*`
/// annotation sites; `skip` holds the enclosing fn's type parameters (rigid
/// vars are not type symbols). Handler signatures and body annotations never
/// pass through the resolver's reference walk, so these sites are their only
/// recording point; where both passes run, assembly dedupes.
pub fn record_type_refs(
    r: &TypeRef,
    types: &HashMap<String, Arc<TypeDecl>>,
    skip: &HashSet<String>,
    refs: &mut RefSink,
) {
    match r {
        TypeRef::Named(id) => {
            if types.contains_key(&id.name) && !skip.contains(&id.name) {
                refs.record(id.span, SymbolKind::Type, &id.name);
            }
        }
        TypeRef::Fn(params, ret, _) => {
            for p in params {
                record_type_refs(p, types, skip, refs);
            }
            record_type_refs(ret, types, skip, refs);
        }
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            record_type_refs(a, types, skip, refs);
            record_type_refs(b, types, skip, refs);
        }
        TypeRef::Option(t, _)
        | TypeRef::Effect(t, _)
        | TypeRef::HttpResult(t, _)
        | TypeRef::Query(t, _)
        | TypeRef::Stream(t, _)
        | TypeRef::Connection(t, _)
        | TypeRef::History(t, _)
        | TypeRef::List(t, _)
        | TypeRef::Set(t, _) => record_type_refs(t, types, skip, refs),
        // v0.157 (ADR 0183): a `Name[Arg, …]` application records the generic
        // type's name plus every argument.
        TypeRef::App { name, args, .. } => {
            if types.contains_key(&name.name) && !skip.contains(&name.name) {
                refs.record(name.span, SymbolKind::Type, &name.name);
            }
            for a in args {
                record_type_refs(a, types, skip, refs);
            }
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

/// #712: resolve a type reference that appears in an *expression* position the
/// resolver does not walk for handler bodies — explicit call type arguments
/// (`identity[T](x)`), `Json.decode[T]`, and lambda parameter annotations
/// (`(x: T) => …`). On failure the reference is silently dropped by the bare
/// `resolve_type_ref_in`, so an unknown type in a handler body would compile
/// clean; this reports `bynk.resolve.unknown_type` instead, and records the
/// resolved type's references for the IDE on success. The resolver still covers
/// `fn`/method bodies, and the checker runs only after the resolver returns Ok
/// (`bynk-emit`'s pipeline sequences `resolve(..)?` then `check(..)`), so this
/// never double-reports.
pub(crate) fn resolve_expr_type_ref(r: &TypeRef, ctx: &mut Ctx) -> Option<TyId> {
    let tys = ctx.tys;
    match resolve_type_ref_in(r, &ctx.input.types, &ctx.type_vars, tys) {
        Some(ty) => {
            record_type_refs(r, &ctx.input.types, &ctx.type_vars, ctx.refs);
            Some(ty)
        }
        None => {
            ctx.errors.push(unresolved_type_ref_error(
                r,
                &ctx.input.types,
                &ctx.type_vars,
            ));
            None
        }
    }
}

/// #712: the diagnostic for a type reference that fails to resolve. Points at
/// the exact offending name when one can be identified (`identity[Missing](5)`
/// → the `Missing` span), falling back to the whole reference otherwise.
fn unresolved_type_ref_error(
    r: &TypeRef,
    types: &HashMap<String, Arc<TypeDecl>>,
    vars: &HashSet<String>,
) -> CompileError {
    match first_unresolved_type_name(r, types, vars) {
        Some(id) => CompileError::new(
            "bynk.resolve.unknown_type",
            id.span,
            format!("unknown type `{}`", id.name),
        )
        .with_note(
            "in scope are the base types (`Int`, `Float`, `String`, `Bool`, `Duration`, \
             `Instant`, `Bytes`), the built-in generics (`List`, `Map`, `Option`, `Result`, …), \
             `ValidationError`, and the types this unit declares or imports",
        ),
        None => CompileError::new(
            "bynk.resolve.unknown_type",
            r.span(),
            "this type does not resolve",
        ),
    }
}

/// #712: the first type name in `r` that names neither a declared type nor an
/// in-scope type variable — the reason `resolve_type_ref_in` returned `None`.
fn first_unresolved_type_name<'a>(
    r: &'a TypeRef,
    types: &HashMap<String, Arc<TypeDecl>>,
    vars: &HashSet<String>,
) -> Option<&'a Ident> {
    match r {
        TypeRef::Named(id) => {
            (!types.contains_key(&id.name) && !vars.contains(&id.name)).then_some(id)
        }
        TypeRef::App { name, args, .. } => {
            if !types.contains_key(&name.name) && !vars.contains(&name.name) {
                return Some(name);
            }
            args.iter()
                .find_map(|a| first_unresolved_type_name(a, types, vars))
        }
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            first_unresolved_type_name(a, types, vars)
                .or_else(|| first_unresolved_type_name(b, types, vars))
        }
        TypeRef::Option(t, _)
        | TypeRef::Effect(t, _)
        | TypeRef::HttpResult(t, _)
        | TypeRef::List(t, _)
        | TypeRef::Set(t, _)
        | TypeRef::Query(t, _)
        | TypeRef::Stream(t, _)
        | TypeRef::Connection(t, _)
        | TypeRef::History(t, _) => first_unresolved_type_name(t, types, vars),
        TypeRef::Fn(params, ret, _) => params
            .iter()
            .find_map(|p| first_unresolved_type_name(p, types, vars))
            .or_else(|| first_unresolved_type_name(ret, types, vars)),
        TypeRef::Base(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::ValidationError(_)
        | TypeRef::JsonError(_)
        | TypeRef::Unit(_) => None,
    }
}

/// v0.154 (ADR 0178): the declared error embedding that converts `source_err`
/// into `target_err`, if one exists. When `target_err` is a sum declaring
/// `embeds E as V` with `E` compatible with `source_err`, returns
/// `(sum_type_name, variant_name)` — the variant a value of `source_err`
/// auto-wraps into. One level only: the source must match a declared embedding
/// directly. Used by `?` in the checker (to accept the conversion) and the
/// emitter (to lower the `Err`-wrap) from the **same** rule, so the two cannot
/// diverge.
pub fn embedding_for(
    target_err: TyId,
    source_err: TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
) -> Option<(String, String)> {
    let target_node = tys.get(target_err);
    let Ty::Named { name, .. } = &*target_node else {
        return None;
    };
    let decl = types.get(name)?;
    let TypeBody::Sum(sum) = &decl.body else {
        return None;
    };
    for clause in &sum.embeds {
        if let Some(src) = resolve_type_ref(&clause.source_type, types, tys)
            && compatible(source_err, src, tys)
        {
            return Some((name.clone(), clause.variant.name.clone()));
        }
    }
    None
}

pub fn resolve_type_ref(
    r: &TypeRef,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
) -> Option<TyId> {
    let ty = match r {
        TypeRef::Base(b, _) => Ty::Base(*b),
        TypeRef::Named(id) => return type_from_decl(id, types, tys),
        // v0.20a: a function type. Effectfulness is structural (ret is
        // Effect[_]); nothing extra to record.
        TypeRef::Fn(params, ret, _) => {
            let params: Option<Vec<TyId>> = params
                .iter()
                .map(|p| resolve_type_ref(p, types, tys))
                .collect();
            Ty::Fn {
                params: params?,
                ret: resolve_type_ref(ret, types, tys)?,
            }
        }
        TypeRef::Result(t, e, _) => Ty::Result(
            resolve_type_ref(t, types, tys)?,
            resolve_type_ref(e, types, tys)?,
        ),
        TypeRef::Option(t, _) => Ty::Option(resolve_type_ref(t, types, tys)?),
        TypeRef::Effect(t, _) => Ty::Effect(resolve_type_ref(t, types, tys)?),
        TypeRef::HttpResult(t, _) => Ty::HttpResult(resolve_type_ref(t, types, tys)?),
        TypeRef::List(t, _) => Ty::List(resolve_type_ref(t, types, tys)?),
        TypeRef::Set(t, _) => Ty::Set(resolve_type_ref(t, types, tys)?),
        TypeRef::Query(t, _) => Ty::Query(resolve_type_ref(t, types, tys)?),
        TypeRef::Stream(t, _) => Ty::Stream(resolve_type_ref(t, types, tys)?),
        TypeRef::Connection(t, _) => Ty::Connection(resolve_type_ref(t, types, tys)?),
        TypeRef::Map(k, v, _) => Ty::Map(
            resolve_type_ref(k, types, tys)?,
            resolve_type_ref(v, types, tys)?,
        ),
        TypeRef::QueueResult(_) => Ty::QueueResult,
        TypeRef::Asset(_) => Ty::Asset,
        TypeRef::RequestHeaders(_) => Ty::RequestHeaders,
        TypeRef::TokenClaims(_) => Ty::TokenClaims,
        TypeRef::BoundaryRejection(_) => Ty::BoundaryRejection,
        // v0.119 (ADR 0155): `History[Agent]` is not a value type — it is a
        // test-only generator handled directly in `check_property_body`. It never
        // resolves as an ordinary type, so a stray `History[…]` in a value
        // position fails to resolve (the resolver reports `outside_property`).
        TypeRef::History(_, _) => return None,
        // v0.157 (ADR 0183): `Name[Arg, …]` — a user generic-type application.
        TypeRef::App { name, args, .. } => {
            let decl = types.get(&name.name)?;
            let args: Option<Vec<TyId>> = args
                .iter()
                .map(|a| resolve_type_ref(a, types, tys))
                .collect();
            return Some(named_ty_with_args(decl, args?, tys));
        }
        TypeRef::ValidationError(_) => Ty::ValidationError,
        TypeRef::JsonError(_) => Ty::JsonError,
        TypeRef::Unit(_) => Ty::Unit,
    };
    Some(tys.intern(ty))
}

/// `t` is usable where `u` is expected.
///
/// T3.6b: deliberately **no** `t == u` fast path, tempting as interning makes
/// one. `compatible` is not reflexive — `Actor`/`ActorSum` are sealed boundary
/// values that fall through to the `false` arm below even against themselves
/// (they are matched, never assigned), so short-circuiting on id equality
/// would silently make them assignable.
pub fn compatible(t: TyId, u: TyId, tys: &Types) -> bool {
    let (t_node, u_node) = (tys.get(t), tys.get(u));
    match (&*t_node, &*u_node) {
        // R4.3: `Ty::Error` is compatible with everything, in both positions
        // — the failure that produced it was already diagnosed at its own
        // site, and a mismatch diagnostic naming it here would be a second
        // report of the same failure, not a new one. Ordered first so it
        // takes priority over the more specific arms below.
        (Ty::Error, _) | (_, Ty::Error) => true,
        (Ty::Base(a), Ty::Base(b)) => a == b,
        // v0.157 (ADR 0183): two named types are compatible when they share a
        // name and kind and their applied type arguments are pairwise
        // compatible. Records are immutable (`readonly` fields), so the
        // arguments are covariant — like `List`/`Option`.
        (
            Ty::Named {
                name: a,
                kind: ka,
                args: aa,
            },
            Ty::Named {
                name: b,
                kind: kb,
                args: ba,
            },
        ) => {
            a == b
                && ka == kb
                && aa.len() == ba.len()
                && aa.iter().zip(ba).all(|(x, y)| compatible(*x, *y, tys))
        }
        // Refined → base (widening).
        (
            Ty::Named {
                kind: NamedKind::Refined(b),
                ..
            },
            Ty::Base(target),
        ) => b == target,
        (Ty::Base(_), Ty::Named { .. }) => false,
        (Ty::Result(t1, e1), Ty::Result(t2, e2)) => {
            compatible(*t1, *t2, tys) && compatible(*e1, *e2, tys)
        }
        (Ty::Option(a), Ty::Option(b)) => compatible(*a, *b, tys),
        (Ty::Effect(a), Ty::Effect(b)) => compatible(*a, *b, tys),
        (Ty::HttpResult(a), Ty::HttpResult(b)) | (Ty::HttpOutcome(a), Ty::HttpOutcome(b)) => {
            compatible(*a, *b, tys)
        }
        // v0.20b: collections are covariant in their element/value types;
        // Map keys must match exactly — key-position widening would split a
        // map's keys across refined/base identities at lookup time.
        (Ty::List(a), Ty::List(b)) => compatible(*a, *b, tys),
        // K6a (#2596): a `Set`'s element must match exactly, as a `Map` key must.
        (Ty::Set(a), Ty::Set(b)) => a == b,
        // v0.100: `Stream[T]` is covariant in its element, like `List`/`Effect`.
        // (Assignability only — streams are not value-comparable for `==`.)
        (Ty::Stream(a), Ty::Stream(b)) => compatible(*a, *b, tys),
        // v0.91: `Query[T]` is covariant in its element, like `List`/`Stream`.
        // (Assignability only — queries are not value-comparable for `==`.)
        (Ty::Query(a), Ty::Query(b)) => compatible(*a, *b, tys),
        // v0.102: a `Connection[F]` is assignable to itself (the linearity pass
        // governs the move). Held values have identity, not value-equality, so
        // they are not `==`-comparable (guarded in the `Eq`/`NotEq` arm).
        (Ty::Connection(a), Ty::Connection(b)) => compatible(*a, *b, tys),
        (Ty::Map(k1, v1), Ty::Map(k2, v2)) => k1 == k2 && compatible(*v1, *v2, tys),
        (Ty::QueueResult, Ty::QueueResult) | (Ty::BoundaryRejection, Ty::BoundaryRejection) => true,
        (Ty::Asset, Ty::Asset) => true,
        (Ty::RequestHeaders, Ty::RequestHeaders) => true,
        (Ty::TokenClaims, Ty::TokenClaims) => true,
        (Ty::ValidationError, Ty::ValidationError) => true,
        (Ty::JsonError, Ty::JsonError) => true,
        (Ty::Unit, Ty::Unit) => true,
        // v0.20a: function types — **contravariant** in parameters, covariant
        // in the return type. `compatible(t, u, tys)` is "t usable where u is
        // expected" and is already asymmetric (refined → base widening), so
        // the per-position argument order flips for params: a function
        // expecting the *wider* param type is usable where one expecting the
        // narrower is required — and crucially, the covariant direction would
        // let unvalidated base values flow into a refined-typed body.
        (Ty::Fn { params: p, ret: r }, Ty::Fn { params: q, ret: s }) => {
            p.len() == q.len()
                && p.iter().zip(q).all(|(a, b)| compatible(*b, *a, tys))
                && compatible(*r, *s, tys)
        }
        // v0.20a: rigid type variables (a generic fn's own body) match by
        // name. Flexible vars never reach `compatible` — they are eliminated
        // by substitution during call-site instantiation.
        (Ty::Var(a), Ty::Var(b)) => a == b,
        // Everything else is incompatible: cross-variant pairs, and the
        // sealed boundary values (`Actor`/`ActorSum` are only ever matched,
        // never assigned). The left side is enumerated — no `_` — so adding
        // a `Ty` variant fails to compile here instead of silently making
        // the new type incompatible with itself (the trap `Query` fell into).
        (
            Ty::Base(_)
            | Ty::Named { .. }
            | Ty::Result(..)
            | Ty::Option(_)
            | Ty::Effect(_)
            | Ty::HttpResult(_)
            | Ty::HttpOutcome(_)
            | Ty::QueueResult
            | Ty::BoundaryRejection
            | Ty::Asset
            | Ty::RequestHeaders
            | Ty::TokenClaims
            | Ty::List(_)
            | Ty::Set(_)
            | Ty::Map(..)
            | Ty::Query(_)
            | Ty::Stream(_)
            | Ty::Connection(_)
            | Ty::ValidationError
            | Ty::JsonError
            | Ty::Unit
            | Ty::Actor(_)
            | Ty::ActorSum(_)
            | Ty::Fn { .. }
            | Ty::Var(_),
            _,
        ) => false,
    }
}

/// Check a `do e` statement: the effect-context requirement (when
/// `check_context`) and that the operand is an `Effect[()]`.
pub(super) fn check_do(d: &DoStmt, check_context: bool, ctx: &mut Ctx) {
    let tys = ctx.tys;
    // v0.146 (ADR 0170): `do e` — perform a unit effect as a
    // statement. Effectful context only, like `<-`; nothing is bound,
    // so the operand MUST be `Effect[()]`. A valued reply is rejected
    // (`bynk.effect.do_requires_unit`): throwing away a real result
    // stays explicit with `let _ <- e`.
    if check_context && !ctx.effectful {
        ctx.errors.push(
                        CompileError::new(
                            "bynk.effect.do_in_pure_context",
                            d.span,
                            "the `do` statement can only be used inside an effectful body (one returning `Effect[T]`)",
                        )
                        .with_label(
                            ctx.return_ty_span,
                            format!("enclosing return type is `{}`", ctx.return_ty.display(tys)),
                        )
                        .with_note(
                            "change the enclosing function/handler's return type to `Effect[...]`",
                        ),
                    );
    }
    let unit = tys.intern(Ty::Unit);
    let expected = tys.intern(Ty::Effect(unit));
    let rhs_ty = type_of(&d.value, Some(expected), ctx);
    check_unbound_effects(&d.value, true, ctx);
    match rhs_ty.map(|t| tys.get(t)).as_deref() {
        Some(Ty::Effect(inner)) if *inner == unit => {}
        Some(Ty::Effect(inner)) => {
            ctx.errors.push(
                            CompileError::new(
                                "bynk.effect.do_requires_unit",
                                d.value.span,
                                format!(
                                    "a `do` statement requires an `Effect[()]`, but this is `Effect[{}]` — its result would be silently dropped",
                                    inner.display(tys)
                                ),
                            )
                            .with_note(
                                "`do e` performs a unit effect; to await and discard a real result, write `let _ <- e` instead",
                            ),
                        );
        }
        Some(other) => {
            ctx.errors.push(
                CompileError::new(
                    "bynk.effect.do_on_non_effect",
                    d.value.span,
                    format!(
                        "a `do` statement requires an `Effect[()]` value, but got `{}`",
                        other.display(tys)
                    ),
                )
                .with_note(
                    "`do` performs an effect; its operand must be a call returning `Effect[()]`",
                ),
            );
        }
        None => {}
    }
}

/// Check one `let x <- e` binding: the effect-context requirement (when
/// `check_context`), the annotation, the `Effect[T]` payload and the call-site
/// principal. Returns the bound type, or `None` when the value failed to type
/// (already reported). Binding the name is the caller's job, so a `par` block can
/// bind its names only after every binding is checked.
pub(super) fn check_effect_let(l: &LetStmt, check_context: bool, ctx: &mut Ctx) -> Option<TyId> {
    let tys = ctx.tys;
    if check_context && !ctx.effectful {
        ctx.errors.push(
                CompileError::new(
                    "bynk.effect.bind_in_pure_context",
                    l.span,
                    "the `<-` operator can only be used inside an effectful body (one returning `Effect[T]`)",
                )
                .with_label(
                    ctx.return_ty_span,
                    format!("enclosing return type is `{}`", ctx.return_ty.display(tys)),
                )
                .with_note(
                    "change the enclosing function/handler's return type to `Effect[...]`, or use `let ... =` for a pure binding",
                ),
            );
    }
    // Determine the inner Effect[T] payload type for the binding.
    let annot_ty = l.type_annot.as_ref().and_then(|a| {
        // v0.20b: the enclosing fn's type parameters are legal
        // in body annotations (`let init: List[B] = …`).
        let r = resolve_type_ref_in(a, &ctx.input.types, &ctx.type_vars, tys);
        if r.is_none() {
            ctx.errors.push(CompileError::new(
                "bynk.resolve.unknown_type",
                a.span(),
                "type in `let` annotation does not resolve",
            ));
        } else {
            record_type_refs(a, &ctx.input.types, &ctx.type_vars, ctx.refs);
        }
        r
    });
    // The expected type for the RHS is `Effect[annot]` if annot present.
    let rhs_expected = annot_ty.map(|t| tys.intern(Ty::Effect(t)));
    let mut rhs_ty = type_of(&l.value, rhs_expected, ctx);
    // #1934: a test-service address driven `by Nobody` observes the
    // auth seam, so it yields an `HttpOutcome`. The principal is on
    // the statement, so the call alone cannot say so.
    if l.principal.as_ref().is_some_and(|p| {
        p.actor.name == "Nobody" || calls::is_bad_signature_principal(&p.actor.name)
    }) && let Some(t) = rhs_ty
    {
        let t = calls::nobody_outcome(&l.value, t, ctx);
        if let Some(te) = ctx.expr_types.get_mut(&l.value.id) {
            te.ty = t;
        }
        rhs_ty = Some(t);
    }
    check_unbound_effects(&l.value, true, ctx);
    // v0.182 (#664): validate the call-site principal against the
    // addressed handler — including the *absent* case, where an
    // identity-carrying handler driven with no `by` would silently
    // drop the identity.
    calls::check_effect_let_principal(&l.value, l.principal.as_ref(), ctx);
    let inner_ty = match rhs_ty.map(|t| tys.get(t)).as_deref() {
        Some(Ty::Effect(t)) => Some(*t),
        Some(_) => {
            ctx.errors.push(
                CompileError::new(
                    "bynk.effect.bind_on_non_effect",
                    l.value.span,
                    format!(
                        "the `<-` operator requires an `Effect[T]` value, but got `{}`",
                        rhs_ty.expect("matched Some").display(tys)
                    ),
                )
                .with_note(
                    "use `let ... =` for a pure binding, or wrap the value with `Effect.pure(...)`",
                ),
            );
            None
        }
        None => None,
    };
    let final_ty = match (annot_ty, inner_ty) {
        (Some(annot), Some(rhs)) => {
            if !compatible(rhs, annot, tys) {
                ctx.errors.push(CompileError::new(
                        "bynk.types.let_annotation_mismatch",
                        l.value.span,
                        format!(
                            "let-binding's value has type `Effect[{}]`, but the annotation declares `Effect[{}]`",
                            rhs.display(tys),
                            annot.display(tys)
                        ),
                    ));
            }
            annot
        }
        (Some(annot), None) => annot,
        (None, Some(rhs)) => rhs,
        // #1663: the value failed to type (already reported).
        // Bind the name to the error type anyway, so its uses
        // absorb instead of echoing as unknown names.
        (None, None) => return None,
    };
    Some(final_ty)
}

/// Bind a checked `let x <- e`'s name in the innermost scope and record its inlay
/// hint and local, visible from `scope_start` to `scope_end`.
pub(super) fn bind_effect_let(
    l: &LetStmt,
    final_ty: TyId,
    scope_start: usize,
    scope_end: usize,
    ctx: &mut Ctx,
) {
    let tys = ctx.tys;
    if l.name.name != "_" {
        // v0.27 (ADR 0056): as for `let =`, but `final_ty` here
        // is the peeled `Effect[T]` payload — the binding's
        // actual type, which is what the hint must show.
        // #2010: a `let … else`'s hidden binder is not the author's
        // name, so it gets no inlay hint and no entry in the locals.
        if !is_generated_binding(&l.name.name) {
            if l.type_annot.is_none() {
                ctx.hints
                    .record(l.name.span, format!(": {}", final_ty.display(tys)));
            }
            ctx.locals.record(
                l.name.name.clone(),
                l.name.span,
                crate::locals::LocalKind::Let,
                final_ty.display(tys),
                Span {
                    file: l.span.file,
                    start: scope_start,
                    end: scope_end,
                },
            );
        }
        ctx.bind(l.name.name.clone(), final_ty);
    }
}

pub(crate) fn type_of_block(block: &Block, expected: Option<TyId>, ctx: &mut Ctx) -> Option<TyId> {
    let tys = ctx.tys;
    ctx.push_scope();
    for stmt in &block.statements {
        if let Statement::Let(l) | Statement::EffectLet(l) = stmt
            && let Some(le) = &l.let_else
            && !le.remove_else.is_empty()
        {
            ctx.let_else_removals
                .insert(l.name.name.clone(), le.remove_else.clone());
        }
        match stmt {
            Statement::Let(l) => {
                let annot_ty = l.type_annot.as_ref().and_then(|a| {
                    // v0.20b: the enclosing fn's type parameters are legal
                    // in body annotations (`let init: List[B] = …`).
                    let r = resolve_type_ref_in(a, &ctx.input.types, &ctx.type_vars, tys);
                    if r.is_none() {
                        ctx.errors.push(CompileError::new(
                            "bynk.resolve.unknown_type",
                            a.span(),
                            "type in `let` annotation does not resolve",
                        ));
                    } else {
                        record_type_refs(a, &ctx.input.types, &ctx.type_vars, ctx.refs);
                    }
                    r
                });
                let rhs_ty = type_of(&l.value, annot_ty, ctx);
                check_unbound_effects(&l.value, false, ctx);
                let final_ty = match (annot_ty, rhs_ty) {
                    (Some(annot), Some(rhs)) => {
                        if !compatible(rhs, annot, tys) {
                            ctx.errors.push(
                                CompileError::new(
                                    "bynk.types.let_annotation_mismatch",
                                    l.value.span,
                                    format!(
                                        "let binding's value has type `{}`, but the annotation declares `{}`",
                                        rhs.display(tys),
                                        annot.display(tys)
                                    ),
                                )
                                .with_label(
                                    l.type_annot.as_ref().unwrap().span(),
                                    "declared type annotation",
                                ),
                            );
                        }
                        annot
                    }
                    (Some(annot), None) => annot,
                    (None, Some(rhs)) => rhs,
                    // #1663: the value failed to type (already reported).
                    // Bind the name to the error type anyway, so its uses
                    // absorb instead of echoing as unknown names.
                    (None, None) => {
                        if l.name.name != "_" {
                            ctx.bind(l.name.name.clone(), tys.intern(Ty::Error));
                        }
                        continue;
                    }
                };
                if l.name.name != "_" {
                    // #2010: a `let … else`'s hidden binder is not the author's
                    // name, so it gets no inlay hint and no entry in the locals.
                    if !is_generated_binding(&l.name.name) {
                        // v0.27 (ADR 0056): an annotation-absent binding gets an
                        // inferred-type inlay hint at the binding name.
                        if l.type_annot.is_none() {
                            ctx.hints
                                .record(l.name.span, format!(": {}", final_ty.display(tys)));
                        }
                        // v0.31: in scope from after this statement to block end.
                        ctx.locals.record(
                            l.name.name.clone(),
                            l.name.span,
                            crate::locals::LocalKind::Let,
                            final_ty.display(tys),
                            Span {
                                file: l.span.file,
                                start: l.span.end,
                                end: block.span.end,
                            },
                        );
                    }
                    ctx.bind(l.name.name.clone(), final_ty);
                }
            }
            Statement::EffectLet(l) => {
                let Some(final_ty) = check_effect_let(l, true, ctx) else {
                    // #1663: the value failed to type (already reported).
                    // Bind the name to the error type anyway, so its uses
                    // absorb instead of echoing as unknown names.
                    if l.name.name != "_" {
                        ctx.bind(l.name.name.clone(), tys.intern(Ty::Error));
                    }
                    continue;
                };
                bind_effect_let(l, final_ty, l.span.end, block.span.end, ctx);
            }
            Statement::Expect(a) => {
                if !ctx.in_test_body {
                    ctx.errors.push(
                        CompileError::new(
                            "bynk.expect.outside_case",
                            a.span,
                            "`expect` is only valid inside a `case` body",
                        )
                        .with_note(
                            "expectations verify predicates at test runtime; use them only inside `case \"...\" { ... }` blocks",
                        ),
                    );
                }
                let val_ty = type_of(&a.value, Some(tys.intern(Ty::Base(BaseType::Bool))), ctx);
                if let Some(actual) = val_ty
                    && !compatible(actual, tys.intern(Ty::Base(BaseType::Bool)), tys)
                {
                    ctx.errors.push(CompileError::new(
                        "bynk.expect.not_bool",
                        a.value.span,
                        format!(
                            "`expect` predicate has type `{}`, but a `Bool` is required",
                            actual.display(tys),
                        ),
                    ));
                }
            }
            Statement::Advance(a) => check_advance(a, ctx),
            Statement::Par(p) => par::check_par(p, block.span.end, ctx),
            Statement::Send(s) => {
                // v0.79: `~> e` — fire-and-forget. Effectful context only, like
                // `<-`; the reply is never awaited, so nothing is bound.
                if !ctx.effectful {
                    ctx.errors.push(
                        CompileError::new(
                            "bynk.send.in_pure_context",
                            s.span,
                            "the `~>` send can only be used inside an effectful body (one returning `Effect[T]`)",
                        )
                        .with_label(
                            ctx.return_ty_span,
                            format!("enclosing return type is `{}`", ctx.return_ty.display(tys)),
                        )
                        .with_note(
                            "change the enclosing function/handler's return type to `Effect[...]`",
                        ),
                    );
                }
                // The reply must be `Effect[()]`. A real payload (value or error)
                // would be silently dropped by a fire-and-forget send — the error
                // gate ([DECISION C/D]). `let _ <- e` is the honest spelling for
                // "await and discard".
                let unit = tys.intern(Ty::Unit);
                let expected = tys.intern(Ty::Effect(unit));
                let rhs_ty = type_of(&s.value, Some(expected), ctx);
                // #2017: an agent handler has no execution context to hand a
                // fire-and-forget promise to — it runs inside the agent's
                // Durable Object, whose `deps` carry no `__exec` — so a `~>`
                // there checked clean and then failed `tsc`. Refuse it here,
                // on both targets. A store operation gets its own code: it is
                // not a message at all but a write to the handler's staged
                // state, committed atomically at the handler boundary.
                // Typed first, so the RHS's own errors still surface and the
                // `Callee::Store` classification (recorded by `type_of`) is
                // available. When this fires, the reply-shape checks below
                // are skipped: their remedy is moot once the send itself is
                // refused.
                if ctx.in_agent_handler {
                    if let Some(Callee::Store { field, op }) = ctx.callees.get(&s.value.id) {
                        let msg = format!(
                            "`~> {field}.{op}(…)` sends a store operation, but a store write is part of the agent handler's atomic commit — there is nothing to send without waiting for"
                        );
                        let note = format!(
                            "drop the `~>` and perform the write as a plain statement, `do {field}.{op}(…)`, or with `let _ <- {field}.{op}(…)`; it is staged and committed with the rest of the handler"
                        );
                        ctx.errors.push(
                            CompileError::new("bynk.send.store_op", s.span, msg).with_note(note),
                        );
                    } else {
                        ctx.errors.push(
                            CompileError::new(
                                "bynk.send.in_agent_handler",
                                s.span,
                                "the `~>` send cannot be used in an agent handler: an agent handler has no execution context to keep a fire-and-forget send alive",
                            )
                            .with_note(
                                "await the effect instead with `let _ <- ...` (or `do ...` for an `Effect[()]`), or move the send to the service handler that calls the agent",
                            ),
                        );
                    }
                }
                match rhs_ty.map(|t| tys.get(t)).as_deref() {
                    _ if ctx.in_agent_handler => {}
                    Some(Ty::Effect(inner)) if *inner == unit => {}
                    Some(Ty::Effect(inner)) => {
                        ctx.errors.push(
                            CompileError::new(
                                "bynk.send.requires_unit",
                                s.value.span,
                                format!(
                                    "`~>` requires an `Effect[()]` reply, but this send returns `Effect[{}]` — its result would be silently dropped",
                                    inner.display(tys)
                                ),
                            )
                            .with_note(
                                "a `~>` send never awaits a reply, so it is reserved for empty replies; to await and discard a real result, write `let _ <- ...` instead",
                            ),
                        );
                    }
                    Some(other) => {
                        ctx.errors.push(
                            CompileError::new(
                                "bynk.send.non_effect",
                                s.value.span,
                                format!(
                                    "the `~>` send requires an `Effect[()]` value, but got `{}`",
                                    other.display(tys)
                                ),
                            )
                            .with_note("`~>` sends an effectful call; the target must be a call returning `Effect[()]`"),
                        );
                    }
                    None => {}
                }
            }
            Statement::Do(d) => check_do(d, true, ctx),
            Statement::Assign(a) => {
                // v0.81 (storage track): `cell := expr` — the unconditional `Cell`
                // write. The target must be a `store Cell` field; the value must
                // match the cell's element type; and (the §10 read-modify-write
                // rule) the RHS must not read the cell being written.
                match ctx.store_fields.get(&a.target.name).cloned() {
                    // A name that isn't a store field at all, or is one of a
                    // different kind, is the same "not a Cell" diagnostic.
                    None
                    | Some(
                        StoreField::Map(..)
                        | StoreField::Set(_)
                        | StoreField::Cache(..)
                        | StoreField::Log(_),
                    ) => {
                        ctx.errors.push(
                            CompileError::new(
                                "bynk.cell.invalid_target",
                                a.target.span,
                                format!(
                                    "`:=` writes a `Cell` store field, but `{}` is not one",
                                    a.target.name
                                ),
                            )
                            .with_note(
                                "the `:=` write form applies only to a `store <name>: Cell[T]` field",
                            ),
                        );
                        type_of(&a.value, None, ctx);
                    }
                    Some(StoreField::Cell(elem_ty)) => {
                        // §10: a `:=` whose RHS reads its own LHS is a hidden
                        // read-modify-write — require `.update(fn)` instead, so the
                        // dependency is visible (and retry-safe).
                        if expr_reads_ident(&a.value, &a.target.name) {
                            ctx.errors.push(
                                CompileError::new(
                                    "bynk.cell.self_reference",
                                    a.span,
                                    format!(
                                        "the `:=` right-hand side reads `{0}`, the cell being \
                                         written — this is a read-modify-write",
                                        a.target.name
                                    ),
                                )
                                .with_note(
                                    "use `<cell>.update(fn)` for a read-modify-write so the \
                                     dependency on the prior value is explicit",
                                ),
                            );
                        }
                        if let Some(vt) = type_of(&a.value, Some(elem_ty), ctx)
                            && !compatible(vt, elem_ty, tys)
                        {
                            ctx.errors.push(CompileError::new(
                                "bynk.types.type_mismatch",
                                a.value.span,
                                format!(
                                    "this `:=` writes `{}`, but the cell `{}` holds `{}`",
                                    vt.display(tys),
                                    a.target.name,
                                    elem_ty.display(tys)
                                ),
                            ));
                        }
                    }
                }
            }
        }
    }
    let ty = type_of(&block.tail, expected, ctx);
    check_unbound_effects(&block.tail, true, ctx);
    let ty = maybe_auto_lift(ty, expected, tys);
    // T3.4: this block previously wrote its own auto-lifted type into
    // `expr_types` at `block.span` (bug #844's era — recording it only when
    // `block.span != block.tail.span`, to avoid clobbering a synthetic
    // single-expression block's more specific tail entry). `Block` has no
    // `ExprId` of its own to key that write with now, and — checked, not
    // assumed — nothing in the workspace ever read it: the only caller that
    // has a real enclosing expression to attribute it to (`ExprKind::Block`,
    // `checker.rs`'s own `type_of` dispatch) already gets an identical entry
    // for free from `type_of`'s own choke-point write on the way back out,
    // since the parser sets that expression's span to `block.span` exactly.
    // The other eight callers (function/handler bodies, `if` branches,
    // `match` arm bodies) never had a real position to attribute it to
    // either, span-keyed or not. Dropped rather than worked around.
    ctx.pop_scope();
    ty
}

/// v0.7.1 tail-position auto-lift. If the expected type is `Effect[T]` and
/// the computed type is `T` (not itself an `Effect[_]`), lift it to
/// `Effect[T]`. Otherwise leave the type alone — the surrounding compatibility
/// check will report any genuine mismatch.
fn maybe_auto_lift(ty: Option<TyId>, expected: Option<TyId>, tys: &Types) -> Option<TyId> {
    if let Some(actual) = ty
        && let Some(exp) = expected
        && let Ty::Effect(et) = &*tys.get(exp)
        && !actual.is_effect(tys)
        && compatible(actual, *et, tys)
    {
        return Some(tys.intern(Ty::Effect(actual)));
    }
    ty
}

/// Whether a value of type `ty` may fill an interpolation hole (v0.43, ADR
/// 0075): a base scalar, or a refinement of one (which widens to its base for
/// display). Opaque types are excluded — their base is hidden, so a value must
/// be `.raw`-ed out first.
fn interpolable(ty: TyId, tys: &Types) -> bool {
    matches!(
        &*tys.get(ty),
        Ty::Base(_)
            | Ty::Named {
                kind: NamedKind::Refined(_),
                ..
            }
    )
}

pub(crate) fn type_of(expr: &Expr, expected: Option<TyId>, ctx: &mut Ctx) -> Option<TyId> {
    let tys = ctx.tys;
    let admitted = ctx.admitted_literals.get(&expr.id).copied();
    let ty = match &expr.kind {
        // #2039: a literal a cross-context argument already admitted to the
        // callee's refined type at its position.
        _ if admitted.is_some() => admitted,
        // v0.9.4: a literal in a refined-expected position takes the refined
        // type (validated now); otherwise it keeps its base type.
        // v0.20a: a lambda. With an expected function type, params type
        // contextually and the body checks against the expected return; in an
        // unconstrained position, every param must be annotated and
        // effectfulness is inferred bottom-up by a syntactic pre-scan.
        ExprKind::Lambda(lambda) => check_lambda(lambda, expected, ctx),
        ExprKind::IntLit { .. } => {
            admit_refined_literal(expr, expected, ctx).or(Some(tys.intern(Ty::Base(BaseType::Int))))
        }
        ExprKind::FloatLit { .. } => admit_refined_literal(expr, expected, ctx)
            .or(Some(tys.intern(Ty::Base(BaseType::Float)))),
        // v0.86 (ADR 0112): a `Duration` literal always takes the base
        // `Duration` (no refined `Duration` types exist).
        ExprKind::DurationLit { .. } => Some(tys.intern(Ty::Base(BaseType::Duration))),
        ExprKind::StrLit(_) => admit_refined_literal(expr, expected, ctx)
            .or(Some(tys.intern(Ty::Base(BaseType::String)))),
        // An interpolated string (v0.43, ADR 0075). Each hole must type to a
        // base scalar (String/Int/Float/Bool) or a *refinement* of one — those
        // have a well-defined display form (Int/Float via the ADR 0074
        // `toString` contract, Bool as `true`/`false`; a refined value widens
        // to its base, e.g. `Subject` displays as its `String`). Records,
        // sums, opaque types (whose base is deliberately hidden — `.raw` it
        // first), and other types are rejected, foreclosing JS's
        // `[object Object]` footgun. The result is always a `String`.
        ExprKind::InterpStr(parts) => {
            for part in parts {
                let InterpPart::Hole(hole) = part else {
                    continue;
                };
                match type_of(hole, None, ctx) {
                    Some(ty) if interpolable(ty, tys) => {}
                    Some(other) => ctx.errors.push(
                        CompileError::new(
                            "bynk.types.interpolation_non_scalar",
                            hole.span,
                            format!("type `{}` has no string form here", other.display(tys)),
                        )
                        .with_note(
                            "interpolation holes accept the base scalar types (String, Int, Float, Bool) or a refinement of one; map other values to a String first",
                        ),
                    ),
                    // The hole already produced its own error — don't pile on.
                    None => {}
                }
            }
            Some(tys.intern(Ty::Base(BaseType::String)))
        }
        ExprKind::BoolLit(_) => Some(tys.intern(Ty::Base(BaseType::Bool))),
        // v0.20b: a list literal. Elements check against the expected
        // element type when one is supplied (so refined literals admit,
        // v0.9.4); an empty `[]` has no inferable element type without one.
        ExprKind::ListLit(elems) => {
            let expected_elem = expected.and_then(|t| peel_to_list(t, tys));
            if elems.is_empty() {
                match expected_elem {
                    Some(t) => Some(tys.intern(Ty::List(t))),
                    None => {
                        ctx.errors.push(
                            CompileError::new(
                                "bynk.types.uninferable_element_type",
                                expr.span,
                                "an empty `[]` has no inferable element type",
                            )
                            .with_note(
                                "annotate the binding (`let xs: List[T] = []`) or use the empty list where a `List[T]` is expected",
                            ),
                        );
                        None
                    }
                }
            } else {
                let mut elem_ty: Option<TyId> = expected_elem;
                for e in elems {
                    let Some(t) = type_of(e, elem_ty, ctx) else {
                        continue;
                    };
                    match &elem_ty {
                        Some(et) => {
                            if !compatible(t, *et, tys) {
                                ctx.errors.push(CompileError::new(
                                    "bynk.types.list_element_mismatch",
                                    e.span,
                                    format!(
                                        "list element has type `{}`, but the list's element type is `{}`",
                                        t.display(tys),
                                        et.display(tys)
                                    ),
                                ));
                            }
                        }
                        None => elem_ty = Some(t),
                    }
                }
                elem_ty.map(|t| tys.intern(Ty::List(t)))
            }
        }
        ExprKind::Ident(id) => {
            // v0.94 (ADR 0120): a bare `store Map` ident used as a **value** — not
            // a method receiver, which the `MethodCall` arm dispatches — is a lazy
            // `Query[V]` over the whole map (e.g. the `other` side of a join). It
            // is not in the value scope, so it never shadows a local.
            if ctx.lookup(id.name.as_str()).is_none()
                && let Some(StoreField::Map(_, v)) = ctx.store_fields.get(&id.name).cloned()
            {
                Some(tys.intern(Ty::Query(v)))
            }
            // v0.9: a bare ident may name an HttpResult variant. Resolve to
            // HttpResult only when (a) the surrounding type implies it, or
            // (b) no user sum-type variant of the same name exists. This
            // keeps `NotFound` resolving to a user `StockError` variant
            // when the caller expects a domain Result.
            else if ctx.lookup(id.name.as_str()).is_none()
                && let Some(v) = http_variant(&id.name)
            {
                let user_owns = ctx.input.types.values().any(|t| {
                    matches!(&t.body, TypeBody::Sum(s)
                        if s.variants.iter().any(|var| var.name.name == id.name))
                });
                let http_implied = expected
                    .map(|t| peel_to_http_result(t, tys).is_some())
                    .unwrap_or(false)
                    || peel_to_http_result(ctx.return_ty, tys).is_some();
                if http_implied || !user_owns {
                    // P6.21/P6.23 (review of #1244/#1247): `Callee::Intrinsic`
                    // recorded here — a namespace-qualified built-in-sum
                    // variant reference, the shape P6.1's Decision C had
                    // excluded from bare-global resolution until a sink like
                    // this existed for it.
                    ctx.callees.insert(
                        expr.id,
                        Callee::Intrinsic {
                            ns: HTTP_RESULT,
                            op: v.name.to_string(),
                        },
                    );
                    check_http_variant(id.span, v, &[], expected, ctx)
                } else {
                    check_ident_at(expr.id, id, expected, ctx)
                }
            } else if ctx.lookup(id.name.as_str()).is_none()
                && let Some(qv) = queue_variant(&id.name)
                && (expected.is_some_and(|t| peel_to_queue_result(t, tys))
                    || peel_to_queue_result(ctx.return_ty, tys))
            {
                // v0.44: a bare QueueResult variant (`Ack`) in a queue handler.
                ctx.callees.insert(
                    expr.id,
                    Callee::Intrinsic {
                        ns: QUEUE_RESULT,
                        op: qv.name.to_string(),
                    },
                );
                check_queue_variant(id.span, qv, &[], ctx)
            } else {
                check_ident_at(expr.id, id, expected, ctx)
            }
        }
        ExprKind::Paren(inner) => type_of(inner, expected, ctx),
        ExprKind::Call {
            name,
            type_args,
            args,
        } => {
            // v0.9: HttpResult variant call. Prefer HttpResult when the
            // surrounding type implies it; otherwise defer to fn/user-variant
            // resolution and only fall back to HttpResult when nothing else
            // owns the name.
            //
            // `http_variant`/`queue_variant` are cheap keyword lookups; gate
            // the expensive context peel and — above all — the O(types×variants)
            // scan for user sum-variant owners behind them. The common case is
            // an ordinary function call whose name is neither keyword, so it
            // must not pay for either. The owner scan is further deferred behind
            // `http_implied`, since `unowned` only matters when the surrounding
            // type does not already imply HttpResult.
            if let Some(v) = http_variant(&name.name) {
                let http_implied = expected
                    .map(|t| peel_to_http_result(t, tys).is_some())
                    .unwrap_or(false)
                    || peel_to_http_result(ctx.return_ty, tys).is_some();
                let owned_elsewhere = || {
                    ctx.input.fns.contains_key(&name.name)
                        || ctx.input.types.values().any(|t| {
                            matches!(&t.body, TypeBody::Sum(s)
                                if s.variants.iter().any(|var| var.name.name == name.name))
                        })
                };
                if http_implied || !owned_elsewhere() {
                    ctx.callees.insert(
                        expr.id,
                        Callee::Intrinsic {
                            ns: HTTP_RESULT,
                            op: v.name.to_string(),
                        },
                    );
                    check_http_variant(expr.span, v, args, expected, ctx)
                } else {
                    // Falling straight to `check_call` (rather than the
                    // `queue_variant` else-if below) relies on the http and
                    // queue variant keyword sets being disjoint, so an http
                    // name could never have taken the queue branch anyway.
                    check_call(name, type_args, args, expr.span, expected, expr.id, ctx)
                }
            } else if let Some(qv) = queue_variant(&name.name)
                && (expected.is_some_and(|t| peel_to_queue_result(t, tys))
                    || peel_to_queue_result(ctx.return_ty, tys))
            {
                // v0.44: a QueueResult variant call (`Retry(reason)`).
                ctx.callees.insert(
                    expr.id,
                    Callee::Intrinsic {
                        ns: QUEUE_RESULT,
                        op: qv.name.to_string(),
                    },
                );
                check_queue_variant(expr.span, qv, args, ctx)
            } else {
                check_call(name, type_args, args, expr.span, expected, expr.id, ctx)
            }
        }
        ExprKind::UnaryOp(op, inner) => check_unary(*op, inner, expr.span, ctx),
        ExprKind::BinOp(op, lhs, rhs) => check_binop(*op, lhs, rhs, ctx),
        ExprKind::Block(b) => type_of_block(b, expected, ctx),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => check_if(cond, then_block, else_block, expr.span, expected, ctx),
        ExprKind::Ok(inner) => check_ok(inner, expr.span, expected, ctx),
        ExprKind::Err(inner) => check_err(inner, expr.span, expected, ctx),
        ExprKind::Some(inner) => check_some(inner, expr.span, expected, ctx),
        ExprKind::None => check_none(expr.span, expected, ctx),
        ExprKind::Question(inner) => check_question(inner, expr.span, ctx),
        ExprKind::RecordConstruction { type_name, fields } => {
            let ty = check_record_construction(type_name, fields, expected, expr.span, ctx)?;
            // P4c (#2628): a `SqlStatement`'s `sql` is a literal.
            d1::check_statement_fields(ty, fields, ctx);
            Some(ty)
        }
        ExprKind::FieldAccess { receiver, field } if is_run_id(receiver, field, ctx) => {
            // #2138 (track §3.7): `run.id`, the test run's identity, in a test
            // body where nothing else is named `run`.
            ctx.callees.insert(
                expr.id,
                Callee::Intrinsic {
                    ns: RUN_ID_NS,
                    op: "id".to_string(),
                },
            );
            Some(tys.intern(Ty::Base(BaseType::String)))
        }
        ExprKind::FieldAccess { receiver, field } => {
            // #2341 (L4): `ErrorStatus.X` is a `RawError` status, read
            // syntactically at that argument; anywhere else it has no value.
            if let ExprKind::Ident(id) = &receiver.kind
                && id.name == "ErrorStatus"
                && ctx.lookup(id.name.as_str()).is_none()
                && !ctx.input.types.contains_key(id.name.as_str())
            {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.http.error_status_position",
                        expr.span,
                        format!(
                            "`ErrorStatus.{}` names a status only as `RawError`'s first argument",
                            field.name
                        ),
                    )
                    .with_note("write the status where the response is built: `RawError(ErrorStatus.Forbidden, body, contentType)`; a helper can build the body"),
                );
                return None;
            }
            // v0.9: `HttpResult.Variant` qualified nullary variant access.
            if let ExprKind::Ident(id) = &receiver.kind
                && ctx.lookup(id.name.as_str()).is_none()
                && id.name == HTTP_RESULT
            {
                if let Some(v) = http_variant(&field.name) {
                    if !matches!(v.payload, HttpVariantPayload::None) {
                        ctx.errors.push(CompileError::new(
                            "bynk.types.variant_missing_payload",
                            field.span,
                            format!(
                                "`HttpResult.{}` has a payload — call it with an argument",
                                v.name
                            ),
                        ));
                        return None;
                    }
                    ctx.callees.insert(
                        expr.id,
                        Callee::Intrinsic {
                            ns: HTTP_RESULT,
                            op: v.name.to_string(),
                        },
                    );
                    check_http_variant(field.span, v, &[], expected, ctx)
                } else {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.unknown_static_member",
                        field.span,
                        format!("`HttpResult` has no variant named `{}`", field.name),
                    ));
                    None
                }
            } else {
                check_field_access(receiver, field, expected, ctx)
            }
        }
        ExprKind::MethodCall {
            receiver,
            method,
            type_args,
            args,
        } => {
            // `<field>.<op>(…)` on a `store` field — effectful storage
            // operations, dispatched by receiver provenance (a bare ident
            // naming a store field). Finding #36: one lookup into the unified
            // `store_fields` map, then dispatch by kind, instead of five
            // sequential per-kind lookups.
            //
            // Note: unlike the other store kinds, a `Cell` field is
            // deliberately bound into scope by `self_scope` (v0.81: "each
            // `Cell` store field is a bare local of its element type") so a
            // bare read derefs it — so `ctx.lookup` legitimately finds it and
            // no `is_none()` guard belongs there; a local sharing a cell's
            // name is a scope-construction question, not a dispatch-order
            // one. Every other kind requires `ctx.lookup(...).is_none()` so a
            // local that happens to share a store field's name is not
            // shadowed by the store dispatch.
            if let ExprKind::Ident(id) = &receiver.kind
                && let Some(field) = ctx.store_fields.get(&id.name).cloned()
                && (matches!(field, StoreField::Cell(_)) || ctx.lookup(id.name.as_str()).is_none())
            {
                match field {
                    // v0.82 (ADR 0110): `<map>.<op>(…)` on a `store Map[K, V]`
                    // field — effectful storage-map operations.
                    StoreField::Map(k, v) => {
                        // v0.91 (ADR 0115): a query builder/terminal lifts the
                        // store map into a lazy `Query[V]` over its values; an
                        // entry op (`put`/`get`/…) stays the effectful map
                        // operation.
                        if is_query_op(&method.name) {
                            // v0.107 (slice 4): record the receiver's lifted
                            // `Query[V]` type (otherwise unrecorded — the
                            // dispatch keys off the store field, not a typed
                            // receiver). This is the receiver's true type for
                            // any query op; its load-bearing use is the
                            // linearity pass, which now sees a held-bearing
                            // collection and lends the closure parameter of
                            // `forEach`/`parTraverse` as borrowed —
                            // otherwise `ty_of(receiver)` is `None` and the
                            // no-consume-in-a-broadcast rule is silently
                            // unenforced.
                            ctx.expr_types.insert(
                                receiver.id,
                                TypedExpr {
                                    span: receiver.span,
                                    ty: tys.intern(Ty::Query(v)),
                                },
                            );
                            let result = check_query_kernel_method(method, args, v, expr.span, ctx);
                            // P6.2 (#1143, R6.12): role is read back from
                            // this call's own resolved type where possible —
                            // `is_query_op` above only decides "lift or
                            // not", never "builder or terminal" (`query_role`
                            // itself, not this call site).
                            let role = query_role(result, &method.name, tys);
                            ctx.callees.insert(
                                expr.id,
                                Callee::Query {
                                    field: id.name.clone(),
                                    op: method.name.clone(),
                                    role,
                                },
                            );
                            result
                        } else {
                            ctx.callees.insert(
                                expr.id,
                                Callee::Store {
                                    field: id.name.clone(),
                                    op: method.name.clone(),
                                },
                            );
                            check_store_map_op(method, args, k, v, expr.span, ctx)
                        }
                    }
                    // v0.83: `<set>.<op>(…)` on a `store Set[T]` field —
                    // effectful storage-set ops.
                    StoreField::Set(t) => {
                        ctx.callees.insert(
                            expr.id,
                            Callee::Store {
                                field: id.name.clone(),
                                op: method.name.clone(),
                            },
                        );
                        check_store_set_op(method, args, t, expr.span, ctx)
                    }
                    // v0.87 (ADR 0113): `<cache>.<op>(…)` on a `store
                    // Cache[K, V]` field — the storage-map ops plus a `given
                    // Clock` requirement (eviction).
                    StoreField::Cache(k, v, _ttl) => {
                        ctx.callees.insert(
                            expr.id,
                            Callee::Store {
                                field: id.name.clone(),
                                op: method.name.clone(),
                            },
                        );
                        check_store_cache_op(method, args, k, v, expr.span, ctx)
                    }
                    // v0.95 (ADR 0121): `<log>.<op>(…)` on a `store Log[T]`
                    // field — `append` is the effectful non-idempotent write
                    // (`given Clock`); the time-window roots and general
                    // builders lift the log into a lazy `Query[T]` over its
                    // entry values. Unlike `Map`, the store-vs-query split
                    // lives *inside* `check_store_log_op` itself (the
                    // window-root vocabulary `since`/`before`/`between`/
                    // `recent`/`reversed` plus its own `is_query_op`
                    // fallthrough, `calls.rs:1879-1913`) — mirrored here by
                    // name so `Callee::Query` is recorded only for the same
                    // vocabulary `check_store_log_op` itself treats as a
                    // query op, not for every non-`append` name (an unknown
                    // op — `check_store_log_op`'s own `other =>` arm reports
                    // `bynk.store.unknown_op` — gets neither `Callee`, since
                    // dispatch's own conclusion is that it is not a valid
                    // call at all).
                    StoreField::Log(t) => match method.name.as_str() {
                        "append" => {
                            ctx.callees.insert(
                                expr.id,
                                Callee::Store {
                                    field: id.name.clone(),
                                    op: method.name.clone(),
                                },
                            );
                            check_store_log_op(method, args, t, expr.span, ctx)
                        }
                        name if matches!(
                            name,
                            "since" | "before" | "between" | "recent" | "reversed"
                        ) || is_query_op(name) =>
                        {
                            let result = check_store_log_op(method, args, t, expr.span, ctx);
                            let role = query_role(result, &method.name, tys);
                            ctx.callees.insert(
                                expr.id,
                                Callee::Query {
                                    field: id.name.clone(),
                                    op: method.name.clone(),
                                    role,
                                },
                            );
                            result
                        }
                        _ => check_store_log_op(method, args, t, expr.span, ctx),
                    },
                    // v0.98 (ADR 0125): `<cell>.update(f)` on a `store
                    // Cell[T]` field — the one method-shaped cell op (read is
                    // the bare name, write is `:=`).
                    StoreField::Cell(t) => {
                        ctx.callees.insert(
                            expr.id,
                            Callee::Store {
                                field: id.name.clone(),
                                op: method.name.clone(),
                            },
                        );
                        check_store_cell_op(method, args, t, expr.span, ctx)
                    }
                }
            }
            // v0.9: `HttpResult.Variant(args)` — explicit HttpResult construction.
            else if let ExprKind::Ident(id) = &receiver.kind
                && ctx.lookup(id.name.as_str()).is_none()
                && id.name == HTTP_RESULT
            {
                if let Some(v) = http_variant(&method.name) {
                    ctx.callees.insert(
                        expr.id,
                        Callee::Intrinsic {
                            ns: HTTP_RESULT,
                            op: v.name.to_string(),
                        },
                    );
                    check_http_variant(expr.span, v, args, expected, ctx)
                } else {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.unknown_static_member",
                        method.span,
                        format!("`HttpResult` has no variant named `{}`", method.name),
                    ));
                    None
                }
            } else {
                check_method_call(
                    receiver, method, type_args, args, expr.span, expected, expr.id, ctx,
                )
            }
        }
        ExprKind::Match { discriminant, arms } => {
            check_match(discriminant, arms, expr.span, expected, ctx)
        }
        ExprKind::Is { value, pattern } => check_is(value, pattern, expr.span, ctx),
        ExprKind::UnitLit => Some(tys.intern(Ty::Unit)),
        ExprKind::EffectPure(inner) => {
            let expected_inner = match expected.map(|e| tys.get(e)).as_deref() {
                Some(Ty::Effect(t)) => Some(*t),
                _ => None,
            };
            let inner_ty = type_of(inner, expected_inner, ctx)?;
            Some(tys.intern(Ty::Effect(inner_ty)))
        }
        ExprKind::RecordSpread {
            type_name,
            base,
            overrides,
        } => {
            let ty = check_record_spread(
                type_name.as_ref(),
                base,
                overrides,
                expr.span,
                expected,
                ctx,
            )?;
            d1::check_statement_fields(ty, overrides, ctx);
            Some(ty)
        }
        ExprKind::Expect(inner) => check_expect(inner, expr.span, ctx),
        ExprKind::Val { type_ref, args } => check_val(type_ref, args, expr.span, ctx),
        ExprKind::Observation(o) => check_observation(o, expr.span, ctx),
        ExprKind::Faults(f) => check_faults(f, expr.span, ctx),
        ExprKind::Trace { cap, op } => check_trace(cap, op, expr.span, ctx),
        // Slice C: a `Wire(<String>)` reached through the ordinary expression
        // checker is *misplaced* — a valid `Wire` is intercepted by the service-
        // address argument checker (`check_address_args`), which validates the
        // inner and the `system` tier. Anywhere else it is an error. The inner is
        // still typed so a mistake inside it is reported too.
        ExprKind::Wire(inner) => {
            let _ = type_of(inner, Some(tys.intern(Ty::Base(BaseType::String))), ctx);
            ctx.errors.push(
                CompileError::new(
                    "bynk.test.wire_needs_system",
                    expr.span,
                    "`Wire(...)` may only be passed as an argument to a service address in a `system`-tier case",
                )
                .with_note(
                    "`Wire` hands raw, pre-validation input to the boundary; there is no wire to be raw about at `unit`, and it is meaningless outside a service address",
                ),
            );
            None
        }
    };
    // T3.3b (R4.3, R2.5, R4.9): `expr_types` is total for every expression
    // `type_of` is called on — a `None` result (whether from a diagnosed
    // failure or a deliberate, undiagnosed non-type such as an untyped
    // test-body binding) records `Ty::Error` rather than leaving the span
    // unrecorded. This changes only what gets *written*; every caller of
    // `type_of` still sees its actual `Option<TyId>` return value and every
    // existing `?`/`.or(...)` control-flow site is unaffected — `Ty::Error`
    // only becomes observable to an external reader of `expr_types` (the
    // emitter, the LSP), never to internal checker logic.
    ctx.expr_types.insert(
        expr.id,
        TypedExpr {
            span: expr.span,
            ty: ty.unwrap_or_else(|| tys.intern(Ty::Error)),
        },
    );
    ty
}

/// #1658 (runtime-semantics track S9): an `Effect[T]` value in an effectful
/// body must be bound (`<-`), sequenced (`do`), returned (the block's tail),
/// or handed to something that takes an `Effect`. The emitter translates an
/// effectful call to an eager `Promise`, so a call that is *built* but not
/// bound still runs, unawaited, racing everything after it: `let e =
/// Counter("k").bump()` performed the write with no diagnostic. This flags the
/// value positions where an `Effect` can only be built and abandoned:
/// - the right-hand side of a plain `let` (`allowed == false` at the root),
///   including `let _ = …`;
/// - a list literal's element. No API takes a `List[Effect[T]]`, and a list's
///   element type is inferred from its elements, so it cannot vouch for them;
/// - the payload of `Some`/`Ok`/`Err`, a sum-variant constructor, or a record
///   field.
///
/// It does not descend into call arguments, receivers, lambdas or nested
/// blocks: an argument is the higher-order use the language keeps (its
/// parameter decides), and nested blocks are checked as blocks. Pure bodies
/// are untouched: effectful calls are already rejected there.
fn check_unbound_effects(e: &Expr, allowed: bool, ctx: &mut Ctx) {
    if !ctx.effectful {
        return;
    }
    let tys = ctx.tys;
    let ty_of = |e: &Expr, ctx: &Ctx| ctx.expr_types.get(&e.id).map(|t| t.ty);
    let is_effect = |t: Option<TyId>| t.is_some_and(|t| matches!(&*tys.get(t), Ty::Effect(_)));
    if !allowed && is_effect(ty_of(e, ctx)) {
        ctx.errors.push(
            CompileError::new(
                "bynk.effect.unbound_effect",
                e.span,
                "this `Effect` is started here and never awaited — it runs eagerly, out of order with everything after it",
            )
            .with_note(
                "bind its result with `let x <- …`, run it for its effect with `do …`, or return it as the body's value",
            ),
        );
        return;
    }
    match &e.kind {
        ExprKind::Paren(inner) => check_unbound_effects(inner, allowed, ctx),
        ExprKind::Some(x) | ExprKind::Ok(x) | ExprKind::Err(x) => {
            check_unbound_effects(x, false, ctx);
        }
        ExprKind::ListLit(args) => {
            for a in args {
                check_unbound_effects(a, false, ctx);
            }
        }
        // A variant constructor in every spelling: `Loaded(x)` is a `Call`;
        // the qualified `ApiResult.Loaded(x)` parses as a `MethodCall` on the
        // type name. Both resolve to `Callee::Ctor` on their own expression
        // (review of #1694).
        ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. }
            if matches!(ctx.callees.get(&e.id), Some(Callee::Ctor { .. })) =>
        {
            for a in args {
                check_unbound_effects(a, false, ctx);
            }
        }
        ExprKind::RecordConstruction { fields, .. }
        | ExprKind::RecordSpread {
            overrides: fields, ..
        } => {
            for f in fields {
                if let Some(v) = &f.value {
                    check_unbound_effects(v, false, ctx);
                }
            }
        }
        _ => {}
    }
}

// ==== Peel helpers (unwrap Effect / Result / Option / List / Map) ====

/// Peel one optional `Effect[_]` wrapper to expose an underlying `HttpResult[T]`.
pub(crate) fn peel_to_http_result(ty: TyId, tys: &Types) -> Option<TyId> {
    match &*tys.get(ty) {
        Ty::HttpResult(inner) => Some(*inner),
        Ty::Effect(inner) => peel_to_http_result(*inner, tys),
        _ => None,
    }
}

/// v0.44: peel an optional `Effect[_]` to detect an underlying `QueueResult`.
fn peel_to_queue_result(ty: TyId, tys: &Types) -> bool {
    match &*tys.get(ty) {
        Ty::QueueResult => true,
        Ty::Effect(inner) => peel_to_queue_result(*inner, tys),
        _ => false,
    }
}

fn surrounding_result(
    expected: Option<TyId>,
    return_ty: TyId,
    tys: &Types,
) -> Option<(TyId, TyId)> {
    if let Some(t) = expected
        && let Some(pair) = peel_to_result(t, tys)
    {
        return Some(pair);
    }
    peel_to_result(return_ty, tys)
}

/// Peel one optional `Effect[_]` wrapper to expose an underlying `Result[T, E]`.
/// Used by `Ok` / `Err` checking in v0.7.1 so that bare constructors in
/// `Effect[Result[T, E]]` tail positions can pick up the surrounding type's
/// parameters via the auto-lift propagation.
fn peel_to_result(ty: TyId, tys: &Types) -> Option<(TyId, TyId)> {
    match &*tys.get(ty) {
        Ty::Result(t, e) => Some((*t, *e)),
        Ty::Effect(inner) => peel_to_result(*inner, tys),
        _ => None,
    }
}

/// Companion to `peel_to_result` for `Option[T]`.
fn peel_to_option(ty: TyId, tys: &Types) -> Option<TyId> {
    match &*tys.get(ty) {
        Ty::Option(t) => Some(*t),
        Ty::Effect(inner) => peel_to_option(*inner, tys),
        _ => None,
    }
}

/// Companion to `peel_to_result` for `List[T]` (v0.20b) — the expected
/// element type of a list literal, looking through `Effect[_]` so tail
/// auto-lift positions still propagate it.
fn peel_to_list(ty: TyId, tys: &Types) -> Option<TyId> {
    match &*tys.get(ty) {
        Ty::List(t) => Some(*t),
        Ty::Effect(inner) => peel_to_list(*inner, tys),
        _ => None,
    }
}

/// K6a (#2596): companion to `peel_to_list` for `Set[T]`.
fn peel_to_set(ty: TyId, tys: &Types) -> Option<TyId> {
    match &*tys.get(ty) {
        Ty::Set(t) => Some(*t),
        Ty::Effect(inner) => peel_to_set(*inner, tys),
        _ => None,
    }
}

/// Companion to `peel_to_list` for `Map[K, V]` (v0.20b).
fn peel_to_map(ty: TyId, tys: &Types) -> Option<(TyId, TyId)> {
    match &*tys.get(ty) {
        Ty::Map(k, v) => Some((*k, *v)),
        Ty::Effect(inner) => peel_to_map(*inner, tys),
        _ => None,
    }
}

// ==== Structural compatibility and variant introspection ====

/// A flattened view of a type's variants (name + payload types).
///
/// `pub` since P6.4 (design/tracks/the-ir.md §6, #1157, Decision A):
/// `bynk-emit::ir::lower`'s pattern-lowering needs the exact same uniform
/// view this function already gives the checker — a user sum, `Result`,
/// `Option`, `ActorSum` and `HttpResult` all flattened into one `name` +
/// `payload` shape, with no `Arc<TypeDecl>` required (`Callee::Ctor`'s own
/// identity scheme never fires for `Ok`/`Err`/`Some`/`None`, ADR 0333's
/// `#1145` Decision B). No behaviour change — a reachability change only,
/// the same shape ADR 0333 already gave `Callee`.
pub struct VariantInfo {
    pub name: String,
    pub payload: Vec<(String, TyId)>,
}

/// Project a return type produced in the consumed context's namespace into
/// the caller's namespace by re-resolving named types that exist on both
/// sides. The structural shape stays the same; the brand changes.
fn rebrand_return_type(
    t: TyId,
    caller_types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
) -> TyId {
    let node = tys.get(t);
    match &*node {
        Ty::Named { name, kind, args } => {
            // If the caller's namespace has the same name, prefer the caller's
            // view (it carries the caller's brand at emission time). Otherwise
            // keep the consumed-context name; the caller can hold it opaquely.
            // Applied type arguments (a generic record) are preserved either
            // way — though a generic record is non-boundary, so this path only
            // ever sees the empty-args non-generic case in practice.
            if let Some(decl) = caller_types.get(name) {
                named_ty_with_args(decl, args.clone(), tys)
            } else {
                tys.intern(Ty::Named {
                    name: name.clone(),
                    kind: kind.clone(),
                    args: args.clone(),
                })
            }
        }
        Ty::Result(t, e) => tys.intern(Ty::Result(
            rebrand_return_type(*t, caller_types, tys),
            rebrand_return_type(*e, caller_types, tys),
        )),
        Ty::Option(t) => tys.intern(Ty::Option(rebrand_return_type(*t, caller_types, tys))),
        Ty::Effect(t) => tys.intern(Ty::Effect(rebrand_return_type(*t, caller_types, tys))),
        Ty::HttpResult(t) => tys.intern(Ty::HttpResult(rebrand_return_type(*t, caller_types, tys))),
        Ty::HttpOutcome(t) => {
            tys.intern(Ty::HttpOutcome(rebrand_return_type(*t, caller_types, tys)))
        }
        Ty::List(t) => tys.intern(Ty::List(rebrand_return_type(*t, caller_types, tys))),
        Ty::Set(t) => tys.intern(Ty::Set(rebrand_return_type(*t, caller_types, tys))),
        Ty::Query(t) => tys.intern(Ty::Query(rebrand_return_type(*t, caller_types, tys))),
        Ty::Stream(t) => tys.intern(Ty::Stream(rebrand_return_type(*t, caller_types, tys))),
        Ty::Connection(t) => tys.intern(Ty::Connection(rebrand_return_type(*t, caller_types, tys))),
        Ty::Map(k, v) => tys.intern(Ty::Map(
            rebrand_return_type(*k, caller_types, tys),
            rebrand_return_type(*v, caller_types, tys),
        )),
        // R4.3: `Ty::Error` carries no name to rebrand — pass it through.
        Ty::Error
        | Ty::Base(_)
        | Ty::QueueResult
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::TokenClaims
        | Ty::ValidationError
        | Ty::JsonError
        | Ty::Unit
        | Ty::Actor(_)
        | Ty::ActorSum(_) => t,
        // v0.20a: function types are confined to non-boundary positions
        // (`bynk.types.function_at_boundary`), so a cross-context return can
        // never carry one; Vars never escape call checking.
        Ty::Fn { .. } | Ty::Var(_) => t,
    }
}

/// #1941: the first type name [`rebrand_return_type`] would swap for the
/// caller's declaration of a different shape. A returned value takes the
/// caller's type of the same name (§6.5), so that swap would type the
/// provider's value as an unrelated type: the caller's `Box` reading fields
/// the provider's `Box` lacks. Names the caller does not bind keep the
/// provider's declaration and need no check.
///
/// `caller_types` is the table the rebrand uses, so this check and the
/// rebrand agree on which names are swapped. `receiving` is the caller's
/// [`projected_types`], which resolves the names the caller's declaration
/// reaches but does not bind (an export's unexported field type).
fn rebrand_conflict(
    t: TyId,
    provider_types: &HashMap<String, Arc<TypeDecl>>,
    caller_types: &HashMap<String, Arc<TypeDecl>>,
    receiving: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
) -> Option<String> {
    let walk = |inner: TyId| rebrand_conflict(inner, provider_types, caller_types, receiving, tys);
    let node = tys.get(t);
    match &*node {
        Ty::Named { name, .. } => {
            let (Some(caller_decl), Some(provider_decl)) =
                (caller_types.get(name), provider_types.get(name))
            else {
                return None;
            };
            if Arc::ptr_eq(caller_decl, provider_decl) {
                return None;
            }
            let rebranded = rebrand_return_type(t, caller_types, tys);
            (!structurally_compatible(t, rebranded, provider_types, receiving, tys))
                .then(|| name.clone())
        }
        Ty::Result(a, b) | Ty::Map(a, b) => walk(*a).or_else(|| walk(*b)),
        Ty::Option(a) | Ty::Effect(a) | Ty::HttpResult(a) | Ty::List(a) | Ty::Set(a) => walk(*a),
        // `Query`, `Stream` and `Connection` are non-boundary, so a
        // cross-context return never carries one.
        _ => None,
    }
}

/// Structural compatibility check for values crossing a context boundary
/// (v0.6 §4.3). The two types may be expressed in different namespaces
/// (caller-side / callee-side type tables), so we walk them in parallel
/// against their respective tables.
fn structurally_compatible(
    arg: TyId,
    param: TyId,
    arg_types: &HashMap<String, Arc<TypeDecl>>,
    param_types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
) -> bool {
    structurally_compatible_inner(arg, param, arg_types, param_types, tys, &mut HashSet::new())
}

/// [`projected_types`]' result: the projected table, and the names it leaves
/// out as ambiguous.
pub(crate) type Projection = (HashMap<String, Arc<TypeDecl>>, BTreeSet<String>);

/// #1941: the caller's type table, extended with the types a value from a
/// consumed context can carry here without the caller naming them. A
/// consumed service may return a type its context does not export (§6.5:
/// a value crosses whether or not its type is exported), and the caller's
/// table lacks it, so comparing that value on its way back used to find
/// nothing and reject it against its own declaration.
///
/// A `Ty::Named` keeps only the bare name, not the context it came from. A
/// name the caller does not bind is therefore resolved only when every
/// consumed context that declares it declares a structurally compatible
/// type, so whichever one the value came from compares the same. Otherwise
/// the name is left out, and a value of it is rejected rather than compared
/// against the wrong declaration. The names left out are returned too, for
/// the rejection to explain.
pub(crate) fn projected_types(
    caller_types: &HashMap<String, Arc<TypeDecl>>,
    consumed_types: &HashMap<String, HashMap<String, Arc<TypeDecl>>>,
    tys: &Types,
) -> Projection {
    type Table = HashMap<String, Arc<TypeDecl>>;
    let mut candidates: HashMap<&String, Vec<(&Arc<TypeDecl>, &Table)>> = HashMap::new();
    // Consumed contexts in name order, so when several agree on a name the
    // same declaration is projected on every run.
    let mut tables: Vec<(&String, &Table)> = consumed_types.iter().collect();
    tables.sort_by_key(|(context, _)| *context);
    for (_, table) in tables {
        for (name, decl) in table {
            if !caller_types.contains_key(name) {
                candidates.entry(name).or_default().push((decl, table));
            }
        }
    }
    let mut projected = caller_types.clone();
    let mut ambiguous = BTreeSet::new();
    for (name, decls) in candidates {
        let (first, first_table) = decls[0];
        let agree = decls[1..].iter().all(|&(decl, table)| {
            Arc::ptr_eq(first, decl)
                || (first.type_params.is_empty()
                    && decl.type_params.is_empty()
                    && structural_compare_named(
                        (name, &[]),
                        (name, &[]),
                        first_table,
                        table,
                        tys,
                        &mut HashSet::new(),
                    )
                    && structural_compare_named(
                        (name, &[]),
                        (name, &[]),
                        table,
                        first_table,
                        tys,
                        &mut HashSet::new(),
                    ))
        });
        if agree {
            projected.insert(name.clone(), first.clone());
        } else {
            ambiguous.insert(name.clone());
        }
    }
    (projected, ambiguous)
}

/// #1948: the caller's [`projected_types`], when it consumes a context, for
/// reading a value of a type the caller does not name: a consumed service's
/// unexported return type, or an export's unexported field type. `None` for
/// a unit that consumes nothing, whose own table is all there is. Projecting
/// clones the caller's table, so call it only once that table has missed.
pub(crate) fn consumer_projection(ctx: &Ctx) -> Option<Projection> {
    let consumed = &ctx.input.cross_context.consumed_types;
    (!consumed.is_empty()).then(|| projected_types(&ctx.input.types, consumed, ctx.tys))
}

/// #1948: a type's declaration by name: the caller's own, else the one
/// [`consumer_projection`] resolves. `None` for a name neither binds,
/// including one the projection leaves ambiguous.
pub(crate) fn lookup_type_decl(name: &str, ctx: &Ctx) -> Option<Arc<TypeDecl>> {
    if let Some(decl) = ctx.input.types.get(name) {
        return Some(decl.clone());
    }
    consumer_projection(ctx)?.0.get(name).cloned()
}

/// #1941: a note for a cross-context argument whose type names one of
/// [`projected_types`]' ambiguous names, which no declaration resolves.
pub(crate) fn ambiguous_projection_note(
    t: TyId,
    ambiguous: &BTreeSet<String>,
    tys: &Types,
) -> Option<String> {
    let node = tys.get(t);
    match &*node {
        Ty::Named { name, args, .. } => {
            if ambiguous.contains(name) {
                return Some(format!(
                    "more than one consumed context declares a type `{name}`, with different \
                     shapes, so a value of it cannot be traced to one of them; rename one"
                ));
            }
            args.iter()
                .find_map(|a| ambiguous_projection_note(*a, ambiguous, tys))
        }
        Ty::Result(a, b) | Ty::Map(a, b) => ambiguous_projection_note(*a, ambiguous, tys)
            .or_else(|| ambiguous_projection_note(*b, ambiguous, tys)),
        Ty::Option(a) | Ty::Effect(a) | Ty::HttpResult(a) | Ty::List(a) | Ty::Set(a) => {
            ambiguous_projection_note(*a, ambiguous, tys)
        }
        _ => None,
    }
}

fn structurally_compatible_inner(
    arg: TyId,
    param: TyId,
    arg_types: &HashMap<String, Arc<TypeDecl>>,
    param_types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
    visited: &mut HashSet<(String, String)>,
) -> bool {
    let (arg_node, param_node) = (tys.get(arg), tys.get(param));
    match (&*arg_node, &*param_node) {
        // R4.3: as in `compatible` — an already-diagnosed side is compatible
        // with anything, so a cross-context signature check doesn't report
        // the same failure a second time as a signature mismatch.
        (Ty::Error, _) | (_, Ty::Error) => true,
        (Ty::Base(a), Ty::Base(b)) => a == b,
        (Ty::ValidationError, Ty::ValidationError) => true,
        (Ty::JsonError, Ty::JsonError) => true,
        (Ty::Unit, Ty::Unit) => true,
        (Ty::Result(t1, e1), Ty::Result(t2, e2)) => {
            structurally_compatible_inner(*t1, *t2, arg_types, param_types, tys, visited)
                && structurally_compatible_inner(*e1, *e2, arg_types, param_types, tys, visited)
        }
        (Ty::Option(a), Ty::Option(b)) => {
            structurally_compatible_inner(*a, *b, arg_types, param_types, tys, visited)
        }
        (Ty::Effect(a), Ty::Effect(b)) => {
            structurally_compatible_inner(*a, *b, arg_types, param_types, tys, visited)
        }
        (Ty::HttpResult(a), Ty::HttpResult(b)) | (Ty::HttpOutcome(a), Ty::HttpOutcome(b)) => {
            structurally_compatible_inner(*a, *b, arg_types, param_types, tys, visited)
        }
        // The boundary-crossing collections walk their element types like
        // `Result`/`Option` — without these arms an identical `List[Int]`
        // was rejected against itself at a context boundary.
        (Ty::List(a), Ty::List(b)) | (Ty::Set(a), Ty::Set(b)) => {
            structurally_compatible_inner(*a, *b, arg_types, param_types, tys, visited)
        }
        (Ty::Map(k1, v1), Ty::Map(k2, v2)) => {
            structurally_compatible_inner(*k1, *k2, arg_types, param_types, tys, visited)
                && structurally_compatible_inner(*v1, *v2, arg_types, param_types, tys, visited)
        }
        (Ty::QueueResult, Ty::QueueResult) | (Ty::BoundaryRejection, Ty::BoundaryRejection) => true,
        (Ty::Asset, Ty::Asset) => true,
        (Ty::RequestHeaders, Ty::RequestHeaders) => true,
        (Ty::TokenClaims, Ty::TokenClaims) => true,
        (
            Ty::Named {
                name: an, args: aa, ..
            },
            Ty::Named {
                name: bn, args: ba, ..
            },
        ) => {
            // v0.157 (ADR 0183): applied type arguments must match structurally
            // too — `Paginated[String]` and `Paginated[Int]` are not the same
            // brand. (#1736: live now that a generic type's fields compare at
            // these arguments, in `structural_compare_named`.)
            if aa.len() != ba.len()
                || !aa.iter().zip(ba).all(|(x, y)| {
                    structurally_compatible_inner(*x, *y, arg_types, param_types, tys, visited)
                })
            {
                return false;
            }
            // Cycle break: once we've started comparing (an, bn) we trust
            // the recursive case to succeed.
            let key = (an.clone(), bn.clone());
            if !visited.insert(key.clone()) {
                return true;
            }
            let ok =
                structural_compare_named((an, aa), (bn, ba), arg_types, param_types, tys, visited);
            visited.remove(&key);
            ok
        }
        // Refined-named widens to its base; tolerate one-sided widening only
        // when comparing within the same nominal name (handled above) or when
        // the param accepts a plain base.
        (
            Ty::Named {
                kind: NamedKind::Refined(b),
                ..
            },
            Ty::Base(target),
        ) => b == target,
        // Everything else cannot cross a context boundary: cross-variant
        // pairs, and the non-boundary types (`Effect` payloads are unwrapped
        // before this check; `Query`/`Stream`/`Connection`/`Fn`/`Var` and the
        // sealed actor bindings never cross). The left side is enumerated —
        // no `_` — so adding a `Ty` variant fails to compile here instead of
        // silently rejecting the new type against itself (the trap the
        // collections fell into).
        (
            Ty::Base(_)
            | Ty::Named { .. }
            | Ty::Result(..)
            | Ty::Option(_)
            | Ty::Effect(_)
            | Ty::HttpResult(_)
            | Ty::HttpOutcome(_)
            | Ty::QueueResult
            | Ty::BoundaryRejection
            | Ty::Asset
            | Ty::RequestHeaders
            | Ty::TokenClaims
            | Ty::List(_)
            | Ty::Set(_)
            | Ty::Map(..)
            | Ty::Query(_)
            | Ty::Stream(_)
            | Ty::Connection(_)
            | Ty::ValidationError
            | Ty::JsonError
            | Ty::Unit
            | Ty::Actor(_)
            | Ty::ActorSum(_)
            | Ty::Fn { .. }
            | Ty::Var(_),
            _,
        ) => false,
    }
}

/// #2087: whether two declarations have the same members by name: the
/// fields of a record, or the variants (and their payload fields) of a sum,
/// in order. With the name and span, this tells one declaration (or a copy of
/// it with its references renamed) from another at the same byte range of a
/// different file.
fn same_members(a: &TypeDecl, b: &TypeDecl) -> bool {
    let fields = |fs: &[RecordField]| fs.iter().map(|f| f.name.name.clone()).collect::<Vec<_>>();
    match (&a.body, &b.body) {
        (TypeBody::Record(x), TypeBody::Record(y)) => fields(&x.fields) == fields(&y.fields),
        (TypeBody::Sum(x), TypeBody::Sum(y)) => {
            x.variants.len() == y.variants.len()
                && x.variants.iter().zip(&y.variants).all(|(v, w)| {
                    v.name.name == w.name.name
                        && v.payload
                            .iter()
                            .map(|f| &f.name.name)
                            .eq(w.payload.iter().map(|f| &f.name.name))
                })
        }
        (TypeBody::Refined { base: x, .. }, TypeBody::Refined { base: y, .. })
        | (TypeBody::Opaque { base: x, .. }, TypeBody::Opaque { base: y, .. }) => x == y,
        _ => false,
    }
}

fn structural_compare_named(
    (arg_name, arg_args): (&str, &[TyId]),
    (param_name, param_args): (&str, &[TyId]),
    arg_types: &HashMap<String, Arc<TypeDecl>>,
    param_types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
    visited: &mut HashSet<(String, String)>,
) -> bool {
    // The "same nominal name" case is the most common: both sides derive
    // the same commons type. Compare their structural shapes.
    let Some(arg_decl) = arg_types.get(arg_name) else {
        return false;
    };
    let Some(param_decl) = param_types.get(param_name) else {
        return false;
    };
    // #2087: one declaration on both sides (its applied arguments already
    // compared) is the same type, field for field. Its fields' types need not
    // resolve in the caller's table: a borrowed type's field may name a type
    // the caller cannot bind unambiguously (#1941), which the callee's table
    // holds under a qualified key (`shop_order__Code`).
    //
    // The name stays in the test: most spans carry `FileId::UNKNOWN` (they
    // come from the lexer), so two declarations in different files at the
    // same byte range have equal spans. The name, the span and the members'
    // names together identify one declaration; a copy `contract_types_for`
    // made (its references renamed) keeps all three.
    if arg_name == param_name
        && (Arc::ptr_eq(arg_decl, param_decl)
            || (arg_decl.span == param_decl.span && same_members(arg_decl, param_decl)))
    {
        return true;
    }
    // #1736: a generic declaration's fields name its type parameters
    // (`item: T`), which resolve against neither side's type table, so every
    // generic record or sum used to compare as incompatible, even against
    // itself. Each side's fields are resolved at that side's *applied*
    // arguments (`Envelope[Int]` → `item: Int`) with [`instantiate_field_ty`],
    // whose arity guard turns a mis-applied reference into `None` (rejected),
    // so the bodies compare as instantiated. The arguments were already
    // compared pairwise by the caller.
    match (&arg_decl.body, &param_decl.body) {
        (
            TypeBody::Refined {
                base: ab,
                refinement: ar,
                ..
            },
            TypeBody::Refined {
                base: bb,
                refinement: br,
                ..
            },
        ) => {
            if ab != bb {
                return false;
            }
            refinements_match(ar.as_ref(), br.as_ref())
        }
        (
            TypeBody::Opaque {
                base: ab,
                refinement: ar,
                ..
            },
            TypeBody::Opaque {
                base: bb,
                refinement: br,
                ..
            },
        ) => {
            // Opaque types must share a name to be compatible (a context's
            // opaque cannot be reinterpreted as a different context's opaque).
            if arg_name != param_name {
                return false;
            }
            if ab != bb {
                return false;
            }
            refinements_match(ar.as_ref(), br.as_ref())
        }
        (TypeBody::Record(a), TypeBody::Record(b)) => {
            if a.fields.len() != b.fields.len() {
                return false;
            }
            for af in &a.fields {
                let Some(bf) = b.fields.iter().find(|f| f.name.name == af.name.name) else {
                    return false;
                };
                let at = instantiate_field_ty(arg_decl, arg_args, &af.type_ref, arg_types, tys);
                let bt =
                    instantiate_field_ty(param_decl, param_args, &bf.type_ref, param_types, tys);
                let (Some(at), Some(bt)) = (at, bt) else {
                    return false;
                };
                if !structurally_compatible_inner(at, bt, arg_types, param_types, tys, visited) {
                    return false;
                }
            }
            true
        }
        (TypeBody::Sum(a), TypeBody::Sum(b)) => {
            if a.variants.len() != b.variants.len() {
                return false;
            }
            for av in &a.variants {
                let Some(bv) = b.variants.iter().find(|v| v.name.name == av.name.name) else {
                    return false;
                };
                if av.payload.len() != bv.payload.len() {
                    return false;
                }
                for (af, bf) in av.payload.iter().zip(bv.payload.iter()) {
                    if af.name.name != bf.name.name {
                        return false;
                    }
                    let at = instantiate_field_ty(arg_decl, arg_args, &af.type_ref, arg_types, tys);
                    let bt = instantiate_field_ty(
                        param_decl,
                        param_args,
                        &bf.type_ref,
                        param_types,
                        tys,
                    );
                    let (Some(at), Some(bt)) = (at, bt) else {
                        return false;
                    };
                    if !structurally_compatible_inner(at, bt, arg_types, param_types, tys, visited)
                    {
                        return false;
                    }
                }
            }
            true
        }
        _ => false,
    }
}

/// v0.177 (#643): two refinements match when their **canonical forms** are
/// equal — a *set* comparison, not a positional one.
///
/// This retires the v0.6 §4.3 foot-gun the status doc named: predicates were
/// compared by `zip`, so `String where NonEmpty, MaxLen(10)` and
/// `String where MaxLen(10), NonEmpty` — the same type — spuriously failed to
/// match. Predicates are conjunctive and side-effect-free, so their order
/// carries no meaning and comparing it was always accidental.
///
/// The comparison routes through `contract::canon_refinement`, the same function
/// that feeds the cross-context contract hash, and deliberately so: if the
/// matcher and the hash disagreed about what "the same refinement" is, a
/// contract could type-check at compile time and 409 at runtime — the worst
/// failure available to this increment. One normal form, two consumers.
///
/// The asymmetry is unchanged: a *more* restrictive sending side is admitted
/// into a more permissive receiving one, but not the reverse.
///
/// One behavioural consequence of sharing the form: it de-duplicates, so
/// `where NonEmpty, NonEmpty` now matches `where NonEmpty`. That is correct — a
/// conjunction is idempotent, so they are the same type — and it must hold on
/// the hash side regardless, or two contexts spelling the same type differently
/// would fail closed against each other.
fn refinements_match(a: Option<&Refinement>, b: Option<&Refinement>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(_), None) => true, // sending side is more restrictive — receiving is more permissive
        (None, Some(_)) => false,
        (Some(a), Some(b)) => {
            crate::contract::canon_refinement(Some(a)) == crate::contract::canon_refinement(Some(b))
        }
    }
}

/// `pub` since P6.4 (#1157, Decision A) — see [`VariantInfo`]'s own doc
/// comment for why `bynk-emit` needs this exact function rather than a
/// re-derived copy (R5.11, for the IR side).
pub fn variants_of(
    ty: TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
) -> Option<Vec<VariantInfo>> {
    match &*tys.get(ty) {
        Ty::Named {
            kind: NamedKind::Sum,
            name,
            args,
        } => {
            let decl = types.get(name)?;
            if let TypeBody::Sum(s) = &decl.body {
                Some(
                    s.variants
                        .iter()
                        .map(|v| VariantInfo {
                            name: v.name.name.clone(),
                            payload: v
                                .payload
                                .iter()
                                .map(|f| {
                                    // #593: for a generic sum, substitute the
                                    // instantiation's arguments into each payload
                                    // type (`Some(v: T)` over `Opt[Int]` ⇒ `Int`),
                                    // exactly as a generic record's fields are read
                                    // at an instantiation. `instantiate_field_ty`
                                    // degrades to a plain resolve for a non-generic
                                    // sum (empty `args`).
                                    let t =
                                        instantiate_field_ty(decl, args, &f.type_ref, types, tys)
                                            .unwrap_or_else(|| tys.intern(Ty::Base(BaseType::Int)));
                                    (f.name.name.clone(), t)
                                })
                                .collect(),
                        })
                        .collect(),
                )
            } else {
                None
            }
        }
        Ty::Result(t, e) => Some(vec![
            VariantInfo {
                name: "Ok".to_string(),
                payload: vec![("value".to_string(), *t)],
            },
            VariantInfo {
                name: "Err".to_string(),
                payload: vec![("error".to_string(), *e)],
            },
        ]),
        Ty::Option(t) => Some(vec![
            VariantInfo {
                name: "Some".to_string(),
                payload: vec![("value".to_string(), *t)],
            },
            VariantInfo {
                name: "None".to_string(),
                payload: vec![],
            },
        ]),
        // v0.52: a multi-actor sum matches on the resolved actor. Each member's
        // variant is named by the actor and binds that actor's identity
        // *directly* (`User(u)` ⇒ `u : UserId` — the arm already names the
        // actor, so no `.identity` indirection). A unit-identity member
        // (`Visitor`, `Webhook`) binds nothing.
        Ty::ActorSum(members) => Some(
            members
                .iter()
                .map(|(name, id)| VariantInfo {
                    name: name.clone(),
                    payload: match &*tys.get(*id) {
                        Ty::Unit => vec![],
                        _ => vec![("identity".to_string(), *id)],
                    },
                })
                .collect(),
        ),
        Ty::HttpResult(t) => Some(
            HTTP_VARIANTS
                .iter()
                .map(|v| VariantInfo {
                    name: v.name.to_string(),
                    payload: match v.payload {
                        HttpVariantPayload::None => vec![],
                        HttpVariantPayload::Value => vec![("value".to_string(), *t)],
                        HttpVariantPayload::Message => {
                            vec![(
                                "message".to_string(),
                                tys.intern(Ty::Base(BaseType::String)),
                            )]
                        }
                        HttpVariantPayload::Location => {
                            vec![(
                                "location".to_string(),
                                tys.intern(Ty::Base(BaseType::String)),
                            )]
                        }
                        HttpVariantPayload::Streamed => {
                            let elem = tys.intern(Ty::Base(BaseType::String));
                            vec![("stream".to_string(), tys.intern(Ty::Stream(elem)))]
                        }
                        // v0.111: the first two-field payload. Field names are
                        // kept byte-identical to the runtime union in
                        // `bynk-emit/runtime/src/http.ts`. An `HttpResult` is
                        // construct-only in handler position (never scrutinised),
                        // so this binding exists for exhaustiveness, not a path.
                        HttpVariantPayload::Raw => vec![
                            ("body".to_string(), tys.intern(Ty::Base(BaseType::Bytes))),
                            (
                                "contentType".to_string(),
                                tys.intern(Ty::Base(BaseType::String)),
                            ),
                        ],
                        HttpVariantPayload::RawTagged => vec![
                            ("body".to_string(), tys.intern(Ty::Base(BaseType::Bytes))),
                            (
                                "contentType".to_string(),
                                tys.intern(Ty::Base(BaseType::String)),
                            ),
                            ("etag".to_string(), tys.intern(Ty::Base(BaseType::String))),
                        ],
                        // K-streams (#2623): a test drains the bound body.
                        HttpVariantPayload::RawStreamed => {
                            let bytes = tys.intern(Ty::Base(BaseType::Bytes));
                            vec![
                                ("body".to_string(), tys.intern(Ty::Stream(bytes))),
                                (
                                    "contentType".to_string(),
                                    tys.intern(Ty::Base(BaseType::String)),
                                ),
                            ]
                        }
                        // #2341 (L4): the status matches as its number
                        // (`RawError(403, _, _)`); `ErrorStatus` names are a
                        // construction-site vocabulary, not a type.
                        HttpVariantPayload::RawError => vec![
                            ("status".to_string(), tys.intern(Ty::Base(BaseType::Int))),
                            ("body".to_string(), tys.intern(Ty::Base(BaseType::Bytes))),
                            (
                                "contentType".to_string(),
                                tys.intern(Ty::Base(BaseType::String)),
                            ),
                        ],
                    },
                })
                .collect(),
        ),
        // #1934: a case matches a queue handler's verdict (`expect r is Ack`).
        // Field names are the runtime union's in `bynk-emit/runtime/src/queue.ts`.
        Ty::QueueResult => Some(
            QUEUE_VARIANTS
                .iter()
                .map(|v| VariantInfo {
                    name: v.name.to_string(),
                    payload: match v.payload {
                        QueueVariantPayload::None => vec![],
                        QueueVariantPayload::Message => {
                            vec![("reason".to_string(), tys.intern(Ty::Base(BaseType::String)))]
                        }
                    },
                })
                .collect(),
        ),
        // #1934: the `system`-tier wire outcome. Field names are the runtime
        // union's in `bynk-emit/runtime/src/http.ts` (`HttpOutcome<T>`).
        Ty::HttpOutcome(t) => Some(vec![
            VariantInfo {
                name: "Rejected".to_string(),
                payload: vec![("value".to_string(), tys.intern(Ty::BoundaryRejection))],
            },
            VariantInfo {
                name: "Handled".to_string(),
                payload: vec![("value".to_string(), tys.intern(Ty::HttpResult(*t)))],
            },
        ]),
        Ty::BoundaryRejection => Some(
            BOUNDARY_REJECTIONS
                .iter()
                .map(|(name, has_detail)| VariantInfo {
                    name: (*name).to_string(),
                    payload: if *has_detail {
                        vec![("detail".to_string(), tys.intern(Ty::Base(BaseType::String)))]
                    } else {
                        vec![]
                    },
                })
                .collect(),
        ),
        _ => None,
    }
}

/// #1934: the variants of `BoundaryRejection`, each with whether it carries a
/// `detail: String` (the router's description of a refused body or path). The
/// first three are the router's `BoundaryError` kinds; `Unauthorized` is the
/// auth seam's `401` to a request with no credential, and `MethodNotAllowed`
/// the router's `405` to a method the path declares no handler for. The last two
/// are the router's `413` (a capped body over its cap) and `415` (a `@raw` route's
/// other `Content-Type`). Kept in
/// sync with `responseToHttpOutcome` in `bynk-emit/runtime/src/http.ts`.
pub const BOUNDARY_REJECTIONS: &[(&str, bool)] = &[
    ("RefinementViolation", true),
    ("MalformedJson", true),
    ("StructuralMismatch", true),
    ("Unauthorized", false),
    ("MethodNotAllowed", false),
    ("PayloadTooLarge", true),
    ("UnsupportedMediaType", true),
];

// ── v0.9.2: agent state-field zeroability ──────────────────────────────────
//
// Fresh agent state is the zero-value record (finding #10): a never-seen key
// reads `0` / `false` / `""` / `None` rather than `undefined`. A type is
// *zeroable* when it has a defined zero; agent state fields must be zeroable,
// since a fresh key has no committed value to load. Non-zeroable fields (a
// non-Option sum, an opaque type, or a refined type whose refinement excludes
// the underlying zero) are a compile error until explicit-initialiser syntax
// lands.

/// #2137: `advance <Cap> by <amount>` — only in a test `case` body
/// (`bynk.test.advance_outside_case`), only on `Clock`
/// (`bynk.test.advance_not_clock`), and by a `Duration`
/// (`bynk.test.advance_not_duration`). A `Duration` literal can't be negated,
/// so a negative amount is always computed, and faults at run time.
fn check_advance(a: &AdvanceStmt, ctx: &mut Ctx) {
    let tys = ctx.tys;
    if !ctx.in_test_body {
        ctx.errors.push(
            CompileError::new(
                "bynk.test.advance_outside_case",
                a.keyword.merge(a.cap.span),
                "`advance` is only valid inside a `case` body",
            )
            .with_note(
                "`advance Clock by <Duration>` moves the test harness's clock; production code reads real time",
            ),
        );
    }
    if a.cap.name != "Clock" {
        ctx.errors.push(
            CompileError::new(
                "bynk.test.advance_not_clock",
                a.cap.span,
                format!(
                    "`advance` moves only `Clock`, but names `{}`",
                    a.cap.name
                ),
            )
            .with_note("the harness owns one time source, the first-party `Clock`; write `advance Clock by <Duration>`"),
        );
    }
    let duration = tys.intern(Ty::Base(BaseType::Duration));
    let ty = type_of(&a.amount, Some(duration), ctx);
    if let Some(actual) = ty
        && !compatible(actual, duration, tys)
    {
        ctx.errors.push(
            CompileError::new(
                "bynk.test.advance_not_duration",
                a.amount.span,
                format!(
                    "`advance Clock by` takes a `Duration`, but this is `{}`",
                    actual.display(tys)
                ),
            )
            .with_note("write a duration literal such as `15.minutes`"),
        );
    }
}

/// #2138: the [`Callee::Intrinsic`] namespace of `run.id`.
pub const RUN_ID_NS: &str = "test.run";

/// #2138 (track §3.7): whether `receiver.field` is `run.id` — a test body's
/// per-run identity, a `String` — rather than a field of something named
/// `run`. A fixed value in process; a fresh one for each `bynk test
/// --against` run, so ids built from it don't collide between runs.
fn is_run_id(receiver: &Expr, field: &Ident, ctx: &Ctx) -> bool {
    ctx.in_test_body
        && field.name == "id"
        && matches!(&receiver.kind, ExprKind::Ident(id) if id.name == "run" && ctx.lookup("run").is_none())
}

#[cfg(test)]
mod generics_tests {
    use super::*;

    fn var(tys: &Types, n: &str) -> TyId {
        tys.intern(Ty::Var(n.to_string()))
    }
    fn int(tys: &Types) -> TyId {
        tys.intern(Ty::Base(BaseType::Int))
    }
    fn string(tys: &Types) -> TyId {
        tys.intern(Ty::Base(BaseType::String))
    }

    #[test]
    fn unify_binds_and_holds() {
        let tys = &Types::new();
        let mut s = HashMap::new();
        assert!(unify(var(tys, "A"), int(tys), &mut s, tys));
        assert_eq!(s.get("A"), Some(&int(tys)));
        // Same binding again: fine. A different one: conflict.
        assert!(unify(var(tys, "A"), int(tys), &mut s, tys));
        assert!(!unify(var(tys, "A"), string(tys), &mut s, tys));
    }

    #[test]
    fn unify_walks_structure() {
        let tys = &Types::new();
        let mut s = HashMap::new();
        let pattern = tys.intern(Ty::Fn {
            params: vec![var(tys, "A")],
            ret: tys.intern(Ty::Effect(var(tys, "B"))),
        });
        let actual = tys.intern(Ty::Fn {
            params: vec![int(tys)],
            ret: tys.intern(Ty::Effect(string(tys))),
        });
        assert!(unify(pattern, actual, &mut s, tys));
        assert_eq!(s.get("A"), Some(&int(tys)));
        assert_eq!(s.get("B"), Some(&string(tys)));
    }

    #[test]
    fn substitute_grounds_fully() {
        let tys = &Types::new();
        let mut s = HashMap::new();
        s.insert("A".to_string(), int(tys));
        let inner = tys.intern(Ty::Fn {
            params: vec![var(tys, "A")],
            ret: var(tys, "A"),
        });
        let t = tys.intern(Ty::Option(inner));
        let g = substitute(t, &s, tys);
        assert!(!contains_var(g, tys));
    }

    /// The §2 invariant (pinned per the plan): every expected-driven feature
    /// in `type_of` matches *concrete* `Ty` variants, so a Var-bearing
    /// expected imposes no constraint — `compatible` must simply reject
    /// Var-vs-ground pairs rather than panic or accept.
    #[test]
    fn var_bearing_expected_is_benign() {
        let tys = &Types::new();
        assert!(!compatible(int(tys), var(tys, "A"), tys));
        assert!(!compatible(var(tys, "A"), int(tys), tys));
        // Rigid vars: name equality only.
        assert!(compatible(var(tys, "A"), var(tys, "A"), tys));
        assert!(!compatible(var(tys, "A"), var(tys, "B"), tys));
    }
}

/// T3.6b (R4.1/R4.2): the interner's dedup property, and the `Hash`/`Eq`/`Ord`
/// it rests on.
///
/// T3.6a shipped the derives; these tests (added while scoping T3.6b, see the
/// identity-and-totality track doc §9) pinned the property `intern` would
/// depend on before it existed. Now that it does, they pin it directly: two
/// types built through *different construction paths* but structurally
/// identical must intern to the **same** `TyId` (or the checker's `TyId`
/// equality — which `unify` and `Ty::Map`'s key comparison now rely on —
/// would be unsound), and structurally different types must never collide.
///
/// Dedup being by the *shallow* `Ty` is exactly what makes this work: every
/// recursive field is already a `TyId`, so equal children imply equal parents
/// by induction. The nested cases below are what test that induction.
#[cfg(test)]
mod ty_hash_eq_ord_tests {
    use super::*;
    use std::collections::HashSet;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    fn hash_of(ty: &Ty) -> u64 {
        let mut h = DefaultHasher::new();
        ty.hash(&mut h);
        h.finish()
    }

    /// `map_entry_ty` (a real constructor) vs. the raw `Ty::Named` literal it
    /// builds — two different construction paths for the same type.
    #[test]
    fn map_entry_ty_matches_its_own_raw_literal() {
        let tys = &Types::new();
        let int = tys.intern(Ty::Base(BaseType::Int));
        let string = tys.intern(Ty::Base(BaseType::String));
        let via_constructor = map_entry_ty(int, string, tys);
        let via_literal = tys.intern(Ty::Named {
            name: MAP_ENTRY.to_string(),
            kind: NamedKind::Record,
            args: vec![int, string],
        });
        assert_eq!(via_constructor, via_literal);
        assert_eq!(
            hash_of(&tys.get(via_constructor)),
            hash_of(&tys.get(via_literal))
        );
    }

    /// A deeply nested type built two separate times interns to one `TyId` —
    /// the property `unify`'s "matches its prior binding exactly" now rests on.
    #[test]
    fn structurally_identical_nested_types_intern_to_one_id() {
        let tys = &Types::new();
        let build = || {
            let int = tys.intern(Ty::Base(BaseType::Int));
            let opt = tys.intern(Ty::Option(int));
            let list = tys.intern(Ty::List(opt));
            let string = tys.intern(Ty::Base(BaseType::String));
            tys.intern(Ty::Map(string, list))
        };
        let a = build();
        let before = tys.len();
        let b = build();
        assert_eq!(a, b);
        // Re-building it added nothing: every node was already interned.
        assert_eq!(tys.len(), before);
        assert_eq!(hash_of(&tys.get(a)), hash_of(&tys.get(b)));
    }

    /// Structurally different types must get distinct ids — the flip side of
    /// the dedup property above.
    #[test]
    fn structurally_different_types_get_distinct_ids() {
        let tys = &Types::new();
        let int = tys.intern(Ty::Base(BaseType::Int));
        let string = tys.intern(Ty::Base(BaseType::String));
        let ids = HashSet::from([
            tys.intern(Ty::List(int)),
            tys.intern(Ty::List(string)),
            tys.intern(Ty::Option(int)),
        ]);
        assert_eq!(ids.len(), 3);
    }

    /// Interning the same type repeatedly grows the table exactly once.
    #[test]
    fn repeated_interning_grows_the_table_once() {
        let tys = &Types::new();
        let int = tys.intern(Ty::Base(BaseType::Int));
        let bool_ = tys.intern(Ty::Base(BaseType::Bool));
        let before = tys.len();
        let ids: HashSet<TyId> = (0..3).map(|_| map_entry_ty(int, bool_, tys)).collect();
        assert_eq!(ids.len(), 1);
        assert_eq!(tys.len(), before + 1);
    }

    /// `resolve`-round-trip: an id resolves to the node it was minted from.
    #[test]
    fn an_id_resolves_to_the_node_it_was_interned_from() {
        let tys = &Types::new();
        let int = tys.intern(Ty::Base(BaseType::Int));
        assert_eq!(&*tys.get(int), &Ty::Base(BaseType::Int));
        let list = tys.intern(Ty::List(int));
        assert_eq!(&*tys.get(list), &Ty::List(int));
    }

    /// A `TyId` is only meaningful in the table it was minted from — the one
    /// new failure mode interning introduces. It fails loudly and by name;
    /// pinned so the diagnosis stays cheap for the next reader who wires two
    /// tables together by accident.
    ///
    /// This is the *shorter*-table shape, which both migration bugs had and
    /// which a bounds check alone catches. Its sibling below is the shape
    /// that actually needs the tag.
    #[test]
    #[should_panic(expected = "resolved against a table it was not interned into")]
    fn an_id_from_another_table_is_a_named_panic_not_a_silent_wrong_answer() {
        let a = Types::new();
        let b = Types::new();
        let id = a.intern(Ty::Base(BaseType::Int));
        let _ = b.get(id);
    }

    /// The sharp edge of the same guard: a foreign id whose index is merely
    /// *in range*. Before `TyId` carried its table's tag this returned an
    /// unrelated `Ty` — here, `Bool` for an id minted from `Int` — and the
    /// caller went on to mis-diagnose or mis-emit with no panic at all. The
    /// bounds check cannot see this one, so it is the case worth pinning.
    ///
    /// Debug-only, because that is where the tag exists; a release build
    /// still has the length check the sibling above covers.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "resolved against a table it was not interned into")]
    fn an_in_range_id_from_another_table_panics_rather_than_resolving_wrongly() {
        let a = Types::new();
        let b = Types::new();
        let id = a.intern(Ty::Base(BaseType::Int));
        b.intern(Ty::Base(BaseType::Bool));
        assert_eq!(a.len(), b.len(), "the index must be in range for b");
        let _ = b.get(id);
    }

    /// T3.6b's own soundness guard: `compatible` is deliberately **not**
    /// reflexive for the sealed boundary values, so interning must not be
    /// short-circuited on id equality. Pinned here because the fast path is
    /// the obvious "optimisation" a later reader would add.
    #[test]
    fn compatible_is_not_reflexive_for_sealed_actor_types() {
        let tys = &Types::new();
        let id_ty = tys.intern(Ty::Base(BaseType::String));
        let actor = tys.intern(Ty::Actor(id_ty));
        assert!(!compatible(actor, actor, tys));
        let sum = tys.intern(Ty::ActorSum(vec![("User".to_string(), id_ty)]));
        assert!(!compatible(sum, sum, tys));
        // …while an ordinary type still is.
        assert!(compatible(id_ty, id_ty, tys));
    }
}

/// Characterization pins for `checker.rs`'s pure free functions (v0.29.10
/// slice 0). These pin *current* behaviour ahead of the upcoming module split
/// so the verbatim moves are verifiable. Any surprising behaviour is pinned
/// as-is, flagged with a comment — these are not specifications.
#[cfg(test)]
mod pure_helper_pins {
    use super::*;
    use bynk_syntax::ast::{FloatBound, RefinementPred};

    // -- small constructors ------------------------------------------------

    fn sp() -> Span {
        Span::new(0, 0)
    }
    fn ident(n: &str) -> Ident {
        Ident {
            name: n.to_string(),
            span: sp(),
        }
    }
    fn var(tys: &Types, n: &str) -> TyId {
        tys.intern(Ty::Var(n.to_string()))
    }
    fn int(tys: &Types) -> TyId {
        tys.intern(Ty::Base(BaseType::Int))
    }
    fn string(tys: &Types) -> TyId {
        tys.intern(Ty::Base(BaseType::String))
    }
    fn expr(kind: ExprKind) -> Expr {
        Expr {
            id: ExprId::SYNTHETIC,
            kind,
            span: sp(),
        }
    }
    fn pred(kind: PredKind) -> RefinementPred {
        RefinementPred { kind, span: sp() }
    }
    fn refinement(preds: Vec<PredKind>) -> Refinement {
        Refinement {
            predicates: preds.into_iter().map(pred).collect(),
            span: sp(),
        }
    }
    fn fbound(value: f64) -> FloatBound {
        FloatBound {
            value,
            lexeme: value.to_string(),
            span: Span::new(0, 0),
        }
    }
    fn ibound(value: i64) -> IntBound {
        IntBound {
            value,
            span: Span::new(0, 0),
        }
    }
    /// An `InRange` predicate from two int values (test convenience).
    fn in_range(lo: i64, hi: i64) -> PredKind {
        PredKind::InRange(ibound(lo), ibound(hi))
    }
    fn refined_decl(name: &str, base: BaseType, refinement: Option<Refinement>) -> TypeDecl {
        TypeDecl {
            derives: Vec::new(),
            derives_kw: None,
            name: ident(name),
            type_params: Vec::new(),
            body: TypeBody::Refined {
                base,
                base_span: sp(),
                refinement,
            },
            documentation: None,
            span: sp(),
            trivia: bynk_syntax::ast::Trivia::default(),
            private: None,
        }
    }
    fn record_decl(name: &str) -> TypeDecl {
        TypeDecl {
            derives: Vec::new(),
            derives_kw: None,
            name: ident(name),
            type_params: Vec::new(),
            body: TypeBody::Record(bynk_syntax::ast::RecordBody {
                trailing_comments: Default::default(),
                fields: vec![],
                span: sp(),
            }),
            documentation: None,
            span: sp(),
            trivia: bynk_syntax::ast::Trivia::default(),
            private: None,
        }
    }

    // -- unify -------------------------------------------------------------

    #[test]
    fn unify_identical_concrete_types() {
        let tys = &Types::new();
        let mut s = HashMap::new();
        assert!(unify(int(tys), int(tys), &mut s, tys));
        assert!(s.is_empty());
    }

    #[test]
    fn unify_var_binds_in_subst() {
        let tys = &Types::new();
        let mut s = HashMap::new();
        assert!(unify(var(tys, "A"), string(tys), &mut s, tys));
        assert_eq!(s.get("A"), Some(&string(tys)));
    }

    #[test]
    fn unify_nested_generic_binds() {
        let tys = &Types::new();
        // List[A] vs List[Int] binds A := Int.
        let mut s = HashMap::new();
        let pat = tys.intern(Ty::List(var(tys, "A")));
        let act = tys.intern(Ty::List(int(tys)));
        assert!(unify(pat, act, &mut s, tys));
        assert_eq!(s.get("A"), Some(&int(tys)));
    }

    #[test]
    fn unify_surprise_concrete_mismatch_returns_true() {
        let tys = &Types::new();
        // SURPRISING (pinned as-is): `unify`'s catch-all is `_ => true`, so a
        // ground-vs-ground mismatch (Int vs String) and a constructor mismatch
        // (List vs Option) both *succeed* here — `compatible` owns those
        // diagnostics post-substitution, not `unify`.
        let mut s = HashMap::new();
        assert!(unify(int(tys), string(tys), &mut s, tys));
        assert!(unify(
            tys.intern(Ty::List(int(tys))),
            tys.intern(Ty::Option(int(tys))),
            &mut s,
            tys
        ));
        // The only false paths: a Var rebind conflict and an Fn arity mismatch.
        let mut s2 = HashMap::new();
        assert!(unify(var(tys, "A"), int(tys), &mut s2, tys));
        assert!(!unify(var(tys, "A"), string(tys), &mut s2, tys));
        let mut s3 = HashMap::new();
        let f1 = tys.intern(Ty::Fn {
            params: vec![int(tys)],
            ret: int(tys),
        });
        let f2 = tys.intern(Ty::Fn {
            params: vec![int(tys), int(tys)],
            ret: int(tys),
        });
        assert!(!unify(f1, f2, &mut s3, tys));
    }

    // -- substitute --------------------------------------------------------

    #[test]
    fn substitute_replaces_bound_var() {
        let tys = &Types::new();
        let mut s = HashMap::new();
        s.insert("A".to_string(), int(tys));
        assert_eq!(substitute(var(tys, "A"), &s, tys), int(tys));
    }

    #[test]
    fn substitute_recurses_into_nested() {
        let tys = &Types::new();
        let mut s = HashMap::new();
        s.insert("A".to_string(), string(tys));
        let t = tys.intern(Ty::Map(var(tys, "A"), int(tys)));
        assert_eq!(
            substitute(t, &s, tys),
            tys.intern(Ty::Map(string(tys), int(tys))),
        );
    }

    #[test]
    fn substitute_leaves_unbound_var_alone() {
        let tys = &Types::new();
        let s = HashMap::new();
        assert_eq!(substitute(var(tys, "Z"), &s, tys), var(tys, "Z"));
    }

    // -- contains_var / contains_flexible_var ------------------------------

    #[test]
    fn contains_var_positive_and_negative() {
        let tys = &Types::new();
        assert!(contains_var(tys.intern(Ty::Option(var(tys, "A"))), tys));
        assert!(!contains_var(tys.intern(Ty::Option(int(tys))), tys));
        assert!(!contains_var(int(tys), tys));
    }

    #[test]
    fn contains_flexible_var_respects_rigid_set() {
        let tys = &Types::new();
        let mut rigid = HashSet::new();
        rigid.insert("A".to_string());
        // A is rigid → not flexible.
        assert!(!contains_flexible_var(var(tys, "A"), &rigid, tys));
        // B is not rigid → flexible.
        assert!(contains_flexible_var(var(tys, "B"), &rigid, tys));
        // No vars at all → not flexible.
        assert!(!contains_flexible_var(int(tys), &rigid, tys));
    }

    // -- peel_to_* ---------------------------------------------------------

    #[test]
    fn peel_to_result_matches_and_misses() {
        let tys = &Types::new();
        let r = tys.intern(Ty::Result(int(tys), string(tys)));
        assert_eq!(peel_to_result(r, tys), Some((int(tys), string(tys))));
        assert_eq!(peel_to_result(int(tys), tys), None);
        // Pinned: peels through Effect[_].
        assert_eq!(
            peel_to_result(tys.intern(Ty::Effect(r)), tys),
            Some((int(tys), string(tys)))
        );
    }

    #[test]
    fn peel_to_option_matches_and_misses() {
        let tys = &Types::new();
        assert_eq!(
            peel_to_option(tys.intern(Ty::Option(int(tys))), tys),
            Some(int(tys))
        );
        assert_eq!(peel_to_option(int(tys), tys), None);
    }

    #[test]
    fn peel_to_list_matches_and_misses() {
        let tys = &Types::new();
        assert_eq!(
            peel_to_list(tys.intern(Ty::List(string(tys))), tys),
            Some(string(tys))
        );
        assert_eq!(peel_to_list(int(tys), tys), None);
    }

    #[test]
    fn peel_to_map_matches_and_misses() {
        let tys = &Types::new();
        let m = tys.intern(Ty::Map(string(tys), int(tys)));
        assert_eq!(peel_to_map(m, tys), Some((string(tys), int(tys))));
        assert_eq!(peel_to_map(int(tys), tys), None);
    }

    #[test]
    fn peel_to_http_result_matches_and_misses() {
        let tys = &Types::new();
        assert_eq!(
            peel_to_http_result(tys.intern(Ty::HttpResult(int(tys))), tys),
            Some(int(tys)),
        );
        assert_eq!(peel_to_http_result(int(tys), tys), None);
    }

    // -- maybe_auto_lift ---------------------------------------------------

    #[test]
    fn maybe_auto_lift_lifts_into_expected_effect() {
        let tys = &Types::new();
        // T lifts to Effect[T] when expected is Effect[T] and T is not effectful.
        let expected = tys.intern(Ty::Effect(int(tys)));
        let lifted = maybe_auto_lift(Some(int(tys)), Some(expected), tys);
        assert_eq!(lifted, Some(tys.intern(Ty::Effect(int(tys)))));
    }

    #[test]
    fn maybe_auto_lift_leaves_non_matching_alone() {
        let tys = &Types::new();
        // Already Effect[_]: untouched.
        let expected = tys.intern(Ty::Effect(int(tys)));
        assert_eq!(
            maybe_auto_lift(Some(tys.intern(Ty::Effect(int(tys)))), Some(expected), tys),
            Some(tys.intern(Ty::Effect(int(tys)))),
        );
        // Expected not an Effect: untouched.
        assert_eq!(
            maybe_auto_lift(Some(int(tys)), Some(int(tys)), tys),
            Some(int(tys))
        );
        // None type: untouched.
        assert_eq!(maybe_auto_lift(None, Some(expected), tys), None);
    }

    // -- const_literal -----------------------------------------------------

    #[test]
    fn const_literal_extracts_literals() {
        assert!(matches!(
            const_literal(&expr(ExprKind::int_lit(7))),
            Some(ConstLit::Int(7)),
        ));
        assert!(matches!(
            const_literal(&expr(ExprKind::BoolLit(true))),
            Some(ConstLit::Bool(true)),
        ));
        assert!(matches!(
            const_literal(&expr(ExprKind::StrLit("hi".into()))),
            Some(ConstLit::Str(s)) if s == "hi",
        ));
        assert!(matches!(
            const_literal(&expr(ExprKind::FloatLit {
                value: 1.5,
                lexeme: "1.5".into(),
            })),
            Some(ConstLit::Float(_)),
        ));
        // Unary-neg on an int literal folds.
        let neg = expr(ExprKind::UnaryOp(
            UnaryOp::Neg,
            Box::new(expr(ExprKind::int_lit(3))),
        ));
        assert!(matches!(const_literal(&neg), Some(ConstLit::Int(-3))));
    }

    #[test]
    fn const_literal_rejects_non_literals() {
        assert!(const_literal(&expr(ExprKind::Ident(ident("x")))).is_none());
    }

    // -- eval_predicate ----------------------------------------------------

    #[test]
    fn eval_predicate_int_and_float() {
        assert!(eval_predicate(&PredKind::NonNegative, &ConstLit::Int(0)));
        assert!(!eval_predicate(&PredKind::NonNegative, &ConstLit::Int(-1)));
        assert!(eval_predicate(&PredKind::Positive, &ConstLit::Int(1)));
        assert!(!eval_predicate(&PredKind::Positive, &ConstLit::Int(0)));
        assert!(eval_predicate(&in_range(1, 10), &ConstLit::Int(5),));
        assert!(!eval_predicate(&in_range(1, 10), &ConstLit::Int(11),));
    }

    #[test]
    fn eval_predicate_string() {
        assert!(eval_predicate(
            &PredKind::MinLength(2),
            &ConstLit::Str("ab".into()),
        ));
        assert!(!eval_predicate(
            &PredKind::MinLength(3),
            &ConstLit::Str("ab".into()),
        ));
        assert!(eval_predicate(
            &PredKind::NonEmpty,
            &ConstLit::Str("x".into()),
        ));
        assert!(!eval_predicate(
            &PredKind::NonEmpty,
            &ConstLit::Str(String::new()),
        ));
        assert!(eval_predicate(
            &PredKind::Matches("[a-z]+".into()),
            &ConstLit::Str("abc".into()),
        ));
        assert!(!eval_predicate(
            &PredKind::Matches("[a-z]+".into()),
            &ConstLit::Str("ABC".into()),
        ));
    }

    #[test]
    fn eval_predicate_base_mismatch_is_vacuously_true() {
        // SURPRISING (pinned as-is): a predicate/literal base mismatch returns
        // `true` — base/predicate mismatch is a declaration-time error reported
        // elsewhere, not by construction-time eval.
        assert!(eval_predicate(&PredKind::MinLength(5), &ConstLit::Int(0),));
    }

    // -- literal_matches_base ----------------------------------------------

    #[test]
    fn literal_matches_base_pairs() {
        assert!(literal_matches_base(&ConstLit::Int(1), BaseType::Int));
        assert!(literal_matches_base(
            &ConstLit::Str("x".into()),
            BaseType::String,
        ));
        assert!(!literal_matches_base(&ConstLit::Int(1), BaseType::String));
        assert!(!literal_matches_base(&ConstLit::Unit, BaseType::Int));
    }

    // -- type_decl_base / type_decl_refinement -----------------------------

    #[test]
    fn type_decl_base_refined_vs_record() {
        let refined = refined_decl("Age", BaseType::Int, None);
        assert_eq!(type_decl_base(&refined), Some(BaseType::Int));
        assert_eq!(type_decl_base(&record_decl("Pt")), None);
    }

    #[test]
    fn type_decl_refinement_present_vs_absent() {
        let with = refined_decl(
            "Age",
            BaseType::Int,
            Some(refinement(vec![PredKind::Positive])),
        );
        assert!(type_decl_refinement(&with).is_some());
        let without = refined_decl("Raw", BaseType::Int, None);
        assert!(type_decl_refinement(&without).is_none());
        assert!(type_decl_refinement(&record_decl("Pt")).is_none());
    }

    // -- check_*_refinement_consistency ------------------------------------

    #[test]
    fn int_refinement_consistency() {
        // Consistent: 1..=10 with Positive — no error.
        let mut errs = vec![];
        check_int_refinement_consistency(
            &refinement(vec![PredKind::Positive, in_range(1, 10)]),
            &mut errs,
        );
        assert!(errs.is_empty());
        // Inconsistent: InRange(10, 1) is empty → exactly one error.
        let mut errs = vec![];
        check_int_refinement_consistency(&refinement(vec![in_range(10, 1)]), &mut errs);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].category, "bynk.types.empty_refinement");
    }

    #[test]
    fn float_refinement_consistency() {
        // Consistent range.
        let mut errs = vec![];
        check_float_refinement_consistency(
            &refinement(vec![PredKind::InRangeF(fbound(0.0), fbound(1.0))]),
            &mut errs,
        );
        assert!(errs.is_empty());
        // Empty: 5.0..=1.0 → one error.
        let mut errs = vec![];
        check_float_refinement_consistency(
            &refinement(vec![PredKind::InRangeF(fbound(5.0), fbound(1.0))]),
            &mut errs,
        );
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].category, "bynk.types.empty_refinement");
        // Degenerate-but-exclusive: Positive with InRangeF(0.0, 0.0) → lo==hi
        // and lo_exclusive → one error.
        let mut errs = vec![];
        check_float_refinement_consistency(
            &refinement(vec![
                PredKind::Positive,
                PredKind::InRangeF(fbound(0.0), fbound(0.0)),
            ]),
            &mut errs,
        );
        assert_eq!(errs.len(), 1);
    }

    #[test]
    fn string_refinement_consistency() {
        // Consistent: MinLength(1), MaxLength(10).
        let mut errs = vec![];
        check_string_refinement_consistency(
            &refinement(vec![PredKind::MinLength(1), PredKind::MaxLength(10)]),
            &mut errs,
        );
        assert!(errs.is_empty());
        // min > max → one error.
        let mut errs = vec![];
        check_string_refinement_consistency(
            &refinement(vec![PredKind::MinLength(10), PredKind::MaxLength(2)]),
            &mut errs,
        );
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].category, "bynk.types.empty_refinement");
        // Conflicting exact lengths → TWO errors (pinned as-is): the explicit
        // `Length(3)`/`Length(5)` conflict push, *plus* the subsequent
        // min_len(5) > max_len(3) empty-range push (each `Length` clamps both
        // bounds to itself).
        let mut errs = vec![];
        check_string_refinement_consistency(
            &refinement(vec![PredKind::Length(3), PredKind::Length(5)]),
            &mut errs,
        );
        assert_eq!(errs.len(), 2);
        assert!(
            errs.iter()
                .all(|e| e.category == "bynk.types.empty_refinement")
        );
    }

    /// R12.2/T1.8: `NonEmpty` folds to `MinLength(1)` in `contract::canon_predicate`,
    /// so `refinements_match` — which routes through that same canonical form —
    /// must now treat the two spellings as the same refinement. Before the fold
    /// this was `false`.
    #[test]
    fn refinements_match_treats_non_empty_and_min_length_one_as_equal() {
        let a = refinement(vec![PredKind::NonEmpty]);
        let b = refinement(vec![PredKind::MinLength(1)]);
        assert!(refinements_match(Some(&a), Some(&b)));
        // …and the fold is exactly `MinLength(1)`, not a subsumption rule: a
        // strictly tighter refinement must still fail to match. Without this,
        // widening the fold into "`MinLength(2)` implies `MinLength(1)`, so
        // admit it" would leave every assertion in T1.8 passing while a
        // genuinely tighter callee refinement is accepted across a boundary.
        let c = refinement(vec![PredKind::MinLength(2)]);
        assert!(!refinements_match(Some(&a), Some(&c)));
        assert!(!refinements_match(Some(&c), Some(&a)));
    }

    // -- numeric_mix -------------------------------------------------------

    #[test]
    fn numeric_mix_int_float_pairs() {
        assert!(numeric_mix(Some(BaseType::Int), Some(BaseType::Float)));
        assert!(numeric_mix(Some(BaseType::Float), Some(BaseType::Int)));
        assert!(!numeric_mix(Some(BaseType::Int), Some(BaseType::Int)));
        assert!(!numeric_mix(Some(BaseType::Float), Some(BaseType::Float)));
        assert!(!numeric_mix(None, Some(BaseType::Int)));
    }
}
