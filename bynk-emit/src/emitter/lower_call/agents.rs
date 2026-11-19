//! Agent construction and agent calls (back-half paydown 1A.27, #3728): the inline call
//! `Agent(<key>).method(args)`, the let-bound call `x.method(args)` and (below) `AgentName(key)`.
//! Moved out of `lower.rs` byte for byte (Decision D) before they convert to nodes.

use bynk_check::checker::Callee;

use super::super::lower::{Built, LoweredExpr, Pre, node_or_text, str_node};
use super::super::lower_ctx::LowerCtx;
use super::super::lower_list::{call, id, member};
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
) -> Option<Built> {
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
            let mut children: Vec<LoweredExpr> =
                args.iter().map(|a| pre.lower_node(a, cx)).collect();
            children.push(LoweredExpr::Node(id("deps")));
            return Some(node_or_text(
                children,
                |nodes| call(member(id("this"), &method.name), nodes),
                |texts| format!("this.{}({})", method.name, texts.join(", ")),
            ));
        }
        cx.record_agent_call(&name.name, &method.name);
        // The key is lowered first, so its hoisted statements precede the call.
        let key = ctor_args
            .first()
            .map(|a| pre.lower_node(a, cx))
            .unwrap_or_else(|| LoweredExpr::Node(str_node("default")));
        let instance = cx.agent_construct_node(&name.name, key);
        let mut children: Vec<LoweredExpr> = vec![instance.into()];
        children.extend(args.iter().map(|a| pre.lower_node(a, cx)));
        // #2355: `this.deps` in a provider body.
        children.push(LoweredExpr::Node(cx.cap_deps_node()));
        let built = node_or_text(
            children,
            |mut nodes| {
                let instance = nodes.remove(0);
                call(member(instance, &method.name), nodes)
            },
            |texts| format!("{}.{}({})", texts[0], method.name, texts[1..].join(", ")),
        );
        let target = format!("{}.{}", name.name, method.name);
        return Some(trace_built(cx, &target, e.span.start, built));
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
) -> Option<Built> {
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
    if let ExprKind::Ident(id_) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Agent { .. }))
    {
        if let Some(agent) = cx.local_agent_vars.get(&id_.name).cloned() {
            cx.record_agent_call(&agent, &method.name);
        }
        let mut children: Vec<LoweredExpr> = args.iter().map(|a| pre.lower_node(a, cx)).collect();
        children.push(LoweredExpr::Node(cx.cap_deps_node()));
        // #908: `local_agent_vars` is keyed by the bynk source name, which a
        // re-`let` may have renamed — resolve to the actual JS binding rather
        // than assuming the source name is still it.
        let recv = cx
            .resolved_local_name(&id_.name)
            .unwrap_or_else(|| id_.name.clone());
        let built = node_or_text(
            children,
            |nodes| call(member(id(&recv), &method.name), nodes),
            |texts| format!("{}.{}({})", recv, method.name, texts.join(", ")),
        );
        let target = match cx.local_agent_vars.get(&id_.name) {
            Some(agent) => format!("{agent}.{}", method.name),
            None => method.name.clone(),
        };
        return Some(trace_built(cx, &target, e.span.start, built));
    }
    None
}

/// The trace wrapper over a built agent call (1A.24's `trace_step_node` when it is a node, the
/// string wrapper when an argument is still text).
fn trace_built(cx: &LowerCtx, target: &str, offset: usize, built: Built) -> Built {
    match built {
        Ok(node) => Ok(cx.trace_step_node("agent", target, offset, node)),
        Err(text) => Err(cx.trace_step("agent", target, offset, text)),
    }
}

/// `AgentName(key)` (the `Callee::AgentInit` branch of `lower_call`): the factory call over
/// the first lowered argument. Panics (ADR 0334) when there is none.
pub(in crate::emitter) fn lower_agent_init(
    name: &Ident,
    args_lowered: Vec<LoweredExpr>,
    cx: &mut LowerCtx,
) -> Built {
    let arg_count = args_lowered.len();
    let key = args_lowered.into_iter().next().unwrap_or_else(|| {
        panic!(
            "bynk internal error (ADR 0334): Callee::AgentInit records agent \
             construction `{}(...)` with {arg_count} args, but every AgentDecl \
             declares exactly one key field — the checker already accepted this call",
            name.name,
        )
    });
    cx.agent_construct_node(&name.name, key)
}
