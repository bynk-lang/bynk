# 0915 — One regex dialect: Unicode mode, a leading flag group, a `RegExp` built once

- **Status:** Accepted (v0.660)

**Context.** `Matches` was validated and run without flags (`new RegExp("^(?:" + p + ")$")` on every check), while the `String` regex methods accepted a leading `(?i)`/`(?m)`/`(?s)` group. Without `u`, `\p{L}` is unavailable and `.` or a negated class matches half a surrogate pair.

**Decision.**
- Every Bynk regex runs with the `u` flag: the checker validates and ReDoS-analyses under it, the compile-time evaluation of a constant uses it, and the kernel's and `Matches`' emitted `RegExp` carry it. Rejected: the `v` flag (newer and stricter, less widely supported by OpenAPI tooling and `regress`) and opt-in Unicode mode (keeps the surrogate-splitting default).
- `Matches` takes the regex literal's leading flag group, `(?i)` and `(?s)` each at most once, through the same split as the kernel (`bynk_syntax::ast::leading_regex_flag_group`). `(?m)` means nothing under `Matches`' whole-string anchoring and is refused with a note.
- A refinement's `RegExp` is built once. It is built by the runtime's `__bynkRegExp(source, flags)`, which memoises per source and flags, rather than hoisted as a module constant beside each type's constructor: the same check text also runs inline in boundary decoders, the generated client and the editor's check panel, none of which share the owner's module scope. The effect the issue asks for holds (one compiled `RegExp` per pattern), and a non-global, non-sticky `RegExp`'s `test` keeps no state, so sharing is safe.
- OpenAPI carries the flags honestly: a `(?i)` pattern is written out (`a` as `[aA]`, a class gains the other case of its letters and ranges), `(?s)` widens `.` to `[\s\S]`, and a pattern that cannot be (a property or code-point escape under `i`, a non-ASCII letter, a range across cases) is left out with a note in the schema `description`.

**Consequences.** A pattern valid only without `u` stops compiling (`bynk.types.invalid_regex`, noting the mode); nothing in the repository's fixtures, examples or standard library needed changing. `\w`, `\d` and `\b` stay ASCII, documented with `\p{L}` and `\p{Nd}` named as the Unicode forms.
