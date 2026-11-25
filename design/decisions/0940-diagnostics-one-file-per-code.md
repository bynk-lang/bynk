# 0940 — The diagnostic reference is one generated file per code, with the index assembled at site build time

- **Status:** Accepted (v0.678.2)

**Context.** `site/src/generated/diagnostics.json` and `site/src/content/docs/reference/diagnostics.md` were generated from the registry and committed. Two changes that each added a diagnostic both regenerated both files. Git often merged the two copies as text, but the result was not what the generator writes, so the merge queue ejected the second change, and on 24 November five changes were serialised in a chain, each costing a full CI cycle.

**Decision.** The registry renders one document per code to `site/src/generated/diagnostics/<code>.json`, which is committed and checked by `diagnostics_registry::generated_diagnostics_json_is_up_to_date` (each file current, and no file for a code the registry lacks). The index page is not committed: `site/scripts/build-diagnostics-index.mjs` assembles it from those files in the site's `prebuild` and `predev` hooks, and `src/content/docs/reference/diagnostics.md` is gitignored. The per-code page and `check-redirects.mjs` read the directory. The registry itself and `src/data/diagnostic-links.json` stay hand-edited.

**Consequences.** Two changes that add different codes touch different generated files and merge in either order with no regeneration step. The index renders as before. CI and the release workflow run the assembler before the steps that need the page (the sidebar check, the Book bundle). `bynk explain` and the per-code pages are unchanged.
