# 0937 — A suite sees its target's function signatures under declared type names

- **Status:** Accepted (v0.677.3)

**Context.** A suite is not a consuming context (#2890): it names a consumed type by the name its unit declares, never by the target's `as` alias. The target's own functions are written with the alias, so a case calling `fn tag() -> Tag` resolved `Tag` against a view that does not bind it. The call's type became `Ty::Error`, and the suite emitter then panicked with internal error #28.

**Decision.** The privileged suite view rewrites the target's own function, method and type declarations, replacing each alias with the key the view files the aliased declaration under: the declared name when the view holds that same declaration under it, otherwise `{target}__{name}` (as a clashing borrowed type is filed, #2087, #2901). So a target's own `Code` beside `Code as Tag` is never confused with the consumed one. The alias stays unnameable in a suite, so #2890 is unchanged.

**Consequences.** A suite can call and check any target function whatever its aliases, with or without importing the name. No internal error is reachable from this shape.
