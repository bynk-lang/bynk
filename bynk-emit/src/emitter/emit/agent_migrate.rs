//! Agent migration: the `migrate from N` step functions and the dispatcher that runs them
//! (`migrate_step_dispatch`, `migrate_dispatcher_decl`, `migration_step_decl`).

use super::{
    persist_global, persist_present, persist_recast, persist_record_ty, type_base_is_string,
    wire_record_ty,
};
use crate::emit_model::EmitProjectCtx;
use crate::emitter::Pre;
use crate::emitter::lower_ctx::LowerCtx;
use crate::emitter::serialisation;
use crate::emitter::zero_value::zero_expr as zero_value_expr;
use crate::ts_build::{as_expr, binary, call, const_, ident, let_decl, member, num_lit, str_lit};
use bynk_check::checker::TypedCommons;
use bynk_syntax::ast::{RecordField, TypeBody, TypeRef};
use std::collections::HashSet;

/// `if (__v >= <lo> && __v <= <from>) return <step_fn>(stored);`: one arm of an agent's
/// migration dispatcher.
pub(super) fn migrate_step_dispatch(lo: i64, from: i64, step_fn: &str) -> bynk_ts::TsStmt {
    let bound = |op: bynk_ts::TsBinaryOp, n: i64| bynk_ts::TsExpr::Binary {
        op,
        left: Box::new(ident("__v")),
        right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(n.to_string()))),
    };
    bynk_ts::TsStmt::if_stmt(
        bynk_ts::TsExpr::Binary {
            op: bynk_ts::TsBinaryOp::And,
            left: Box::new(bound(bynk_ts::TsBinaryOp::GreaterThanEq, lo)),
            right: Box::new(bound(bynk_ts::TsBinaryOp::LessThanEq, from)),
        },
        bynk_ts::TsStmt::return_stmt(
            Some(bynk_ts::TsExpr::Call {
                callee: Box::new(ident(step_fn)),
                args: vec![ident("stored")],
            }),
            None,
        ),
        None,
    )
}

/// `function <migrate_fn>(stored): Record<string, unknown>`: reads the stored version (1 when
/// absent), runs the nearest step covering it, else returns the record as it is.
pub(super) fn migrate_dispatcher_decl(
    migrate_fn: &str,
    dispatch: Vec<bynk_ts::TsStmt>,
) -> bynk_ts::TsStmt {
    let stored = || bynk_ts::TsExpr::Ident("stored".to_string());
    let version = || bynk_ts::TsExpr::Index {
        object: Box::new(stored()),
        index: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str("__v".to_string()))),
    };
    let read_version = bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__v".to_string()),
        None,
        bynk_ts::TsExpr::Conditional {
            test: Box::new(bynk_ts::TsExpr::Binary {
                op: bynk_ts::TsBinaryOp::StrictEq,
                left: Box::new(version()),
                right: Box::new(bynk_ts::TsExpr::Ident("undefined".to_string())),
            }),
            consequent: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num("1".to_string()))),
            alternate: Box::new(bynk_ts::TsExpr::As {
                expr: Box::new(version()),
                ty: bynk_ts::TsType::named("number"),
            }),
        },
        None,
    );
    let mut body = vec![read_version];
    body.extend(dispatch);
    body.push(bynk_ts::TsStmt::return_stmt(Some(stored()), None));
    bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Function {
            name: migrate_fn.to_string(),
            generics: Vec::new(),
            params: vec![bynk_ts::TsParam::new(
                "stored".to_string(),
                Some(wire_record_ty()),
            )],
            return_type: Some(wire_record_ty()),
            body,
            is_async: false,
            inline: false,
        },
        None,
    )
}

/// What a migration step function is built over, beyond the step itself.
pub(super) struct MigrateNames<'a> {
    pub(super) agent_name: &'a str,
    pub(super) state_ty: &'a str,
    pub(super) zero_fn: &'a str,
    /// The functions a step calls on the carried state, present when the agent has them.
    pub(super) rehydrate_fn: Option<&'a str>,
    pub(super) hydrate_fn: Option<&'a str>,
    pub(super) encode_fn: Option<&'a str>,
    /// The current shape's `store Map` fields.
    pub(super) map_names: &'a HashSet<String>,
}

/// `["a", "b"]`: a list of names as an array literal.
fn hist_str_array(names: &[String]) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::array(names.iter().map(str_lit).collect())
}

/// `stored["name"]`.
fn stored_at(key: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Index {
        object: Box::new(ident("stored")),
        index: Box::new(key),
    }
}

/// `const <name>: globalThis.Record<string, unknown> = {};`.
fn empty_record_const(name: &str) -> bynk_ts::TsStmt {
    bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident(name.to_string()),
        Some(wire_record_ty()),
        bynk_ts::TsExpr::object(Vec::new()),
        None,
    )
}

/// `for (const __k of <keys>) if (<test>) <into>[__k] = stored[__k];`: copy the stored keys
/// that pass `test` (written over `__k`) into the record `into`.
fn copy_stored_keys(keys: bynk_ts::TsExpr, test: bynk_ts::TsExpr, into: &str) -> bynk_ts::TsStmt {
    bynk_ts::TsStmt::for_of(
        "__k",
        keys,
        bynk_ts::TsStmt::if_stmt(
            test,
            bynk_ts::TsStmt::assign(
                bynk_ts::TsExpr::Index {
                    object: Box::new(ident(into)),
                    index: Box::new(ident("__k")),
                },
                stored_at(ident("__k")),
                None,
            ),
            None,
        ),
        None,
    )
}

/// One field of `__ow`, the stored record's old fields gathered for the old type's decoder.
fn migrate_old_field_entry(
    f: &RecordField,
    plan: &bynk_check::schema_registry::MigrationStep,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> bynk_ts::TsObjectEntry {
    let n = &f.name.name;
    let raw = match &f.type_ref {
        // A store `Map` is a `Record<string, V>`; a `Map` value's wire form is an entries
        // array. A `Cell` holding a map is already in that wire form.
        TypeRef::Map(k, _, _) if plan.store_maps.contains(n) => {
            let key = if type_base_is_string(k, &commons.types) {
                ident("__k")
            } else {
                call(persist_global("Number"), vec![ident("__k")])
            };
            let entries = call(
                persist_global("Object.entries"),
                vec![as_expr(stored_at(str_lit(n)), persist_record_ty())],
            );
            call(
                member(entries, "map"),
                vec![bynk_ts::TsExpr::Arrow {
                    params: vec![bynk_ts::TsParam::array_pattern(
                        vec!["__k".to_string(), "__v".to_string()],
                        None,
                    )],
                    is_async: false,
                    generics: Vec::new(),
                    return_type: None,
                    body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(
                        bynk_ts::TsExpr::array(vec![key, ident("__v")]),
                    ))),
                }],
            )
        }
        TypeRef::Base(..)
        | TypeRef::Named(_)
        | TypeRef::Result(..)
        | TypeRef::Option(..)
        | TypeRef::Effect(..)
        | TypeRef::HttpResult(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::List(..)
        | TypeRef::Map(..)
        | TypeRef::Set(..)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
        | TypeRef::Unit(_)
        | TypeRef::Fn(..)
        | TypeRef::App { .. } => stored_at(str_lit(n)),
    };
    // A field the stored record lacks (an additive predecessor, Decision C) takes the old
    // type's declared default (the record decoder applies it), or else its type's zero.
    if f.init.is_some() {
        return bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Paren(Box::new(
            bynk_ts::TsExpr::Conditional {
                test: Box::new(persist_present(n)),
                consequent: Box::new(bynk_ts::TsExpr::object(vec![(n.clone(), raw)])),
                alternate: Box::new(bynk_ts::TsExpr::object(Vec::new())),
            },
        )));
    }
    match bynk_check::checker::zero_value(&f.type_ref, f.refinement.as_ref(), &commons.types) {
        Some(z) => bynk_ts::TsObjectEntry::prop(
            n.clone(),
            bynk_ts::TsExpr::Conditional {
                test: Box::new(persist_present(n)),
                consequent: Box::new(raw),
                alternate: Box::new(serialisation::serialise_expr(
                    &f.type_ref,
                    &zero_value_expr(&z),
                    &ctx.runtime_use,
                )),
            },
        ),
        None => bynk_ts::TsObjectEntry::prop(n.clone(), raw),
    }
}

/// The step's decode of the old record: `const __ow = {..}; const __od = <decode>; if (..)
/// throw ..; const <old> = __od.value;`. Binds the old name in `cx`.
fn migrate_decode_old(
    names: &MigrateNames<'_>,
    plan: &bynk_check::schema_registry::MigrationStep,
    step: &bynk_syntax::ast::MigrateDecl,
    (commons, ctx): (&TypedCommons, &EmitProjectCtx),
    cx: &mut LowerCtx,
    body: &mut Vec<bynk_ts::TsStmt>,
) {
    if let Some(old) = &step.old
        && let TypeRef::Named(id) = &old.type_ref
        && let Some(decl) = commons.types.get(&id.name)
        && let TypeBody::Record(r) = &decl.body
    {
        let entries = r
            .fields
            .iter()
            .map(|f| migrate_old_field_entry(f, plan, commons, ctx))
            .collect();
        let decoded = serialisation::deserialise_expr(
            &old.type_ref,
            &bynk_ts::TsExpr::Paren(Box::new(as_expr(
                ident("__ow"),
                bynk_ts::TsType::named("never"),
            ))),
            &old.name.name,
            &ctx.runtime_use,
        );
        let old_js = cx.bind_local_name(&old.name.name);
        body.push(const_("__ow", bynk_ts::TsExpr::object_entries(entries)));
        body.push(const_("__od", decoded));
        let od = || ident("__od");
        body.push(bynk_ts::TsStmt::if_stmt(
            binary(
                bynk_ts::TsBinaryOp::StrictEq,
                member(od(), "tag"),
                str_lit("Err"),
            ),
            bynk_ts::TsStmt::throw_stmt(
                call(
                    ident("__rehydrationViolation"),
                    vec![str_lit(names.agent_name), member(od(), "error")],
                ),
                None,
            ),
            None,
        ));
        body.push(const_(&old_js, member(od(), "value")));
    }
}

/// The step's items inside its `try`: each `let` and each assignment's hoisted statements and
/// value, ending in `__out = { ...__c, f: v, .. } as <State>;`. Every assignment reads the carried
/// state, never another assignment (record-literal semantics).
fn migrate_step_try_body(
    names: &MigrateNames<'_>,
    step: &bynk_syntax::ast::MigrateDecl,
    cx: &mut LowerCtx,
) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = Vec::new();
    let mut entries = vec![bynk_ts::TsObjectEntry::Spread(ident("__c"))];
    for item in &step.items {
        match item {
            bynk_syntax::ast::MigrateItem::Let(l) => {
                let mut pre = Pre::new();
                let v = pre.lower_node(&l.value, cx);
                stmts.extend(pre.ts_stmts());
                let js = cx.bind_local_name(&l.name.name);
                stmts.push(const_(&js, v));
            }
            bynk_syntax::ast::MigrateItem::Assign(asg) => {
                let mut pre = Pre::new();
                let v = pre.lower_node(&asg.value, cx);
                stmts.extend(pre.ts_stmts());
                let n = &asg.target.name;
                let v = if names.map_names.contains(n) {
                    // A `Map` value is a `ReadonlyMap`; a store `Map` is a handle keyed by
                    // `String(k)`, built fresh from the zero state so it carries its
                    // `@indexed` indexers.
                    let fill = bynk_ts::TsStmt::for_of(
                        "[__k, __v]",
                        v,
                        bynk_ts::TsStmt::expr_stmt(
                            call(
                                member(ident("__h"), "put"),
                                vec![
                                    call(persist_global("String"), vec![ident("__k")]),
                                    ident("__v"),
                                ],
                            ),
                            None,
                        ),
                        None,
                    );
                    let handle = bynk_ts::TsExpr::Arrow {
                        params: vec![bynk_ts::TsParam::new("__h".to_string(), None)],
                        is_async: false,
                        generics: Vec::new(),
                        return_type: None,
                        body: Box::new(bynk_ts::TsArrowBody::Block(vec![
                            fill,
                            bynk_ts::TsStmt::return_stmt(Some(ident("__h")), None),
                        ])),
                    };
                    call(
                        bynk_ts::TsExpr::Paren(Box::new(handle)),
                        vec![member(call(ident(names.zero_fn), Vec::new()), n)],
                    )
                } else {
                    v
                };
                entries.push(bynk_ts::TsObjectEntry::prop(n.clone(), v));
            }
            bynk_syntax::ast::MigrateItem::Drop(_) => {}
        }
    }
    stmts.push(bynk_ts::TsStmt::assign(
        ident("__out"),
        as_expr(
            bynk_ts::TsExpr::object_entries(entries),
            bynk_ts::TsType::named(names.state_ty),
        ),
        None,
    ));
    stmts
}

/// `function <step_fn>(stored: Record<string, unknown>): Record<string, unknown>`: one `migrate
/// from N` step, carrying a stored (wire-shaped) record from version N to the current shape.
pub(super) fn migration_step_decl(
    names: &MigrateNames<'_>,
    step_fn: &str,
    plan: &bynk_check::schema_registry::MigrationStep,
    step: &bynk_syntax::ast::MigrateDecl,
    env: (&TypedCommons, &EmitProjectCtx),
    cx: &mut LowerCtx,
) -> bynk_ts::TsStmt {
    let state_ty = || bynk_ts::TsType::named(names.state_ty);
    let has_own = |key: bynk_ts::TsExpr| {
        call(
            persist_global("Object.prototype.hasOwnProperty.call"),
            vec![ident("stored"), key],
        )
    };
    let mut body = Vec::new();
    // 1. Decode the old fields through the old type's own codec.
    migrate_decode_old(names, plan, step, env, cx, &mut body);
    // 2. The carried state: version N's fields the current shape keeps unchanged, decoded as
    //    today; every other field at its zero.
    body.push(empty_record_const("__picked"));
    body.push(copy_stored_keys(
        hist_str_array(&plan.carried),
        has_own(ident("__k")),
        "__picked",
    ));
    body.push(bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__c".to_string()),
        None,
        as_expr(
            bynk_ts::TsExpr::object_entries(vec![
                bynk_ts::TsObjectEntry::Spread(call(ident(names.zero_fn), Vec::new())),
                bynk_ts::TsObjectEntry::Spread(ident("__picked")),
            ]),
            state_ty(),
        ),
        None,
    ));
    for f in [names.rehydrate_fn, names.hydrate_fn].into_iter().flatten() {
        body.push(bynk_ts::TsStmt::expr_stmt(
            call(
                ident(f),
                vec![ident("__c"), persist_recast(ident("__picked"), state_ty())],
            ),
            None,
        ));
    }
    // 3. The body. A throw is `MigrationFailed`, never a bare 500 (Decision D).
    body.push(let_decl("__out", Some(state_ty()), None));
    body.push(bynk_ts::TsStmt::try_catch(
        bynk_ts::TsStmt::block(migrate_step_try_body(names, step, cx), None),
        None::<String>,
        bynk_ts::TsStmt::block(
            vec![bynk_ts::TsStmt::throw_stmt(
                call(
                    ident("__migrationFailed"),
                    vec![str_lit(names.agent_name), num_lit(plan.from)],
                ),
                None,
            )],
            None,
        ),
        None,
    ));
    // 4. Back to the wire, keeping any key version N did not know.
    let encoded = match names.encode_fn {
        Some(f) => call(ident(f), vec![ident("__out")]),
        None => bynk_ts::TsExpr::Paren(Box::new(persist_recast(
            ident("__out"),
            persist_record_ty(),
        ))),
    };
    body.push(const_(
        "__known",
        bynk_ts::TsExpr::New {
            callee: Box::new(bynk_ts::TsExpr::TypeApp {
                callee: Box::new(persist_global("Set")),
                type_args: vec![bynk_ts::TsType::named("string")],
            }),
            args: vec![hist_str_array(&plan.known)],
        },
    ));
    body.push(empty_record_const("__keep"));
    body.push(copy_stored_keys(
        call(persist_global("Object.keys"), vec![ident("stored")]),
        binary(
            bynk_ts::TsBinaryOp::And,
            binary(
                bynk_ts::TsBinaryOp::StrictNotEq,
                ident("__k"),
                str_lit("__v"),
            ),
            bynk_ts::TsExpr::Unary {
                op: bynk_ts::TsUnaryOp::Not,
                expr: Box::new(call(member(ident("__known"), "has"), vec![ident("__k")])),
            },
        ),
        "__keep",
    ));
    body.push(bynk_ts::TsStmt::return_stmt(
        Some(bynk_ts::TsExpr::object_entries(vec![
            bynk_ts::TsObjectEntry::Spread(ident("__keep")),
            bynk_ts::TsObjectEntry::Spread(encoded),
        ])),
        None,
    ));
    bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Function {
            name: step_fn.to_string(),
            generics: Vec::new(),
            params: vec![bynk_ts::TsParam::new(
                "stored".to_string(),
                Some(wire_record_ty()),
            )],
            return_type: Some(wire_record_ty()),
            body,
            is_async: false,
            inline: false,
        },
        None,
    )
}

/// 1A.32c-1 review: no golden fixture reaches an agent's `migrate from N`
/// dispatcher, so this pins the node form against the text the old `writeln!`
/// built (`format!("function {migrate_fn}(stored: ...) {{\n  const __v = ...;\n{dispatch}  return stored;\n}}\n")`),
/// byte for byte, for a dispatcher with two steps.
#[cfg(test)]
mod migrate_dispatcher_tests {
    use super::{migrate_dispatcher_decl, migrate_step_dispatch};

    #[test]
    fn the_migration_dispatcher_prints_as_the_old_text_did() {
        let decl = migrate_dispatcher_decl(
            "__migrateCounterState",
            vec![
                migrate_step_dispatch(1, 1, "__migrateCounterFrom1"),
                migrate_step_dispatch(2, 3, "__migrateCounterFrom2"),
            ],
        );
        let old = "function __migrateCounterState(stored: globalThis.Record<string, unknown>): globalThis.Record<string, unknown> {\n  const __v = stored[\"__v\"] === undefined ? 1 : stored[\"__v\"] as number;\n  if (__v >= 1 && __v <= 1) return __migrateCounterFrom1(stored);\n  if (__v >= 2 && __v <= 3) return __migrateCounterFrom2(stored);\n  return stored;\n}\n";
        assert_eq!(bynk_ts::print_stmt(&decl, 0), old);
    }
}
