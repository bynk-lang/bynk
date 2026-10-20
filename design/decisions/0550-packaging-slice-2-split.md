# 0550 — Packaging slice 2 ships as 2a (path) and 2b (git, store, lock)

- **Status:** Accepted (v0.376)

**Context.** Track §9's slice 2 held an in-compiler half (graph resolution, cross-package loading,
alias naming, exports, emit layout, the editor) and a fetch half (git, the store, sha256, the lock,
offline modes, git hardening). A path dependency needs none of the fetch half: it is the user's own
working copy, never locked by hash (§6.3).

**Decision.**

- **2a** (#2223) resolves path dependencies end to end, with no new crate and no network.
- **2b** adds git sources, the content-addressed store, sha256, `bynk.lock`, `--locked` and
  `--offline`, and `[dev-dependencies]` — refused until then, with the slice named, rather than
  accepted and ignored.
- **Range syntax** is validated in slice 4, where registry ranges are first interpreted; path and git
  dependencies carry no range.
- A dependency adapter's npm `requires` is refused until slice 3 folds them; a dependency's `asset`
  is refused until assets get a per-package namespace.

**Consequences.** Each refusal names the slice that lifts it, so a manifest never builds while
ignoring what it declares (#1665).
