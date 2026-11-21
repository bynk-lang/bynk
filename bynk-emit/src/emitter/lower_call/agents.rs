//! Agent construction and agent calls (back-half paydown 1A.27, #3728): the inline call
//! `Agent(<key>).method(args)`, the let-bound call `x.method(args)` and `AgentName(key)`
//! (`lower_agent_init`, further down). Each is built as a node when its key and arguments are nodes
//! and falls back to the same text otherwise; they were moved here from `lower.rs` byte for byte
//! first (Decision D).

use bynk_check::checker::Callee;

use super::super::lower::{Pre, str_node};
use super::super::lower_ctx::LowerCtx;
use super::super::lower_list::{call, id, member};
use super::super::*;

/// Lower an inline agent invocation, `Agent(<key>).method(args)`, or `None` when the call is not one. A node
/// (`Call(Member(<construction>, method), args)`, wrapped in the trace step on a traced build) when the key and
/// arguments are nodes, else the text form.
pub(in crate::emitter) fn lower_inline_agent_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
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
            let mut children: Vec<bynk_ts::TsExpr> =
                args.iter().map(|a| pre.lower_node(a, cx)).collect();
            children.push(id("deps"));
            return Some({
                let nodes = children;
                call(member(id("this"), &method.name), nodes)
            });
        }
        cx.record_agent_call(&name.name, &method.name);
        // The key is lowered first, so its hoisted statements precede the call.
        let key = ctor_args
            .first()
            .map(|a| pre.lower_node(a, cx))
            .unwrap_or_else(|| str_node("default"));
        let instance = cx.agent_construct_node(&name.name, key);
        let mut children: Vec<bynk_ts::TsExpr> = vec![instance];
        children.extend(args.iter().map(|a| pre.lower_node(a, cx)));
        // #2355: `this.deps` in a provider body.
        children.push(cx.cap_deps_node());
        let built = {
            let mut nodes = children;
            let instance = nodes.remove(0);
            call(member(instance, &method.name), nodes)
        };
        let target = format!("{}.{}", name.name, method.name);
        return Some(trace_built(cx, &target, e.span.start, built));
    }
    None
}

/// Lower a method call on a let-bound agent instance, or `None` when the call is not one. A node
/// (`Call(Member(<binding>, method), args)`, wrapped in the trace step on a traced build) when the arguments are
/// nodes, else the text form.
pub(in crate::emitter) fn lower_let_bound_agent_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    // Let-bound agent invocation. `let x = Agent(key); x.method(args)`
    // — the statement emitter recorded `x` as an agent variable when
    // it lowered the let. Method calls on `x` go straight to the
    // class instance with `deps` threaded through.
    //
    // P6.21 (partial, continued): reads `Callee::Agent` instead of
    // `cx.local_agent_vars.contains_key(&id.name)` — see the inline
    // agent-invocation branch above. 1A.33: the agent the binding refers to is the callee's
    // own `agent` (the checker resolved it from the receiver's type), so `record_agent_call`
    // and the trace target read it from there and the statement emitter no longer records
    // `let x = Agent(key)` by name. The #908 rename below is lowering state
    // (`resolved_local_name`): it answers what the JavaScript binding is called.
    if let ExprKind::Ident(id_) = &receiver.kind
        && let Some(Callee::Agent { agent, .. }) = cx.commons().callee(e.id)
    {
        let agent = agent.clone();
        cx.record_agent_call(&agent, &method.name);
        let mut children: Vec<bynk_ts::TsExpr> =
            args.iter().map(|a| pre.lower_node(a, cx)).collect();
        children.push(cx.cap_deps_node());
        // #908: the bynk source name may have been renamed by a re-`let` — resolve to the
        // actual JS binding rather than assuming the source name is still it.
        let recv = cx
            .resolved_local_name(&id_.name)
            .unwrap_or_else(|| id_.name.clone());
        let built = {
            let nodes = children;
            call(member(id(&recv), &method.name), nodes)
        };
        let target = format!("{agent}.{}", method.name);
        return Some(trace_built(cx, &target, e.span.start, built));
    }
    None
}

/// The trace wrapper over a built agent call (1A.24's `trace_step_node` when it is a node, the
/// string wrapper when an argument is still text).
fn trace_built(
    cx: &LowerCtx,
    target: &str,
    offset: usize,
    built: bynk_ts::TsExpr,
) -> bynk_ts::TsExpr {
    cx.trace_step_node("agent", target, offset, built)
}

/// `AgentName(key)` (the `Callee::AgentInit` branch of `lower_call`): the factory call over
/// the first lowered argument. No argument is a missing fact (ADR 0334, 1A.34): every
/// `AgentDecl` declares exactly one key field, which the checker's arity check enforces.
pub(in crate::emitter) fn lower_agent_init(
    name: &Ident,
    args_lowered: Vec<bynk_ts::TsExpr>,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    let key = args_lowered.into_iter().next().unwrap_or_else(|| {
        cx.missing_fact(
            name.span,
            format_args!(
                "key argument of the agent construction `{}(...)`",
                name.name
            ),
        )
    });
    cx.agent_construct_node(&name.name, key)
}

#[cfg(test)]
mod tests {
    use super::super::super::lower::ident_node;
    use super::*;
    use crate::emitter::ctx::{BodyMode, HandlerShared, ModuleCtx, UnitFacts};
    use crate::emitter::runtime_use::RuntimeUse;
    use bynk_check::checker::TypedCommons;
    use bynk_check::project_model::BuildTarget;

    fn node(text: &str) -> bynk_ts::TsExpr {
        ident_node(text)
    }

    fn printed(built: bynk_ts::TsExpr) -> String {
        bynk_ts::print_expr(&built)
    }

    /// `agent_construct_node` for one build target and body kind: the handler's deps shape
    /// widens either way.
    fn construct(workers: bool, provider: bool, key: bynk_ts::TsExpr) -> (bynk_ts::TsExpr, bool) {
        let commons = TypedCommons::empty();
        let cross = bynk_check::resolver::CrossContextInfo::default();
        let ru = RuntimeUse::default();
        let mut module = ModuleCtx::new(&commons, &cross, &ru, UnitFacts::detached());
        if workers {
            module.target = BuildTarget::Workers;
        }
        let handler = HandlerShared {
            deps_on_this: provider,
            ..HandlerShared::default()
        };
        let mode = if provider {
            BodyMode::ProviderOp { handler }
        } else {
            BodyMode::ServiceHandler {
                handler,
                deps_identity_binder: None,
                actor_sum_binder: None,
            }
        };
        let mut cx = LowerCtx::new(module, mode);
        let built = cx.agent_construct_node("Room", key);
        let widened = cx.handler().is_some_and(|h| h.agents_instantiated);
        (built, widened)
    }

    #[test]
    fn a_bundle_construction_passes_only_the_key() {
        for provider in [false, true] {
            let (built, widened) = construct(false, provider, node("k"));
            assert!(widened);
            assert_eq!(printed(built), "__makeRoom(k)");
        }
    }

    /// Workers threads `deps.env`, and `this.deps.env` in a provider body, whose composer
    /// supplies it (#2355).
    #[test]
    fn a_workers_construction_threads_the_deps_env() {
        for (provider, deps) in [(false, "deps"), (true, "this.deps")] {
            let want = format!("__makeRoom(k, {deps}.env)");
            let (built, widened) = construct(true, provider, node("k"));
            assert!(widened);
            assert_eq!(printed(built), want);
        }
    }

    /// A malformed init (no key argument) is a missing argument-arity fact (ADR 0334, 1A.34),
    /// recorded at the agent's name, and lowering continues with the placeholder.
    #[test]
    fn a_malformed_init_is_a_missing_arity_fact() {
        let commons = TypedCommons::empty();
        let cross = bynk_check::resolver::CrossContextInfo::default();
        let ru = RuntimeUse::default();
        let module = ModuleCtx::new(&commons, &cross, &ru, UnitFacts::detached());
        let mut cx = LowerCtx::new(module, BodyMode::Method);
        let name = Ident {
            name: "Room".to_string(),
            span: bynk_syntax::span::Span::new(5, 9),
        };
        // Lowering continues past the fault.
        let _ = lower_agent_init(&name, vec![], &mut cx);
        let faults = ru.take_faults();
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert_eq!(faults[0].span, name.span);
        assert!(
            faults[0]
                .what
                .contains("key argument of the agent construction `Room(...)`"),
            "{faults:?}"
        );
    }
}
