---
level: patch
changelog: The handlers paths and cross-context handlers imports of a Workers build come from the `WorkerLayout` rather than being spelled in place, with the identity layout unchanged (#3562, G2b step 2c-1)
---

No user-visible change: without a `[deploy.workers]` table every path and import
specifier is byte-identical (the `positive_fixtures` goldens are the proof).
`WorkerLayout` gains `handlers_source_path`, `handlers_output_path`,
`file_source_path`, `own_handlers_specifier` and `handlers_specifier`; the
driver, the emitter's cross-context fallback, and the generated compose and
entry modules use them. The group layout itself is switched on in step 2c-4.
