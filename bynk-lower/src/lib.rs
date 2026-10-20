//! `bynk-lower`: the AST-analysis helpers `bynk-emit` reads resolved
//! declaration-level facts through — handler kinds and `given` clauses,
//! service protocols, store-field shapes, capability op and attached-method
//! signatures, route cache/limit annotations, event-subscriber shapes, and
//! the store-write walk behind `emit_agent`'s implicit-commit decision. Each
//! takes a checked program (or its `TypedCommons`) and a syntax-tree node
//! and returns a `bynk_ir` value; none lowers an expression body.
//!
//! **Reachability, corrected twice and now settled (Slice D0 of `#1542`,
//! the IR cutover, closing summary in `design/archive/retired-tracks.md`).** Through the crate carve (Arc D,
//! P7.12) this paragraph claimed nothing here was reached from `bynk-emit`'s
//! emission path; that was false — `lower_event_subscriber_shapes_ir`
//! (called from `bynk-emit/src/project.rs`) went through
//! `lower_service_item_ir`, which lowers every handler's own *body*, so the
//! whole recursive expression-lowering machinery (`lower_service_handler_ir`
//! → `lower_service_handler_body_ir` → `lower_block_ir` → `lower_expr_ir`/
//! `lower_stmt_ir`) ran in production for every `from Events(E)` service and
//! had its result discarded. Slice D0 repointed that one caller at the
//! shape-only helpers it actually needs (see its own doc comment), so the
//! original claim is now *true* by construction rather than false by
//! oversight: **every item constructor and the expression/statement/body
//! lowering beneath it — `lower_service_item_ir`, `lower_agent_item_ir`,
//! `lower_provider_item_ir`, `lower_fn_item_ir`, `lower_type_item_ir`,
//! `lower_capability_item_ir`, the handler/store-field/commit-shape/
//! invariant/transition helpers, `lower_fn_body_ir`, `lower_block_ir`,
//! `lower_expr_ir` and everything they call — has no caller outside this
//! crate's own test module.** The 30 August 2026 review's "zero callers"
//! finding was right about the end state and wrong about the path; the
//! track doc's §10.2 has the trace. Slice D1 of the same track then deleted
//! that machinery outright — 48 functions, the lowering context's scope
//! stack/temp counter/return-type/store-queryable fields, and every
//! `todo!()` this crate ever carried — so `rustc`'s own dead-code analysis,
//! not this paragraph, is the reachability record from here on. What stays
//! is the AST-analysis helper vocabulary `bynk-emit` consumes today —
//! `lower_handler_kind_ir`, `lower_handler_given_ir`,
//! `lower_protocol_ir{,_from_commons}`, `lower_type_shape_ir`,
//! `lower_service_handler_signature_ir`, `body_writes_state`, and their
//! siblings — each with a production call site.
//!
//! **Totality discipline (ADR 0334, Q2), as it stands after Slice D1:** the
//! rule was "every entry point takes a certified `&CheckedProgram`, so a
//! per-expression type lookup that misses is a compiler bug, not a
//! recoverable state." The per-expression lookup went with the expression
//! lowering, so what the rule now governs is narrower: a helper that reads
//! a *declaration's* own type references still takes `&CheckedProgram` by
//! default (`lower_type_shape_ir`'s doc comment has the full argument — a
//! bare `TypedCommons` is not certified by construction), and the four that
//! take a bare `&TypedCommons` instead — `body_writes_state`,
//! `lower_protocol_ir_from_commons`, `capability_op_sig_from_commons`,
//! `lower_attached_fn_sig_ir_from_types` — each document why their reads
//! cannot reach a certification-dependent panic. The same discipline
//! `bynk-emit/src/emitter/emit.rs`'s `lower_workers_cross_context_call`
//! applies to its own `bynk.emit.unresolved_cross_context_signature` panic.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bynk_check::checker::{self, Callee, CheckedProgram, Ty, TyId, TypedCommons, Types};
use bynk_check::resolver::MethodTable;
use bynk_syntax::ast::{
    ActorDecl, Block, CapRef, CapabilityDecl, CapabilityOp, CommonsItem, EventPattern,
    EventPatternValue, Expr, ExprKind, FnDecl, FnName, Handler, HandlerKind, HttpMethod,
    LiteralValue, MatchBody, ProviderDecl, QualifiedName, ServiceProtocol, Statement, StoreField,
    TypeBody, TypeDecl, TypeRef, expr_children,
};

use bynk_ir::{
    ActorSeamIr, CacheIr, CapRefIr, ConstVal, EventPatternIr, EventPatternValueIr,
    EventSubscriberShape, FnSig, IrHandlerKind, IrHttpMethod, OpSig, ProtocolIr, StoreFieldIr,
    StoreKindIr, TypeShape,
};

/// The resolution context the AST-analysis helpers below share: the checked
/// program's own `TypedCommons` (for type interning and declared-type
/// lookups) plus the enclosing fn/method's own rigid type variables
/// (`fn identity[T](x: T)`, and a generic type's own params on one of its
/// methods), needed by `resolve_type_ref_in` the same way `Ctx::type_vars`
/// is (`bynk-check/src/checker.rs`); `resolve_type_ref` (no `vars` set)
/// would otherwise resolve a rigid `T` as an unknown declared type and
/// silently fail.
///
/// Slice D1 of `#1542` (the IR cutover, `design/archive/retired-tracks.md`) slimmed
/// this from the expression lowerer's full context — the lexical scope
/// stack, the synthetic-temp counter, the enclosing return type and the
/// agent handler's store-queryable field set all went with the body
/// lowering that was their only reader. What remains is exactly what a
/// signature/shape reader needs.
pub(crate) struct LowerIrCtx<'a> {
    program: &'a TypedCommons,
    type_vars: HashSet<String>,
}

impl<'a> LowerIrCtx<'a> {
    fn new(program: &'a CheckedProgram, type_vars: HashSet<String>) -> Self {
        Self::from_commons(program.program(), type_vars)
    }

    /// #1187's own closing scoping pass: a `TypedCommons`-only constructor,
    /// for the call paths (`lower_op_sig_ir_from_commons` and
    /// `lower_attached_fn_sig_ir_from_types`, below) that never have a
    /// `&CheckedProgram` to unwrap. See `lower_op_sig_ir_from_commons`'s
    /// own doc comment for why that is sound.
    fn from_commons(commons: &'a TypedCommons, type_vars: HashSet<String>) -> Self {
        Self {
            program: commons,
            type_vars,
        }
    }

    /// A type reference resolved in this pass's own rigid-variable scope —
    /// the `resolve_type_ref_in` counterpart to `Ctx::resolve_type_ref_in`
    /// call sites (e.g. `checker.rs:2816,2897`), not the bare
    /// `resolve_type_ref` (which has no `vars` set and cannot resolve a
    /// generic fn/method's own type parameters).
    fn resolve_type_ref(&self, r: &bynk_syntax::ast::TypeRef) -> Option<TyId> {
        checker::resolve_type_ref_in(
            r,
            &self.program.types,
            &self.type_vars,
            &self.program.ty_intern,
        )
    }

    fn unit_ty(&self) -> TyId {
        self.program.ty_intern.intern(Ty::Unit)
    }
}

/// `(params, given, ret, effectful)` — [`lower_service_handler_signature_ir`]'s
/// return shape (#1187's slice 5), a named alias rather than a bare tuple
/// because its consumer (`emit_service`, `bynk-emit/src/emitter/emit.rs`)
/// has to spell it out in a function signature. (The agent-handler
/// signature reader that shared it went with Slice D1 of `#1542`.)
pub type HandlerSignatureIr = (Vec<(String, TyId)>, Vec<String>, TyId, bool);

/// P6.50 (design/tracks/the-ir.md §6b): a return type's own syntactic
/// `Effect[...]` wrapper — `TypeRef::Effect(_, _)`, not the *resolved*
/// `Ty::Effect(_)` shape `lower_handler_signature_ir` reads above via
/// `cx.program.ty_intern`. Relocated here from `emitter/emit.rs` (its
/// original home, `#[allow(dead_code)]`-free and with eight call sites
/// across `emit.rs`/`workers.rs`/`workers_entry.rs`) because
/// [`lower_service_handler_signature_ir`] below was already calling *up*
/// into it (`bynk_emit::emitter::is_effectful_return`) — the `Ast → Ir` boundary
/// running backwards, an `Ir`-side lowering function reaching into the
/// `emitter` module it should only ever be called *from*. `emit.rs` and
/// friends now call `bynk_lower::is_effectful_return` instead (relocated
/// again at the P7.12 crate carve — `emitter`/`ir::lower` are now separate
/// crates, `bynk-emit`/`bynk-lower` respectively).
pub fn is_effectful_return(r: &TypeRef) -> bool {
    matches!(r, TypeRef::Effect(_, _))
}

/// A service handler's resolved *signature* — `params`/`given`/`ret`/
/// `effectful` — and never its body. This is what `emit_service`
/// (`bynk-emit/src/emitter/emit.rs`) reads per handler, and what
/// `lower_event_subscriber_shapes_ir` reads for a subscriber's parameter
/// count. Mirrors [`body_writes_state`]'s posture (#1196): a narrow,
/// standalone reader of already-resolved data, applied to signature data
/// instead of a body walk.
///
/// A service handler's own param type is *not* resolution-checked by the
/// checker at all (`1199_service_handler_unresolvable_param_type_no_ice`
/// pins it), so a resolve miss degrades to `Ty::Unit` here rather than
/// panicking — the same fallback `lower_protocol_ir` documents for a
/// WebSocket frame type, and for the same reason: an ADR 0334 panic may
/// only assert a guarantee the checker actually gives. (The agent-handler
/// signature reader that did panic on a miss was correct for *its* input,
/// which the checker does guarantee resolves; it went with the body
/// lowering in Slice D1 of `#1542`.)
pub fn lower_service_handler_signature_ir(
    h: &Handler,
    program: &CheckedProgram,
) -> HandlerSignatureIr {
    let cx = LowerIrCtx::new(program, HashSet::new());
    let params: Vec<(String, TyId)> = h
        .params
        .iter()
        .map(|p| {
            let ty = cx
                .resolve_type_ref(&p.type_ref)
                .unwrap_or_else(|| cx.unit_ty());
            (p.name.name.clone(), ty)
        })
        .collect();
    let given: Vec<String> = h.given.iter().map(|c| c.key().to_string()).collect();
    let ret = cx
        .resolve_type_ref(&h.return_type)
        .unwrap_or_else(|| cx.unit_ty());
    let effectful = is_effectful_return(&h.return_type);
    (params, given, ret, effectful)
}

/// A `type` declaration's resolved structure as a [`TypeShape`] — the
/// reader `emitter.rs`'s own `type_shape_for` calls directly. (Slice 1 of
/// `#1542` split this out of a full `IrItem::Type` constructor whose single
/// field it was, ending a build-then-`unreachable!`-discard round-trip;
/// Slice D1 then deleted that constructor, leaving this as the type
/// reader.)
///
/// Takes a certified `&CheckedProgram`, matching this module's own
/// categorical discipline (this file's own header doc: "every entry point
/// here takes a `&CheckedProgram`"), even though only
/// `TypedCommons::types`/`ty_intern` are read — no per-expression
/// `expr_types` lookup is involved (Q2, `design/tracks/the-ir.md` §3.2), but
/// *which fields are read* isn't the discipline; *which failures are allowed
/// to `panic!`* is. Every panic below asserts "the checker already accepted
/// this declaration" — true only once `certify` has run: a bare
/// `TypedCommons` is not certified by construction (`checker.rs`'s own
/// `CheckedProgram` doc notes the project/batch path holds per-unit
/// `TypedCommons` values *before* that unit's build-wide gate is decided),
/// so accepting one here would make `resolve_type_ref_in` returning `None` a
/// reachable, not just a buggy, outcome.
pub fn lower_type_shape_ir(decl: &Arc<TypeDecl>, program: &CheckedProgram) -> TypeShape {
    let program = program.program();
    let type_vars: HashSet<String> = decl
        .type_params
        .iter()
        .map(|tp| tp.name.name.clone())
        .collect();
    let resolve = |r: &bynk_syntax::ast::TypeRef| {
        checker::resolve_type_ref_in(r, &program.types, &type_vars, &program.ty_intern)
    };
    match &decl.body {
        TypeBody::Record(r) => TypeShape::Record {
            fields: r
                .fields
                .iter()
                .map(|f| {
                    let ty = resolve(&f.type_ref).unwrap_or_else(|| {
                        panic!(
                            "bynk internal error (ADR 0334): field `{}` of type `{}` does not \
                             resolve, but the checker already accepted this declaration",
                            f.name.name, decl.name.name
                        )
                    });
                    (f.name.name.clone(), ty)
                })
                .collect(),
        },
        TypeBody::Sum(s) => TypeShape::Sum {
            variants: s
                .variants
                .iter()
                .map(|v| {
                    let payload = v
                        .payload
                        .iter()
                        .map(|vf| {
                            let ty = resolve(&vf.type_ref).unwrap_or_else(|| {
                                panic!(
                                    "bynk internal error (ADR 0334): field `{}` of variant `{}` \
                                     of type `{}` does not resolve, but the checker already \
                                     accepted this declaration",
                                    vf.name.name, v.name.name, decl.name.name
                                )
                            });
                            (vf.name.name.clone(), ty)
                        })
                        .collect();
                    (v.name.name.clone(), payload)
                })
                .collect(),
            embeds: s
                .embeds
                .iter()
                .map(|e| {
                    let source = resolve(&e.source_type).unwrap_or_else(|| {
                        panic!(
                            "bynk internal error (ADR 0334): `embeds` clause source type on \
                             variant `{}` of type `{}` does not resolve, but the checker \
                             already accepted this declaration",
                            e.variant.name, decl.name.name
                        )
                    });
                    (source, e.variant.name.clone())
                })
                .collect(),
        },
        TypeBody::Refined {
            base, refinement, ..
        } => TypeShape::Refined {
            base: *base,
            refinement: refinement.clone(),
            opaque: false,
        },
        TypeBody::Opaque {
            base, refinement, ..
        } => TypeShape::Refined {
            base: *base,
            refinement: refinement.clone(),
            opaque: true,
        },
    }
}

/// A store field's own element/key/value type, resolved with no rigid type
/// variables in scope — `AgentDecl` carries no `type_params` of its own
/// (its own struct shape, `bynk-syntax/src/ast.rs:908-934`), so
/// [`lower_store_field_ir`] never needs the `fn_rigid_type_vars`-shaped
/// seeding every fn/method-level constructor here does. Shared by every
/// arm of that function's own kind dispatch, and by [`lower_store_field_shape_ir`].
///
/// **Not an ADR 0334 `.expect()`-style panic on a resolve miss, deliberately**
/// — the same posture `lower_op_sig_ir`'s own doc comment already argues
/// for a capability op's `params`/`return_type`, confirmed empirically for
/// store fields specifically (#1187's Agent state-field slice, step 0):
/// `store x: Cell[Bogus] = "hello"` certified then (exit 0, no diagnostic),
/// so a `Bogus` store-field type reached this pass with no `expr_types`/
/// `types` entry at all. Since #1679 the resolver rejects an unknown store
/// field type (`bynk.resolve.unknown_type`), so this fallback is defensive:
/// it keeps lowering total rather than asserting a guarantee it does not
/// itself check.
fn resolve_store_field_ty(cx: &LowerIrCtx, r: &bynk_syntax::ast::TypeRef) -> TyId {
    cx.resolve_type_ref(r).unwrap_or_else(|| cx.unit_ty())
}

/// A `@name(<duration literal>)` annotation's own millisecond value — the
/// shared "find the annotation, read its first argument's `DurationLit`"
/// step both `@ttl` (`Cache`) and `@retain` (`Log`) need, factored out so
/// the two [`lower_store_field_ir`] arms are one call each rather than two
/// near-identical inline `find`/`and_then` chains. Mirrors, but does not
/// call, `cache_ttl_millis` (`bynk-check/src/context_checks.rs`) and the
/// shipped emitter's own equivalent extraction (`emit.rs`'s
/// `store_cache_fields`/`store_log_fields`) — those thread a
/// `&mut Vec<CompileError>` for a missing-`@ttl` diagnostic and are private
/// to their own module, so reusing them directly here is not architecturally
/// available; see [`lower_store_field_ir`]'s own doc comment for why this is
/// an accepted, named duplication rather than a gap this slice closes.
fn duration_millis_annotation(
    annotations: &[bynk_syntax::ast::Annotation],
    name: &str,
) -> Option<i64> {
    annotations
        .iter()
        .find(|a| a.name.name == name)
        .and_then(|a| match a.args.first().map(|arg| &arg.value.kind) {
            Some(ExprKind::DurationLit { millis, .. }) => Some(*millis),
            _ => None,
        })
}

/// The shape half of [`lower_store_field_ir`] — its `kind`/`indexed`
/// computation, factored out so [`lower_store_field_shape_ir`] can share it
/// without either duplicating the `Cell`/`Map`/`Set`/`Cache`/`Log` dispatch
/// or paying for a `&mut LowerIrCtx` it never needs (nothing here lowers an
/// expression).
fn store_field_kind_and_indexed(f: &StoreField, cx: &LowerIrCtx) -> (StoreKindIr, Vec<String>) {
    let head = f.kind.head.name.as_str();
    let kind = match head {
        "Cell" => StoreKindIr::Cell(resolve_store_field_ty(cx, &f.kind.args[0])),
        "Map" => StoreKindIr::Map(
            resolve_store_field_ty(cx, &f.kind.args[0]),
            resolve_store_field_ty(cx, &f.kind.args[1]),
        ),
        "Set" => StoreKindIr::Set(resolve_store_field_ty(cx, &f.kind.args[0])),
        "Cache" => {
            let k = resolve_store_field_ty(cx, &f.kind.args[0]);
            let v = resolve_store_field_ty(cx, &f.kind.args[1]);
            let ttl = duration_millis_annotation(&f.annotations, "ttl").unwrap_or_else(|| {
                panic!(
                    "bynk internal error (ADR 0334): `Cache` field `{}` has no resolvable \
                     `@ttl` millis, but the checker already accepted this declaration — \
                     bynk.store.cache_ttl_required gates a missing or malformed `@ttl` \
                     before certify",
                    f.name.name
                )
            });
            StoreKindIr::Cache(k, v, ttl)
        }
        "Log" => {
            let elem = resolve_store_field_ty(cx, &f.kind.args[0]);
            let retain = duration_millis_annotation(&f.annotations, "retain");
            StoreKindIr::Log(elem, retain)
        }
        other => panic!(
            "bynk internal error (ADR 0334): store field `{}` has storage kind `{other}`, which \
             cannot reach a certified program — only Cell/Map/Set/Cache/Log are functional \
             (Queue is gated by bynk.store.kind_unsupported before certify)",
            f.name.name
        ),
    };
    // [DECISION C]/[DECISION E]: one entry per distinct `by:` argument,
    // declaration order, no sort — legal only on `Map` (`ANNOTATIONS`'s own
    // registry), so this is empty by construction for every other kind.
    // Deduplicated: `validate_indexed_keys` (`context_checks.rs`) validates
    // each `by:` argument independently with no duplicate check, so
    // `@indexed(by: k, by: k)` certifies — mirrors the shipped emitter's own
    // `store_map_indexes` guard (`emit.rs`'s `!fields.contains(&k.name)`),
    // grounded during P6.7's own review (#1163): dropping this would mean a
    // duplicate `by:` produces the same sibling index table twice.
    let mut indexed: Vec<String> = Vec::new();
    for arg in f
        .annotations
        .iter()
        .filter(|a| a.name.name == "indexed")
        .flat_map(|a| &a.args)
    {
        if let (Some(l), ExprKind::Ident(k)) = (&arg.label, &arg.value.kind)
            && l.name == "by"
            && !indexed.contains(&k.name)
        {
            indexed.push(k.name.clone());
        }
    }
    (kind, indexed)
}

/// A store field's storage *shape* — its `Cell`/`Map`/`Set`/`Cache`/`Log`
/// kind and `@indexed` keys (via `store_field_kind_and_indexed`), with
/// `init` always `None`. This is the entry point `emit_agent`'s own state
/// section actually needs; a `Cell` field's zero/initial-value expression is
/// rendered by the emitter from the AST, never lowered here. (An
/// `init`-lowering sibling existed until Slice D1 of `#1542`; it lowered the
/// initialiser through the deleted expression lowerer and had no caller.)
///
/// The field's own type reference falls back to `Ty::Unit` on a resolve
/// miss rather than panicking — no checker pass validates a store field's
/// type reference, only its shape and annotation legality, so `store x:
/// Cell[Bogus]` certifies today and this reader must not turn that into an
/// ICE (`store_field_falls_back_to_unit_on_an_unresolvable_type_like_the_checker_does`
/// pins it).
pub fn lower_store_field_shape_ir(f: &StoreField, program: &CheckedProgram) -> StoreFieldIr {
    let cx = LowerIrCtx::new(program, HashSet::new());
    let (kind, indexed) = store_field_kind_and_indexed(f, &cx);
    StoreFieldIr {
        field: f.name.name.clone(),
        kind,
        indexed,
    }
}

/// [DECISION B]/[DECISION C] (#1165): does `body` reach a mutating
/// `Callee::Store` write, or an unconditional `Statement::Assign` (`:=`),
/// anywhere — including inside a nested `if`/`match`/lambda? Drives, as of
/// #1196 (the #1187 emitter-cutover track's own R6.5 stake), `emit_agent`'s
/// (`bynk-emit/src/emitter/emit.rs`) real implicit-commit-wrapper decision
/// (it also drove the IR-side `CommitShape::Transactional` decision until
/// Slice D1 of `#1542` deleted that constructor) — its previous own
/// name-matching `block_writes_state`
/// (`emitter.rs`) is deleted, this function is its sole, direct
/// replacement. The walk's own shape is that deleted function's own
/// already-correct skeleton reused structurally, not re-derived:
/// `Block`/`If`/`Match` are hand-matched so crossing a nested block
/// re-enters the statement-aware case (an `expr_children` descent alone
/// flattens a block straight to its statements' *values*, losing the
/// `Statement::Assign` tag), everywhere else recurses over
/// `expr_children`'s total child iterator.
///
/// Unlike the deleted function's own name-based `mutating_op`, this walk
/// needs no per-kind receiver-name set: a `Callee::Store { op, .. }` already
/// carries the field's own resolved identity (the checker only ever records
/// one for a method the field's own kind actually declares), so `op`'s
/// membership in the store-op registry's mutating set
/// (`bynk_check::store_ops::is_mutating_any`, #2194) is unambiguous checked
/// flat, across every kind at once — a locally-shadowed name that would false-positive
/// `mutating_op` cannot false-positive here at all, the exact fix Decision
/// B's own Risk names, and the exact defect `#1196_agent_write_detection_
/// via_resolved_callee`'s own fixture pins at the emitted-output level.
pub fn body_writes_state(body: &Block, program: &TypedCommons) -> bool {
    fn is_mutating_store_write(e: &Expr, program: &TypedCommons) -> bool {
        match program.callees.get(&e.id) {
            // #2194: the mutating set is the store-op registry's own
            // `mutates` flag. A `Callee::Store` already carries the op
            // resolved against its field's kind, so the op name alone decides.
            Some(Callee::Store { op, .. }) => bynk_check::store_ops::is_mutating_any(op),
            _ => false,
        }
    }
    fn stmt(s: &Statement, program: &TypedCommons) -> bool {
        match s {
            Statement::Assign(_) => true,
            Statement::Let(l) | Statement::EffectLet(l) => expr(&l.value, program),
            Statement::Expect(a) => expr(&a.value, program),
            Statement::Send(s) => expr(&s.value, program),
            Statement::Do(d) => expr(&d.value, program),
            Statement::Advance(a) => expr(&a.amount, program),
        }
    }
    fn expr(e: &Expr, program: &TypedCommons) -> bool {
        if is_mutating_store_write(e, program) {
            return true;
        }
        match &e.kind {
            ExprKind::Block(b) => body_writes_state(b, program),
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                expr(cond, program)
                    || body_writes_state(then_block, program)
                    || body_writes_state(else_block, program)
            }
            // #1800: a guard is evaluated like any other expression, so a
            // write in one (`Some(x) if if x > 0 { hits := x; true } …`) runs
            // and must be committed. Skipping it dropped the commit wrapper.
            ExprKind::Match { discriminant, arms } => {
                expr(discriminant, program)
                    || arms.iter().any(|a| {
                        a.guard.as_ref().is_some_and(|g| expr(g, program))
                            || match &a.body {
                                MatchBody::Expr(e) => expr(e, program),
                                MatchBody::Block(b) => body_writes_state(b, program),
                            }
                    })
            }
            _ => expr_children(e).into_iter().any(|c| expr(c, program)),
        }
    }
    body.statements.iter().any(|s| stmt(s, program)) || expr(&body.tail, program)
}

/// P6.11 ([DECISION C], #1171): reshape a `from Events(E { … })` pattern
/// into a real [`EventPatternIr`] — pure structural reshaping, no
/// `&CheckedProgram` parameter and no resolution: every field's own
/// matched value is either already a literal or an unresolved variant tag,
/// neither needing a `TyId`. `EventPatternValue::Literal` lowers through
/// the same `LiteralValue -> ConstVal` match [`lower_pattern_ir`] already
/// uses for `Pattern::Literal`'s identical closed set.
fn lower_event_pattern_ir(pattern: &EventPattern) -> EventPatternIr {
    EventPatternIr {
        fields: pattern
            .fields
            .iter()
            .map(|f| {
                let value = match &f.value {
                    EventPatternValue::Literal { value, .. } => {
                        EventPatternValueIr::Const(match value {
                            LiteralValue::Int(n) => ConstVal::Int(*n),
                            LiteralValue::Str(s) => ConstVal::Str(s.clone()),
                            LiteralValue::Bool(b) => ConstVal::Bool(*b),
                        })
                    }
                    EventPatternValue::Variant { variant, .. } => EventPatternValueIr::Variant {
                        tag: variant.name.clone(),
                    },
                };
                (f.name.name.clone(), value)
            })
            .collect(),
    }
}

/// P6.11 ([DECISION A], #1171): lower a service's own `from <protocol>`
/// header into a real [`ProtocolIr`] — standalone, takes the sub-node
/// rather than the owning `ServiceDecl` (mirrors
/// [`lower_store_field_shape_ir`]), so a `from websocket`/`from Events`
/// fixture can pin the descriptor by itself.
///
/// `WebSocket`/`Events`'s own type refs resolve through the `Ty::Unit`
/// fallback, not an ADR 0334 panic — deliberately: `resolver.rs` skips
/// `CommonsItem::Service` in every one of its own type-ref-resolution
/// passes (`resolver.rs:303-304`/`493-494`/`577-578`), and the one checker
/// site that does resolve a WebSocket frame type itself falls back to
/// `Ty::Unit` on a miss rather than erroring (`context_checks.rs:775-778`).
/// Panicking here would make this an ADR-0334 site asserting a guarantee
/// the checker doesn't actually give (the agent `key_ty` site that once did,
/// review of #1169, went with the item assembly in Slice D1 of `#1542`).
pub fn lower_protocol_ir(protocol: &ServiceProtocol, program: &CheckedProgram) -> ProtocolIr {
    lower_protocol_ir_from_commons(protocol, program.program())
}

/// P6.24a: a `TypedCommons`-only sibling of [`lower_protocol_ir`], the same
/// split `lower_op_sig_ir`/`lower_op_sig_ir_from_commons` already
/// established — for a call site holding only a unit's own `TypedCommons`,
/// never a `&CheckedProgram` (`emitter.rs`'s `emit_project_imports`, a
/// header-import-collection pass that runs well outside the per-declaration
/// emission loop any `CheckedProgram` is threaded through). Sound for the
/// identical reason: nothing here reads a per-expression type, the one
/// lookup whose `.expect()`-panic needed a genuinely certified program.
pub fn lower_protocol_ir_from_commons(
    protocol: &ServiceProtocol,
    commons: &TypedCommons,
) -> ProtocolIr {
    let cx = LowerIrCtx::from_commons(commons, HashSet::new());
    match protocol {
        ServiceProtocol::Call => ProtocolIr::Call,
        ServiceProtocol::Http => ProtocolIr::Http,
        ServiceProtocol::Cron => ProtocolIr::Cron,
        ServiceProtocol::Queue { name } => ProtocolIr::Queue { name: name.clone() },
        ServiceProtocol::WebSocket { in_type, out_type } => ProtocolIr::WebSocket {
            in_ty: cx.resolve_type_ref(in_type).unwrap_or_else(|| cx.unit_ty()),
            out_ty: cx
                .resolve_type_ref(out_type)
                .unwrap_or_else(|| cx.unit_ty()),
        },
        ServiceProtocol::Events {
            event_type,
            pattern,
            schema_dispatch,
        } => ProtocolIr::Events {
            event: cx
                .resolve_type_ref(event_type)
                .unwrap_or_else(|| cx.unit_ty()),
            pattern: pattern.as_ref().map(lower_event_pattern_ir),
            schema_dispatch: schema_dispatch.as_ref().map(|d| {
                let bynk_syntax::ast::SchemaVersionPattern::Literal(version) = d.pattern;
                version
            }),
        },
    }
}

/// #1228: a GET handler's own `@cache(maxAge:, scope:)` freshness policy —
/// [`bynk_ir::CacheIr`]'s own doc comment has the full grounding for why
/// this is a standalone per-route reader. Field-for-
/// field the same extraction `emitter/workers_entry.rs`'s own (now
/// superseded) `cache_policy_for` did: only a `GET` yields a policy;
/// project validation (`bynk.http.cache_*`) has already rejected a
/// `@cache` anywhere else, and a malformed `maxAge` there, so a missing or
/// ill-formed annotation here simply yields `None` — no `&CheckedProgram`
/// needed, the same posture `lower_policy_ir`'s own doc comment already
/// argues for: `maxAge`/`scope` are already-resolved syntactic literals
/// (`ExprKind::DurationLit`/`Ident`), not a type this pass would ever need
/// to resolve.
pub fn lower_route_cache_ir(h: &Handler) -> Option<CacheIr> {
    if !matches!(
        h.kind,
        HandlerKind::Http {
            method: HttpMethod::Get,
            ..
        }
    ) {
        return None;
    }
    let ann = h.annotations.iter().find(|a| a.name.name == "cache")?;
    let mut max_age_millis: Option<i64> = None;
    let mut scope = "private";
    for arg in &ann.args {
        match arg.label.as_ref().map(|l| l.name.as_str()) {
            Some("maxAge") => {
                if let ExprKind::DurationLit { millis, .. } = &arg.value.kind {
                    max_age_millis = Some(*millis);
                }
            }
            Some("scope") => {
                if let ExprKind::Ident(id) = &arg.value.kind
                    && id.name == "public"
                {
                    scope = "public";
                }
            }
            _ => {}
        }
    }
    Some(CacheIr {
        max_age_secs: max_age_millis? / 1000,
        scope,
    })
}

/// #1228: a route's own `@limit(maxBody:)` annotation, if present — the
/// override half of `emitter/workers_entry.rs`'s own (now superseded)
/// `effective_max_body`; the service-wide `limits { maxBody }` fallback
/// stays that function's own concern (read from a *service*'s `limits {}`
/// block, not a per-route `Handler`, so it does not move here). Project validation
/// (`bynk.http.limit_*`/`limits_*`) has already rejected a malformed or
/// misplaced `@limit`, so an absent/ill-formed annotation here simply
/// yields `None` — the caller's own service-default fallback still
/// applies. No `&CheckedProgram` needed, same reasoning as
/// [`lower_route_cache_ir`]: `maxBody` is an already-resolved
/// `ExprKind::IntLit`, not a type.
pub fn lower_route_limit_ir(h: &Handler) -> Option<i64> {
    let ann = h.annotations.iter().find(|a| a.name.name == "limit")?;
    for arg in &ann.args {
        if arg.label.as_ref().map(|l| l.name.as_str()) == Some("maxBody")
            && let ExprKind::IntLit { value: n, .. } = &arg.value.kind
            && *n > 0
        {
            return Some(*n);
        }
    }
    None
}

/// Every `from Events(E)` service in `program`'s own unit, captured as an
/// [`bynk_ir::EventSubscriberShape`] keyed by service name — see that
/// struct's own doc comment for why this is captured now rather than
/// re-derived cross-unit at compose time (P6.47, `#1254`).
///
/// Slice D0 of `#1542` (the IR cutover, `design/archive/retired-tracks.md`; `#1574`):
/// reads the two facts it returns from the shape-only helpers that own them —
/// [`lower_protocol_ir`] for `schema_dispatch`, and [`lower_handler_kind_ir`]
/// plus [`lower_service_handler_signature_ir`] for the `Event` handler's
/// parameter count. Before D0 this function went through
/// `lower_service_item_ir`, which lowers every handler's *body* to `IrExpr`
/// through the expression lowerer and then discards it — the one production
/// route into that lowerer, and the detour §10.2 of the track doc names (the
/// body lowering's own `unreachable!()` safety argument covered
/// `lower_fn_body_ir`'s callers only, never this path). The values are
/// identical by construction: `lower_service_handler_ir` itself took its
/// `params` from [`lower_service_handler_signature_ir`] and its `kind` from
/// [`lower_handler_kind_ir`], and `lower_service_item_ir` its `protocol` from
/// [`lower_protocol_ir`] — this function now calls those three directly and
/// skips the body.
///
/// The `ServiceProtocol::Events` pre-filter stays first: a cheap, structural
/// "which services even have a shape to capture" check (the same match
/// [`lower_protocol_ir`] performs), not a raw-AST *read* of anything the IR
/// side owns.
pub fn lower_event_subscriber_shapes_ir(
    program: &CheckedProgram,
) -> HashMap<String, EventSubscriberShape> {
    let mut out = HashMap::new();
    for item in &program.program().commons.items {
        if let CommonsItem::Service(s) = item
            && matches!(&s.protocol, ServiceProtocol::Events { .. })
        {
            let ProtocolIr::Events {
                schema_dispatch, ..
            } = lower_protocol_ir(&s.protocol, program)
            else {
                panic!(
                    "bynk internal error: lower_protocol_ir did not return \
                     ProtocolIr::Events for a service whose own AST protocol is \
                     ServiceProtocol::Events"
                )
            };
            let two_param_handler = s
                .handlers
                .iter()
                .find(|h| matches!(lower_handler_kind_ir(&h.kind), IrHandlerKind::Event))
                .is_some_and(|h| lower_service_handler_signature_ir(h, program).0.len() == 2);
            out.insert(
                s.name.name.clone(),
                EventSubscriberShape {
                    two_param_handler,
                    schema_dispatch: schema_dispatch.is_some(),
                },
            );
        }
    }
    out
}

/// A capability declaration's resolved op signatures, in declaration order
/// — what `emitter.rs`'s own capability-item loop reads (the caller already
/// holds the capability's name from the AST declaration). Slice 1 of
/// `#1542` split this out of a full `IrItem::Capability` constructor to end
/// a build-then-discard-`def` round-trip; Slice D1 deleted that constructor.
pub fn lower_capability_ops_ir(cap: &CapabilityDecl, program: &CheckedProgram) -> Vec<OpSig> {
    cap.ops
        .iter()
        .map(|op| lower_op_sig_ir(op, program))
        .collect()
}

/// P6.29 (design/tracks/the-ir.md §6a): the `TypedCommons`-only counterpart to
/// [`lower_capability_ops_ir`], for call sites (`emitter/lower.rs`'s
/// `cap_op_param_names`) that have a `TypedCommons` in hand but no
/// `CheckedProgram` — `LowerCtx`/`ModuleCtx` never carry one (see
/// `lower_op_sig_ir_from_commons`, this function's own single-op sibling,
/// for the identical reason it exists as a separate entry point rather than a
/// thin wrapper over the `CheckedProgram`-driven `lower_op_sig_ir`).
///
/// Resolves one capability operation's signature by name — "find the op
/// named `op` on the capability named `cap`" has no IR-native replacement
/// (nothing indexes capabilities by name once lowered), so this still walks
/// `TypedCommons::commons.items` the same way the code it replaces did.
/// First match in item order; `None` on no match, mirroring the caller's own
/// prior fallthrough-to-empty behaviour exactly.
pub fn capability_op_sig_from_commons(
    commons: &TypedCommons,
    cap: &str,
    op: &str,
) -> Option<OpSig> {
    commons.commons.items.iter().find_map(|item| {
        let CommonsItem::Capability(c) = item else {
            return None;
        };
        if c.name.name != cap {
            return None;
        }
        c.ops
            .iter()
            .find(|o| o.name.name == op)
            .map(|o| lower_op_sig_ir_from_commons(o, commons))
    })
}

/// P6.12 (#1173): lower one capability operation's own signature into a real
/// [`bynk_ir::OpSig`] — the reference's own capability-item sketch
/// names this type (`ops: Vec<OpSig>`) but never defines it. Resolves
/// `params`/`return_ty` in the scope `op.type_params` names, the same
/// per-op rigid-variable seeding `context_checks::build_capability_op_info`
/// (`bynk-check/src/context_checks.rs`) already gives a generic op for the
/// checker-facing `CapabilityOpInfo` — an op's own `[T, …]` list is scoped to
/// the op itself, not the capability (`CapabilityDecl` carries no
/// `type_params` of its own), so this seeds a fresh [`LowerIrCtx`] per op
/// rather than reusing `fn_rigid_type_vars`'s fn/method-shaped
/// receiver-widening, which does not apply here.
///
/// **Not an ADR 0334 `.expect()`-style panic on a resolve miss,
/// deliberately** — the same posture [`lower_agent_item_ir`]'s own `key_ty`
/// doc comment already argues for `agent.key_type`, and for the identical
/// reason: a capability op's own `params`/`return_type` are never actually
/// resolution-checked by the checker at all. The resolver skips
/// `CommonsItem::Capability` outright (`resolver.rs:301/491/575`, "v0.5
/// items are resolved via a separate context-level pass"); the context-level
/// pass that replaces it, `check_capability_decls`, only calls
/// `checker::record_type_refs`, which silently does nothing on a name absent
/// from `types` rather than erroring (`checker.rs:2593-2597`); and
/// `build_capability_op_info` itself, the checker-facing constructor this
/// pass mirrors, degrades to `Ty::Unit` on the same miss rather than
/// treating it as impossible (`context_checks.rs:36,40`). `capability Store
/// { fn get(k: Bogus) -> Effect[Int] }` certifies today, silently. Panicking
/// here on a state the checker itself accepts would make this the first ADR
/// 0334 site in this module to assert a guarantee that does not actually
/// hold — mirror the checker's own fallback instead (review of #1182).
fn lower_op_sig_ir(op: &CapabilityOp, program: &CheckedProgram) -> OpSig {
    lower_op_sig_ir_from_commons(op, program.program())
}

/// #1187's own closing scoping pass: a `TypedCommons`-only sibling of
/// `lower_op_sig_ir`, for the one real call site that never has a
/// `&CheckedProgram` — `emitter/lower.rs`'s `cap_op_param_names`, feeding
/// `trace(Cap.op)`/`with`-predicate observation lowering
/// (`bynk.test`'s DSL). That call path's own `TypedCommons` is a synthetic,
/// hand-assembled project-wide view (`project/tests_emit.rs`'s
/// `synthetic_typed_commons_for_target`, merging every consumed unit's own
/// `capability` declarations into one scratch commons for lookup) — never
/// itself the output of `certify`, so wrapping it as a `CheckedProgram`
/// here would misrepresent an uncertified value as certified
/// (`CheckedProgram`'s own doc comment, `bynk-check/src/checker.rs`, warns
/// against exactly this). Splitting this out is sound precisely because
/// this function never reads a per-expression type — the one lookup whose
/// `.expect()`-panic needed a genuinely certified program, and the reason
/// this module's own file-level doc comment gives for taking
/// `&CheckedProgram` by default elsewhere. `resolve_type_ref`/`unit_ty()` (below) both degrade via
/// `.unwrap_or_else` and read nothing `TypedCommons` doesn't already expose
/// directly.
fn lower_op_sig_ir_from_commons(op: &CapabilityOp, commons: &TypedCommons) -> OpSig {
    let type_vars: HashSet<String> = op
        .type_params
        .iter()
        .map(|tp| tp.name.name.clone())
        .collect();
    let cx = LowerIrCtx::from_commons(commons, type_vars);
    let params: Vec<(String, TyId)> = op
        .params
        .iter()
        .map(|p| {
            let ty = cx
                .resolve_type_ref(&p.type_ref)
                .unwrap_or_else(|| cx.unit_ty());
            (p.name.name.clone(), ty)
        })
        .collect();
    let return_ty = cx
        .resolve_type_ref(&op.return_type)
        .unwrap_or_else(|| cx.unit_ty());
    OpSig {
        name: op.name.name.clone(),
        type_params: op
            .type_params
            .iter()
            .map(|tp| tp.name.name.clone())
            .collect(),
        params,
        return_ty,
    }
}

/// P6.18: [`bynk_ir::FnSig`]'s own constructor — a `fn`'s own resolved
/// signature, for a call site holding only that fn's *declaring* unit's own
/// combined types (`bynk_check::symbols::combined_types_for`'s return shape),
/// never a `CheckedProgram`. The one real call site
/// (`bynk-emit/src/project.rs`'s `build_emit_unit_ctx`) reads a `uses`-
/// imported *foreign* unit's own attached methods, whose own `CheckedProgram`
/// does not survive past that unit's own `check_unit_files` iteration — the
/// same "dropped before any later, project-wide pass runs" shape
/// `unit_callees` (#1202)/`EventSubscriberShape` (#1232) both work around,
/// except here no project-wide accumulator is needed at all: unlike a
/// `Callee`/event-subscriber-shape classification (checker-only facts, never
/// re-derivable from raw declarations alone), a fn signature's own
/// `params`/`return_type` are ordinary type references, resolvable from that
/// unit's own declared types the same way [`lower_op_sig_ir_from_commons`]
/// already resolves a capability op's — so a bare types map is sufficient,
/// the same non-`CheckedProgram` scope that function already established.
///
/// Only the method's own `[T, …]` list seeds the rigid-variable scope, not
/// its generic receiver's — the one real caller (`emit_forwarded_methods`)
/// never renders `self`'s own type through this value at all (it takes the
/// *consumer* context's own rebranded type name directly), so resolving
/// `fn_receiver_ty` here would be dead work.
fn lower_fn_sig_ir_from_types(
    f: &FnDecl,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
) -> FnSig {
    let type_vars: HashSet<String> = f
        .type_params
        .iter()
        .map(|tp| tp.name.name.clone())
        .collect();
    let unit_ty = || tys.intern(Ty::Unit);
    let params: Vec<(String, TyId)> = f
        .params
        .iter()
        .map(|p| {
            let ty = checker::resolve_type_ref_in(&p.type_ref, types, &type_vars, tys)
                .unwrap_or_else(unit_ty);
            (p.name.name.clone(), ty)
        })
        .collect();
    let return_ty = checker::resolve_type_ref_in(&f.return_type, types, &type_vars, tys)
        .unwrap_or_else(unit_ty);
    let name = match &f.name {
        FnName::Method { method_name, .. } => method_name.name.clone(),
        FnName::Free(id) => id.name.clone(),
    };
    FnSig {
        name,
        has_self: f.has_self,
        params,
        return_ty,
    }
}

/// P6.x (#1137): `lower_fn_sig_ir_from_types` over an entire
/// [`MethodTable`]'s own instance + static entries — the attached-method
/// gathering [`bynk-emit`'s `build_emit_unit_ctx`] needs for a `uses`-imported
/// type. Filters to [`FnName::Method`] before lowering: `ResolverMethodTable`
/// only ever collects attached methods in practice (`bynk-check/src/resolver.rs`'s
/// own doc comment on [`MethodTable`]), but the filter stays as a defensive
/// match rather than an assumption, matching the caller's own pre-existing
/// posture one step earlier — this just moves that posture in front of the
/// lowering call instead of behind it, so the `FnName` read (and the filter
/// itself) never has to leave this module.
pub fn lower_attached_fn_sig_ir_from_types(
    mt: &MethodTable,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
) -> Vec<FnSig> {
    mt.instance
        .values()
        .chain(mt.statics.values())
        .filter(|f| matches!(f.name, FnName::Method { .. }))
        .map(|f| lower_fn_sig_ir_from_types(f, types, tys))
        .collect()
}

/// P6.24a: pure, unconditional [`HandlerKind`] → [`IrHandlerKind`]
/// conversion — every field is already fully resolved at parse time, so
/// unlike almost every other function in this module this one takes no
/// `&CheckedProgram`/`&TypedCommons` at all and can never miss.
pub fn lower_handler_kind_ir(k: &HandlerKind) -> IrHandlerKind {
    match k {
        HandlerKind::Call => IrHandlerKind::Call,
        HandlerKind::Http { method, path } => IrHandlerKind::Http {
            method: lower_http_method_ir(*method),
            path: path.clone(),
        },
        HandlerKind::Cron { expr } => IrHandlerKind::Cron { expr: expr.clone() },
        HandlerKind::Message => IrHandlerKind::Message,
        HandlerKind::Open => IrHandlerKind::Open,
        HandlerKind::Close => IrHandlerKind::Close,
        HandlerKind::Event => IrHandlerKind::Event,
    }
}

/// [`lower_handler_kind_ir`]'s own `HttpMethod` half.
fn lower_http_method_ir(m: HttpMethod) -> IrHttpMethod {
    match m {
        HttpMethod::Get => IrHttpMethod::Get,
        HttpMethod::Post => IrHttpMethod::Post,
        HttpMethod::Put => IrHttpMethod::Put,
        HttpMethod::Patch => IrHttpMethod::Patch,
        HttpMethod::Delete => IrHttpMethod::Delete,
    }
}

/// A provider's own `given` clause, resolved standalone — the entry point
/// `bynk-emit/src/project.rs`'s `instantiate_provider_ts_expr` actually
/// calls. This never touches the provider's `ops` or their bodies; the
/// full-provider assembly that did (and lowered every `Bynk` op body through
/// the expression lowerer) had no caller and went with Slice D1 of `#1542`.
pub fn lower_provider_given_ir(provider: &ProviderDecl) -> Vec<CapRefIr> {
    provider.given.iter().map(lower_cap_ref_ir).collect()
}

/// #1187's slice 6 plumbing (sibling of [`lower_provider_given_ir`]): a
/// handler's own `given` clause, resolved standalone — the entry point for
/// `project.rs`'s `plan_agent_given_deps`, `EmitProjectCtx::
/// agent_method_givens`, and `emitter/workers.rs`'s own `given` collection.
/// Reuses `lower_cap_ref_ir` verbatim; a handler's `given` is syntactically
/// identical to a provider's (`bynk_syntax::ast::CapRef`), so this is the
/// same one-line adapter, not a new design.
pub fn lower_handler_given_ir(h: &Handler) -> Vec<CapRefIr> {
    h.given.iter().map(lower_cap_ref_ir).collect()
}

/// #1187's slice 3: a handler's resolved actor-verification seam — the same
/// "narrow, standalone reader of already-resolved data" precedent
/// [`body_writes_state`]/[`lower_service_handler_signature_ir`] established,
/// applied to `bynk-check`'s own five actor-seam resolvers
/// (`bynk-check/src/actors.rs`) instead of a full handler assembly.
/// [`ActorSeamIr`]'s own doc comment has the full grounding for the
/// priority order and for the deliberately-missing `Signature` variant.
///
/// Replaces the hand-duplicated "try N resolvers, branch on which
/// returned `Some`" call sites this slice converts: `emit_service`
/// (`emitter/emit.rs`) and `emit_worker_compose`'s HTTP-dispatch match
/// (`emitter/workers.rs`). Deliberately does **not** yet replace every
/// caller of the five resolvers — `secrets.rs`'s `declared_secrets` unions
/// *all* matching seams' secrets rather than picking one (a different
/// shape this enum doesn't model), and the remaining call sites
/// (`emitter/workers_entry.rs`, `emitter/workers.rs`'s other two sites,
/// `emitter/emit.rs`'s `any_service_binds_caller`/`emit_make_surface`/
/// `ws_open_hosts_for`, `project/tests_emit.rs`) each call exactly one
/// resolver with nothing to collapse against — converting them to build a
/// five-variant enum just to immediately match out one arm would add
/// indirection without removing any real duplication.
pub fn lower_actor_seam_ir(handler: &Handler, actors: &HashMap<String, ActorDecl>) -> ActorSeamIr {
    if let Some(members) = bynk_check::actors::sum_members_for(handler, actors) {
        return ActorSeamIr::Sum(members);
    }
    if let Some(members) =
        bynk_check::actors::call_admission_for(handler, actors).and_then(|a| a.sum)
    {
        return ActorSeamIr::CallSum(members);
    }
    if let Some(seam) = bynk_check::actors::bearer_seam_for(handler, actors) {
        return ActorSeamIr::Bearer(seam);
    }
    if let Some(seam) = bynk_check::actors::oidc_seam_for(handler, actors) {
        return ActorSeamIr::Oidc(seam);
    }
    if let Some(binder) = bynk_check::actors::caller_binder_for(handler, actors) {
        return ActorSeamIr::Caller(binder);
    }
    ActorSeamIr::None
}

/// P6.14 (#1174, review of #1186): adapt one `given` entry into a real
/// [`bynk_ir::CapRefIr`] — [`CapRefIr`]'s own doc comment has the full
/// grounding for the `QualifiedName -> String` flattening and for why a
/// `Some` prefix is preserved unresolved.
fn lower_cap_ref_ir(cap_ref: &CapRef) -> CapRefIr {
    CapRefIr {
        context: cap_ref.context.as_ref().map(QualifiedName::joined),
        name: cap_ref.name.name.clone(),
    }
}

/// Decision E: targeted minimal fixtures, one per node kind this slice
/// covers, staying strictly inside the subset [`lower_expr_ir`]/
/// [`lower_stmt_ir`] actually implement — not a walk over the real
/// `bynkc/tests/fixtures/positive` corpus, which hits an unimplemented
/// `Match`/`Call` arm within a few lines of almost any real fixture.
#[cfg(test)]
mod tests {
    use super::*;
    use bynk_check::checker::CheckedProgram;
    use bynk_check::hints::HintSink;
    use bynk_check::index::RefSink;
    use bynk_check::locals::LocalsSink;
    use bynk_check::requirements::RequirementSink;
    use bynk_check::{checker, context_checks, resolver, symbols};
    use bynk_project::UnitKind;
    use bynk_syntax::ast::{AgentDecl, Commons, CommonsItem, HandlerKind, ServiceDecl, SourceUnit};
    use bynk_syntax::span::Span;
    use bynk_syntax::{lexer, parser};

    /// Like [`checked_program`], but for source that declares an `agent`
    /// and/or a `service` — both are only legal inside a `context`, not a
    /// bare `commons` (`bynk.agent.outside_context`/the service
    /// equivalent), so `source` is parsed as a context unit and its items
    /// re-wrapped into a [`Commons`] value before re-using the same
    /// `resolve`/`check` pipeline `checked_program` does.
    /// `resolver::resolve`/`checker::check` both already treat
    /// `CommonsItem::Agent`/`Service` as inert (v0.5 declaration kinds "go
    /// through the context-level v0.5 path", `resolver.rs`'s own comment)
    /// — real agent/service checking (`store` field kinds, handler bodies,
    /// `expr_types` for a `Cell` initialiser, actor bindings) only happens
    /// via `context_checks::check_context_declarations`, called here by
    /// hand with a [`symbols::UnitTable`] built directly from the checked
    /// commons' own agent/service/actor and local `capability` items (a
    /// handler's `given <Cap>` resolves against `table.capabilities` —
    /// populated here so a fixture can declare its own local capability,
    /// but still no cross-context `uses`/`consumes` in any fixture this
    /// helper is given, so `resolver::CrossContextInfo::default()` is
    /// exact, not an approximation — in particular, no `from Events(E)`
    /// fixture is possible here, since a real subscription needs
    /// `consumes bynk { Events }`).
    ///
    /// **P6.11 (#1171) adds `services`/`actors` to the table and a
    /// pre-`resolve` `inject_service_defaults` pass.** `table.actors`
    /// matters even for a fixture using only the prelude (`Caller`,
    /// `Visitor`): `actor_identity_ty` resolves a *local* `actor`
    /// declaration through `table.actors` and silently falls through to
    /// the prelude/`Ty::Unit` otherwise
    /// (`context_checks.rs::actor_identity_ty`), so a `by u: Buyer`
    /// fixture without this would assert against a wrong `TyId` with no
    /// error at all. `inject_service_defaults` stands in for
    /// `bynk-check/src/analysis.rs`'s own pipeline-phase-2b call — this
    /// reduced harness has no such phase — so a fixture relying on a
    /// service-level `by`/`given` default (rather than declaring one per
    /// handler) would otherwise silently see it un-inherited, pinning the
    /// wrong fact with no failure to signal it.
    fn checked_context_program(source: &str) -> CheckedProgram {
        let tokens = lexer::tokenize(source).expect("lex");
        let unit = parser::parse_unit(&tokens, source).expect("parse");
        let SourceUnit::Context(mut ctx) = unit else {
            panic!("expected a context unit, got {unit:?}")
        };
        for item in &mut ctx.items {
            if let CommonsItem::Service(svc) = item {
                bynk_check::project_model::inject_service_defaults(svc);
            }
        }
        let commons = Commons {
            name: ctx.name,
            items: ctx.items,
            uses: ctx.uses,
            documentation: ctx.documentation,
            form: ctx.form,
            span: ctx.span,
            trivia: ctx.trivia,
            trailing_comments: ctx.trailing_comments,
        };
        let resolved = resolver::resolve(commons).expect("resolve");
        let mut typed = checker::check(resolved).expect("check");
        let agents: HashMap<String, AgentDecl> = typed
            .commons
            .items
            .iter()
            .filter_map(|item| match item {
                CommonsItem::Agent(a) => Some((a.name.name.clone(), a.clone())),
                _ => None,
            })
            .collect();
        let services: HashMap<String, ServiceDecl> = typed
            .commons
            .items
            .iter()
            .filter_map(|item| match item {
                CommonsItem::Service(s) => Some((s.name.name.clone(), s.clone())),
                _ => None,
            })
            .collect();
        let actors: HashMap<String, bynk_syntax::ast::ActorDecl> = typed
            .commons
            .items
            .iter()
            .filter_map(|item| match item {
                CommonsItem::Actor(a) => Some((a.name.name.clone(), a.clone())),
                _ => None,
            })
            .collect();
        // A `given <Cap>` clause on a handler resolves against
        // `table.capabilities` (`context_checks.rs`'s own `capability_info_map`
        // construction) — populated here from this fixture's own local
        // `capability` declarations so a handler can legitimately declare one
        // (e.g. `Log.append`'s own `given Clock` requirement), the same
        // "no cross-context uses/consumes" scope this helper's own doc
        // comment already names for `agents`/`types`.
        let capabilities: HashMap<String, bynk_syntax::ast::CapabilityDecl> = typed
            .commons
            .items
            .iter()
            .filter_map(|item| match item {
                CommonsItem::Capability(c) => Some((c.name.name.clone(), c.clone())),
                _ => None,
            })
            .collect();
        // P6.14 (#1174): `check_provider_decls` (the pass that actually
        // type-checks a `ProviderOp`'s own body via `check_handler_body`)
        // reads `table.providers`, keyed by capability name — same "one
        // provider per capability in v0.5" convention
        // `symbols::UnitTable::providers`'s own doc comment names. Populated
        // here for the same reason `capabilities` is (above): without it, a
        // fixture's own `provides` declaration is silently never checked at
        // all, not merely under-checked (feedback memory
        // "bynk-emit test harness scope").
        let providers: HashMap<String, ProviderDecl> = typed
            .commons
            .items
            .iter()
            .filter_map(|item| match item {
                CommonsItem::Provider(p) => Some((p.capability.name.clone(), p.clone())),
                _ => None,
            })
            .collect();
        let table = symbols::UnitTable {
            kind: Some(UnitKind::Context),
            types: typed.types.clone(),
            agents,
            services,
            actors,
            capabilities,
            providers,
            ..symbols::UnitTable::default()
        };
        let tys = typed.ty_intern.clone();
        let errors = context_checks::check_context_declarations(
            &mut typed,
            &table,
            &resolver::CrossContextInfo::default(),
            true,
            &HashSet::new(),
            &HashMap::new(),
            &mut RefSink::new(),
            &mut HintSink::new(),
            &mut LocalsSink::new(),
            &mut RequirementSink::new(),
            &tys,
        );
        checker::certify(typed, errors).expect("certify")
    }

    fn find_agent<'a>(program: &'a CheckedProgram, name: &str) -> &'a AgentDecl {
        program
            .program()
            .commons
            .items
            .iter()
            .find_map(|item| match item {
                CommonsItem::Agent(a) if a.name.name == name => Some(a),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no agent named `{name}` in this fixture"))
    }

    /// `lower_actor_seam_ir`'s own `actors` map — rebuilt the same way
    /// `checked_context_program` builds its own throwaway `table.actors`
    /// (not itself exposed on `CheckedProgram`), since the resolvers it
    /// wraps take the same `HashMap<String, ActorDecl>` shape the real
    /// `table.actors`/`ctx.actors` emitter-side callers already carry.
    fn actors_map(program: &CheckedProgram) -> HashMap<String, ActorDecl> {
        program
            .program()
            .commons
            .items
            .iter()
            .filter_map(|item| match item {
                CommonsItem::Actor(a) => Some((a.name.name.clone(), a.clone())),
                _ => None,
            })
            .collect()
    }

    fn find_service<'a>(program: &'a CheckedProgram, name: &str) -> &'a ServiceDecl {
        program
            .program()
            .commons
            .items
            .iter()
            .find_map(|item| match item {
                CommonsItem::Service(s) if s.name.name == name => Some(s),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no service named `{name}` in this fixture"))
    }

    /// `find_handler` (below) matches on `method_name`, which is always
    /// `None` for a service handler — useless here. Matches on
    /// `HandlerKind` equality instead; not a unique identity on its own (a
    /// service may declare several handlers sharing one `HandlerKind`, all
    /// `on call`), so a fixture with more than one same-kind handler must
    /// index `service.handlers`/the lowered `handlers` slice directly
    /// instead of calling this twice.
    fn find_service_handler<'a>(service: &'a ServiceDecl, kind: &HandlerKind) -> &'a Handler {
        service
            .handlers
            .iter()
            .find(|h| &h.kind == kind)
            .unwrap_or_else(|| {
                panic!(
                    "no handler of kind {kind:?} on service `{}`",
                    service.name.name
                )
            })
    }

    fn find_store_field<'a>(agent: &'a AgentDecl, name: &str) -> &'a StoreField {
        agent
            .store_fields
            .iter()
            .find(|f| f.name.name == name)
            .unwrap_or_else(|| {
                panic!(
                    "no store field named `{name}` on agent `{}`",
                    agent.name.name
                )
            })
    }

    /// [`lower_store_field_shape_ir`] reads a store field's shape and never
    /// its initialiser — pinned against the two initialiser forms that once
    /// tripped the (since deleted) `init`-lowering sibling: `= None`
    /// (`223_store_cell_agent`'s `store paymentRef: Cell[Option[AuthId]] =
    /// None`) and an `is`-expression (`1029_agent_static_init_hoist`'s
    /// `store active: Cell[Bool] = if true { 5 is PositiveInt } else {
    /// false }`). Both are real, certified fixtures; the shape reader lowers
    /// both fields cleanly because it does not touch `init` at all.
    #[test]
    fn store_field_shape_ir_does_not_panic_on_none_or_is_initialisers() {
        let program = checked_context_program(
            r#"
context demo

type AuthId = String where NonEmpty

agent Order {
  key id: String
  store paymentRef: Cell[Option[AuthId]] = None

  on call touch() -> Effect[()] {
    Effect.pure(())
  }
}
"#,
        );
        let agent = find_agent(&program, "Order");
        let payment_ref = find_store_field(agent, "paymentRef");
        let ir = lower_store_field_shape_ir(payment_ref, &program);
        assert_eq!(ir.field, "paymentRef");
        assert!(matches!(ir.kind, StoreKindIr::Cell(_)));

        let program = checked_context_program(
            r#"
context demo

type PositiveInt = Int where Positive

agent Meter {
  key id: String
  store active: Cell[Bool] = if true { 5 is PositiveInt } else { false }

  on call touch() -> Effect[()] {
    Effect.pure(())
  }
}
"#,
        );
        let agent = find_agent(&program, "Meter");
        let active = find_store_field(agent, "active");
        let ir = lower_store_field_shape_ir(active, &program);
        assert_eq!(ir.field, "active");
        assert!(matches!(ir.kind, StoreKindIr::Cell(_)));
    }

    /// Review of #1209: pins the one load-bearing ordering decision
    /// `ActorSeamIr`'s own doc comment argues for — `sum_members_for`
    /// ahead of `bearer_seam_for` — at `lower_actor_seam_ir` itself, not
    /// only four fixture-hops away via a full `emit_service`/`bless` run.
    /// `bearer_seam_for` has no `by.is_sum()` guard of its own and resolves
    /// off `by.primary()`, so a Bearer-first sum (`by who: User | Visitor`
    /// with `User`'s own scheme `Bearer`) would resolve as `ActorSeamIr::
    /// Bearer` instead of `ActorSeamIr::Sum` if the two resolvers were ever
    /// tried in the other order.
    #[test]
    fn lower_actor_seam_ir_tries_sum_ahead_of_bearer_for_a_bearer_first_sum() {
        let program = checked_context_program(
            r#"
context demo

type UserId = String

actor User { auth = Bearer(secret = "AUTH_SECRET"), identity = UserId }

service Api from http {
  on GET("/whoami") () -> Effect[HttpResult[String]] by who: User | Visitor {
    Effect.pure(Ok("ok"))
  }
}
"#,
        );
        let service = find_service(&program, "Api");
        let handler = &service.handlers[0];
        let actors = actors_map(&program);
        let seam = lower_actor_seam_ir(handler, &actors);
        let ActorSeamIr::Sum(members) = &seam else {
            panic!("expected ActorSeamIr::Sum for a Bearer-first sum `by` clause, got {seam:?}");
        };
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].actor_name, "User");
        assert_eq!(members[1].actor_name, "Visitor");
    }

    /// [`lower_service_handler_signature_ir`] is `emit_service`'s entry
    /// point for a handler's resolved signature (#1187's slice 5, review of
    /// #1196) and never reads the body. Pinned against the shape that
    /// motivated it: an ordinary `from http` handler body constructing
    /// `Ok(...)` directly, not routed through the `fn ok(s) ->
    /// HttpResult[String] { Ok(s) }` indirection other fixtures in this
    /// module use — historically the body shape a full handler lowering
    /// panicked on, and still the clearest demonstration that this reader is
    /// signature-only.
    #[test]
    fn service_handler_signature_lowers_without_touching_a_body_that_constructs_ok() {
        let program = checked_context_program(
            r#"
context demo

service Api from http {
  on GET("/ping") () -> Effect[HttpResult[String]] by v: Visitor {
    Effect.pure(Ok("pong"))
  }
}
"#,
        );
        let service = find_service(&program, "Api");
        let handler = find_service_handler(
            service,
            &HandlerKind::Http {
                method: bynk_syntax::ast::HttpMethod::Get,
                path: "/ping".to_string(),
            },
        );
        let (params, _given, ret, effectful) =
            lower_service_handler_signature_ir(handler, &program);
        assert!(params.is_empty(), "`() -> ...` declares no parameters");
        assert!(effectful, "an `Effect[...]` return type");
        assert!(matches!(
            &*program.program().ty_intern.get(ret),
            Ty::Effect(_)
        ));
    }

    /// Mirrors `bynkc/tests/fixtures/positive/236_websocket_chatroom` in
    /// full — `on open`/`on message`/`on close` all present, the same
    /// shape the real fixture uses — so this fixture's own tests can cover
    /// both the owned (`on open`) and borrowed (`on message`/`on close`,
    /// P6.13, #1179) `connection` cases. A held `Connection` needs real
    /// disposal to certify (the linearity pass), so `on open` transfers it
    /// into a trivial `Room` agent rather than dropping it.
    fn websocket_service_fixture() -> CheckedProgram {
        checked_context_program(
            r#"
context demo

type RoomId = String
type UserId = String
type ServerFrame = { text: String }
type ClientFrame = { text: String }

actor Participant { auth = Bearer(secret = "AUTH_SECRET"), identity = UserId }

service ChatGateway from websocket(in: ClientFrame, out: ServerFrame) {
  on open (roomId: RoomId) -> Effect[()] by user: Participant {
    let _ <- connection.send(ServerFrame { text: "welcome" })
    let _ <- Room(roomId).join(user.identity, connection)
    ()
  }

  on message (roomId: RoomId, frame: ClientFrame) -> Effect[()] by user: Participant {
    let _ <- connection.send(ServerFrame { text: frame.text })
    let _ <- Room(roomId).post(user.identity, frame.text)
    ()
  }

  on close (roomId: RoomId) -> Effect[()] by user: Participant {
    let _ <- Room(roomId).leave(user.identity)
    ()
  }
}

agent Room {
  key id: RoomId
  store members: Set[UserId]
  store conns: Map[UserId, Connection[ServerFrame]]

  on call join(u: UserId, conn: Connection[ServerFrame]) -> Effect[()] {
    let _ <- members.add(u)
    let _ <- conns.put(u, conn)
    ()
  }

  on call leave(u: UserId) -> Effect[()] {
    let _ <- members.remove(u)
    let _ <- conns.remove(u)
    ()
  }

  on call post(sender: UserId, text: String) -> Effect[()] {
    let _ <- conns.parTraverse((c: Connection[ServerFrame]) => c.send(ServerFrame { text: text }))
    ()
  }
}
"#,
        )
    }

    #[test]
    fn websocket_protocol_descriptor_lowers_its_frame_types() {
        let program = websocket_service_fixture();
        let service = find_service(&program, "ChatGateway");
        let ir = lower_protocol_ir(&service.protocol, &program);
        let ProtocolIr::WebSocket { in_ty, out_ty } = ir else {
            panic!("expected ProtocolIr::WebSocket, got {:?}", ir)
        };
        let tys = &program.program().ty_intern;
        assert_eq!(in_ty.display(tys), "ClientFrame");
        assert_eq!(out_ty.display(tys), "ServerFrame");
    }

    #[test]
    fn lower_event_pattern_ir_reshapes_literal_and_variant_fields() {
        // Review of #1180: the `Events` protocol path had zero coverage.
        // `lower_protocol_ir`'s own `Events` arm genuinely can't be driven
        // through `checked_context_program` — a real `from Events(E)`
        // subscription needs `consumes bynk { Events }`, which this
        // reduced harness's `CrossContextInfo::default()` doesn't support
        // (the same limitation named in `checked_context_program`'s own
        // doc comment). `lower_event_pattern_ir` itself has no such
        // excuse: it takes no `&CheckedProgram`, resolves nothing, and
        // cannot panic — pure AST reshaping — so it's pinned directly
        // against a parsed (not checked or certified) `EventPattern`.
        let source = r#"
context demo

type Status = | Active | Inactive

event OrderPlaced = {
  status: Status,
  count: Int,
}

service Subscriber from Events(OrderPlaced { status: Active, count: 3, .. }) {
  on event(o: OrderPlaced) -> Effect[()] {
    Effect.pure(())
  }
}
"#;
        let tokens = lexer::tokenize(source).expect("lex");
        let unit = parser::parse_unit(&tokens, source).expect("parse");
        let SourceUnit::Context(ctx) = unit else {
            panic!("expected a context unit, got {unit:?}")
        };
        let service = ctx
            .items
            .iter()
            .find_map(|item| match item {
                CommonsItem::Service(s) if s.name.name == "Subscriber" => Some(s),
                _ => None,
            })
            .expect("no service named `Subscriber` in this fixture");
        let ServiceProtocol::Events { pattern, .. } = &service.protocol else {
            panic!(
                "expected ServiceProtocol::Events, got {:?}",
                service.protocol
            )
        };
        let pattern = pattern
            .as_ref()
            .expect("expected a structural pattern on this Events subscription");

        let ir = lower_event_pattern_ir(pattern);
        assert_eq!(ir.fields.len(), 2, "declaration order preserved");
        assert_eq!(ir.fields[0].0, "status");
        assert!(
            matches!(&ir.fields[0].1, EventPatternValueIr::Variant { tag } if tag == "Active"),
            "expected a bare nullary variant tag (the AST's own optional qualifying type_name \
             dropped), got {:?}",
            ir.fields[0].1
        );
        assert_eq!(ir.fields[1].0, "count");
        assert!(
            matches!(
                &ir.fields[1].1,
                EventPatternValueIr::Const(ConstVal::Int(3))
            ),
            "expected a literal Int constant, got {:?}",
            ir.fields[1].1
        );
    }

    /// P6.29 (design/tracks/the-ir.md §6a): pins `capability_op_sig_from_commons`
    /// against the same fixture as its `CheckedProgram`-driven sibling above —
    /// same param names/order, found by name alone from `TypedCommons`, no
    /// `CheckedProgram` needed at the call site (`emitter/lower.rs`'s
    /// `cap_op_param_names` only ever had one).
    #[test]
    fn capability_op_sig_from_commons_finds_the_named_op() {
        let program = checked_context_program(
            r#"
context demo

capability Store {
  fn get(key: String) -> Effect[Int]
  fn put(key: String, value: Int) -> Effect[()]
}
"#,
        );
        let commons = program.program();

        let get = capability_op_sig_from_commons(commons, "Store", "get")
            .expect("Store.get should resolve");
        assert_eq!(get.params.len(), 1);
        assert_eq!(get.params[0].0, "key");

        let put = capability_op_sig_from_commons(commons, "Store", "put")
            .expect("Store.put should resolve");
        assert_eq!(put.params.len(), 2);
        assert_eq!(put.params[0].0, "key");
        assert_eq!(put.params[1].0, "value");

        // Same fallthrough-to-`None` behaviour the by-hand AST walk it
        // replaces had, for both an unknown capability and a known
        // capability's unknown op — mirrors `cap_op_param_names`'s own prior
        // fallthrough-to-empty-`Vec` at the caller.
        assert!(capability_op_sig_from_commons(commons, "NoSuchCap", "get").is_none());
        assert!(capability_op_sig_from_commons(commons, "Store", "no_such_op").is_none());
    }

    #[test]
    fn lower_cap_ref_ir_local_capability_has_no_context() {
        let cap_ref = CapRef {
            context: None,
            name: bynk_syntax::ast::Ident {
                name: "Clock".to_string(),
                span: Span::default(),
            },
            span: Span::default(),
        };
        let ir = lower_cap_ref_ir(&cap_ref);
        assert_eq!(ir.context, None);
        assert_eq!(ir.name, "Clock");
    }

    #[test]
    fn lower_cap_ref_ir_preserves_a_cross_context_prefix() {
        // `given B.Cap` (v0.15) is out of `checked_context_program`'s own
        // fixture scope (no cross-context `uses`/`consumes`, feedback
        // memory "bynk-emit test harness scope") — pins `lower_cap_ref_ir`'s
        // own `QualifiedName -> String` flattening directly against a
        // hand-built `CapRef`, the same posture the external-provider test
        // above already takes for a branch the fixture cannot reach.
        let cap_ref = CapRef {
            context: Some(QualifiedName {
                parts: vec![bynk_syntax::ast::Ident {
                    name: "Billing".to_string(),
                    span: Span::default(),
                }],
                span: Span::default(),
            }),
            name: bynk_syntax::ast::Ident {
                name: "Ledger".to_string(),
                span: Span::default(),
            },
            span: Span::default(),
        };
        let ir = lower_cap_ref_ir(&cap_ref);
        assert_eq!(ir.context.as_deref(), Some("Billing"));
        assert_eq!(ir.name, "Ledger");
    }

    /// Review of #1229 (#1228): `lower_route_cache_ir`/`lower_route_limit_ir`
    /// take no `&CheckedProgram`, resolve nothing, and cannot panic — the same
    /// posture `lower_event_pattern_ir`'s own test above already established a
    /// parsed-not-checked fixture for, and for the identical reason here: their
    /// defensive branches (a non-`GET` handler, a `maxAge`-less `@cache`, a
    /// non-positive `maxBody`) are exactly the shapes `bynk-check`'s
    /// `bynk.http.cache_on_non_get`/`cache_bad_max_age`/`limit_bad_max_body`
    /// (`bynk-check/src/context_checks.rs`) already reject, so a real checked
    /// program can never reach them and the fixture bless run never exercises
    /// them either.
    fn parsed_only_context(source: &str) -> bynk_syntax::ast::Context {
        let tokens = lexer::tokenize(source).expect("lex");
        let unit = parser::parse_unit(&tokens, source).expect("parse");
        let SourceUnit::Context(ctx) = unit else {
            panic!("expected a context unit, got {unit:?}")
        };
        ctx
    }

    fn parsed_handler<'a>(
        ctx: &'a bynk_syntax::ast::Context,
        service: &str,
        index: usize,
    ) -> &'a Handler {
        let service = ctx
            .items
            .iter()
            .find_map(|item| match item {
                CommonsItem::Service(s) if s.name.name == service => Some(s),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no service named `{service}` in this fixture"));
        &service.handlers[index]
    }

    #[test]
    fn lower_route_cache_ir_reads_maxage_and_scope_off_a_get_handler() {
        let ctx = parsed_only_context(
            r#"
context demo

service Api from http {
  @cache(maxAge: 5.minutes, scope: public)
  on GET("/config") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("cfg")
  }

  @cache(maxAge: 30.seconds)
  on GET("/private") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("priv")
  }

  on GET("/plain") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("plain")
  }
}
"#,
        );
        let public_cache = lower_route_cache_ir(parsed_handler(&ctx, "Api", 0))
            .unwrap_or_else(|| panic!("expected Some(CacheIr) for a well-formed @cache"));
        assert_eq!(public_cache.max_age_secs, 300, "5.minutes in whole seconds");
        assert_eq!(public_cache.scope, "public");

        let default_scope_cache = lower_route_cache_ir(parsed_handler(&ctx, "Api", 1))
            .unwrap_or_else(|| panic!("expected Some(CacheIr) with no explicit scope:"));
        assert_eq!(default_scope_cache.max_age_secs, 30);
        assert_eq!(
            default_scope_cache.scope, "private",
            "no scope: argument written — must default to private"
        );

        assert!(
            lower_route_cache_ir(parsed_handler(&ctx, "Api", 2)).is_none(),
            "no @cache annotation at all must yield None"
        );
    }

    #[test]
    fn lower_route_cache_ir_returns_none_for_a_non_get_handler_even_with_a_cache_annotation() {
        // `bynk.http.cache_on_non_get` already rejects this at check time — this
        // pins the lowering function's own independent guard, not reachable
        // through a real certified program.
        let ctx = parsed_only_context(
            r#"
context demo

service Api from http {
  @cache(maxAge: 5.minutes)
  on POST("/items") (body: String) -> Effect[HttpResult[String]] by v: Visitor {
    Created(body)
  }
}
"#,
        );
        assert!(
            lower_route_cache_ir(parsed_handler(&ctx, "Api", 0)).is_none(),
            "a @cache on a non-GET handler must not construct a CacheIr"
        );
    }

    #[test]
    fn lower_route_cache_ir_discards_an_otherwise_well_formed_scope_when_maxage_is_missing() {
        // `bynk.http.cache_bad_max_age` already rejects a maxAge-less @cache at
        // check time — this pins that `scope`'s own well-formedness does not
        // rescue a missing `maxAge` into a partial CacheIr.
        let ctx = parsed_only_context(
            r#"
context demo

service Api from http {
  @cache(scope: public)
  on GET("/broken") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("x")
  }
}
"#,
        );
        assert!(
            lower_route_cache_ir(parsed_handler(&ctx, "Api", 0)).is_none(),
            "a well-formed scope: must not survive a missing maxAge:"
        );
    }

    #[test]
    fn lower_route_limit_ir_reads_maxbody_off_a_route_annotation() {
        let ctx = parsed_only_context(
            r#"
context demo

service Api from http {
  @limit(maxBody: 26_214_400)
  on POST("/bulk") (body: String) -> Effect[HttpResult[String]] by v: Visitor {
    Created(body)
  }

  on POST("/upload") (body: String) -> Effect[HttpResult[String]] by v: Visitor {
    Created(body)
  }
}
"#,
        );
        assert_eq!(
            lower_route_limit_ir(parsed_handler(&ctx, "Api", 0)),
            Some(26_214_400)
        );
        assert!(
            lower_route_limit_ir(parsed_handler(&ctx, "Api", 1)).is_none(),
            "no @limit annotation at all must yield None — the caller applies the \
             service-wide default, this function does not know it"
        );
    }

    #[test]
    fn lower_route_limit_ir_returns_none_for_a_non_positive_maxbody() {
        // `bynk.http.limit_bad_max_body` already rejects a non-positive maxBody
        // at check time — this pins the lowering function's own independent
        // guard. `None` here matters specifically because the caller's own
        // fallback composition (`effective_max_body`) treats it as "no
        // route-level override," falling through to the service-wide default —
        // not as "an explicit zero-byte cap."
        let ctx = parsed_only_context(
            r#"
context demo

service Api from http {
  @limit(maxBody: 0)
  on POST("/zero") (body: String) -> Effect[HttpResult[String]] by v: Visitor {
    Created(body)
  }
}
"#,
        );
        assert!(
            lower_route_limit_ir(parsed_handler(&ctx, "Api", 0)).is_none(),
            "a non-positive maxBody must not construct Some(0)"
        );
    }

    /// Every `body_writes_state` classification (#1165's [DECISION B]/
    /// [DECISION C], the shipped `emit_agent` implicit-commit decision) lives
    /// on one agent — each handler exercises exactly one case so a failing
    /// assertion names its own scenario unambiguously. Until Slice D1 of
    /// `#1542` these pinned the same function through the deleted IR-side
    /// `CommitShape` constructor; they now pin it directly.
    fn store_write_fixture() -> CheckedProgram {
        checked_context_program(
            r#"
context demo

type Box = { n: Int }

fn Box.put(self, x: Int) -> Effect[()] {
  Effect.pure(())
}

capability Clock {
  fn now() -> Effect[Int]
}

provides Clock = FixedClock {
  fn now() -> Effect[Int] {
    42
  }
}

agent Widget {
  key id: String
  store items: Map[String, Int]
  store active: Cell[Bool] = true
  store tags: Set[String]
  store history: Log[String]

  on call readOnlyPlain() -> Effect[()] {
    Effect.pure(())
  }

  on call readOnlyQuery() -> Effect[Int] {
    items.size()
  }

  on call nestedMutation(xs: List[String], flag: Bool) -> Effect[()] {
    if flag {
      match flag {
        true => xs.forEach((x) => items.put(x, 1))
        false => Effect.pure(())
      }
    } else {
      Effect.pure(())
    }
  }

  on call bareAssign(v: Bool) -> Effect[()] {
    active := v
    Effect.pure(())
  }

  on call shadowedName(items: Box, x: Int) -> Effect[()] {
    let _ <- items.put(x)
    Effect.pure(())
  }

  on call cellUpdate() -> Effect[()] {
    let _ <- active.update((b) => !b)
    Effect.pure(())
  }

  on call setAdd(t: String) -> Effect[()] {
    let _ <- tags.add(t)
    Effect.pure(())
  }

  on call logAppend(t: String) -> Effect[()] given Clock {
    let _ <- history.append(t)
    Effect.pure(())
  }
}
"#,
        )
    }

    fn find_handler<'a>(agent: &'a AgentDecl, name: &str) -> &'a Handler {
        agent
            .handlers
            .iter()
            .find(|h| h.method_name.as_ref().is_some_and(|m| m.name == name))
            .unwrap_or_else(|| panic!("no handler named `{name}` on agent `{}`", agent.name.name))
    }

    /// A queue consumer's own body is the one shape this module's own
    /// pre-existing gaps make genuinely unlowerable today, not just
    /// awkward to fixture around: `Effect[QueueResult]` is mandatory
    /// (`bynk.queue.return_not_https`-adjacent gate, `context_checks.rs:
    /// 3730-3744`), and every `QueueResult` value — `Ack`, `NotFound`,
    /// `Retry(reason)` — is a bare or qualified built-in-sum variant
    /// reference (a contextual, `expected`-type-driven disambiguation).
    /// Unlike `HttpResult`'s `Ok`/`Err`, `QueueResult`'s
    /// own variants also don't resolve inside an ordinary free `fn` body at
    /// all (confirmed empirically: `bynk.resolve.unknown_name`) — the
    /// checker's own special-case for them (`checker.rs:3507`) is reached
    /// only via a real handler body's own `Ctx::return_ty`, which the
    /// resolver's eager pass over an ordinary `fn` never sets up — so the
    /// `fn ok(s) -> HttpResult[String] { Ok(s) }` indirection every other
    /// HTTP/cron fixture in this module uses has no queue-shaped
    /// equivalent. Two tests, not one, cover what's actually true here.
    fn queue_service_fixture() -> CheckedProgram {
        checked_context_program(
            r#"
context demo

type EmailJob = { to: String }

service Outbox from queue("orders") {
  on message(m: EmailJob) -> Effect[QueueResult] {
    Ack
  }
}
"#,
        )
    }

    #[test]
    fn a_queue_services_protocol_and_handler_signature_lower_correctly() {
        // The protocol descriptor (standalone, mirroring `lower_protocol_ir`'s
        // own precedent for `from websocket`) and the handler's own
        // `params`/`given`/`effectful` via `lower_service_handler_signature_ir`
        // — the two shape readers `bynk-emit` consumes for a queue service.
        let program = queue_service_fixture();
        let service = find_service(&program, "Outbox");
        assert!(matches!(
            lower_protocol_ir(&service.protocol, &program),
            ProtocolIr::Queue { name } if name == "orders"
        ));
        let handler = find_service_handler(service, &HandlerKind::Message);
        let (params, given, _ret, effectful) =
            lower_service_handler_signature_ir(handler, &program);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].0, "m");
        assert!(given.is_empty());
        assert!(effectful, "every service handler returns Effect[T]");
    }

    #[test]
    fn store_field_with_an_unresolvable_type_is_rejected_before_lowering() {
        // #1187's Agent state-field slice, step 0, found that no checker pass
        // validated a store field's own type reference, so `store x:
        // Cell[Bogus]` certified and reached `resolve_store_field_ty`, whose
        // `Ty::Unit` fallback this test used to pin. #1679 closed that gap: the
        // resolver now walks agent store field types, so the program is
        // rejected with `bynk.resolve.unknown_type` before lowering, and the
        // fallback is defensive only.
        let source = r#"
context demo

agent Widget {
  key id: String
  store x: Cell[Bogus] = "hello"

  on call touch() -> Effect[()] {
    Effect.pure(())
  }
}
"#;
        let tokens = lexer::tokenize(source).expect("lex");
        let unit = parser::parse_unit(&tokens, source).expect("parse");
        let SourceUnit::Context(ctx) = unit else {
            panic!("expected a context unit, got {unit:?}")
        };
        let commons = Commons {
            name: ctx.name,
            items: ctx.items,
            uses: ctx.uses,
            documentation: ctx.documentation,
            form: ctx.form,
            span: ctx.span,
            trivia: ctx.trivia,
            trailing_comments: ctx.trailing_comments,
        };
        let Err(errors) = resolver::resolve(commons) else {
            panic!("an unknown store type must be rejected by the resolver")
        };
        assert!(
            errors
                .iter()
                .any(|e| e.category == "bynk.resolve.unknown_type" && e.message.contains("Bogus")),
            "{errors:?}"
        );
    }

    #[test]
    fn body_writes_state_is_false_for_a_plain_body() {
        let program = store_write_fixture();
        let handler = find_handler(find_agent(&program, "Widget"), "readOnlyPlain");
        assert!(!body_writes_state(&handler.body, program.program()));
    }

    #[test]
    fn body_writes_state_is_false_for_a_non_mutating_store_read() {
        let program = store_write_fixture();
        let handler = find_handler(find_agent(&program, "Widget"), "readOnlyQuery");
        assert!(!body_writes_state(&handler.body, program.program()));
    }

    #[test]
    fn body_writes_state_is_false_for_a_locally_shadowed_store_field_name() {
        let program = store_write_fixture();
        let handler = find_handler(find_agent(&program, "Widget"), "shadowedName");
        assert!(!body_writes_state(&handler.body, program.program()));
    }

    #[test]
    fn body_writes_state_is_true_for_a_write_nested_in_if_match_lambda() {
        let program = store_write_fixture();
        let handler = find_handler(find_agent(&program, "Widget"), "nestedMutation");
        assert!(body_writes_state(&handler.body, program.program()));
    }

    #[test]
    fn body_writes_state_is_true_for_a_bare_cell_assign() {
        let program = store_write_fixture();
        let handler = find_handler(find_agent(&program, "Widget"), "bareAssign");
        assert!(body_writes_state(&handler.body, program.program()));
    }

    #[test]
    fn body_writes_state_is_true_for_a_cell_update_method_call() {
        let program = store_write_fixture();
        let handler = find_handler(find_agent(&program, "Widget"), "cellUpdate");
        assert!(body_writes_state(&handler.body, program.program()));
    }

    #[test]
    fn body_writes_state_is_true_for_a_set_add_method_call() {
        let program = store_write_fixture();
        let handler = find_handler(find_agent(&program, "Widget"), "setAdd");
        assert!(body_writes_state(&handler.body, program.program()));
    }

    #[test]
    fn body_writes_state_is_true_for_a_log_append_method_call() {
        let program = store_write_fixture();
        let handler = find_handler(find_agent(&program, "Widget"), "logAppend");
        assert!(body_writes_state(&handler.body, program.program()));
    }

    fn checked_program(source: &str) -> CheckedProgram {
        let tokens = lexer::tokenize(source).expect("lex");
        let (commons, warnings) = parser::parse_with_warnings(&tokens, source).expect("parse");
        let resolved = resolver::resolve(commons).expect("resolve");
        let typed = checker::check(resolved).expect("check");
        checker::certify(typed, warnings).expect("certify")
    }

    fn find_type<'a>(program: &'a CheckedProgram, name: &str) -> &'a Arc<TypeDecl> {
        program
            .program()
            .types
            .get(name)
            .unwrap_or_else(|| panic!("no type named `{name}` in this fixture"))
    }

    fn find_capability<'a>(program: &'a CheckedProgram, name: &str) -> &'a CapabilityDecl {
        program
            .program()
            .commons
            .items
            .iter()
            .find_map(|item| match item {
                CommonsItem::Capability(c) if c.name.name == name => Some(c),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no capability named `{name}` in this fixture"))
    }

    fn find_provider<'a>(program: &'a CheckedProgram, name: &str) -> &'a ProviderDecl {
        program
            .program()
            .commons
            .items
            .iter()
            .find_map(|item| match item {
                CommonsItem::Provider(p) if p.provider_name.name == name => Some(p),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no provider named `{name}` in this fixture"))
    }

    // The six `type_shape_*` tests below, the capability/provider/handler-kind
    // tests after them, and the `body_writes_state_*` tests above were all
    // re-created by Slice D1 of `#1542`: each pinned a kept helper only
    // through a deleted item constructor (`IrItem::Type`/`Capability`/
    // `Provider`/`Service`) and now calls the helper directly, asserting the
    // same facts minus the constructor's own wrapper field.

    #[test]
    fn type_shape_record_resolves_fields_and_generic_rigid_vars() {
        let program = checked_program(
            r#"
commons demo {
  type Box[T] = { value: T }
}
"#,
        );
        let shape = lower_type_shape_ir(find_type(&program, "Box"), &program);
        let TypeShape::Record { fields } = &shape else {
            panic!("expected TypeShape::Record, got {shape:?}")
        };
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].0, "value");
        assert!(matches!(
            &*program.program().ty_intern.get(fields[0].1),
            Ty::Var(name) if name == "T"
        ));
    }

    #[test]
    fn type_shape_sum_resolves_variant_payloads_and_embeds() {
        let program = checked_program(
            r#"
commons demo {
  type PaymentError = enum { Declined, InsufficientFunds }

  type OrderError =
    | OutOfStock(sku: String, qty: Int)
    | Payment(reason: PaymentError)
    embeds PaymentError as Payment
}
"#,
        );
        let shape = lower_type_shape_ir(find_type(&program, "OrderError"), &program);
        let TypeShape::Sum { variants, embeds } = &shape else {
            panic!("expected TypeShape::Sum, got {shape:?}")
        };
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].0, "OutOfStock");
        assert_eq!(variants[0].1.len(), 2);
        assert_eq!(variants[0].1[0].0, "sku");
        assert!(matches!(
            &*program.program().ty_intern.get(variants[0].1[0].1),
            Ty::Base(bynk_syntax::ast::BaseType::String)
        ));
        assert_eq!(variants[0].1[1].0, "qty");
        assert!(matches!(
            &*program.program().ty_intern.get(variants[0].1[1].1),
            Ty::Base(bynk_syntax::ast::BaseType::Int)
        ));
        assert_eq!(variants[1].0, "Payment");
        assert_eq!(variants[1].1.len(), 1);
        assert_eq!(variants[1].1[0].0, "reason");

        assert_eq!(embeds.len(), 1);
        let (source, tag) = &embeds[0];
        assert_eq!(tag, "Payment");
        let Ty::Named { name, .. } = &*program.program().ty_intern.get(*source) else {
            panic!("expected embeds source to resolve to a named type")
        };
        assert_eq!(name, "PaymentError");
    }

    #[test]
    fn type_shape_refined_and_opaque_cover_bare_and_predicated_and_opaque_forms() {
        let program = checked_program(
            r#"
commons demo {
  type Age = Int where Positive
  type UserId = opaque Int
  type Bare = Int
}
"#,
        );
        let TypeShape::Refined {
            base,
            refinement,
            opaque,
        } = lower_type_shape_ir(find_type(&program, "Age"), &program)
        else {
            panic!("expected TypeShape::Refined for Age")
        };
        assert_eq!(base, bynk_syntax::ast::BaseType::Int);
        assert!(refinement.is_some());
        assert!(!opaque);

        let TypeShape::Refined {
            refinement, opaque, ..
        } = lower_type_shape_ir(find_type(&program, "UserId"), &program)
        else {
            panic!("expected TypeShape::Refined for UserId")
        };
        assert!(refinement.is_none());
        assert!(opaque);

        let TypeShape::Refined {
            refinement, opaque, ..
        } = lower_type_shape_ir(find_type(&program, "Bare"), &program)
        else {
            panic!("expected TypeShape::Refined for Bare")
        };
        assert!(refinement.is_none());
        assert!(!opaque);
    }

    #[test]
    fn type_shape_sum_covers_a_payload_less_variant() {
        let program = checked_program(
            r#"
commons demo {
  type PaymentError = enum { Declined, InsufficientFunds }
}
"#,
        );
        let shape = lower_type_shape_ir(find_type(&program, "PaymentError"), &program);
        let TypeShape::Sum { variants, embeds } = &shape else {
            panic!("expected TypeShape::Sum, got {shape:?}")
        };
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].0, "Declined");
        assert!(
            variants[0].1.is_empty(),
            "a bare variant carries no payload"
        );
        assert_eq!(variants[1].0, "InsufficientFunds");
        assert!(variants[1].1.is_empty());
        assert!(embeds.is_empty());
    }

    #[test]
    fn type_shape_record_drops_a_fields_own_inline_refinement() {
        // Decision B extension, `bynk-ir`'s own `TypeShape::Record` doc
        // comment: a field's inline `where` clause is a construction-time
        // constraint the checker already enforces, not part of the emitted
        // shape — pin that the field still lowers to `(name, ty)` with the
        // refinement silently absent, not that lowering rejects it.
        let program = checked_program(
            r#"
commons demo {
  type Account = { balance: Int where NonNegative }
}
"#,
        );
        let shape = lower_type_shape_ir(find_type(&program, "Account"), &program);
        let TypeShape::Record { fields } = &shape else {
            panic!("expected TypeShape::Record, got {shape:?}")
        };
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].0, "balance");
        assert!(matches!(
            &*program.program().ty_intern.get(fields[0].1),
            Ty::Base(bynk_syntax::ast::BaseType::Int)
        ));
    }

    #[test]
    fn type_shape_record_resolves_a_generic_type_application_field() {
        // The `TypeRef::App` arm of `resolve_type_ref_in` — the one arm
        // that returns `None` for an unknown/unapplied name, and so the one
        // most likely to silently hit this pass's own ADR 0334 panic if the
        // field's resolution were ever wired up wrong.
        let program = checked_program(
            r#"
commons demo {
  type Box[T] = { value: T }
  type Wrapper = { boxed: Box[Int] }
}
"#,
        );
        let shape = lower_type_shape_ir(find_type(&program, "Wrapper"), &program);
        let TypeShape::Record { fields } = &shape else {
            panic!("expected TypeShape::Record, got {shape:?}")
        };
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].0, "boxed");
        let Ty::Named { name, args, .. } = &*program.program().ty_intern.get(fields[0].1) else {
            panic!("expected `boxed` to resolve to a named type")
        };
        assert_eq!(name, "Box");
        assert_eq!(args.len(), 1);
        assert!(matches!(
            &*program.program().ty_intern.get(args[0]),
            Ty::Base(bynk_syntax::ast::BaseType::Int)
        ));
    }

    #[test]
    fn lower_capability_ops_ir_assembles_ops_in_declaration_order() {
        let program = checked_context_program(
            r#"
context demo

capability Store {
  fn get(key: String) -> Effect[Int]
  fn put(key: String, value: Int) -> Effect[()]
}
"#,
        );
        let ops = lower_capability_ops_ir(find_capability(&program, "Store"), &program);
        assert_eq!(ops.len(), 2, "declaration order preserved");

        assert_eq!(ops[0].name, "get");
        assert!(ops[0].type_params.is_empty());
        assert_eq!(ops[0].params.len(), 1);
        assert_eq!(ops[0].params[0].0, "key");
        assert!(matches!(
            &*program.program().ty_intern.get(ops[0].params[0].1),
            Ty::Base(bynk_syntax::ast::BaseType::String)
        ));
        // `return_ty` is Effect-wrapped, not peeled — `get`'s declared
        // `Effect[Int]` resolves whole, the same convention `FnSig::ret`
        // uses.
        assert!(matches!(
            &*program.program().ty_intern.get(ops[0].return_ty),
            Ty::Effect(inner) if matches!(
                &*program.program().ty_intern.get(*inner),
                Ty::Base(bynk_syntax::ast::BaseType::Int)
            )
        ));

        assert_eq!(ops[1].name, "put");
        assert_eq!(ops[1].params.len(), 2);
        assert_eq!(ops[1].params[0].0, "key");
        assert_eq!(ops[1].params[1].0, "value");
        assert!(matches!(
            &*program.program().ty_intern.get(ops[1].params[1].1),
            Ty::Base(bynk_syntax::ast::BaseType::Int)
        ));
    }

    #[test]
    fn lower_provider_given_ir_preserves_declaration_order_including_unused_entries() {
        // `given Random, Clock` (reverse-alphabetical, deliberately) pins
        // that `given`'s own declaration order survives — neither op body
        // below calls either capability, pinning review of #1186's point
        // that an *unused* `given` entry must still appear (it feeds R8.1's
        // own `deps` constructor, not anything the op bodies reference).
        let program = checked_context_program(
            r#"
context demo

capability Clock {
  fn now() -> Effect[Int]
}

capability Random {
  fn next() -> Effect[Int]
}

capability Store {
  fn get(key: String) -> Effect[Int]
  fn put(key: String, value: Int) -> Effect[()]
}

provides Store = MemStore given Random, Clock {
  fn get(key: String) -> Effect[Int] {
    Effect.pure(0)
  }
  fn put(key: String, value: Int) -> Effect[()] {
    Effect.pure(())
  }
}
"#,
        );
        let given = lower_provider_given_ir(find_provider(&program, "MemStore"));
        assert_eq!(
            given.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
            vec!["Random", "Clock"],
            "given's own declaration order preserved, unused entries included"
        );
        assert!(given.iter().all(|g| g.context.is_none()));
    }

    #[test]
    fn lower_provider_given_ir_reads_an_external_providers_given_too() {
        // v0.17: an external (bodiless) provider is only legal inside an
        // `adapter` unit (`bynk-check/src/symbols.rs`), which this test
        // harness's own `checked_context_program` cannot build (it only
        // ever parses a `context`). `lower_provider_given_ir` never reads
        // `program` at all, so this hand-constructs the `ProviderDecl` the
        // parser would produce for `provides Store = ExternalStore given
        // Clock` inside an adapter. Nothing in the grammar or checker gates
        // `given` on `external`, so an external provider's `given` must come
        // through the same way a Bynk one's does (review of #1187's own
        // Provider given/deps-wiring slice).
        let provider = ProviderDecl {
            capability: bynk_syntax::ast::Ident {
                name: "Store".to_string(),
                span: Span::default(),
            },
            provider_name: bynk_syntax::ast::Ident {
                name: "ExternalStore".to_string(),
                span: Span::default(),
            },
            given: vec![CapRef {
                context: None,
                name: bynk_syntax::ast::Ident {
                    name: "Clock".to_string(),
                    span: Span::default(),
                },
                span: Span::default(),
            }],
            ops: Vec::new(),
            external: true,
            documentation: None,
            span: Span::default(),
            trivia: Default::default(),
        };
        let given = lower_provider_given_ir(&provider);
        assert_eq!(given.len(), 1);
        assert_eq!(given[0].context, None);
        assert_eq!(given[0].name, "Clock");
    }

    #[test]
    fn an_http_service_lowers_its_protocol_and_per_handler_route_kind() {
        let program = checked_context_program(
            r#"
context demo

fn ok(s: String) -> HttpResult[String] { Ok(s) }

service Api from http {
  on GET("/ping") () -> Effect[HttpResult[String]] by v: Visitor {
    Effect.pure(ok("pong"))
  }
}
"#,
        );
        let service = find_service(&program, "Api");
        assert!(matches!(
            lower_protocol_ir(&service.protocol, &program),
            ProtocolIr::Http
        ));
        assert_eq!(
            lower_handler_kind_ir(&service.handlers[0].kind),
            IrHandlerKind::Http {
                method: IrHttpMethod::Get,
                path: "/ping".to_string(),
            },
            "the route binding lives per-handler — this is why ProtocolIr::Http itself \
             carries no payload"
        );
    }

    #[test]
    fn a_cron_service_lowers_its_schedule_from_the_handler_not_the_protocol() {
        let program = checked_context_program(
            r#"
context demo

fn done() -> Result[(), String] { Ok(()) }

service Sweeper from cron {
  on schedule("*/5 * * * *") () -> Effect[Result[(), String]] {
    Effect.pure(done())
  }
}
"#,
        );
        let service = find_service(&program, "Sweeper");
        assert!(matches!(
            lower_protocol_ir(&service.protocol, &program),
            ProtocolIr::Cron
        ));
        assert_eq!(
            lower_handler_kind_ir(&service.handlers[0].kind),
            IrHandlerKind::Cron {
                expr: "*/5 * * * *".to_string()
            }
        );
    }
}
