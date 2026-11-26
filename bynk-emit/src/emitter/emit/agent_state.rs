//! Agent state: the `loadState` and `commitState` functions, hydration, and the persistence guards
//! and entries (`emit_agent_load_commit_state`, `emit_agent_state_persistence`).

use super::{
    AgentStoreLayout, MigrateNames, agent_gates_rehydration, declaration_location,
    emit_lazy_field_spec, is_codecable, migrate_dispatcher_decl, migrate_step_dispatch,
    migration_step_decl, step_ranges_expr, store_field_key_type, type_base_is_string,
    unique_store_indexes,
};
use crate::emit_model::EmitProjectCtx;
use crate::emitter::Pre;
use crate::emitter::ctx::{BodyMode, ModuleCtx};
use crate::emitter::lower_ctx::{LowerCtx, collection_names, size_binding, size_reads};
use crate::emitter::serialisation;
use crate::ts_build::{as_expr, binary, call, const_, ident, member, str_lit};
use bynk_check::checker::TypedCommons;
use bynk_syntax::ast::{AgentDecl, RecordField, TypeRef};
use std::collections::HashSet;

/// The names and facts about an agent's state that `loadState` refers to: the
/// fresh-state factory, the rehydration/migration functions, and the schema
/// versions the stored layout is checked against (#3167).
pub(super) struct AgentLoadFns<'a> {
    pub(super) zero_fn: &'a String,
    pub(super) fields_const: &'a String,
    pub(super) rehydrate_fn: &'a String,
    pub(super) encode_fn: &'a String,
    pub(super) migrate_fn: &'a String,
    /// The function that fills a loaded state's collection handles from the stored
    /// record, when the agent has any (#3254).
    pub(super) hydrate_fn: &'a String,
    pub(super) has_rehydrate: bool,
    /// Whether the agent has a collection field (#3254).
    pub(super) has_collections: bool,
    /// Whether `commitState` writes through the encoder: the agent has a value that
    /// needs the codec, or a collection handle that must be put in wire form.
    pub(super) has_encode: bool,
    pub(super) step_plans: &'a [bynk_check::schema_registry::MigrationStep],
    pub(super) agent_versions: &'a Option<bynk_check::schema_registry::AgentVersions>,
    /// The decode/encode spec a lazily read agent's storage-backed handles attach with (#3189).
    pub(super) lazy_fields: Option<&'a String>,
}

/// An agent class's two private state methods, `loadState` and `commitState`
/// (#3167): load reads, rehydrates and migrates the stored record; commit
/// checks the agent's invariants and transitions and writes it back. Kept
/// together as one unit, and written onto `out` in that order, because the two
/// are the agent's whole storage contract with the platform.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_agent_load_commit_state(
    methods: &mut Vec<bynk_ts::TsClassMethod>,
    a: &AgentDecl,
    effective_fields: &[RecordField],
    fns: &AgentLoadFns,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    source_text: &str,
) {
    let state_ty = format!("{}State", a.name.name);
    let AgentLoadFns {
        zero_fn,
        fields_const,
        rehydrate_fn,
        encode_fn,
        migrate_fn,
        hydrate_fn,
        has_rehydrate,
        has_collections,
        has_encode,
        step_plans,
        agent_versions,
        lazy_fields,
    } = *fns;
    // Arc C, slice 21 (#1371): the third of step (9)'s own proposed 5-6
    // sub-slices — the class's own wrapper (header/fields/constructor) stays
    // hand-written text (Decision C, the same boundary #1359's own
    // `emit_provider` already established), but `loadState` converts fully
    // to a real `bynk_ts::TsClassMethod` fragment, printed through
    // `print_class_method` (the same fragment entry point #1359 added).
    // `commitState`, this class's OTHER private method, stays deferred as
    // its own separate, later sub-slice — a real, harder remainder found
    // only once actually read (its own invariant/transition predicate
    // lowering writes directly into `out` today, the same "direct write,
    // real position" shape `emit_free_fn`/`emit_contract_guarded_body` had
    // *before* their own conversions needed a sub-builder for `Raw`-
    // embedding, #1352/#1353), not silently folded into "step (9), slice 3
    // done." `this.state.storage.get<{state_ty}>(...)`'s own generic method
    // call has no representation in `TsExpr::Call` (`type_args` was never
    // added — 41 real construction sites across the workspace, far more
    // than a single narrow need like this one justifies touching) — carried
    // as one opaque callee leaf, the same "an odd, one-off shape stays
    // opaque text" precedent P7.9's own `Query[T]` and #1357's own `unique
    // symbol` already established, not a new pattern; now tagged
    // `VerbatimExpr`, not `Ident` (#1539).
    // `TsClassMethod.private` is a real, new, small gap the grounding pass
    // itself predicted (#1366) — `loadState`/`commitState` are the first
    // `private` method sites this whole track has hit (`emit_provider`'s
    // own ops, #1359, were all public).
    let load_state_body = {
        let mut stmts = vec![
            bynk_ts::TsStmt::const_stmt(
                bynk_ts::TsBindingName::Ident("stored".to_string()),
                None,
                bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::VerbatimExpr(
                        format!("this.state.storage.get<{state_ty}>"),
                        bynk_ts::VerbatimOrigin::Emit,
                    )),
                    args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                        "state".to_string(),
                    ))],
                })),
                None,
            ),
            // v0.96 (ADR 0124): a fresh key takes its zero (valid by
            // construction). For a stored record, merge zero-then-stored —
            // D4: a `store` field added in a later deploy and absent from
            // the persisted record takes its default, rather than reading
            // `undefined` — then run the rehydration validation gate on the
            // merged state before any handler reads it (D1/D2).
            bynk_ts::TsStmt::if_stmt(
                bynk_ts::TsExpr::Binary {
                    op: bynk_ts::TsBinaryOp::StrictEq,
                    left: Box::new(bynk_ts::TsExpr::Ident("stored".to_string())),
                    right: Box::new(bynk_ts::TsExpr::Ident("undefined".to_string())),
                },
                bynk_ts::TsStmt::return_stmt(
                    Some(bynk_ts::TsExpr::Call {
                        callee: Box::new(bynk_ts::TsExpr::Ident(zero_fn.clone())),
                        args: Vec::new(),
                    }),
                    None,
                ),
                None,
            ),
        ];
        // Per-entry storage slice 0 (#3189): any `__layout` marker was written
        // by a later build, so fault before anything is decoded, merged or
        // written back. Unconditional: a lockless agent has no `__v` check.
        stmts.push(bynk_ts::TsStmt::expr_stmt(
            bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("__checkStoredLayout".to_string())),
                args: vec![
                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(a.name.name.clone())),
                    bynk_ts::TsExpr::Ident("stored".to_string()),
                ],
            },
            None,
        ));
        // State-migrations S1 (#2211, track §3.2, Decision D): with the
        // schema lock, check the raw record's `__v` marker before anything is
        // merged or decoded, so a record from a newer build (or with no path
        // forward) faults without being read or written back.
        if let Some(v) = &agent_versions {
            stmts.push(check_stored_version_stmt(&a.name.name, v, step_plans));
        }
        // #4482: a `Map` or `Set` renamed with `replaces` keeps its entries under keys that carry its
        // name, so they are moved to the new name before anything reads them.
        if let Some(moves) = move_renamed_entries_stmt(a, agent_versions.as_ref(), step_plans) {
            stmts.push(moves);
        }
        // Per-entry storage (#3189): join the entries a per-entry record keeps
        // outside `"state"` back into the wholesale-shaped record (a wholesale one
        // is returned as it is, and split by its first commit). Everything below
        // reads the joined record, so a migration step and the rehydration gate
        // see what they always saw.
        stmts.push(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("__wholesale".to_string()),
            None,
            bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("__readStored".to_string())),
                args: [
                    bynk_ts::TsExpr::Ident("this".to_string()),
                    bynk_ts::TsExpr::Ident("this.state.storage".to_string()),
                    bynk_ts::TsExpr::Ident("stored".to_string()),
                ]
                .into_iter()
                // #3189: an agent that reads lazily says which stored versions a migration
                // step covers, which `readStored` still reads whole.
                .chain(lazy_fields.map(|_| step_ranges_expr(step_plans)))
                .collect(),
            })),
            None,
        ));
        // State-migrations S2 (#2224 Decision D): a record a step covers runs
        // it first; everything below reads the step's output.
        let src = if step_plans.is_empty() {
            "__wholesale"
        } else {
            let as_ty = |expr: bynk_ts::TsExpr, ty: bynk_ts::TsType| bynk_ts::TsExpr::As {
                expr: Box::new(bynk_ts::TsExpr::As {
                    expr: Box::new(expr),
                    ty: bynk_ts::TsType::named("unknown"),
                }),
                ty,
            };
            stmts.push(bynk_ts::TsStmt::const_stmt(
                bynk_ts::TsBindingName::Ident("__src".to_string()),
                None,
                as_ty(
                    bynk_ts::TsExpr::Call {
                        callee: Box::new(bynk_ts::TsExpr::Ident(migrate_fn.clone())),
                        args: vec![as_ty(
                            bynk_ts::TsExpr::Ident("__wholesale".to_string()),
                            bynk_ts::TsType::named("globalThis.Record<string, unknown>"),
                        )],
                    },
                    bynk_ts::TsType::named(state_ty.clone()),
                ),
                None,
            ));
            "__src"
        };
        // #3793: a `store` field renamed with `replaces` is read from whichever of its names the
        // record holds, after a `migrate` step and before the merge.
        let renames = agent_versions
            .as_ref()
            .map_or(&[][..], |v| v.renames.as_slice());
        let src = if renames.is_empty() {
            src
        } else {
            stmts.push(rename_stored_stmt(src, renames));
            "__renamed"
        };
        stmts.push(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("__zero".to_string()),
            None,
            bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident(zero_fn.clone())),
                args: Vec::new(),
            },
            None,
        ));
        // Track §3.4 (Decision E): name, once per isolate, any stored key the
        // definition no longer declares. It stays in `__merged` and so is
        // written back on commit, never deleted.
        stmts.push(bynk_ts::TsStmt::expr_stmt(
            bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident(
                    "__reportUnknownStoredKeys".to_string(),
                )),
                args: vec![
                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(a.name.name.clone())),
                    bynk_ts::TsExpr::Ident(src.to_string()),
                    bynk_ts::TsExpr::Ident("__zero".to_string()),
                ],
            },
            None,
        ));
        stmts.push(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("__merged".to_string()),
            None,
            bynk_ts::TsExpr::object_entries(vec![
                bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident("__zero".to_string())),
                bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident(src.to_string())),
            ]),
            None,
        ));
        if has_rehydrate {
            stmts.push(bynk_ts::TsStmt::expr_stmt(
                bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Ident(rehydrate_fn.clone())),
                    args: vec![
                        bynk_ts::TsExpr::Ident("__merged".to_string()),
                        bynk_ts::TsExpr::Ident(src.to_string()),
                    ],
                },
                None,
            ));
        }
        // Per-collection handles (#3254): fill each collection the record carries into
        // its handle, in the order the record notes.
        if has_collections {
            stmts.push(hydrate_or_attach_stmt(hydrate_fn, src, lazy_fields));
        }
        stmts.push(bynk_ts::TsStmt::return_stmt(
            Some(bynk_ts::TsExpr::Ident("__merged".to_string())),
            None,
        ));
        stmts
    };
    let load_state_method = bynk_ts::TsClassMethod::new(
        "loadState",
        true,
        Vec::new(),
        Some(bynk_ts::TsType::named_with_args(
            "globalThis.Promise",
            vec![bynk_ts::TsType::named(state_ty.clone())],
        )),
        load_state_body,
    )
    .private();
    methods.push(load_state_method);
    // Arc C, slice 22 (#1373): closes step (9)'s own third sub-slice by
    // landing the deferred remainder, `commitState` (#1371's own `loadState`
    // landed the sub-slice's other half). `record_span` is a documented
    // no-op when a `LowerCtx` has no attached builder (`emitter.rs`'s own
    // doc), and neither the invariant nor the transition `LowerCtx` below
    // is ever given one (`LowerCtx::new(...)`, never `.with_source_map(...)`
    // — `emit_agent`'s own local `source_map` binding isn't threaded into
    // either) — confirmed directly, not assumed, before writing this
    // comment. So despite the grounding pass's own earlier caution that
    // `commitState` would need the same sub-builder/merge care #1352/#1353
    // established, it genuinely does not: there is no source map being
    // written to here, today or after this conversion, so there is nothing
    // to preserve. Each invariant's/transition's own hoisted pre-statements
    // and predicate expression stay opaque (`lower.rs`'s own permanently-
    // excluded general expression lowering, ADR
    // `arc-c-lower-rs-permanent-exclusion`) — the hoisted lines as their own
    // `TsStmt::Raw` (indent baked in manually, since `Raw` prints verbatim
    // with none of its own — depth 2 for an invariant's own top-level
    // check, depth 3 for a transition's own check nested inside the
    // `if (__prior !== undefined)` block), the predicate wrapped in a real
    // `Unary`/`Paren` condition — `!({pred})`, `pred` itself still one
    // opaque `Ident` leaf (#1539: the same `Unary{Not, Paren(Ident(pred))}`
    // shape `contract_guard_if_stmt`, above, already builds for its own
    // opaque predicate; this site used to spell the whole thing as one
    // `TsExpr::Ident(format!("!({pred})"))` string instead of the equivalent
    // real node). Everything else — the `if`/`console.error`/`throw`
    // wrapper, the transition prologue — is real: nothing here needs the
    // general expression lowerer at all.
    let commit_state_body = {
        // Review of #1374: the hoisted-statement `Raw` indent is derived
        // from `class_depth` rather than hardcoded — a hoisting predicate
        // is untested by the fixture corpus today (every real `@invariant`/
        // `transition` fixture's own predicate hoists nothing), so an
        // un-derived literal here would have been silently wrong the day a
        // future caller printed this method at a non-zero depth, with
        // nothing but a fixture no one has written yet to catch it.
        let mut stmts: Vec<bynk_ts::TsStmt> = Vec::new();
        // #4034: the counts a predicate reads with `size()`, bound before the checks.
        let reads = size_reads(a);
        for field in &reads.staged {
            stmts.push(size_const(
                &size_binding(false, field),
                call(
                    member(member(ident("s"), field.as_str()), "size"),
                    Vec::new(),
                ),
            ));
        }
        // v0.80 (§14): evaluate each invariant against the proposed state `s`
        // before the write. A violation throws `InvariantViolation` *before*
        // `storage.put`, so the offending commit never persists
        // (non-persistence of the offending commit — not whole-handler
        // rollback). The refusal is logged with the agent type and
        // invariant name (never the key — see ADR 0107) so it is
        // distinguishable from a crash in the logs.
        if !a.invariants.is_empty() {
            let field_names: HashSet<String> = effective_fields
                .iter()
                .map(|f| f.name.name.clone())
                .collect();
            for inv in &a.invariants {
                let mut cx = LowerCtx::new(
                    ModuleCtx::for_unit(commons, ctx),
                    BodyMode::Invariant {
                        name: "s".to_string(),
                        fields: field_names.clone(),
                        collections: collection_names(a),
                    },
                );
                let mut pre = Pre::new();
                let pred = pre.lower_node(&inv.predicate, &mut cx);
                stmts.extend(violation_check(
                    &a.name.name,
                    &inv.name.name,
                    "invariant",
                    &declaration_location(ctx, source_text, inv.name.span),
                    pred,
                    pre.ts_stmts(),
                ));
            }
        }
        // v0.116 (testing track slice 4): step invariants — evaluate each
        // `transition` against the pre-/post-commit state pair. The old
        // state is still in storage (this method performs the `put`), so
        // reading it here yields the pre-commit snapshot; `undefined` is the
        // genesis commit, which has no prior state to transition from and is
        // skipped (snapshot invariants above still apply). `old`/`new` are
        // lowered to `__old`/`__new` (`new` is a JS reserved word). A
        // violation throws the same `InvariantViolation`-family fault,
        // before the write.
        if !a.transitions.is_empty() {
            let mut transition_body = vec![
                bynk_ts::TsStmt::const_stmt(
                    bynk_ts::TsBindingName::Ident("__old".to_string()),
                    None,
                    bynk_ts::TsExpr::object_entries(vec![
                        bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Ident(zero_fn.clone())),
                            args: Vec::new(),
                        }),
                        bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident(
                            "__prior".to_string(),
                        )),
                    ]),
                    None,
                ),
                bynk_ts::TsStmt::const_stmt(
                    bynk_ts::TsBindingName::Ident("__new".to_string()),
                    None,
                    bynk_ts::TsExpr::Ident("s".to_string()),
                    None,
                ),
            ];
            // #4034: the committed counts `old.<field>.size()` reads.
            for field in &reads.committed {
                transition_body.push(size_const(
                    &size_binding(true, field),
                    call(
                        ident("__committedSize"),
                        vec![
                            member(member(ident("this"), "state"), "storage"),
                            str_lit(field.as_str()),
                            ident("__prior"),
                        ],
                    ),
                ));
            }
            // #1649: the prior state is stored in the wire shape, so decode it
            // the way `loadState` does before a predicate reads `old`. Before
            // #1649 this read was raw and unvalidated.
            if has_rehydrate {
                transition_body.insert(
                    1,
                    bynk_ts::TsStmt::expr_stmt(
                        bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Ident(rehydrate_fn.clone())),
                            args: vec![
                                bynk_ts::TsExpr::Ident("__old".to_string()),
                                bynk_ts::TsExpr::Ident("__prior".to_string()),
                            ],
                        },
                        None,
                    ),
                );
            }
            for tr in &a.transitions {
                let mut cx = LowerCtx::new(
                    ModuleCtx::for_unit(commons, ctx),
                    BodyMode::Transition {
                        old: "__old".to_string(),
                        new: "__new".to_string(),
                        collections: collection_names(a),
                    },
                );
                let mut pre = Pre::new();
                let pred = pre.lower_node(&tr.predicate, &mut cx);
                transition_body.extend(violation_check(
                    &a.name.name,
                    &tr.name.name,
                    "transition",
                    &declaration_location(ctx, source_text, tr.name.span),
                    pred,
                    pre.ts_stmts(),
                ));
            }
            stmts.push(bynk_ts::TsStmt::const_stmt(
                bynk_ts::TsBindingName::Ident("__prior".to_string()),
                None,
                bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::VerbatimExpr(
                        format!("this.state.storage.get<{state_ty}>"),
                        bynk_ts::VerbatimOrigin::Emit,
                    )),
                    args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                        "state".to_string(),
                    ))],
                })),
                None,
            ));
            stmts.push(bynk_ts::TsStmt::if_stmt(
                bynk_ts::TsExpr::Binary {
                    op: bynk_ts::TsBinaryOp::StrictNotEq,
                    left: Box::new(bynk_ts::TsExpr::Ident("__prior".to_string())),
                    right: Box::new(bynk_ts::TsExpr::Ident("undefined".to_string())),
                },
                bynk_ts::TsStmt::block(transition_body, None),
                None,
            ));
        }
        stmts.extend(unique_check(a, commons, ctx, source_text));
        stmts.push(bynk_ts::TsStmt::expr_stmt(
            bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("__commitStored".to_string())),
                args: vec![
                    bynk_ts::TsExpr::Ident("this".to_string()),
                    bynk_ts::TsExpr::Ident("this.state.storage".to_string()),
                    // #1649: the wire shape, the same shape `loadState` decodes.
                    // State-migrations S1 (#2211, Decision D): with the schema
                    // lock, stamped with the class's version in the same `put`,
                    // spread on so the state type is untouched. The commit does
                    // not check the stored marker itself: it relies on every
                    // caller having run `loadState` first, which faults a newer
                    // record before any write. A future path that commits
                    // without loading would overwrite a newer record silently,
                    // so it must check `__v` too (#2213 review).
                    {
                        let payload = if has_encode {
                            bynk_ts::TsExpr::Call {
                                callee: Box::new(bynk_ts::TsExpr::Ident(encode_fn.clone())),
                                args: vec![bynk_ts::TsExpr::Ident("s".to_string())],
                            }
                        } else {
                            bynk_ts::TsExpr::Ident("s".to_string())
                        };
                        match &agent_versions {
                            Some(v) => bynk_ts::TsExpr::object_entries(vec![
                                bynk_ts::TsObjectEntry::Spread(payload),
                                bynk_ts::TsObjectEntry::prop(
                                    "__v".to_string(),
                                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(
                                        v.version.to_string(),
                                    )),
                                ),
                            ]),
                            None => payload,
                        }
                    },
                    bynk_ts::TsExpr::Ident(fields_const.clone()),
                    // Cache reaping (#3189): the state object the handler's `Cache` ops
                    // noted their clock against.
                    bynk_ts::TsExpr::Ident("s".to_string()),
                ],
            })),
            None,
        ));
        stmts
    };
    let (commit_state_body, commit_state_params) =
        commit_state_params(commit_state_body, &state_ty, ctx, fields_const);
    let commit_state_method = bynk_ts::TsClassMethod::new(
        "commitState",
        true,
        commit_state_params,
        Some(bynk_ts::TsType::named_with_args(
            "globalThis.Promise",
            vec![bynk_ts::TsType::named("void")],
        )),
        commit_state_body,
    )
    .private();
    methods.push(commit_state_method);
    // #3931: a handler that calls `self.erase()` commits by deleting the instance's storage.
    if a.handlers
        .iter()
        .any(|h| bynk_model::block_uses_erase(&h.body, &commons.callees))
    {
        methods.push(
            bynk_ts::TsClassMethod::new(
                "eraseState",
                true,
                Vec::new(),
                Some(bynk_ts::TsType::named_with_args(
                    "globalThis.Promise",
                    vec![bynk_ts::TsType::named("void")],
                )),
                vec![bynk_ts::TsStmt::expr_stmt(
                    bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                        callee: Box::new(bynk_ts::TsExpr::Ident("__eraseStored".to_string())),
                        args: vec![
                            bynk_ts::TsExpr::Ident("this".to_string()),
                            bynk_ts::TsExpr::Ident("this.state.storage".to_string()),
                        ],
                    })),
                    None,
                )],
            )
            .private(),
        );
    }
}

/// `commitState`'s parameters and body. #3581: a `bynk dev` build's `commitState` also records the transition. It takes the
/// handler's name as an optional trailing parameter, snapshots what is stored just
/// before the write (the last statement of `body`), and appends `{handler, old, new,
/// at}` to the instance's ring buffer just after it. A commit that throws before the
/// write (an invariant or `transition` refusal) never reaches the append. Returns
/// `body` unchanged, and just the state parameter, for every other build.
fn commit_state_params(
    mut body: Vec<bynk_ts::TsStmt>,
    state_ty: &str,
    ctx: &EmitProjectCtx,
    fields_const: &str,
) -> (Vec<bynk_ts::TsStmt>, Vec<bynk_ts::TsParam>) {
    let mut params = vec![bynk_ts::TsParam::new(
        "s".to_string(),
        Some(bynk_ts::TsType::named(state_ty.to_string())),
    )];
    if !ctx.inspect_agents {
        return (body, params);
    }
    let call = |name: &str, extra: &[&str]| {
        let mut args = vec![
            bynk_ts::TsExpr::Ident("this.state.storage".to_string()),
            bynk_ts::TsExpr::Ident(fields_const.to_string()),
        ];
        args.extend(extra.iter().map(|e| bynk_ts::TsExpr::Ident(e.to_string())));
        bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Ident(name.to_string())),
            args,
        }))
    };
    let before = bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__before".to_string()),
        None,
        call("__historyBefore", &[]),
        None,
    );
    let after =
        bynk_ts::TsStmt::expr_stmt(call("__historyRecord", &["__before", "__handler"]), None);
    body.insert(body.len().saturating_sub(1), before);
    body.push(after);
    params.push(bynk_ts::TsParam::optional(
        "__handler".to_string(),
        Some(bynk_ts::TsType::named("string")),
    ));
    (body, params)
}

/// `globalThis.Record<string, unknown>`: the wire shape a migration step reads and returns.
pub(super) fn wire_record_ty() -> bynk_ts::TsType {
    bynk_ts::TsType::named_with_args(
        "globalThis.Record",
        vec![
            bynk_ts::TsType::named("string"),
            bynk_ts::TsType::named("unknown"),
        ],
    )
}

/// What `loadState` does with the merged state's collections: fill the in-memory handles from the
/// record, or, for a lazily read record (#3189), attach storage-backed ones in their place.
fn hydrate_or_attach_stmt(
    hydrate_fn: &str,
    src: &str,
    lazy_fields: Option<&String>,
) -> bynk_ts::TsStmt {
    let hydrate = bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Ident(hydrate_fn.to_string())),
        args: vec![
            bynk_ts::TsExpr::Ident("__merged".to_string()),
            bynk_ts::TsExpr::Ident(src.to_string()),
        ],
    };
    let Some(spec) = lazy_fields else {
        return bynk_ts::TsStmt::expr_stmt(hydrate, None);
    };
    let attach = bynk_ts::TsExpr::Call {
        callee: Box::new(ident("__attachLazy")),
        args: vec![ident("this.state.storage"), ident("__merged"), ident(spec)],
    };
    let reads_lazily = bynk_ts::TsExpr::Call {
        callee: Box::new(ident("__readsLazily")),
        args: vec![ident("this")],
    };
    bynk_ts::TsStmt::if_else_same_line_stmt(
        reads_lazily,
        bynk_ts::TsStmt::block(vec![bynk_ts::TsStmt::expr_stmt(attach, None)], None),
        bynk_ts::TsStmt::block(vec![bynk_ts::TsStmt::expr_stmt(hydrate, None)], None),
        None,
    )
}

/// What the rest of an agent's emission refers to by name once its state's
/// validation and migration functions are declared (#3167).
pub(super) struct AgentPersistFns {
    /// The rehydration check `loadState` runs on a stored record.
    pub(super) rehydrate_fn: String,
    /// The wire-format encoder `commitState` writes through.
    pub(super) encode_fn: String,
    /// The migration dispatcher `loadState` calls for an older stored layout.
    pub(super) migrate_fn: String,
    /// Fills a loaded state's collection handles from the stored record (#3254).
    pub(super) hydrate_fn: String,
    /// Whether the agent's state needs the rehydration check at all.
    pub(super) has_rehydrate: bool,
    /// Whether the agent has a `Map`, `Set`, `Cache`, `Log` or held `Map` field.
    pub(super) has_collections: bool,
    /// Whether `commitState` writes through the encoder: a value needs the codec, or a
    /// collection handle must be put in wire form.
    pub(super) has_encode: bool,
    /// The migration steps from each older schema version, in order.
    pub(super) step_plans: Vec<bynk_check::schema_registry::MigrationStep>,
    /// The agent's declared schema versions, if it has any.
    pub(super) agent_versions: Option<bynk_check::schema_registry::AgentVersions>,
    /// The lazy-read field spec's name, when the agent reads its collections lazily (#3189).
    pub(super) lazy_fields: Option<String>,
}

/// `globalThis.<path>`, a dotted path such as `Object.keys`, as member accesses.
pub(super) fn persist_global(path: &str) -> bynk_ts::TsExpr {
    path.split('.')
        .fold(ident("globalThis"), |object, property| {
            member(object, property)
        })
}

/// `expr as unknown as ty`: the double cast that reinterprets a value at another type.
pub(super) fn persist_recast(expr: bynk_ts::TsExpr, ty: bynk_ts::TsType) -> bynk_ts::TsExpr {
    as_expr(as_expr(expr, bynk_ts::TsType::named("unknown")), ty)
}

/// `globalThis.Record<string, unknown>`.
pub(super) fn persist_record_ty() -> bynk_ts::TsType {
    bynk_ts::TsType::named_with_args(
        "globalThis.Record",
        vec![
            bynk_ts::TsType::named("string"),
            bynk_ts::TsType::named("unknown"),
        ],
    )
}

/// `{ v: unknown }`: the shape of a `Cache` or `Log` entry's value cell.
fn persist_entry_ty() -> bynk_ts::TsType {
    bynk_ts::TsType::Object(vec![bynk_ts::TsTypeMember::prop(
        "v",
        bynk_ts::TsType::named("unknown"),
    )])
}

/// `{ -readonly [K in keyof source]: source[K] }`: `source` with its `readonly`
/// modifiers removed, so a decode can write through it.
fn persist_writable(source: bynk_ts::TsType) -> bynk_ts::TsType {
    bynk_ts::TsType::mapped(
        "K",
        source.clone(),
        bynk_ts::TsType::indexed(source, bynk_ts::TsType::named("K")),
    )
    .with_mapped_readonly(bynk_ts::TsMappedModifier::Remove)
}

/// `typeof s`.
fn persist_typeof_s() -> bynk_ts::TsType {
    bynk_ts::TsType::type_of(ident("s"))
}

/// `(s as unknown as globalThis.Record<string, unknown>)`: the loaded record seen as
/// what it is at runtime, an untyped record. The collection fields are held as
/// handles (#3254), but in the loaded record they are still the stored records and
/// arrays.
fn persist_record_view() -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Paren(Box::new(persist_recast(ident("s"), persist_record_ty())))
}

/// `(<view>.<field> as ty)`: a collection field of the record view, cast.
fn persist_field_as(field: &str, ty: bynk_ts::TsType) -> bynk_ts::TsExpr {
    as_expr(member(persist_record_view(), field), ty)
}

/// `globalThis.Object.prototype.hasOwnProperty.call(stored, "name")`: whether the stored
/// record carries the field.
pub(super) fn persist_present(name: &str) -> bynk_ts::TsExpr {
    call(
        persist_global("Object.prototype.hasOwnProperty.call"),
        vec![ident("stored"), str_lit(name)],
    )
}

/// `(<value> as unknown as __JsonValue)`: the value laundered to the JSON the boundary
/// deserialiser takes as its input.
pub(super) fn persist_json_input(value: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Paren(Box::new(persist_recast(
        value,
        bynk_ts::TsType::named("__JsonValue"),
    )))
}

/// The decode's first two statements: `const __r = <decoded>;` and the throw on an `Err`
/// that disposes of a failure as an internal `RehydrationViolation` fault.
fn persist_decode(decoded: bynk_ts::TsExpr, agent_name: &str) -> Vec<bynk_ts::TsStmt> {
    let result = || ident("__r");
    vec![
        const_("__r", decoded),
        bynk_ts::TsStmt::if_stmt(
            binary(
                bynk_ts::TsBinaryOp::StrictEq,
                member(result(), "tag"),
                str_lit("Err"),
            ),
            bynk_ts::TsStmt::throw_stmt(
                call(
                    ident("__rehydrationViolation"),
                    vec![str_lit(agent_name), member(result(), "error")],
                ),
                None,
            ),
            None,
        ),
    ]
}

/// `if (<present>) { <then> }`, the guard every rehydration check sits behind.
fn persist_when_present(name: &str, then: Vec<bynk_ts::TsStmt>) -> bynk_ts::TsStmt {
    bynk_ts::TsStmt::if_stmt(
        persist_present(name),
        bynk_ts::TsStmt::block(then, None),
        None,
    )
}

/// `for (const <binding> of <iter>) { <body> }`.
fn persist_for_each(
    binding: &str,
    iter: bynk_ts::TsExpr,
    body: Vec<bynk_ts::TsStmt>,
) -> bynk_ts::TsStmt {
    crate::ts_build::for_of(binding, iter, crate::ts_build::block(body))
}

/// The function `loadState` calls to fill a record's collections into the zero state's handles
/// (#3254): one fill per collection field, from the stored record.
fn hydrate_fn_decl(
    a: &AgentDecl,
    layout: &AgentStoreLayout,
    hydrate_fn: &str,
    zero_fn: &str,
) -> bynk_ts::TsStmt {
    let state_ty = format!("{}State", a.name.name);
    let held_names_all: HashSet<&str> = layout
        .all_held_maps
        .iter()
        .map(|(n, _)| n.name.as_str())
        .collect();
    let record_at = |key: &str| {
        as_expr(
            bynk_ts::TsExpr::Index {
                object: Box::new(ident("__r")),
                index: Box::new(str_lit(key)),
            },
            bynk_ts::TsType::named("never"),
        )
    };
    let mut body = vec![
        const_("__z", call(ident(zero_fn), Vec::new())),
        const_(
            "__w",
            as_expr(
                ident("s"),
                persist_writable(bynk_ts::TsType::named(state_ty.clone())),
            ),
        ),
        const_("__r", persist_recast(ident("stored"), persist_record_ty())),
    ];
    let mut fill = |name: &str, with: &str| {
        let mut args = vec![member(ident("__z"), name), record_at(name)];
        // A held map's insertion order is stored beside its record, as `<field>:order`.
        if with == "__fillHeld" {
            args.push(record_at(&format!("{name}:order")));
        }
        body.push(bynk_ts::TsStmt::if_stmt(
            call(
                persist_global("Object.hasOwn"),
                vec![ident("__r"), str_lit(name)],
            ),
            bynk_ts::TsStmt::block(
                vec![
                    bynk_ts::TsStmt::expr_stmt(call(ident(with), args), None),
                    bynk_ts::TsStmt::assign(
                        member(ident("__w"), name),
                        member(ident("__z"), name),
                        None,
                    ),
                ],
                None,
            ),
            None,
        ));
    };
    for (name, _) in &layout.store_map_fields {
        let held = held_names_all.contains(name.name.as_str());
        fill(&name.name, if held { "__fillHeld" } else { "__fillMap" });
    }
    for (name, _) in &layout.held_maps {
        fill(&name.name, "__fillHeld");
    }
    for (name, _) in &layout.store_set_fields {
        fill(&name.name, "__fillSet");
    }
    for (name, _, _) in &layout.store_cache_fields {
        fill(&name.name, "__fillMap");
    }
    for (name, _, _) in &layout.store_log_fields {
        fill(&name.name, "__fillLog");
    }
    bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Function {
            name: hydrate_fn.to_string(),
            generics: Vec::new(),
            params: vec![
                bynk_ts::TsParam::new(
                    "s".to_string(),
                    Some(bynk_ts::TsType::named(state_ty.clone())),
                ),
                bynk_ts::TsParam::new("stored".to_string(), Some(bynk_ts::TsType::named(state_ty))),
            ],
            return_type: Some(bynk_ts::TsType::named("void")),
            body,
            is_async: false,
            inline: false,
        },
        None,
    )
}

/// The rehydration checks `loadState` runs on a stored record, and the entries the
/// wire-format encoder writes beside `...s` (#1649, ADR 0124).
fn agent_state_checks_and_entries(
    a: &AgentDecl,
    layout: &AgentStoreLayout,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> (Vec<bynk_ts::TsStmt>, Vec<bynk_ts::TsObjectEntry>) {
    let AgentStoreLayout {
        effective_fields,
        held_maps,
        store_map_fields,
        store_map_indexes,
        store_set_fields,
        store_cache_fields,
        store_log_fields,
        ..
    } = layout;
    let mut encode_entries: Vec<bynk_ts::TsObjectEntry> = Vec::new();
    let agent_name = &a.name.name;
    let mut rehydrate_checks: Vec<bynk_ts::TsStmt> = Vec::new();
    // The loaded record is statically typed (its fields are the agent's types),
    // but at runtime its bytes are untrusted, so each value is laundered to
    // `JsonValue` before the boundary deserialiser decodes it.
    let ser = |ty: &TypeRef, value: bynk_ts::TsExpr| {
        serialisation::serialise_expr(ty, &value, &ctx.runtime_use)
    };
    let deser = |ty: &TypeRef, json: bynk_ts::TsExpr, path: &str| {
        serialisation::deserialise_expr(ty, &json, path, &ctx.runtime_use)
    };
    // `Cell[T]` — decode the field value as `T`.
    for f in effective_fields {
        let path = f.name.name.as_str();
        // Only codec-able types have a deserialiser; a non-storable type never
        // reaches a `store` position (the checker rejects it), but guard anyway.
        if !is_codecable(&f.type_ref) {
            continue;
        }
        let d = deser(
            &f.type_ref,
            persist_json_input(member(ident("s"), path)),
            path,
        );
        // The state interface's fields are `readonly`; the decode writes
        // through a mutable view of the freshly merged record. The cast to
        // the field's own type matters on the workers target, where the
        // state interface re-declares a commons type locally and
        // TypeScript's brands make the two nominally distinct, though
        // they share one runtime shape.
        let mut block = persist_decode(d, agent_name);
        block.push(bynk_ts::TsStmt::assign(
            member(
                bynk_ts::TsExpr::Paren(Box::new(as_expr(
                    ident("s"),
                    persist_writable(persist_typeof_s()),
                ))),
                path,
            ),
            persist_recast(
                member(ident("__r"), "value"),
                bynk_ts::TsType::indexed(persist_typeof_s(), bynk_ts::TsType::string_literal(path)),
            ),
            None,
        ));
        rehydrate_checks.push(persist_when_present(path, block));
        encode_entries.push(bynk_ts::TsObjectEntry::prop(
            path,
            ser(&f.type_ref, member(ident("s"), path)),
        ));
    }
    let held_decls_all = &layout.all_held_maps;
    let held_names_all: HashSet<&str> = held_decls_all
        .iter()
        .map(|(n, _)| n.name.as_str())
        .collect();
    // A textual `K` key persists as a `String(k)` Record key, so each entry's key is
    // validated: `for (const [__k] of __wireEntries(<view>.n as never)) { <decode> }`.
    let key_check = |n: &str, k: &TypeRef| {
        let d = deser(k, persist_json_input(ident("__k")), n);
        persist_when_present(
            n,
            vec![persist_for_each(
                "[__k]",
                call(
                    ident("__wireEntries"),
                    vec![persist_field_as(n, bynk_ts::TsType::named("never"))],
                ),
                persist_decode(d, agent_name),
            )],
        )
    };
    // `__wire<Kind>(s.n, ...)`.
    let wire = |kind: &str, n: &str, extra: Option<bynk_ts::TsExpr>| {
        let mut args = vec![member(ident("s"), n)];
        args.extend(extra);
        bynk_ts::TsObjectEntry::prop(n, call(ident(kind), args))
    };
    // `(<param>) => <body>`.
    let arrow = |param: &str, body: bynk_ts::TsExpr| bynk_ts::TsExpr::Arrow {
        params: vec![bynk_ts::TsParam::new(param.to_string(), None)],
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(body))),
    };
    // `Map[K, V]` — validate each entry value against `V`, and each key against
    // `K` when `K` is textual (the key persists as a `String(k)` Record key).
    for (name, v) in store_map_fields {
        let n = &name.name;
        let held = held_names_all.contains(n.as_str());
        if is_codecable(v) {
            let entry = bynk_ts::TsExpr::Index {
                object: Box::new(bynk_ts::TsExpr::Paren(Box::new(persist_field_as(
                    n,
                    persist_record_ty(),
                )))),
                index: Box::new(ident("__k")),
            };
            let d = deser(v, persist_json_input(entry.clone()), n);
            let mut body = persist_decode(d, agent_name);
            body.push(bynk_ts::TsStmt::assign(
                entry,
                member(ident("__r"), "value"),
                None,
            ));
            rehydrate_checks.push(persist_when_present(
                n,
                vec![persist_for_each(
                    "__k",
                    call(
                        persist_global("Object.keys"),
                        vec![persist_field_as(n, bynk_ts::TsType::named("object"))],
                    ),
                    body,
                )],
            ));
        }
        if let Some(k) = store_field_key_type(a, n)
            && type_base_is_string(k, &commons.types)
        {
            rehydrate_checks.push(key_check(n, k));
        }
        if held {
            encode_entries.push(wire("__wireHeld", n, None));
            encode_entries.push(bynk_ts::TsObjectEntry::prop(
                format!("\"{n}:order\""),
                call(ident("__wireHeldOrder"), vec![member(ident("s"), n)]),
            ));
        } else if is_codecable(v) {
            encode_entries.push(wire(
                "__wireMap",
                n,
                Some(arrow("__v", ser(v, ident("__v")))),
            ));
        } else {
            encode_entries.push(wire("__wireMap", n, None));
        }
        // An `@indexed` map's posting lists ride in the wire record beside it (v0.93).
        let mut index_fields: Vec<&String> = store_map_indexes
            .get(n)
            .map(|fs| fs.iter().collect())
            .unwrap_or_default();
        index_fields.sort();
        for f in index_fields {
            encode_entries.push(bynk_ts::TsObjectEntry::prop(
                format!("{n}__idx_{f}"),
                call(
                    ident("__wirePostings"),
                    vec![member(ident("s"), n), str_lit(f)],
                ),
            ));
        }
    }
    // v0.105 (slice 3b-ii): a held `Map[K, Connection]` persists `K → connId`. The
    // connId is an opaque platform string (no value check); validate each textual
    // `K` key, as for a plain map.
    for (name, _) in held_maps {
        let n = &name.name;
        if let Some(k) = store_field_key_type(a, n)
            && type_base_is_string(k, &commons.types)
        {
            rehydrate_checks.push(key_check(n, k));
        }
        encode_entries.push(wire("__wireHeld", n, None));
        encode_entries.push(bynk_ts::TsObjectEntry::prop(
            format!("\"{n}:order\""),
            call(ident("__wireHeldOrder"), vec![member(ident("s"), n)]),
        ));
    }
    // `Set[T]` — the elements are the (textual) Record keys; validate when `T`
    // is textual, else defer (structural string key).
    for (name, t) in store_set_fields {
        let n = &name.name;
        if type_base_is_string(t, &commons.types) {
            rehydrate_checks.push(key_check(n, t));
        }
        encode_entries.push(wire("__wireSet", n, None));
    }
    // `Cache[K, V]` — validate each entry's `.v` against `V`.
    for (name, v, _) in store_cache_fields {
        let n = &name.name;
        if is_codecable(v) {
            let entry_value = || member(ident("__e"), "v");
            let d = deser(v, persist_json_input(entry_value()), n);
            let mut body = persist_decode(d, agent_name);
            body.push(bynk_ts::TsStmt::assign(
                entry_value(),
                member(ident("__r"), "value"),
                None,
            ));
            let cells = persist_field_as(
                n,
                bynk_ts::TsType::named_with_args(
                    "globalThis.Record",
                    vec![bynk_ts::TsType::named("string"), persist_entry_ty()],
                ),
            );
            rehydrate_checks.push(persist_when_present(
                n,
                vec![persist_for_each(
                    "__e",
                    call(persist_global("Object.values"), vec![cells]),
                    body,
                )],
            ));
            encode_entries.push(wire(
                "__wireMap",
                n,
                Some(arrow(
                    "__e",
                    bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::object_entries(vec![
                        bynk_ts::TsObjectEntry::Spread(ident("__e")),
                        bynk_ts::TsObjectEntry::prop("v", ser(v, member(ident("__e"), "v"))),
                    ]))),
                )),
            ));
        } else {
            encode_entries.push(wire("__wireMap", n, None));
        }
    }
    // `Log[T]` — validate each entry's `.v` against `T`.
    for (name, t, _) in store_log_fields {
        let n = &name.name;
        if is_codecable(t) {
            let entry_value = || member(ident("__e"), "v");
            let d = deser(t, persist_json_input(entry_value()), n);
            let mut body = persist_decode(d, agent_name);
            body.push(bynk_ts::TsStmt::assign(
                entry_value(),
                member(ident("__r"), "value"),
                None,
            ));
            let cells = bynk_ts::TsExpr::Paren(Box::new(persist_field_as(
                n,
                bynk_ts::TsType::named_with_args("ReadonlyArray", vec![persist_entry_ty()]),
            )));
            rehydrate_checks.push(persist_when_present(
                n,
                vec![persist_for_each("__e", cells, body)],
            ));
            encode_entries.push(wire(
                "__wireLog",
                n,
                Some(arrow("__v", ser(t, ident("__v")))),
            ));
        } else {
            encode_entries.push(wire("__wireLog", n, None));
        }
    }
    (rehydrate_checks, encode_entries)
}

/// The functions that guard an agent's stored state across a deploy (#3167): the
/// rehydration check `loadState` runs, the wire-format encoder, and the
/// migration step chain for a stored layout from an older schema version.
/// Pushes those declarations onto `stmts`; returns their names and the facts
/// `loadState` needs.
pub(super) fn emit_agent_state_persistence(
    a: &AgentDecl,
    layout: &AgentStoreLayout,
    zero_fn: &str,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    stmts: &mut Vec<bynk_ts::TsStmt>,
) -> AgentPersistFns {
    let state_ty = format!("{}State", a.name.name);
    let AgentStoreLayout {
        effective_fields,
        held_maps,
        store_map_fields,
        store_set_fields,
        store_cache_fields,
        store_log_fields,
        ..
    } = layout;
    // v0.96 (ADR 0124): the rehydration validation gate. `loadState` validates a
    // *loaded* (merged) state against the current type definition before any
    // handler reads it — the load-time twin of the commit-time invariant gate.
    // Each value position (a `Cell`'s `T`, a `Map`/`Cache`'s `V`, a `Log`'s `T`,
    // and a textual `Set` element / `Map` key) is run through the same boundary
    // deserialiser the HTTP/queue seams use; a failure is disposed of as an
    // internal `RehydrationViolation` fault (Q6), never a caller-facing 400.
    // (Non-textual `Map`/`Set` keys persist as structural string keys — refined-
    // key rehydration validation is a named follow-on, ADR 0124 D5.)
    let rehydrate_fn = format!("__rehydrate{}State", a.name.name);
    // #1649 (runtime-semantics track §3.2): agent state is stored in the **wire
    // shape**. `commitState` writes `__encode<Agent>State(s)`, which runs each
    // codec-able value through the boundary serialiser (`kind`-tagged sums,
    // base64 `Bytes`, entries-array value `Map`s). `loadState` merges
    // zero-then-stored and calls `__rehydrate<Agent>State(merged, stored)`,
    // which now *decodes* each value present in `stored` back into the in-memory
    // shape, instead of validating it and discarding the result. Before #1649,
    // `commitState` stored the in-memory shape while the gate validated against
    // the wire shape, so every reload of an enum/`Option`/`Result`/`Bytes`/
    // value-`Map` state faulted (`RehydrationViolation … StructuralMismatch`).
    //
    // A field absent from `stored` keeps its in-memory zero (ADR 0124 D4), and is
    // not decoded. An unknown stored key (a renamed or removed field) rides along
    // untouched and is written back, so #539's migrations still find it.
    let encode_fn = format!("__encode{}State", a.name.name);
    let (rehydrate_checks, encode_entries) =
        agent_state_checks_and_entries(a, layout, commons, ctx);
    // Whether the agent has any collection: its handles are encoded on every commit and
    // filled on every load, whether or not its values need the codec.
    let has_collections = !store_map_fields.is_empty()
        || !held_maps.is_empty()
        || !store_set_fields.is_empty()
        || !store_cache_fields.is_empty()
        || !store_log_fields.is_empty();
    let has_rehydrate = agent_gates_rehydration(a, layout.state, &commons.types);
    let has_encode = has_rehydrate || has_collections;
    // State-migrations S1 (#2211): this class's lock version facts, present
    // only when the build has the schema lock (Decision A).
    let agent_versions = ctx.agent_schema_versions.get(&a.name.name).cloned();
    if has_rehydrate {
        let rehydrate_fn_decl = bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Function {
                name: rehydrate_fn.clone(),
                generics: Vec::new(),
                params: vec![
                    bynk_ts::TsParam::new(
                        "s".to_string(),
                        Some(bynk_ts::TsType::named(state_ty.clone())),
                    ),
                    // #1649: the stored record, so a field absent from it keeps
                    // its in-memory zero instead of being decoded (ADR 0124 D4).
                    bynk_ts::TsParam::new(
                        "stored".to_string(),
                        Some(bynk_ts::TsType::named(state_ty.clone())),
                    ),
                ],
                return_type: Some(bynk_ts::TsType::named("void")),
                body: rehydrate_checks,
                is_async: false,
                inline: false,
            },
            None,
        );
        // #1486: no more explicit trailing `TsStmt::blank(None)` — see
        // `emit_refined_type`'s own identical note.
        stmts.push(rehydrate_fn_decl);
    }
    // Per-collection handles (#3254): `loadState` fills each collection the stored record
    // carries into its handle (the zero state's, so a `Map` keeps its `@indexed` indexers),
    // in the order the record notes. A collection the record lacks stays the empty zero.
    let hydrate_fn = format!("__hydrate{}State", a.name.name);
    if has_collections {
        stmts.push(hydrate_fn_decl(a, layout, &hydrate_fn, zero_fn));
    }
    if has_encode {
        // #1649: the wire-shape encoder `commitState` writes through. Every
        // codec-able value position is serialised; the rest of the record (held
        // maps, `Set`s, `@indexed` posting lists, unknown stored keys) is
        // already wire-shaped and spreads through unchanged.
        let mut entries = vec![bynk_ts::TsObjectEntry::Spread(ident("s"))];
        entries.extend(encode_entries);
        stmts.push(bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Function {
                name: encode_fn.clone(),
                generics: Vec::new(),
                params: vec![bynk_ts::TsParam::new(
                    "s".to_string(),
                    Some(bynk_ts::TsType::named(state_ty.clone())),
                )],
                return_type: Some(persist_record_ty()),
                body: vec![bynk_ts::TsStmt::return_stmt(
                    Some(bynk_ts::TsExpr::object_entries(entries)),
                    None,
                )],
                is_async: false,
                inline: false,
            },
            None,
        ));
    }
    // State-migrations S2 (#2224, track §3.3, Decision D): one function per
    // valid `migrate from N` step, carrying a stored (wire-shaped) record from
    // version N to the current shape, and a dispatcher `loadState` calls after
    // the `__v` check. A step's output is wire-shaped again, so `loadState`'s
    // zero-merge and the ADR 0124 gate then validate it like any record.
    let migrate_fn = format!("__migrate{}State", a.name.name);
    let step_plans: Vec<bynk_check::schema_registry::MigrationStep> = agent_versions
        .as_ref()
        .map(|v| v.steps.clone())
        .unwrap_or_default();
    if !step_plans.is_empty() {
        let agent_name = &a.name.name;
        let cell_names: HashSet<String> = effective_fields
            .iter()
            .map(|f| f.name.name.clone())
            .collect();
        let map_names: HashSet<String> = store_map_fields
            .iter()
            .map(|(n, _)| n.name.clone())
            .collect();
        let mut dispatch: Vec<bynk_ts::TsStmt> = Vec::new();
        for plan in &step_plans {
            let Some(step) = a.migrations.iter().find(|m| m.from == plan.from) else {
                continue;
            };
            let step_fn = format!("__migrate{agent_name}From{}", plan.from);
            // Names a step binds (`old`, its `let`s) shadow carried fields.
            let mut shadowed: HashSet<String> =
                step.old.iter().map(|o| o.name.name.clone()).collect();
            for item in &step.items {
                if let bynk_syntax::ast::MigrateItem::Let(l) = item {
                    shadowed.insert(l.name.name.clone());
                }
            }
            let mut cx = LowerCtx::new(
                ModuleCtx::for_unit(commons, ctx),
                BodyMode::Invariant {
                    name: "__c".to_string(),
                    fields: cell_names.difference(&shadowed).cloned().collect(),
                    collections: HashSet::new(),
                },
            );
            stmts.push(migration_step_decl(
                &MigrateNames {
                    agent_name,
                    state_ty: &state_ty,
                    zero_fn,
                    rehydrate_fn: has_rehydrate.then_some(rehydrate_fn.as_str()),
                    hydrate_fn: has_collections.then_some(hydrate_fn.as_str()),
                    encode_fn: has_encode.then_some(encode_fn.as_str()),
                    map_names: &map_names,
                    composite_keys: &layout.composite_keys,
                },
                &step_fn,
                plan,
                step,
                (commons, ctx),
                &mut cx,
            ));
            dispatch.push(migrate_step_dispatch(plan.lo, plan.from, &step_fn));
        }
        // The nearest step covering the stored version runs; a version no step
        // covers reads by zero-merge (it passed `__checkStoredSchemaVersion`).
        stmts.push(migrate_dispatcher_decl(&migrate_fn, dispatch));
    }
    let lazy_fields = emit_lazy_field_spec(a, layout, commons, ctx, stmts);
    AgentPersistFns {
        rehydrate_fn,
        encode_fn,
        migrate_fn,
        hydrate_fn,
        has_rehydrate,
        has_collections,
        has_encode,
        step_plans,
        agent_versions,
        lazy_fields,
    }
}

/// State-migrations S1 (#2211, track §3.2, Decision D): `__checkStoredSchemaVersion(agent, stored,
/// version, oldest, [steps])`, which checks the raw record's `__v` marker before anything is
/// merged or decoded, so a record from a newer build (or with no path forward) faults without being
/// read or written back. State-migrations S2 (#2224 Decision C): each step's covered range makes
/// those versions readable too.
fn check_stored_version_stmt(
    agent: &str,
    v: &bynk_check::schema_registry::AgentVersions,
    step_plans: &[bynk_check::schema_registry::MigrationStep],
) -> bynk_ts::TsStmt {
    let mut args = vec![
        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(agent.to_string())),
        bynk_ts::TsExpr::Ident("stored".to_string()),
        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(v.version.to_string())),
        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(v.oldest_readable.to_string())),
    ];
    if !step_plans.is_empty() {
        args.push(step_ranges_expr(step_plans));
    }
    bynk_ts::TsStmt::expr_stmt(
        bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Ident(
                "__checkStoredSchemaVersion".to_string(),
            )),
            args,
        },
        None,
    )
}

/// #4482: `await __moveRenamedEntries(this, this.state.storage, [[kind, old, new], …]);`, one pair per
/// `Map` or `Set` field a `replaces` renamed, `kind` the key tag its entries sit under (`m` or `s`).
fn move_renamed_entries_stmt(
    a: &AgentDecl,
    versions: Option<&bynk_check::schema_registry::AgentVersions>,
    step_plans: &[bynk_check::schema_registry::MigrationStep],
) -> Option<bynk_ts::TsStmt> {
    let pairs: Vec<bynk_ts::TsExpr> = versions?
        .renames
        .iter()
        .filter_map(|(old, new)| {
            let field = a.store_fields.iter().find(|f| &f.name.name == new)?;
            let kind = match field.kind.head.name.as_str() {
                "Map" => "m",
                "Set" => "s",
                _ => return None,
            };
            Some(bynk_ts::TsExpr::object_entries(vec![
                bynk_ts::TsObjectEntry::prop("kind", str_lit(kind.to_string())),
                bynk_ts::TsObjectEntry::prop("from", str_lit(old.clone())),
                bynk_ts::TsObjectEntry::prop("to", str_lit(new.clone())),
            ]))
        })
        .collect();
    if pairs.is_empty() {
        return None;
    }
    Some(bynk_ts::TsStmt::expr_stmt(
        bynk_ts::TsExpr::Await(Box::new(call(
            ident("__moveRenamedEntries"),
            vec![
                ident("this"),
                ident("this.state.storage"),
                ident("stored"),
                step_ranges_expr(step_plans),
                bynk_ts::TsExpr::array(pairs),
            ],
        ))),
        None,
    ))
}

/// #3793: `const __renamed = __renameStoredFields(<src>, [[old, new], …]) as typeof <src>;`. Each pair
/// moves a value stored under `old` to `new` unless the record already holds `new`, so a `store`
/// field renamed with `replaces` is read from the record an older build stored.
fn rename_stored_stmt(src: &str, renames: &[(String, String)]) -> bynk_ts::TsStmt {
    let pairs = renames
        .iter()
        .map(|(old, new)| bynk_ts::TsExpr::array(vec![str_lit(old.clone()), str_lit(new.clone())]))
        .collect();
    bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__renamed".to_string()),
        None,
        bynk_ts::TsExpr::As {
            expr: Box::new(call(
                ident("__renameStoredFields"),
                vec![ident(src), bynk_ts::TsExpr::array(pairs)],
            )),
            ty: bynk_ts::TsType::named(format!("typeof {src}")),
        },
        None,
    )
}

/// #4038: `await __checkUnique(agent, s, [[field, index, location], …]);`, the commit-time check of
/// every `@unique(by:)` of the agent, run with the invariants and before the write.
fn unique_check(
    a: &AgentDecl,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    source_text: &str,
) -> Option<bynk_ts::TsStmt> {
    let unique = unique_store_indexes(a, &commons.types);
    if unique.is_empty() {
        return None;
    }
    let declared = unique
        .iter()
        .map(|u| {
            bynk_ts::TsExpr::array(vec![
                str_lit(&u.map),
                str_lit(&u.field),
                str_lit(declaration_location(ctx, source_text, u.span)),
            ])
        })
        .collect();
    Some(bynk_ts::TsStmt::expr_stmt(
        bynk_ts::TsExpr::Await(Box::new(call(
            ident("__checkUnique"),
            vec![
                str_lit(&a.name.name),
                ident("s"),
                bynk_ts::TsExpr::array(declared),
            ],
        ))),
        None,
    ))
}

/// #4034: `const <name> = await <call>;`, a collection count bound for a predicate.
fn size_const(name: &str, call: bynk_ts::TsExpr) -> bynk_ts::TsStmt {
    bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident(name.to_string()),
        None,
        bynk_ts::TsExpr::Await(Box::new(call)),
        None,
    )
}

/// The refusal a failed invariant or transition throws: log, then fault with the
/// declaration's name and location (never the key, ADR 0107).
fn violation_check(
    agent: &str,
    name: &str,
    declaration: &str,
    location: &str,
    pred: bynk_ts::TsExpr,
    pre_stmts: Vec<bynk_ts::TsStmt>,
) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = pre_stmts;
    stmts.push(bynk_ts::TsStmt::if_stmt(
        bynk_ts::TsExpr::Unary {
            op: bynk_ts::TsUnaryOp::Not,
            expr: Box::new(bynk_ts::TsExpr::Paren(Box::new(pred))),
        },
        bynk_ts::TsStmt::block(
            vec![
                bynk_ts::TsStmt::expr_stmt(
                    bynk_ts::TsExpr::Call {
                        callee: Box::new(bynk_ts::TsExpr::Ident(
                            "globalThis.console.error".to_string(),
                        )),
                        args: vec![
                            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(format!(
                                "InvariantViolation {}.{name}",
                                agent
                            ))),
                            bynk_ts::TsExpr::object(vec![
                                (
                                    "agent".to_string(),
                                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(agent.to_string())),
                                ),
                                (
                                    "invariant".to_string(),
                                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(name.to_string())),
                                ),
                            ]),
                        ],
                    },
                    None,
                ),
                bynk_ts::TsStmt::throw_stmt(
                    bynk_ts::TsExpr::Call {
                        callee: Box::new(bynk_ts::TsExpr::Ident(
                            "__invariantViolation".to_string(),
                        )),
                        args: vec![
                            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(agent.to_string())),
                            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(name.to_string())),
                            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(declaration.to_string())),
                            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(location.to_string())),
                        ],
                    },
                    None,
                ),
            ],
            None,
        ),
        None,
    ));
    stmts
}
