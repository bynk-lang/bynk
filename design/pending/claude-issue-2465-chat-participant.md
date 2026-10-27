---
level: minor
changelog: "`vscode-bynk` adds an `@bynk` chat participant (`/explain`, `/learn`, `/architecture`, `/fix`) that answers from the toolchain's language-model tools and cites its sources"
---

## ADR: bynk-chat-participant
title: The `@bynk` chat participant: command set and grounding rule
summary: Four slash commands over the language-model tools, the user's own model, edits as proposals, and cited sources

**Context.** The language-model tools (#2463) let any chat agent ask the
toolchain, but leave it to a general agent to decide when. Learners benefit from
a dedicated entry point that always grounds its answers, with Bynk-specific
commands.

**Decision.** `vscode-bynk` contributes a chat participant `@bynk` with four
commands: `/explain` (the diagnostics on the cursor's line, a code given, or the
selected symbol), `/learn <topic>` (a Book lesson, using `bynk_search_book` when
registered and otherwise saying so and linking the Book), `/architecture` and
`/fix`. It orchestrates the #2463 tools through `vscode.lm.invokeTool` with the
request's `toolInvocationToken`; it adds no request of its own. It uses
`request.model` (the model the user chose; the extension holds no keys).
`/fix` returns its edit as a diff with an **Apply fix** button, never applying it
itself, and the button refuses if the lines changed since. Every answer ends
with the sources the toolchain's output gave (Book links, `path:line`), or says
it has none; the prompt tells the model to answer only from the material and to
call `bynk_check` before claiming code compiles. Desktop-only (`!isWeb`), like
the tools it needs.

**Consequences.** Hover, completion, semantic tokens and signature help are
unchanged: no language construct changes. Citations come from tool output rather
than the model's own claims, so a model cannot invent a source. `/learn` is only
as good as the Book search tool and degrades to a link without it; checking a
lesson's example against the project is left for when that tool exists.
