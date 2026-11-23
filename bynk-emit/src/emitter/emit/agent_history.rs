//! Agent history driver: the history-property driver an agent class gets (`emit_agent_history_driver`) and
//! the `history_*` builders it assembles.

use super::{AgentLoadFns, history_variant_tag, step_ranges_expr, type_base_is_int};
use crate::emit_model::EmitProjectCtx;
use crate::emitter::zero_value::zero_expr as zero_value_expr;
use crate::emitter::{agent_factory_name, ts_ident};
use crate::ts_build::{
    await_expr, binary, call, const_, ident, let_decl, member, not_expr, num_lit, str_lit,
};
use bynk_check::checker::TypedCommons;
use bynk_model::{IrHandlerKind, lower_handler_kind_ir};
use bynk_syntax::ast::{AgentDecl, Handler, Param, TypeDecl};
use std::collections::HashMap;
use std::sync::Arc;

/// The history-property driver (#3167): an exported, test-only function for each
/// agent a `for all run: History[Agent]` property targets, so every other agent's
/// emission is unchanged. Pushes it onto `stmts` after the agent's factory.
pub(super) fn emit_agent_history_driver(
    a: &AgentDecl,
    fns: &AgentLoadFns,
    registry: &str,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    stmts: &mut Vec<bynk_ts::TsStmt>,
) {
    let state_ty = format!("{}State", a.name.name);
    // v0.119 (testing track slice 7, ADR 0155): the history-property driver. Only
    // agents a `for all run: History[Agent]` property targets get this exported
    // test-support function (gated on `history_target_agents`), so every other
    // agent's emission is byte-for-byte unchanged. It drives a generated call
    // sequence through the *real* handlers from a fresh instance, recording each
    // reached step — its call, whether it committed (an invariant/`transition`
    // refusal throws `invariantViolation`, leaving state uncommitted), and the
    // committed `old` → `new` pair — for the runner's predicate. Test-only, so it
    // is stripped from deploy builds with the rest of the test surface.
    if ctx.history_target_agents.contains(&a.name.name) {
        let hs: Vec<&Handler> = a
            .handlers
            .iter()
            .filter(|h| {
                matches!(lower_handler_kind_ir(&h.kind), IrHandlerKind::Call)
                    && h.method_name.is_some()
            })
            .collect();
        let driver = format!("__bynkDriveHistory_{}", a.name.name);
        // P7.2: `call`'s real shape is a discriminated union across `hs` (one
        // variant per handler, built further down from `history_variant_tag` +
        // each handler's own fields) and `deps` varies per handler too (each
        // handler computes its own `deps_ty` from its own `given` set,
        // `build_deps_object_ty_with_surface`'s own per-handler call in
        // `agent_dispatch.rs`) — a single driver-wide type for either needs the
        // intersection/union across every handler this agent's history targets,
        // not a same-line change. `seq`'s own `args` is safe to narrow:
        // it's read back only through `history_arg_expr` call sites below,
        // never given a fixed shape of its own.
        let step_ty = history_step_ty(&state_ty);
        let seq_ty = bynk_ts::TsType::named_with_args(
            "globalThis.Array",
            vec![bynk_ts::TsType::Object(vec![
                bynk_ts::TsTypeMember::prop("h", bynk_ts::TsType::named("number")),
                bynk_ts::TsTypeMember::prop(
                    "args",
                    bynk_ts::TsType::array(bynk_ts::TsType::named("unknown")),
                ),
            ])],
        );
        let mut body = vec![
            bynk_ts::TsStmt::expr_stmt(call(member(ident(registry), "reset"), vec![]), None),
            history_inst_stmt(a, commons),
            const_("__load", history_load_arrow(a, fns, &state_ty)),
            const_("__rej", history_rejected_arrow()),
            bynk_ts::TsStmt::const_stmt(
                bynk_ts::TsBindingName::Ident("__steps".to_string()),
                Some(history_steps_ty(&step_ty)),
                bynk_ts::TsExpr::array(Vec::new()),
                None,
            ),
            const_("__ce", hist_console_error()),
        ];
        body.push(bynk_ts::TsStmt::assign(
            hist_console_error(),
            history_console_mute_arrow(),
            None,
        ));
        // A driven run deliberately provokes rejected steps (an invariant/
        // `transition` refusal), each of which `console.error`s an
        // `InvariantViolation` line from `commitState` before throwing. Mute just
        // those lines for the duration of the drive so the run isn't drowned in
        // expected noise; every other `console.error` still passes through, and the
        // original is restored in `finally`.
        body.push(bynk_ts::TsStmt::try_finally(
            bynk_ts::TsStmt::block(
                vec![
                    history_drive_loop(&hs, &step_ty, commons),
                    bynk_ts::TsStmt::return_stmt(Some(ident("__steps")), None),
                ],
                None,
            ),
            bynk_ts::TsStmt::block(
                vec![bynk_ts::TsStmt::assign(
                    hist_console_error(),
                    ident("__ce"),
                    None,
                )],
                None,
            ),
            None,
        ));
        stmts.push(bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Function {
                name: driver,
                generics: Vec::new(),
                params: vec![
                    bynk_ts::TsParam::new("seq", Some(seq_ty)),
                    bynk_ts::TsParam::new("deps", Some(history_any_ty())),
                ],
                return_type: Some(bynk_ts::TsType::named_with_args(
                    "globalThis.Promise",
                    vec![history_steps_ty(&step_ty)],
                )),
                body,
                is_async: true,
                inline: false,
            })),
            None,
        ));
        // #1486: no trailing blank baked in here — the printer's automatic
        // top-level spacing policy supplies the single blank before whatever
        // statement follows.
    }
}

/// `any`, the one escape the history driver keeps (P7.2): `call` and `deps` take a different
/// shape per handler, so a single driver-wide type would need a per-agent union.
fn history_any_ty() -> bynk_ts::TsType {
    bynk_ts::TsType::named("any")
}

/// `globalThis.console.error`.
fn hist_console_error() -> bynk_ts::TsExpr {
    member(member(ident("globalThis"), "console"), "error")
}

/// `{ call: any, accepted: boolean, old: S, new: S, refusal?: unknown }`: one reached step.
fn history_step_ty(state_ty: &str) -> bynk_ts::TsType {
    let state = || bynk_ts::TsType::named(state_ty.to_string());
    bynk_ts::TsType::Object(vec![
        bynk_ts::TsTypeMember::prop("call", history_any_ty()),
        bynk_ts::TsTypeMember::prop("accepted", bynk_ts::TsType::named("boolean")),
        bynk_ts::TsTypeMember::prop("old", state()),
        bynk_ts::TsTypeMember::prop("new", state()),
        bynk_ts::TsTypeMember::optional_prop("refusal", bynk_ts::TsType::named("unknown")),
    ])
}

/// `globalThis.Array<step>`.
fn history_steps_ty(step_ty: &bynk_ts::TsType) -> bynk_ts::TsType {
    bynk_ts::TsType::named_with_args("globalThis.Array", vec![step_ty.clone()])
}

/// `{ invariantViolation?: unknown }`: the one field a caught throw is checked for.
fn history_violation_ty() -> bynk_ts::TsType {
    bynk_ts::TsType::Object(vec![bynk_ts::TsTypeMember::optional_prop(
        "invariantViolation",
        bynk_ts::TsType::named("unknown"),
    )])
}

/// `(e as { invariantViolation?: unknown }).invariantViolation`.
fn history_violation_of(caught: &str) -> bynk_ts::TsExpr {
    member(
        bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::As {
            expr: Box::new(ident(caught)),
            ty: history_violation_ty(),
        })),
        "invariantViolation",
    )
}

/// `const __inst = <factory>(<key zero>) as any;`. P7.2: deferred, not narrowed. A first
/// attempt dropped this cast, reasoning the factory already declares its agent and `.state`/
/// `.{method}` are real declared members — true, but it broke real `tsc --strict` fixtures
/// (248/249): `history_arg_expr` produces unbranded plain values (`Number(__st.args[0])`),
/// which don't structurally match a branded handler param type (`Amount`) once `__inst` is
/// genuinely agent-typed instead of `any`. Branding `history_arg_expr`'s own output is the
/// real fix, not a same-line change here.
fn history_inst_stmt(a: &AgentDecl, commons: &TypedCommons) -> bynk_ts::TsStmt {
    let key = bynk_check::checker::zero_value(&a.key_type, None, &commons.types)
        .map(|z| zero_value_expr(&z))
        .unwrap_or_else(|| bynk_ts::TsExpr::As {
            expr: Box::new(ident("undefined")),
            ty: bynk_ts::TsType::named("never"),
        });
    const_(
        "__inst",
        bynk_ts::TsExpr::As {
            expr: Box::new(call(ident(agent_factory_name(&a.name.name)), vec![key])),
            ty: history_any_ty(),
        },
    )
}

/// The `async` arrow that reads the instance's stored state the way `loadState` does (#1649,
/// #2211, #2224, #3189): decoded, version-checked, joined and migrated into the state type.
fn history_load_arrow(a: &AgentDecl, fns: &AgentLoadFns, state_ty: &str) -> bynk_ts::TsExpr {
    let AgentLoadFns {
        zero_fn,
        rehydrate_fn,
        migrate_fn,
        hydrate_fn,
        has_rehydrate,
        has_collections,
        step_plans,
        agent_versions,
        ..
    } = *fns;
    let stored = || member(member(member(ident("__inst"), "state"), "storage"), "get");
    let assign_s = |value: bynk_ts::TsExpr| bynk_ts::TsStmt::assign(ident("__s"), value, None);
    let zero = || call(ident(zero_fn), vec![]);
    let mut body = vec![
        // `let`: the stored record is replaced by its joined, wholesale-shaped form below
        // (#3189).
        let_decl(
            "__s",
            None,
            Some(await_expr(call(stored(), vec![str_lit("state")]))),
        ),
        bynk_ts::TsStmt::if_stmt(
            binary(
                bynk_ts::TsBinaryOp::StrictEq,
                ident("__s"),
                ident("undefined"),
            ),
            bynk_ts::TsStmt::return_stmt(Some(zero()), None),
            None,
        ),
        bynk_ts::TsStmt::expr_stmt(
            call(
                ident("__checkStoredLayout"),
                vec![str_lit(&a.name.name), ident("__s")],
            ),
            None,
        ),
    ];
    // State-migrations S1 (#2211): the same `__v` check `loadState` runs.
    if let Some(v) = agent_versions {
        let mut args = vec![
            str_lit(&a.name.name),
            ident("__s"),
            num_lit(v.version),
            num_lit(v.oldest_readable),
        ];
        if !step_plans.is_empty() {
            args.push(step_ranges_expr(step_plans));
        }
        body.push(bynk_ts::TsStmt::expr_stmt(
            call(ident("__checkStoredSchemaVersion"), args),
            None,
        ));
    }
    body.push(assign_s(await_expr(call(
        ident("__readStored"),
        vec![
            ident("__inst"),
            member(member(ident("__inst"), "state"), "storage"),
            ident("__s"),
        ],
    ))));
    // State-migrations S2 (#2224): and the same step dispatch.
    if !step_plans.is_empty() {
        body.push(assign_s(call(ident(migrate_fn), vec![ident("__s")])));
    }
    body.push(const_(
        "__m",
        bynk_ts::TsExpr::object_entries(vec![
            bynk_ts::TsObjectEntry::Spread(zero()),
            bynk_ts::TsObjectEntry::Spread(ident("__s")),
        ]),
    ));
    // #1649: decode the stored (wire-shape) state exactly as `loadState` does.
    for (wanted, func) in [(has_rehydrate, rehydrate_fn), (has_collections, hydrate_fn)] {
        if wanted {
            body.push(bynk_ts::TsStmt::expr_stmt(
                call(ident(func), vec![ident("__m"), ident("__s")]),
                None,
            ));
        }
    }
    body.push(bynk_ts::TsStmt::return_stmt(Some(ident("__m")), None));
    bynk_ts::TsExpr::Arrow {
        params: Vec::new(),
        is_async: true,
        generics: Vec::new(),
        return_type: Some(bynk_ts::TsType::named_with_args(
            "globalThis.Promise",
            vec![bynk_ts::TsType::named(state_ty.to_string())],
        )),
        body: Box::new(bynk_ts::TsArrowBody::Block(body)),
    }
}

/// `(e: unknown) => !!e && (e as { invariantViolation?: unknown }).invariantViolation !== undefined`.
/// P7.2: `e` is a caught throw of unknown shape by construction — a marker type stating
/// exactly the one field this checks, not a blanket escape.
fn history_rejected_arrow() -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Arrow {
        params: vec![bynk_ts::TsParam::new(
            "e",
            Some(bynk_ts::TsType::named("unknown")),
        )],
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(binary(
            bynk_ts::TsBinaryOp::And,
            not_expr(not_expr(ident("e"))),
            binary(
                bynk_ts::TsBinaryOp::StrictNotEq,
                history_violation_of("e"),
                ident("undefined"),
            ),
        )))),
    }
}

/// `(...__a: unknown[]) => { if (typeof __a[0] === "string" && __a[0].startsWith("InvariantViolation")) return; __ce(...__a); }`
/// P7.2: `console.error`'s own real type is `(...data: unknown[]) => void` — `__ce` is already
/// that type by inference, and the replacement matches it exactly, so no cast was ever
/// load-bearing.
fn history_console_mute_arrow() -> bynk_ts::TsExpr {
    let first = || bynk_ts::TsExpr::Index {
        object: Box::new(ident("__a")),
        index: Box::new(num_lit(0)),
    };
    let is_violation = binary(
        bynk_ts::TsBinaryOp::And,
        binary(
            bynk_ts::TsBinaryOp::StrictEq,
            bynk_ts::TsExpr::Unary {
                op: bynk_ts::TsUnaryOp::Typeof,
                expr: Box::new(first()),
            },
            str_lit("string"),
        ),
        call(
            member(first(), "startsWith"),
            vec![str_lit("InvariantViolation")],
        ),
    );
    bynk_ts::TsExpr::Arrow {
        params: vec![
            bynk_ts::TsParam::new(
                "__a",
                Some(bynk_ts::TsType::array(bynk_ts::TsType::named("unknown"))),
            )
            .rest(),
        ],
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(bynk_ts::TsArrowBody::Block(vec![
            bynk_ts::TsStmt::if_stmt(is_violation, bynk_ts::TsStmt::return_stmt(None, None), None),
            bynk_ts::TsStmt::expr_stmt(
                call(
                    ident("__ce"),
                    vec![bynk_ts::TsExpr::Spread(Box::new(ident("__a")))],
                ),
                None,
            ),
        ])),
    }
}

/// v0.119 (ADR 0155): the TS expression for a driven handler argument. An
/// `Int`-based value (bare or refined/opaque) is coerced to `number` — the
/// property generator yields `bigint`, but handler bodies do `number` arithmetic
/// (the same rule the contract attacker follows; the refinement brand is
/// compile-time only, so `Number(...)` is a no-op at runtime). Everything else —
/// `String`/refined-`String`, `Bool` — passes through.
fn history_arg_expr(
    p: &Param,
    i: usize,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> bynk_ts::TsExpr {
    let arg = bynk_ts::TsExpr::Index {
        object: Box::new(member(ident("__st"), "args")),
        index: Box::new(num_lit(i)),
    };
    if type_base_is_int(&p.type_ref, types) {
        call(member(ident("globalThis"), "Number"), vec![arg])
    } else {
        arg
    }
}

/// `switch (__st.h) { case 0: <body>; break; … }`, one case per handler.
fn history_switch(
    hs: &[&Handler],
    body: impl Fn(&Handler) -> Vec<bynk_ts::TsStmt>,
) -> bynk_ts::TsStmt {
    let cases = hs
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let mut stmts = body(h);
            stmts.push(bynk_ts::TsStmt::break_stmt(None));
            bynk_ts::TsSwitchCase {
                labels: vec![num_lit(i)],
                body: stmts,
                default_braced: false,
                case_braced: false,
                span: None,
            }
        })
        .collect();
    bynk_ts::TsStmt::switch_stmt(member(ident("__st"), "h"), cases, None)
}

/// The driver's loop over the generated sequence: run the call, then record the step.
fn history_drive_loop(
    hs: &[&Handler],
    step_ty: &bynk_ts::TsType,
    commons: &TypedCommons,
) -> bynk_ts::TsStmt {
    let run = history_switch(hs, |h| {
        let m = &h.method_name.as_ref().unwrap().name;
        let mut args: Vec<bynk_ts::TsExpr> = h
            .params
            .iter()
            .enumerate()
            .map(|(j, p)| history_arg_expr(p, j, &commons.types))
            .collect();
        args.push(ident("deps"));
        vec![bynk_ts::TsStmt::expr_stmt(
            await_expr(call(member(ident("__inst"), m), args)),
            None,
        )]
    });
    // An invariant/`transition` refusal throws `invariantViolation`: the step is not accepted,
    // and the refusal is the runner's own to read.
    let rejected = bynk_ts::TsStmt::try_catch(
        bynk_ts::TsStmt::block(vec![run], None),
        Some("__e"),
        bynk_ts::TsStmt::block(
            vec![bynk_ts::TsStmt::if_else_stmt(
                call(ident("__rej"), vec![ident("__e")]),
                bynk_ts::TsStmt::block(
                    vec![
                        bynk_ts::TsStmt::assign(
                            ident("__accepted"),
                            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(false)),
                            None,
                        ),
                        bynk_ts::TsStmt::assign(
                            ident("__refusal"),
                            history_violation_of("__e"),
                            None,
                        ),
                    ],
                    None,
                ),
                bynk_ts::TsStmt::block(vec![bynk_ts::TsStmt::throw_stmt(ident("__e"), None)], None),
                None,
            )],
            None,
        ),
        None,
    );
    // P7.2: deferred, same reason as `step_ty`'s own `call` field — a real type here is the
    // same per-handler discriminated union.
    let record = history_switch(hs, |h| {
        let tag = history_variant_tag(&h.method_name.as_ref().unwrap().name);
        let mut entries = vec![bynk_ts::TsObjectEntry::prop(
            "tag".to_string(),
            str_lit(&tag),
        )];
        for (j, p) in h.params.iter().enumerate() {
            entries.push(bynk_ts::TsObjectEntry::prop(
                ts_ident(&p.name.name),
                history_arg_expr(p, j, &commons.types),
            ));
        }
        vec![bynk_ts::TsStmt::assign(
            ident("__call"),
            bynk_ts::TsExpr::object_entries(entries),
            None,
        )]
    });
    let new_state = bynk_ts::TsExpr::Conditional {
        test: Box::new(ident("__accepted")),
        consequent: Box::new(await_expr(call(ident("__load"), vec![]))),
        alternate: Box::new(ident("__old")),
    };
    let step = bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__step".to_string()),
        Some(step_ty.clone()),
        bynk_ts::TsExpr::object(
            [
                ("call", "__call"),
                ("accepted", "__accepted"),
                ("old", "__old"),
                ("new", "__new"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), ident(v)))
            .collect(),
        ),
        None,
    );
    // The refusal is the runner's, not part of the step's Bynk type, so it is
    // defined non-enumerable: `==` and a rendered step never see it.
    let define_refusal = bynk_ts::TsStmt::if_stmt(
        binary(
            bynk_ts::TsBinaryOp::StrictNotEq,
            ident("__refusal"),
            ident("undefined"),
        ),
        bynk_ts::TsStmt::expr_stmt(
            call(
                member(member(ident("globalThis"), "Object"), "defineProperty"),
                vec![
                    ident("__step"),
                    str_lit("refusal"),
                    bynk_ts::TsExpr::object(vec![("value".to_string(), ident("__refusal"))]),
                ],
            ),
            None,
        ),
        None,
    );
    bynk_ts::TsStmt::for_of(
        "__st",
        ident("seq"),
        bynk_ts::TsStmt::block(
            vec![
                const_("__old", await_expr(call(ident("__load"), vec![]))),
                let_decl(
                    "__accepted",
                    None,
                    Some(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(true))),
                ),
                let_decl("__refusal", Some(bynk_ts::TsType::named("unknown")), None),
                rejected,
                const_("__new", new_state),
                let_decl("__call", Some(history_any_ty()), None),
                record,
                step,
                define_refusal,
                bynk_ts::TsStmt::expr_stmt(
                    call(member(ident("__steps"), "push"), vec![ident("__step")]),
                    None,
                ),
            ],
            None,
        ),
        None,
    )
}
