# 0641 — Find references and document highlight include resolving doc-link mentions

- **Status:** Accepted (v0.430)

**Context.** A symbol mentioned in a doc comment (`[Limiter]`) was invisible to Find references, so a declaration's cross-references in documentation could not be found. The LSP `ReferenceContext` has no flag to ask for doc mentions separately.

**Decision.** `textDocument/references` always appends the doc-link mentions that resolve to the symbol, via the shared `doc_link_mentions` reverse lookup (also used by rename), whatever `includeDeclaration` says. `textDocument/documentHighlight` also highlights the same-file mentions. Only links that resolve count; plain prose and unresolved candidates never do.

**Consequences.** Clients group references by file and show the line, so a doc mention is distinguishable from code. Large projects gain no noise from prose, because only resolving links are counted. Hover, completion, semantic tokens and signature help are unchanged.
