---
level: patch
changelog: Internal restructuring: the integration-test doubles (`makeIntegration*`) are no longer in the shipped `storage`, `agent` and `vectorize` runtime modules, so a build's `storage` module is about 670 lines smaller; the doubles are imported from `testing/bindings` and `testing/pglite`, and no production emission changes
---
refactor: the test doubles leave the shipped runtime (1B.3b)

Part of #4047. Part of #3676.

No production emission changes: no production emission names a test double. A test module that uses one now imports it from `__bynk_runtime/testing/bindings.js` (the PGlite double from `testing/pglite.js`), so 30 stored test-module goldens change by their `import` lines and nothing else (the ruled class). The 16 `makeIntegration*` factories in `storage.ts`, `makeIntegrationDoNamespace` in `agent.ts` and `makeIntegrationVectorize` in `vectorize.ts` live in `runtime/src/testing/bindings.ts`. `makeTestState` and `InMemoryStorage` stay in `storage.ts`, as the Bundle target's agent storage. A runtime test fails if a production module defines a `makeIntegration*`.
