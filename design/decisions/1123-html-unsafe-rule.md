# 1123 — `Html.unsafe(markup, reason)` is the one raw door, with a literal reason, allowed in dependencies and listed per package

- **Status:** Accepted (v0.833.1)

**Context.** Real pages need markup the compiler did not check: rendered Markdown, CMS content, a third-party widget. An escape hatch that is too easy undoes the guarantee, and one that is too hard (a warning on every use) teaches people to ignore it. The maintainer ruled on its name and its use in dependencies on 27 November 2026 (track §5.6, Q5).

**Decision A — the name and its shape.** `Html.unsafe(markup: String, reason: String) -> Html` is the only way raw markup enters an `Html`. The name tells a reviewer that the compiler stops checking here and greps with other `unsafe` audits. It is one token, `Html.unsafe`; no other API reaches it, and it lowers to the identity. Rejected: `Html.trusted`, which reads as an assertion of safety rather than a waiver of checking.

**Decision B — the reason is a non-empty string literal.** Not a `const`, so the reason is at the call site, in review and in hover (`bynk.html.unsafe_reason_not_literal`). An empty reason is refused under the same code.

**Decision C — a hint for literal markup, no warning otherwise.** When `markup` is a string literal, a hint says to write `html"…"`, which is checked (`bynk.html.unsafe_literal`). Its fix is machine-applicable only when the literal has no interpolation holes and is a valid fragment: in `Html.unsafe("<p>\(s)</p>", r)` the `String` `s` goes in raw, and the rewrite would escape it, or stop compiling where a context wants an `Href`, so with a hole the hint offers no automatic fix. A run-time argument draws no warning: every real use is dynamic, so a warning would fire on every use. Rejected: a warning on run-time arguments.

**Decision D — dependencies may call it, and every call is listed.** `bynk check --unsafe-html` lists every `Html.unsafe` call site the build compiles, grouped by package with the project's own first, with file, line and reason; the language server shows the same list as a code lens on the `[dependencies]` table. A Markdown renderer is the obvious dependency, and it cannot exist if dependencies may not call the hatch. Rejected: refusing dependencies unless the project opts in per package, which forbids the renderer and adds a knob the listing makes unnecessary.

**Consequences.** `Html.unsafe` belongs to the track's experimental gate, `bynk.html.typed`, like every `Html` operation, and `StabilityPolicy` is the root project's, so a dependency that uses it compiles only in a project that allows the gate. Markup passed to it is outside the guarantee; the track sanitises nothing, and the docs say so. Once the project allows the gate, a dependency's use is trusted; the listing makes that trust visible, not safe.
