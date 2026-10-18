# 0513 — Lightbulb code actions that re-check their edit resolve it lazily

- **Status:** Accepted (v0.357)

**Context.** ADR `lsp-refactorings-and-source-actions` makes every new #2100
action re-analyse the project with its edit applied before offering it.
Source actions are only computed when the client asks for their kind. A quick
fix or refactor, though, comes back on the unfiltered request a client sends
every time the cursor moves. Re-analysing the project there would cost a
whole-project analysis per cursor move.

**Decision.** The server advertises `resolveProvider` for code actions. A
lightbulb-path action that needs the re-check (fill match arms is the first)
is listed after a cheap check that it can produce an edit. It carries no edit,
only `data` naming its target and the document version it was listed against.
`codeAction/resolve` computes the edit from the committed round, re-checks it,
and returns the versioned edit. A resolve against a changed document, or one
whose edit would introduce a diagnostic, is refused with a message the client
shows. Any action the server doesn't recognise resolves to itself.

For fill match arms, the checker attaches the missing arm to each
`bynk.types.non_exhaustive_match` as a `HasPlaceholders` suggestion. Its body is
a typed default of the arm result type, so the suggestion is never a one-click
quick fix or a fix-all candidate. The action is listed only when every missing
witness has one. A result type with no default gets no action, rather than an
arm that doesn't type-check, since the language has no placeholder expression.
On an extensible enum the wildcard its future variants need
(`bynk.types.extensible_match_needs_wildcard`) carries the same suggestion, so
the fill adds a trailing `_ => <default>` arm along with the missing declared
variants, rather than leaving a match the re-check would refuse.

**Consequences.** Listing a lightbulb action stays as cheap as before. The
re-check runs only when someone picks the action. For these actions "offered"
means "resolves to an edit": a listed action can still be refused at resolve.
The CLI shows the new suggestion as a `help:` note on the non-exhaustive
diagnostic. A missing multi-field variant is now named with one `_` per field
(`Rect(_, _)`), which can be written as a pattern.
