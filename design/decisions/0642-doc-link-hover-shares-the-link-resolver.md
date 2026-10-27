# 0642 — Doc-link hover resolves through the same resolver as the link

- **Status:** Accepted (v0.432)

**Context.** The first doc-link hover (#2460) resolved with the unit scope alone. Doc links now resolve from the declaration their comment is attached to, then the unit scope, then unit-qualified names, so a hover that skipped the first and last steps would preview a different target from the one the link opens, or report a working link unresolved.

**Decision.** The hover rung resolves with `resolve_doc_link_in`, passing the hovered block's attached declaration as context. A target with an index key renders with `describe_symbol`; one without (a parameter) renders from the locals table. When nothing resolves, `doc_link_matches` still distinguishes an ambiguous name from an absent one. There is one resolution path for links, linkification and hover.

**Consequences.** Preview and click-through cannot disagree. `resolve_doc_link_key` remains as the unit-scope step of `resolve_doc_link_in`.
