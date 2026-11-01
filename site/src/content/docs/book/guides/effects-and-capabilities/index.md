---
title: Effects & capabilities
---
Effects are explicit and capability-gated: a handler declares what it needs via
`given`, providers supply implementations, and adapters wrap the host boundary.

**Understand**
- [Understand the capability model](/book/guides/effects-and-capabilities/understand-the-capability-model/)
- [Synchronous and asynchronous sends](/book/guides/effects-and-capabilities/synchronous-and-asynchronous-sends/)
- [What a join guarantees on failure](/book/guides/effects-and-capabilities/what-a-join-guarantees-on-failure/)
- [What a timeout stops](/book/guides/effects-and-capabilities/what-a-timeout-stops/)

**Do**
- [Run independent calls at the same time](/book/guides/effects-and-capabilities/run-independent-calls-at-the-same-time/)
- [Bound how long a call may take](/book/guides/effects-and-capabilities/bound-how-long-a-call-may-take/)
- [Ask two sources and take the first answer](/book/guides/effects-and-capabilities/ask-two-sources-and-take-the-first-answer/)
- [Compose a provider from other capabilities](/book/guides/effects-and-capabilities/compose-a-provider/)
- [Share a capability across contexts](/book/guides/effects-and-capabilities/share-across-contexts/)
- [Wrap a library as an adapter](/book/guides/effects-and-capabilities/wrap-a-library/)
- [Use more than one KV namespace](/book/guides/effects-and-capabilities/use-several-kv-namespaces/)
- [Call a Worker outside the project](/book/guides/effects-and-capabilities/call-a-foreign-worker/)
- [Cache responses at Cloudflare's edge](/book/guides/effects-and-capabilities/cache-at-the-edge/)
- [Sign and verify data](/book/guides/effects-and-capabilities/sign-and-verify-data/)

**See also:** [Reference — Capabilities & providers](/book/reference/capabilities/),
[Adapters](/book/reference/adapters/).
