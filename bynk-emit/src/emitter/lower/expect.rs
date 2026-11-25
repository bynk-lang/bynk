//! Test-domain expressions (back-half paydown 1A.17, 2.2b, #4087): `expect`, fault claims
//! (`<call> faults`), recorded calls, `trace` and observations. Moved out of `lower.rs` byte for
//! byte.

use bynk_check::actors::CallSitePrincipal;
use bynk_syntax::ast::{Expr, FaultsExpr, ObservationExpr, ObservationMatcher};

use super::super::lower_ctx::LowerCtx;
use super::{
    Pre, arrow_call, call_site_principal, cap_op_param_names, lower_expr, member_node, one, paren,
};
use crate::ts_build::{binary, ident, num_lit, str_lit};

/// v0.59: the `location` string an `expect` failure carries. With a test-body
/// [`AssertLoc`](crate::emitter::ctx::AssertLoc) in scope it is a real, escaped
/// `path:line:col` (so `--format json` consumers can link to the source);
/// otherwise it falls back to the bare byte offset (`expect`s only appear in test
/// bodies, so the fallback is defensive).
pub(in crate::emitter) fn expect_location(cx: &LowerCtx, offset: usize) -> String {
    crate::emitter::escape_ts_string(&expect_location_raw(cx, offset))
}

/// [`expect_location`] before escaping, for a `Str` node (the printer escapes
/// it with the same one function, `escape_ts_literal`).
pub(in crate::emitter) fn expect_location_raw(cx: &LowerCtx, offset: usize) -> String {
    match cx.assert_loc() {
        Some(loc) => {
            let (line, col) = bynk_syntax::span::line_col(&loc.source, offset);
            // Normalise to forward slashes so the location is identical on
            // Windows (where `PathBuf` joins with `\`) — matching the
            // diagnostic path rendering and the committed goldens.
            let path = loc.rel_path.replace('\\', "/");
            format!("{path}:{line}:{col}")
        }
        None => format!("offset {offset}"),
    }
}

/// v0.112: the trimmed source text of an `expect` predicate, embedded in the
/// structural failure report (`expect <src>`), unescaped: it goes into a `Str`
/// node (the printer escapes it).
/// Empty when no test-body source is in scope (defensive — `expect`s only appear
/// in test bodies).
pub(in crate::emitter) fn expect_source_text_raw(
    cx: &LowerCtx,
    span: bynk_syntax::span::Span,
) -> String {
    match cx.assert_loc() {
        Some(loc) => loc
            .source
            .get(span.start..span.end)
            .unwrap_or("")
            .trim()
            .to_string(),
        None => String::new(),
    }
}

/// v0.9.1: expect as an expression (v0.112, renamed from `assert`).
/// Emit a runtime helper call that returns void (i.e., evaluates to
/// `undefined` at runtime and is treated as the unit value `()` in Bynk
/// terms). The expression form reports the predicate source only — the
/// statement form carries the structural expected-vs-actual report.
pub(super) fn expect_built(inner: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let value = pre.lower_node(inner, cx);
    let span_start = inner.span.start;
    let span_end = inner.span.end;
    let location = expect_location_raw(cx, span_start);
    let mut src = format!("expect {}", expect_source_text_raw(cx, inner.span));
    // #3980: a failed `is`, or an `expect false` that is a `match` arm, reports the value tested.
    let report = match super::expect_was::subject_report(inner, cx) {
        Some(r) => Some(r),
        None => super::expect_was::arm_report(inner, cx).map(|(head, r)| {
            src = [src.as_str(), " ", head.as_str()].concat();
            r
        }),
    };
    let src_detail = report.as_ref().map_or(src.clone(), |r| r.detail(&src));
    // The `Paren` keeps today's `((value))` bytes; the `Str` nodes
    // escape exactly as `escape_ts_string` does.
    {
        let c = vec![value];
        bynk_ts::TsExpr::Call {
            callee: Box::new(ident("__bynkExpect")),
            args: vec![
                bynk_ts::TsExpr::Paren(Box::new(one(c))),
                str_lit(location.clone()),
                num_lit(span_start.to_string()),
                num_lit(span_end.to_string()),
                str_lit(src_detail),
            ]
            .into_iter()
            .chain(report.iter().flat_map(|r| r.runtime_args()))
            .collect(),
        }
    }
}

/// Lower a fault claim, `<call> faults` (#1706), to a `Bool` JavaScript
/// expression: an async IIFE that awaits the call and yields `true` when it
/// threw. An `ExpectationError` is re-thrown, not counted — a failed `expect`
/// inside the call's own evaluation is the case failing, not the call
/// faulting. Every other throw counts: an injected `stub … fails`, a real
/// provider's failure, an invariant violation. A fault is untyped, so the
/// claim does not discriminate between them.
///
/// The call is lowered as an `<-` subject is, under the claim's own call-site
/// principal (#1812) set exactly as the effect-let arm sets it — the
/// identity the addressed handler reads, or `by Nobody`'s no-credential
/// driver — and never an enclosing one. Whatever the call (or the identity)
/// hoists is placed inside the `try`, not in the enclosing statement's
/// prelude: a fault raised while evaluating the call's arguments is the call
/// faulting too, and must not escape the claim.
pub(super) fn lower_faults(
    faults: &FaultsExpr,
    span: bynk_syntax::span::Span,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    let FaultsExpr {
        call,
        principal,
        violates,
    } = faults;
    let saved_identity = cx.call_site_identity.take();
    let saved_actor = cx.call_site_actor.take();
    let saved_no_credential = std::mem::replace(&mut cx.call_site_no_credential, false);
    let saved_bad_signature = cx.call_site_bad_signature.take();
    let mut inner = Pre::new();
    if let Some(principal) = principal {
        // `by Nobody … faults` (and #2135 `by Forged`/`by Stale`) is
        // rejected before emit (`bynk.test.faults_needs_in_process`);
        // mirrored here so the claim lowers as the effect-let arm does
        // regardless.
        let kind = call_site_principal(cx, principal);
        if kind == CallSitePrincipal::Nobody {
            cx.call_site_no_credential = true;
        } else if let CallSitePrincipal::BadCredential(bad) = kind {
            cx.call_site_bad_signature = Some(bad.driver());
        } else {
            cx.call_site_identity = principal
                .identity
                .as_ref()
                .map(|id| inner.lower_node(id, cx));
            cx.call_site_actor = Some(principal.actor.name.clone());
        }
    }
    let value = inner.lower_node(call, cx);
    cx.call_site_identity = saved_identity;
    cx.call_site_actor = saved_actor;
    cx.call_site_no_credential = saved_no_credential;
    cx.call_site_bad_signature = saved_bad_signature;
    // The literal `await` below is a real source of effectfulness for any
    // synchronous-looking IIFE further out — see `emitted_await`.
    cx.emitted_await = true;
    // 1A.17, Decision F: an async arrow IIFE over a `try`/`catch`.
    {
        let c = vec![value];
        use bynk_ts::{TsBinaryOp, TsExpr, TsLit, TsStmt};
        let mut try_body = inner.ts_stmts();
        try_body.push(TsStmt::expr_stmt(
            TsExpr::Await(Box::new(TsExpr::Paren(Box::new(one(c))))),
            None,
        ));
        // #4035: a `violates` claim reports a call that returned itself, with the claim's text.
        let violates_args = violates.as_ref().map(|path| {
            let name = path
                .iter()
                .map(|i| i.name.as_str())
                .collect::<Vec<_>>()
                .join(".");
            (
                name,
                expect_location_raw(cx, span.start),
                format!("expect {}", expect_source_text_raw(cx, span)),
            )
        });
        match &violates_args {
            Some((_, location, src)) => try_body.push(TsStmt::expr_stmt(
                TsExpr::Call {
                    callee: Box::new(ident("__bynkViolatesNone")),
                    args: vec![
                        str_lit(location.clone()),
                        num_lit(span.start.to_string()),
                        num_lit(span.end.to_string()),
                        str_lit(src.clone()),
                    ],
                },
                None,
            )),
            None => try_body.push(TsStmt::return_stmt(
                Some(TsExpr::Lit(TsLit::Bool(false))),
                None,
            )),
        }
        let rethrow = TsStmt::if_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::InstanceOf,
                left: Box::new(ident("__e")),
                right: Box::new(ident("ExpectationError")),
            },
            TsStmt::block(vec![TsStmt::throw_stmt(ident("__e"), None)], None),
            None,
        );
        let caught = match violates_args {
            Some((name, location, src)) => TsExpr::Call {
                callee: Box::new(ident("__bynkViolates")),
                args: vec![
                    ident("__e"),
                    str_lit(name),
                    str_lit(location),
                    num_lit(span.start.to_string()),
                    num_lit(span.end.to_string()),
                    str_lit(src),
                ],
            },
            None => TsExpr::Lit(TsLit::Bool(true)),
        };
        let catch_body = vec![rethrow, TsStmt::return_stmt(Some(caught), None)];
        let try_catch = TsStmt::try_catch(
            TsStmt::block(try_body, None),
            Some("__e"),
            TsStmt::block(catch_body, None),
            None,
        );
        TsExpr::Paren(Box::new(arrow_call(
            Vec::new(),
            Vec::new(),
            true,
            vec![try_catch],
        )))
    }
}

/// `(<obs>.log["Cap.op"] ?? [])`: the calls recorded for one capability
/// operation (v0.117). `obs` is the per-case trace object, always `__obs`.
fn recorded_calls(cx: &LowerCtx, cap: &str, op: &str) -> bynk_ts::TsExpr {
    use bynk_ts::{TsBinaryOp, TsExpr};
    let obs = cx.observation_trace().unwrap_or("__obs");
    TsExpr::Paren(Box::new(TsExpr::Binary {
        op: TsBinaryOp::NullishCoalescing,
        left: Box::new(TsExpr::Index {
            object: Box::new(member_node(obs, "log")),
            index: Box::new(str_lit(format!("{cap}.{op}"))),
        }),
        right: Box::new(TsExpr::array(Vec::new())),
    }))
}

/// #3945: the recorded calls of one generic operation at the type arguments an observation
/// names (`Cap.dedup[Outcome] was called`): `<calls>.filter(__c => __c.typeKey === "[Outcome]")`.
/// An observation without type arguments counts every call, whatever it was made at.
fn keyed_calls(
    calls: bynk_ts::TsExpr,
    type_args: &[bynk_syntax::ast::TypeRef],
    cx: &LowerCtx,
) -> bynk_ts::TsExpr {
    use bynk_ts::{TsArrowBody, TsExpr, TsParam};
    if type_args.is_empty() {
        return calls;
    }
    let recorded = TsExpr::Member {
        object: Box::new(TsExpr::Paren(Box::new(TsExpr::As {
            expr: Box::new(bynk_ts::TsExpr::Ident("__c".to_string())),
            ty: bynk_ts::TsType::Object(vec![bynk_ts::TsTypeMember::Prop {
                name: "typeKey".to_string(),
                ty: bynk_ts::TsType::named("string"),
                optional: true,
                readonly: false,
            }]),
        }))),
        property: "typeKey".to_string(),
    };
    // #4382: a call keyed by a longer resolved list that starts with the named arguments counts.
    let test = crate::ts_build::type_key_matches(
        recorded,
        &crate::emitter::lower_call::platform::type_key_of(type_args, cx),
    );
    TsExpr::Paren(Box::new(TsExpr::Call {
        callee: Box::new(TsExpr::Member {
            object: Box::new(calls),
            property: "filter".to_string(),
        }),
        args: vec![TsExpr::Arrow {
            params: vec![TsParam::new("__c".to_string(), None)],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(test))),
        }],
    }))
}

/// `{ args: unknown[] }`, the recorded call's shape as far as `.args` is read
/// positionally (P7.2).
fn call_args_type() -> bynk_ts::TsType {
    use bynk_ts::{TsType, TsTypeMember};
    TsType::Object(vec![TsTypeMember::prop(
        "args",
        TsType::array(TsType::named("unknown")),
    )])
}

/// `{ n: __c.args[0], … }`: the operation's parameters read positionally off a
/// recorded call.
fn call_fields(names: &[String]) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::object(
        names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                (
                    n.clone(),
                    bynk_ts::TsExpr::Index {
                        object: Box::new(member_node("__c", "args")),
                        index: Box::new(num_lit(i.to_string())),
                    },
                )
            })
            .collect(),
    )
}

/// `trace(Cap.op)` (v0.117): the recorded calls mapped to per-call records.
pub(super) fn trace_node(cx: &LowerCtx, cap: &str, op: &str) -> bynk_ts::TsExpr {
    use bynk_ts::{TsArrowBody, TsExpr, TsParam, TsType};
    let names = cap_op_param_names(cx, cap, op);
    // P7.2: only `.args` is read below (positionally), so `unknown[]` is
    // what's actually known about a recorded call's arguments here — but
    // the mapped-out `{ msg: unknown, … }` shape then flows into whatever
    // this `trace()` result feeds (a test's own `.filter`/projection,
    // typed against the checker's own per-op record type), which needs
    // the real field types, not `unknown`. Asserted into
    // `observation_call_record_types`' own generated alias (same name
    // `checker::call_record_type_name` derives, same test module, so no
    // qualification needed) — legal under `tsc --strict` because every
    // field of that alias is structurally assignable to `unknown`.
    let record_ty = bynk_check::checker::call_record_type_name(cap, op);
    let map = TsExpr::Call {
        callee: Box::new(TsExpr::Member {
            object: Box::new(recorded_calls(cx, cap, op)),
            property: "map".to_string(),
        }),
        args: vec![TsExpr::Arrow {
            params: vec![TsParam::new("__c".to_string(), Some(call_args_type()))],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(call_fields(&names)))),
        }],
    };
    TsExpr::Paren(Box::new(TsExpr::As {
        expr: Box::new(TsExpr::Paren(Box::new(map))),
        ty: TsType::array(TsType::named(record_ty)),
    }))
}

/// Lower an observation (v0.117) to a `Bool` JavaScript expression over the
/// recorded trace object. `total`/`matching` counts drive the sugar; `before`
/// compares call order.
pub(super) fn lower_observation(o: &ObservationExpr, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    use bynk_ts::{TsBinaryOp, TsExpr};
    let calls = keyed_calls(
        recorded_calls(cx, &o.cap.name, &o.op.name),
        &o.type_args,
        cx,
    );
    let length = |calls: &TsExpr| TsExpr::Member {
        object: Box::new(calls.clone()),
        property: "length".to_string(),
    };
    // The text twin of `recorded_calls`, for the fallbacks below.
    match &o.matcher {
        ObservationMatcher::NeverCalled => paren(binary(
            TsBinaryOp::StrictEq,
            length(&calls),
            num_lit("0".to_string()),
        )),
        ObservationMatcher::Before { cap, op } => {
            let calls2 = recorded_calls(cx, &cap.name, &op.name);
            let order = |calls: &TsExpr| TsExpr::Member {
                object: Box::new(TsExpr::Index {
                    object: Box::new(calls.clone()),
                    index: Box::new(num_lit("0".to_string())),
                }),
                property: "order".to_string(),
            };
            let zero = || num_lit("0".to_string());
            let both = binary(
                TsBinaryOp::And,
                binary(TsBinaryOp::GreaterThan, length(&calls), zero()),
                binary(TsBinaryOp::GreaterThan, length(&calls2), zero()),
            );
            paren(binary(
                TsBinaryOp::And,
                both,
                binary(TsBinaryOp::LessThan, order(&calls), order(&calls2)),
            ))
        }
        ObservationMatcher::Called { count, with_pred } => match with_pred {
            None => match count {
                None => paren(binary(
                    TsBinaryOp::GreaterThanEq,
                    length(&calls),
                    num_lit("1".to_string()),
                )),
                Some(c) => {
                    let n = lower_expr(c, cx).expr;
                    {
                        let c = vec![n];
                        paren(binary(TsBinaryOp::StrictEq, length(&calls), paren(one(c))))
                    }
                }
            },
            Some(p) => {
                let names = cap_op_param_names(cx, &o.cap.name, &o.op.name);
                let record_ty = bynk_check::checker::call_record_type_name(&o.cap.name, &o.op.name);
                let pred_lowered = lower_expr(p, cx);
                // The predicate's hoisted statements go in the arrow body.
                let pre_stmts: Vec<bynk_ts::TsStmt> = pred_lowered.pre.into_iter().collect();
                let mut children = vec![pred_lowered.expr];
                if let Some(c) = count {
                    children.push(lower_expr(c, cx).expr);
                }
                let has_count = count.is_some();
                {
                    let mut c = children;
                    let n = has_count.then(|| c.pop()).flatten();
                    let pred = one(c);
                    let mut stmts = Vec::new();
                    // #291: typed through the operation's call-record alias (as
                    // `trace` is), so a predicate may read a record parameter's
                    // fields (`with req.url == …`) — `__c.args` is `unknown[]`.
                    if !names.is_empty() {
                        stmts.push(bynk_ts::TsStmt::const_stmt(
                            bynk_ts::TsBindingName::ObjectPattern(names.clone()),
                            None,
                            TsExpr::As {
                                expr: Box::new(call_fields(&names)),
                                ty: bynk_ts::TsType::named(record_ty.clone()),
                            },
                            None,
                        ));
                    }
                    stmts.extend(pre_stmts);
                    stmts.push(bynk_ts::TsStmt::return_stmt(Some(paren(pred)), None));
                    // P7.2: same shape as `trace` — only `.args` is destructured,
                    // so `unknown[]` is what's actually known.
                    let matching = TsExpr::Member {
                        object: Box::new(TsExpr::Call {
                            callee: Box::new(TsExpr::Member {
                                object: Box::new(calls.clone()),
                                property: "filter".to_string(),
                            }),
                            args: vec![TsExpr::Arrow {
                                params: vec![bynk_ts::TsParam::new(
                                    "__c".to_string(),
                                    Some(call_args_type()),
                                )],
                                is_async: false,
                                generics: Vec::new(),
                                return_type: None,
                                body: Box::new(bynk_ts::TsArrowBody::Multiline(stmts)),
                            }],
                        }),
                        property: "length".to_string(),
                    };
                    match n {
                        None => paren(binary(
                            TsBinaryOp::GreaterThanEq,
                            paren(matching),
                            num_lit("1".to_string()),
                        )),
                        Some(n) => paren(binary(TsBinaryOp::StrictEq, paren(matching), paren(n))),
                    }
                }
            }
        },
    }
}
