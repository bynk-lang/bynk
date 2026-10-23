---
level: patch
probes: emit_diagnostics, diagnostic_coverage
changelog: "On the Workers target, requiring another context's capability whose provider constructs that context's agents is now a compile error (`bynk.capability.provider_agent_cross_context`), not a type error in `tsc` or a silent fallback to in-isolate agent state under `bynk dev`"
---

## ADR: provider-agent-cross-context
title: On Workers, a provider that constructs its context's agents serves only that context
summary: Why another context may not require such a provider's capability on Workers, and why binding the agents into the consumer's Worker is deferred

**Context.** A context-local provider whose body constructs one of its context's agents receives the Worker's `env` through its `deps`, because on Workers the agent factory reaches the agent's Durable Object namespace through `env` (#2355). Another context can require that provider's capability through `exports capability` and `given registry.Yanks`. The consumer's composition root then builds the provider in its own Worker and hands it the consumer's `env`, which binds only the consumer's own Durable Object namespaces. `tsc` rejected the build (`Property 'PACKAGE_INDEX' is missing in type 'Env'`), but strip-only `bynk dev` ran it, and the agent factory silently fell back to agent state held in the isolate.

**Decision.** On the Workers target, a context that requires such a capability is a compile error, `bynk.capability.provider_agent_cross_context`. It is raised from a Workers-only project phase beside the other target-gated checks. The phase walks every `given` the consumer's compose instantiates: service and agent handlers, and local providers, transitively through other contexts' providers. It flags each provider of another context whose ops construct one of that context's agents, once per provider, at the first `given` that reaches it. Whether a provider constructs an agent is one syntactic predicate in `bynk-check`, shared with the emitter's provider class and composers so that the check and the emitted constructor cannot disagree. The bundle target is unaffected. There, every context shares one process, and the agent factory takes only the key.

**Consequences.** The failure moves from a `tsc` error, or a silent runtime divergence, to a compile error. Its note suggests calling a service of the provider's context instead. Supporting the layout remains possible later. That would mean binding the provider context's Durable Object namespace into the consumer's Worker: a `script_name` binding in `wrangler.toml`, the namespace in the consumer's `Env`, and a local runtime that serves both Workers. At that point this check can be retired.
