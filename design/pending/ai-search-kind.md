---
level: minor
changelog: `bynk.cloudflare` gains `AiSearch`, a linked instance kind for an existing AI Search instance named by a required `@instance`, with `search` and `answer`, a typed filter and typed errors, emitted as `[[ai_search]]`; every operation faults until stubbed in a test
probes: emit_abi_shapes, emit_diagnostics, ts_writes
---

## ADR: ai-search-instance-kind
title: AiSearch is a linked instance kind that the deploy finds and never creates
summary: Decisions A–J of #3480: a named, linked instance, search and answer over query and messages, shared chat and filter types, errors as values, stub-only in tests

**Context.** An AI Search instance is configured by someone (a data source, models, chunking, an indexing lifecycle), is often shared, and is neither a stateless per-Worker call (Workers AI) nor state the project owns (Vectorize). That is the case #3301's rule covers: the deploy links and never creates.

**Decision.** Records decisions A–J of #3480. A: `AiSearch` is a named instance with a required `@instance("…")`, `linked: true`, binding prefix `AI_SEARCH_`; the name is used as written in every environment (no per-environment names in v1). B: the deploy links and never creates (the resolver is slice AS2; the linked flag now has a second kind after `StoredSecret`). C: `search(query, options)` takes a plain query and `answer(messages, options)` a conversation, an asymmetry that matches a search box and a chat endpoint; `SearchOptions` carries only options with a clear meaning, and later ones arrive as record fields with defaults. There is no per-call model, because the generation model is the instance owner's setting. D: `ChatMessage` and `TokenUsage` (Workers AI) and `Condition` (Vectorize) are reused; the filter goes on the wire in the filtering page's `$op` form under `ai_search_options.retrieval.filters`, with `query_rewrite` and `reranking` beside `retrieval`. E: streaming is deferred. F: ingestion and instance management are out of scope. G: the guide says when to choose `AiSearch`, `Ai` and `Vectorize`; the compiler decides nothing. H: `bynk dev` goes to the deployed instance, with the shared one-time warning and `[dev] ai-search` acknowledgement (slice AS2). I: tests fault until stubbed. J: errors are values, with the `Search` prefix on `SearchInvalidInput`, `SearchRateLimited`, `SearchQuotaExceeded` and `SearchFailed` because the unprefixed names belong to other error types, and a reply that is not the documented shape is `SearchFailed`, never a fault.

**Consequences.** Experimental, so no 1.0 promise. A program that does not use `AiSearch` has unchanged handler and Worker output; only the emitted `bynk.cloudflare` module grows. The deploy resolver, `bynk dev` and the guide follow in later slices. The error mapping is by message, since Cloudflare does not document error codes for AI Search; anything unrecognised is `SearchFailed`.
