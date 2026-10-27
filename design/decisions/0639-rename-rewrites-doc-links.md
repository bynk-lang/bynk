# 0639 — Rename rewrites doc links that resolve to the renamed symbol

- **Status:** Accepted (v0.428)

**Context.** `textDocument/rename` edited index references only. Doc-link candidates are never index references, so renaming `Limiter` left `[Limiter]` in doc comments, which then silently stopped resolving.

**Decision.** Rename also rewrites every doc link that resolved to the renamed definition before the rename, across the project. Only links that resolve (through `resolve_doc_link`) to that definition are touched, never links that merely share the name. Only the target text is rewritten, so display text in `[the limiter][Limiter]` and the backticks of `` [`Limiter`] `` are kept. Renaming a type also rewrites the owner segment of `[Limiter.decide]`; renaming a member rewrites only its own segment. After the rename's re-analysis, every doc link must resolve to the same definition (modulo the rename); one that resolves differently is reported like any other re-binding. The reverse lookup is a helper, `doc_link_mentions`, that find-references and document-highlight are meant to reuse (#2457); only rename calls it in this increment. The scan is limited to files whose doc scope contains the defining unit.

**Consequences.** A rename no longer leaves dangling doc links. The post-rename index comparison now also accepts the re-keyed members of a renamed type (`Type.member` to `New.member`), which it previously had no way to express. No grammar, checker, emitter or runtime change; hover, completion, semantic tokens and signature help are unchanged because rename does not affect rendering.
