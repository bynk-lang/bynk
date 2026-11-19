//! Agent construction and agent calls (back-half paydown 1A.27, #3728): the inline call
//! `Agent(<key>).method(args)`, the let-bound call `x.method(args)` and (below) `AgentName(key)`.
//! Moved out of `lower.rs` byte for byte (Decision D) before they convert to nodes.

use bynk_check::checker::Callee;

use super::super::lower::Pre;
use super::super::lower_ctx::LowerCtx;
use super::super::*;

/// Lower an inline agent invocation, `Agent(<key>).method(args)`, or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
pub(in crate::emitter) fn lower_inline_agent_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // v0.9.2: inline agent invocation. Source form is
    // `Agent(<key>).method(args)`; receiver parses as
    // `Call(Agent, [<key>])`. Lower to
    // `__makeAgent(<key>).method(args, deps)`. Works in service and
    // agent-handler bodies (deps is the handler's deps parameter) and
    // test bodies (deps is the locally-built __makeTestDeps record).
    //
    // P6.21 (partial, continued): reads `Callee::Agent` (P6.0) instead of
    // `cx.local_agents.contains(&name.name)` — the checker's own
    // `check_method_call` records this the moment a receiver's *checked
    // type* resolves to an agent, uniformly for this shape, the let-bound
    // shape below, and the self-agent WS-transfer case inside this branch —
    // closing a real instance of R6.5's name-matched-receiver defect class
    // (a free function or local sharing an agent's own name could misfire
    // the old check; the checker's own type-driven resolution cannot).
    if let ExprKind::Call {
        name,
        args: ctor_args,
        ..
    } = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Agent { .. }))
    {
        // v0.104 (real-time track slice 3b): when lowering a `from websocket`
        // `on open` body inside its hosting Durable Object, a transfer to the
        // self-agent is a direct `this.method(args, deps)` self-call — the key
        // addresses *this* DO, and the held connection never crosses an RPC
        // boundary (DECISION A). The key expression is **not** emitted: the shape
        // constraint (`bynk.ws.open_transfer_shape`, D2) restricts it to a
        // request-derivable param ident — side-effect-free and equal to this
        // instance's own key — so dropping it is sound. #2959 widened the key to a
        // context `const` or a literal (`bynk.ws.open_transfer_key`), and #2967
        // to a used commons' `const`; all are pure and the edge routed the
        // upgrade by the same value, so it still is.
        if cx.ws_self_agent() == Some(name.name.as_str()) {
            cx.record_agent_call(&name.name, &method.name);
            let args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
            let mut all = args_lowered;
            all.push("deps".to_string());
            return Some(format!(
                "this.{method}({args})",
                method = method.name,
                args = all.join(", ")
            ));
        }
        cx.record_agent_call(&name.name, &method.name);
        let key_arg = ctor_args
            .first()
            .map(|a| pre.lower(a, cx))
            .unwrap_or_else(|| "\"default\"".to_string());
        let instance = cx.agent_construct(&name.name, &key_arg);
        let args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
        let mut all = args_lowered;
        // #2355: `this.deps` in a provider body.
        all.push(cx.cap_deps_expr().to_string());
        let call = format!(
            "{instance}.{method}({args})",
            method = method.name,
            args = all.join(", ")
        );
        let target = format!("{}.{}", name.name, method.name);
        return Some(cx.trace_step("agent", &target, e.span.start, call));
    }
    None
}

/// Lower a method call on a let-bound agent instance, or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
pub(in crate::emitter) fn lower_let_bound_agent_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // Let-bound agent invocation. `let x = Agent(key); x.method(args)`
    // — the statement emitter recorded `x` as an agent variable when
    // it lowered the let. Method calls on `x` go straight to the
    // class instance with `deps` threaded through.
    //
    // P6.21 (partial, continued): reads `Callee::Agent` instead of
    // `cx.local_agent_vars.contains_key(&id.name)` — see the inline
    // agent-invocation branch above. `cx.local_agent_vars` stays: it still
    // answers "which agent does this bind name refer to" (needed for
    // `record_agent_call`'s own bookkeeping and the #908 rename-resolution
    // below), a different question from "is this receiver really an agent
    // instance at all," which `Callee` now settles.
    if let ExprKind::Ident(id) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Agent { .. }))
    {
        if let Some(agent) = cx.local_agent_vars.get(&id.name).cloned() {
            cx.record_agent_call(&agent, &method.name);
        }
        let args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
        let mut all = args_lowered;
        all.push(cx.cap_deps_expr().to_string());
        // #908: `local_agent_vars` is keyed by the bynk source name, which a
        // re-`let` may have renamed — resolve to the actual JS binding rather
        // than assuming the source name is still it.
        let recv = cx
            .resolved_local_name(&id.name)
            .unwrap_or_else(|| id.name.clone());
        let call = format!("{}.{}({})", recv, method.name, all.join(", "));
        let target = match cx.local_agent_vars.get(&id.name) {
            Some(agent) => format!("{agent}.{}", method.name),
            None => method.name.clone(),
        };
        return Some(cx.trace_step("agent", &target, e.span.start, call));
    }
    None
}
