---
level: minor
changelog: Doc links to first-party names such as `[Clock.now]` link to the Book's first-party capabilities reference
---

## ADR: first-party-doc-link-targets
title: First-party doc links target the live Book through a curated table
summary: A curated, test-checked name-to-anchor table, capability granularity, live Book

**Context.** First-party units are synthetic, so their symbols never reach the index and a doc link such as `[Clock.now]` could not resolve to a file. Some capabilities have their own Book headings; others are only table rows.

**Decision.** The mapping is a curated table next to the language server's doc-link code (`first_party_docs`), consulted as a separate step after the project scope finds nothing, and only for units in the declaring unit's scope. Granularity is the capability: an operation links to its capability's entry (table-row capabilities link to their unit's surface section). Targets are on the live Book, as diagnostic links are. A test asserts every mapped anchor exists in the Book source.

**Consequences.** Anchors cannot drift silently. New first-party capabilities need a table row. Operations have no anchors of their own until the Book is restructured.
