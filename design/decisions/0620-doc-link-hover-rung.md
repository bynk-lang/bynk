# 0620 — Hover previews a doc link's target through a dedicated ladder rung

- **Status:** Accepted (v0.409)

**Context.** Hovering a `[Name]` inside a doc comment showed the hover for whatever surrounded it, or nothing, so a link's target could only be checked by following it. The hover ladder's rung order is a contract.

**Decision.** A new rung (4b) runs after the index, store-op, local and handler-header rungs and before the lexical rungs. It is active only when the cursor is inside a doc-link candidate span. A resolved link renders the target with `describe_symbol`, as hovering the name in code would. An unresolved link says so and lists the doc-link scope searched, as a lightweight complement to a diagnostic. `resolve_doc_link_key` returns the `SymbolKey` alongside the `SiteRef`; `resolve_doc_link` delegates to it.

**Consequences.** Doc-link spans are comment text that no index reference or local covers, so the rung cannot shadow earlier rungs. No change to the checker, emitter or runtime.
