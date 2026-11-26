# 0984 — any and all over an index-routed filter read the posting list a page at a time

- **Status:** Accepted (v0.720.1)

**Context.** ADR store-scans-stop-early left an equality `filter` on an `@indexed` `Map` field on its posting-list route, which reads the whole list of that value's postings and every entry they name before the terminal runs. For `any` and `all` the order of the values does not matter, so the list can be read a page at a time and the read can stop at the answer.

**Decision.** A leading equality `filter` on an `@indexed` field (the shape `route_indexed_filter` routes) followed by `filter`, `map` or `flatMap` stages and then `any` or `all` is lowered to one scan over the handle's `lazyByIndex(index, key)`: a page of postings from storage, the entries they name read as one batch, and the scan stops at the answer. The postings are in primary-key order, not insertion order, so the values come in no promised order, which `any` and `all` do not observe. `first`, `firstOrElse` and `take` over a routed filter keep the whole-list read: they depend on insertion order, which only a whole read recovers, exactly as for an unindexed `Map`. The in-memory handle yields the same values from its posting list. A composite index key uses the same canonical encoding as the routed lookup.

**Consequences.** Results are unchanged; only the pages read differ. No stored state changes. Hover on such a terminal says the posting list is read a page at a time. Completion, semantic tokens and signature help are unchanged, because no declaration, type or syntax changed.
