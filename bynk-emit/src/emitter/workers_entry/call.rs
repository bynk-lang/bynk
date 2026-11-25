//! The internal `/_bynk/call/` dispatch: the verify, admission, method and wire-version gates,
//! and the per-handler dispatch of an `on call` service.

use super::respond::{
    case_, check_kind, check_status, default_case, json_error_kind, json_response, json_stringify,
    miss, new_response, own_path_guard, set_fault_route, status_json_headers_obj, text_response,
    try_catch_stmt,
};
use super::wire::{deserialise_call, serialise_call, wire_ident};
use crate::emitter::RuntimeUse;
use crate::ts_build::{
    and_expr, as_expr, await_expr, block, call, cond_expr, const_, const_typed, expr_stmt, ident,
    if_, index, let_typed, member, method_call, null_lit, num_lit, or_expr, paren, return_,
    str_lit, strict_eq, strict_neq, switch_stmt,
};
use bynk_check::symbols::UnitTable;
use bynk_check::wire_format::{self as wf, CallCheck};
use bynk_syntax::ast::Handler;
use bynk_ts::{TsExpr, TsStmt, TsSwitchCase, TsType, TsTypeMember};

/// I2 (#1971): `const __body = await __verifyCall(request, env, "<context>");
/// if (typeof __body !== "string") return __body;`. Every internal dispatch
/// (`/_bynk/call/`, `/_bynk/event/`) makes it first, after only the call
/// dispatch's method check.
///
/// The runtime reads the body under the published cap (`413`), then checks
/// the request's signature under the key this Worker holds for the edge from
/// the claimed caller, bound to `context` as the callee (`401`), and hands back
/// the verified body as text. The prefixes are reachable from the public
/// internet on any route the Worker serves (ADR 0092's "not externally
/// routable" premise does not hold), so a request is refused here, before the
/// service is looked up, the contract compared or the body parsed: an
/// unauthenticated sender learns nothing about which services exist (no 404
/// against 409), never has a hash echoed, and cannot write the skew log line.
/// The refusals are bare words, not JSON detail.
///
/// I3 (#1971): a Worker whose `on call` handlers admit foreign callers passes
/// its table of them (`__FOREIGN_CALLERS`, actor name → secret), so a call
/// naming one is checked under that actor's secret rather than an edge key.
pub(super) fn verify_gate(context: &str, foreign: bool) -> [TsStmt; 2] {
    let mut args = vec![ident("request"), ident("env"), str_lit(context)];
    if foreign {
        args.push(ident(FOREIGN_CALLERS));
    }
    [
        const_("__body", await_expr(call(ident("__verifyCall"), args))),
        if_(
            strict_neq(
                TsExpr::Unary {
                    op: bynk_ts::TsUnaryOp::Typeof,
                    expr: Box::new(ident("__body")),
                },
                str_lit("string"),
            ),
            return_(Some(ident("__body"))),
        ),
    ]
}

/// I3 (#1971): the per-Worker table of the foreign callers its `on call`
/// handlers admit, actor name → the secret their calls verify under.
pub(super) const FOREIGN_CALLERS: &str = "__FOREIGN_CALLERS";

/// #3890: a `Caller` refinement's predicate as a TypeScript boolean over the
/// proven caller name.
pub(super) fn caller_predicate_expr(
    pred: &bynk_check::actors::CallerPredicate,
    from: &TsExpr,
) -> TsExpr {
    use bynk_check::actors::CallerPredicate as P;
    match pred {
        P::IsContext(names) => names
            .iter()
            .map(|n| strict_eq(from.clone(), str_lit(n.clone())))
            .reduce(or_expr)
            .unwrap_or_else(|| ident("false")),
        P::And(l, r) => and_expr(
            paren(caller_predicate_expr(l, from)),
            paren(caller_predicate_expr(r, from)),
        ),
        P::Or(l, r) => or_expr(
            paren(caller_predicate_expr(l, from)),
            paren(caller_predicate_expr(r, from)),
        ),
        P::Not(inner) => TsExpr::Unary {
            op: bynk_ts::TsUnaryOp::Not,
            expr: Box::new(paren(caller_predicate_expr(inner, from))),
        },
    }
}

/// I3 (#1971): the published admission check for one `on call` handler, made
/// after the service is found and before its contract is compared: the
/// caller the signature proved must be one the handler's `by` clause admits.
/// A Bynk context is admitted by `Caller`, a foreign caller only by its own
/// actor, never by `Caller`; anyone else gets a bare `403` (authenticated, not
/// admitted). For a sum it then builds the resolved actor, `__who`.
///
/// `has_foreign` is whether this Worker holds a table of foreign callers
/// (`__FOREIGN_CALLERS`); without one no caller is foreign, and the gate is
/// only emitted for a handler a `Caller` refinement narrows (#3890).
fn admission_gate(admission: &bynk_check::actors::CallAdmission, has_foreign: bool) -> Vec<TsStmt> {
    let from = || ident("__from");
    let mut admitted: Option<TsExpr> = None;
    let mut push = |e: TsExpr| {
        admitted = Some(match admitted.take() {
            Some(a) => or_expr(a, e),
            None => e,
        });
    };
    if admission.contexts {
        let not_foreign = has_foreign.then(|| TsExpr::Unary {
            op: bynk_ts::TsUnaryOp::Not,
            expr: Box::new(ident("__foreign")),
        });
        // #3890: a refinement over `Caller` admits only the contexts its
        // predicate names — the second line of defence behind the compile-time
        // call-edge check, since signing keys are per context pair.
        push(match (&admission.caller_predicate, not_foreign) {
            (Some(pred), Some(nf)) => and_expr(nf, paren(caller_predicate_expr(pred, &from()))),
            (Some(pred), None) => caller_predicate_expr(pred, &from()),
            (None, Some(nf)) => nf,
            (None, None) => ident("true"),
        });
    }
    for f in &admission.foreign {
        push(strict_eq(from(), str_lit(f.actor.clone())));
    }
    let admitted = admitted.unwrap_or_else(|| ident("false"));
    let mut stmts = vec![const_(
        "__from",
        as_expr(
            method_call(
                member(ident("request"), "headers"),
                "get",
                vec![str_lit(wf::CALLER_HEADER)],
            ),
            TsType::named("string"),
        ),
    )];
    // Whether the caller is a foreign one, read only where a context is
    // admitted too.
    if admission.contexts && has_foreign {
        stmts.push(const_(
            "__foreign",
            call(
                ident("globalThis.Object.hasOwn"),
                vec![ident(FOREIGN_CALLERS), from()],
            ),
        ));
    }
    stmts.push(if_(
        TsExpr::Unary {
            op: bynk_ts::TsUnaryOp::Not,
            expr: Box::new(paren(admitted)),
        },
        return_(Some(text_response(
            CallCheck::NotAdmitted.text_body().unwrap_or_default(),
            check_status(CallCheck::NotAdmitted),
        ))),
    ));
    if let Some(members) = &admission.sum {
        // The members in `by` order: a foreign one by its name, `Caller` for
        // any context. Admission above means one of them matches.
        let mut who = TsExpr::object(vec![
            ("tag".to_string(), str_lit(admission.internal_tag())),
            ("identity".to_string(), from()),
        ]);
        for m in members.iter().rev() {
            if let bynk_check::actors::CallSumMember::Foreign(f) = m {
                who = cond_expr(
                    strict_eq(from(), str_lit(f.actor.clone())),
                    TsExpr::object(vec![("tag".to_string(), str_lit(f.actor.clone()))]),
                    who,
                );
            }
        }
        stmts.push(const_typed(
            "__who",
            crate::emitter::emit::call_sum_ts_type(members),
            who,
        ));
    }
    stmts
}

/// #1969 (wire format v1): `if (method !== "POST") return new Response(null,
/// { status: 405, headers: { allow: "POST" } });`, the first statement of the
/// `/_bynk/call/` dispatch. A call is only ever a `POST`; before this, a `GET`
/// reached the body read and failed there as a `500`.
pub(super) fn call_method_gate() -> TsStmt {
    if_(
        strict_neq(ident("method"), str_lit(wf::CALL_METHOD)),
        return_(Some(new_response(vec![
            null_lit(),
            TsExpr::object(vec![
                (
                    "status".to_string(),
                    num_lit(check_status(CallCheck::Method).to_string()),
                ),
                (
                    "headers".to_string(),
                    TsExpr::object(vec![("allow".to_string(), str_lit(wf::CALL_METHOD))]),
                ),
            ]),
        ]))),
    )
}

/// #1969 (wire format v1): `const __wire = request.headers.get("X-Bynk-Wire");
/// if (__wire !== null && __wire !== "1") return <400 UnsupportedWireVersion>;`.
///
/// After the signature, so an unauthenticated sender cannot probe which
/// versions a callee speaks; before the service lookup, because the version
/// decides how everything after it reads. An absent header is version 1. The
/// refusal names the versions this callee accepts, which is how a caller
/// learns them.
fn wire_version_gate() -> Vec<TsStmt> {
    let mut refused = strict_neq(ident("__wire"), null_lit());
    for v in wf::ACCEPTED_WIRE_VERSIONS {
        refused = and_expr(refused, strict_neq(ident("__wire"), str_lit(v.to_string())));
    }
    vec![
        const_(
            "__wire",
            method_call(
                member(ident("request"), "headers"),
                "get",
                vec![str_lit(wf::WIRE_HEADER)],
            ),
        ),
        if_(
            refused,
            return_(Some(json_response(
                json_error_kind(
                    check_kind(CallCheck::WireVersion),
                    vec![(
                        "accepted".to_string(),
                        TsExpr::array(
                            wf::ACCEPTED_WIRE_VERSIONS
                                .iter()
                                .map(|v| num_lit(v.to_string()))
                                .collect(),
                        ),
                    )],
                ),
                check_status(CallCheck::WireVersion),
            ))),
        ),
    ]
}

/// The `/_bynk/call/` dispatch: the published wire format's checks
/// (`bynk_check::wire_format::CALL_CHECKS`) in their published order: method,
/// body size and signature (I2), wire version, then (per service) the service,
/// its contract, the body's JSON and the arguments.
///
/// A member of a Worker group (`member_mode`) answers only the services it declares,
/// and tests that **before** any gate: the gates read the body and verify the
/// signature for this member's context, so a member that missed would otherwise
/// refuse a call signed for another member. A call to a service no member
/// declares is therefore a miss, answered by the group's own `404`. A member
/// with no `on call` service has no such block.
pub(super) fn call_dispatch(
    context: &str,
    member_mode: bool,
    table: &UnitTable,
    service_names: &[&String],
    (own_contracts, accepted_contracts): (
        &std::collections::BTreeMap<String, String>,
        &std::collections::BTreeMap<String, Vec<String>>,
    ),
    has_foreign: bool,
    runtime_use: &RuntimeUse,
) -> Option<TsStmt> {
    let mut call_cases: Vec<TsSwitchCase> = Vec::new();
    let mut call_names: Vec<String> = Vec::new();
    for sname in service_names {
        let service = table.services.get(*sname).unwrap();
        let Some(h) = service.handlers.iter().find(|h| {
            matches!(
                bynk_model::lower_handler_kind_ir(&h.kind),
                bynk_model::IrHandlerKind::Call
            )
        }) else {
            continue;
        };

        // #1825: names the dispatch for the fault log below.
        let mut case_body: Vec<TsStmt> = vec![set_fault_route(format!("call {sname}"))];
        if let Some(admission) = bynk_check::actors::call_admission_for(h, &table.actors)
            && (has_foreign || admission.caller_predicate.is_some())
        {
            case_body.extend(admission_gate(&admission, has_foreign));
        }
        // v0.177 (#643): the deploy-skew check runs **before the body is read**.
        //
        // The caller stamps a hash of its compiled view of this contract; this
        // context stamps its own. They are constants in two separately-deployed
        // artifacts, frozen at two different deploy times — so a mismatch means
        // the deployed callee is not the one the caller was compiled against,
        // which is exactly what `deploy --context NAME` makes possible.
        //
        // First, ahead of even parsing the payload: once the contracts disagree
        // the body's *interpretation* is the thing in doubt, so validating it
        // would report a misleading `StructuralMismatch` for the real fault —
        // and there is no reason to parse a body that is already refused.
        //
        // `409` rather than `400`: the payload is not malformed, and the caller
        // cannot fix it by sending different bytes — the two deployments
        // conflict.
        //
        // An absent header fails closed. A Bynk caller always stamps one, so its
        // absence means a non-Bynk or pre-upgrade caller — and a pre-upgrade
        // caller is a skewed caller by definition. This departs from ADR 0092's
        // conditional posture (a missing `X-Bynk-Caller` fail-closes only on a
        // `by c: Caller` handler) because there is no binder to condition on:
        // identity matters only when read, but the contract always matters. It
        // therefore also precedes the caller check — if the contracts disagree,
        // nothing about the request is trustworthy, including the identity.
        if let Some(expected) = own_contracts.get(*sname) {
            case_body.push(const_(
                "__contract",
                method_call(
                    member(ident("request"), "headers"),
                    "get",
                    vec![str_lit(wf::CONTRACT_HEADER)],
                ),
            ));
            // #1826: the callee decides, so it logs the skew, by name and with
            // both hashes, before refusing: otherwise the diagnosis existed only
            // in a response body passed between two Workers.
            let skew = |actual: TsExpr| {
                vec![
                    ("service".to_string(), str_lit(sname.to_string())),
                    ("expected".to_string(), str_lit(expected.clone())),
                    ("actual".to_string(), actual),
                ]
            };
            // #1826 review: `/_bynk/call/` is reachable from outside, and
            // nothing about the request is trusted yet, so the header is
            // logged bounded to a hash's 16 characters: an operator loses
            // nothing, and a sender can't write arbitrary text to the log.
            // The 409 body echoes it unbounded, to the sender who sent it.
            let logged_actual = method_call(
                call(ident("globalThis.String"), vec![ident("__contract")]),
                "slice",
                vec![num_lit("0"), num_lit("16")],
            );
            // #2554: refused unless it is this contract's hash or one the
            // lock records this callee as still accepting. The 409 below still
            // names the current hash as the expected one.
            let mismatch = accepted_contracts.get(*sname).into_iter().flatten().fold(
                strict_neq(ident("__contract"), str_lit(expected.clone())),
                |all, hash| and_expr(all, strict_neq(ident("__contract"), str_lit(hash.clone()))),
            );
            case_body.push(if_(
                mismatch,
                block(vec![
                    expr_stmt(method_call(
                        ident("globalThis.console"),
                        "error",
                        vec![
                            str_lit(format!("ContractMismatch {context} call {sname}")),
                            TsExpr::object(skew(logged_actual)),
                        ],
                    )),
                    return_(Some(json_response(
                        json_error_kind(check_kind(CallCheck::Contract), skew(ident("__contract"))),
                        check_status(CallCheck::Contract),
                    ))),
                ]),
            ));
        }
        // #1969 (wire format v1): the body is parsed here, so malformed JSON is
        // the caller's fault (`400 MalformedJson`), not a fault in this Worker
        // (`500`). I2 (#1971): it is the text `__verifyCall` read and verified,
        // so the bytes decoded are the bytes signed.
        case_body.push(let_typed("args", TsType::named("__JsonValue")));
        case_body.push(try_catch_stmt(
            vec![TsStmt::assign(
                ident("args"),
                as_expr(
                    method_call(ident("globalThis.JSON"), "parse", vec![ident("__body")]),
                    TsType::named("__JsonValue"),
                ),
                None,
            )],
            None,
            vec![return_(Some(json_response(
                json_error_kind(
                    check_kind(CallCheck::MalformedJson),
                    vec![("details".to_string(), str_lit(wf::MALFORMED_JSON_DETAILS))],
                ),
                check_status(CallCheck::MalformedJson),
            )))],
        ));
        case_body.extend(emit_call_handler_dispatch(
            sname,
            h,
            &table.actors,
            runtime_use,
        ));
        call_names.push(sname.to_string());
        call_cases.push(case_(str_lit(sname.to_string()), case_body));
    }
    call_cases.push(default_case(vec![return_(Some(miss(
        member_mode,
        text_response(
            CallCheck::UnknownService.text_body().unwrap_or_default(),
            check_status(CallCheck::UnknownService),
        ),
    )))]));
    if member_mode && call_names.is_empty() {
        return None;
    }
    let gates = std::iter::once(call_method_gate())
        .chain(verify_gate(context, has_foreign))
        .chain(wire_version_gate())
        .chain([
            const_(
                "servicePath",
                method_call(
                    ident("path"),
                    "slice",
                    vec![member(str_lit(wf::CALL_PATH_PREFIX), "length")],
                ),
            ),
            switch_stmt(ident("servicePath"), call_cases),
        ])
        .collect();
    Some(if_(
        own_path_guard(member_mode, wf::CALL_PATH_PREFIX, &call_names),
        block(gates),
    ))
}

/// [`wire_ident`] for the `call` dispatch, where `body` is an ordinary
/// parameter and the dispatch's own encoded-result local is called `body`.
fn call_wire_ident(name: &str) -> String {
    if name == "body" {
        "__id_body".to_string()
    } else {
        wire_ident(name)
    }
}

/// Synthesise a deps object literal for invoking a handler from the
/// fetch entry point. Mirrors `compose`'s deps construction so handlers
/// see the same shape whether invoked through the surface or directly.
/// Returns the statements to append to the surrounding `switch` case body —
/// `emit_worker_entry`'s own caller extends the case with them.
fn emit_call_handler_dispatch(
    sname: &str,
    h: &Handler,
    actors: &std::collections::HashMap<String, bynk_syntax::ast::ActorDecl>,
    ru: &RuntimeUse,
) -> Vec<TsStmt> {
    let mut stmts: Vec<TsStmt> = Vec::new();
    // v0.54: a `by c: Caller` handler reads the caller's context name from the
    // `X-Bynk-Caller` header (stamped by `callService`) and threads it into the
    // surface call. I2 (#1971): `__verifyCall` already refused a request whose
    // caller holds no key here, so the header is present, non-empty and
    // proved: there is no separate missing-caller check.
    let has_caller = bynk_check::actors::caller_binder_for(h, actors).is_some();
    // I3 (#1971): a `call` sum's resolved actor, built by `admission_gate`.
    let has_who =
        bynk_check::actors::call_admission_for(h, actors).is_some_and(|a| a.sum.is_some());
    if has_caller {
        stmts.push(const_(
            "__caller",
            as_expr(
                method_call(
                    member(ident("request"), "headers"),
                    "get",
                    vec![str_lit(wf::CALLER_HEADER)],
                ),
                TsType::named("string"),
            ),
        ));
    }

    if h.params.len() == 1 {
        let p = &h.params[0];
        let pname = &p.name.name;
        // The binding target must dodge JS reserved words (`class`, `const`, …)
        // that lex as valid Bynk identifiers; the `__r_` temp is already safe.
        let jname = call_wire_ident(pname);
        let dser_call = deserialise_call(&p.type_ref, &ident("args"), "$", ru);
        stmts.push(const_(format!("__r_{pname}"), dser_call));
        stmts.push(if_(
            strict_eq(member(ident(format!("__r_{pname}")), "tag"), str_lit("Err")),
            return_(Some(json_response(
                member(ident(format!("__r_{pname}")), "error"),
                400,
            ))),
        ));
        stmts.push(const_(
            jname.clone(),
            member(ident(format!("__r_{pname}")), "value"),
        ));
        let mut call_args = Vec::new();
        if has_who {
            call_args.push(ident("__who"));
        }
        if has_caller {
            call_args.push(ident("__caller"));
        }
        call_args.push(ident(jname));
        stmts.push(const_(
            "result",
            await_expr(call(member(ident("surface"), sname.to_string()), call_args)),
        ));
    } else {
        let typeof_args = TsExpr::Unary {
            op: bynk_ts::TsUnaryOp::Typeof,
            expr: Box::new(ident("args")),
        };
        stmts.push(if_(
            or_expr(
                or_expr(
                    strict_neq(typeof_args.clone(), str_lit("object")),
                    strict_eq(ident("args"), null_lit()),
                ),
                call(
                    member(ident("globalThis.Array"), "isArray"),
                    vec![ident("args")],
                ),
            ),
            return_(Some(json_response(
                json_error_kind(
                    "StructuralMismatch",
                    vec![
                        ("path".to_string(), str_lit("$")),
                        ("expected".to_string(), str_lit("object")),
                        ("actual".to_string(), typeof_args),
                    ],
                ),
                400,
            ))),
        ));
        stmts.push(const_(
            "argsObj",
            as_expr(ident("args"), index_signature_record_ty()),
        ));
        let mut names = Vec::new();
        for p in &h.params {
            // `pname` is the wire field name (the `argsObj` key + diagnostic path)
            // and must stay verbatim; `jname` is the reserved-word-safe binding
            // the surface call references.
            let pname = &p.name.name;
            let jname = call_wire_ident(pname);
            let dser = deserialise_call(
                &p.type_ref,
                &index(ident("argsObj"), str_lit(pname.clone())),
                &format!("$.{pname}"),
                ru,
            );
            stmts.push(const_(format!("__r_{pname}"), dser));
            stmts.push(if_(
                strict_eq(member(ident(format!("__r_{pname}")), "tag"), str_lit("Err")),
                return_(Some(json_response(
                    member(ident(format!("__r_{pname}")), "error"),
                    400,
                ))),
            ));
            stmts.push(const_(
                jname.clone(),
                member(ident(format!("__r_{pname}")), "value"),
            ));
            names.push(jname);
        }
        // Prepend the caller (when present) without a dangling comma for the
        // zero-param case.
        let mut call_args: Vec<TsExpr> = Vec::new();
        if has_who {
            call_args.push(ident("__who"));
        }
        if has_caller {
            call_args.push(ident("__caller"));
        }
        call_args.extend(names.into_iter().map(ident));
        stmts.push(const_(
            "result",
            await_expr(call(member(ident("surface"), sname.to_string()), call_args)),
        ));
    }
    let ser_expr = serialise_call(&h.return_type, &ident("result"), ru);
    stmts.push(const_("body", ser_expr));
    stmts.push(return_(Some(new_response(vec![
        json_stringify(ident("body")),
        status_json_headers_obj(200),
    ]))));
    stmts
}

/// `{ [k: string]: JsonValue }` — the structural type `emit_call_handler_dispatch`'s
/// own multi-param branch casts `args` to before indexing it by field name.
fn index_signature_record_ty() -> TsType {
    TsType::Object(vec![TsTypeMember::index(
        "k",
        TsType::named("string"),
        TsType::named("__JsonValue"),
    )])
}
