# 0616 — The VS Code extension exposes the toolchain to chat agents as read-only language-model tools

- **Status:** Accepted (v0.406)

**Context.** Bynk is young and pre-1.0, so a general-purpose model has little
training data on it, and what it has dates as the language moves. A model
asked to write or fix Bynk invents syntax and misreads diagnostics. The
toolchain already holds exact, version-matched answers to most of its
questions: the explanation of a code (`bynk/explain`), a unit's declarations
(`bynk/documentationModel`), the architecture (`bynk/architectureModel`), a
handler's wire contract (`bynk/wireContract`), hover and definition, and
`bynk check`. Until now they were reachable only through panels a model can't
call. VS Code's language-model tool API (`contributes.languageModelTools`,
`vscode.lm.registerTool`) lets an extension offer callable tools to any chat
agent in the editor.

**Decision.**

- The extension registers seven tools: `bynk_get_diagnostics`,
  `bynk_explain_diagnostic`, `bynk_describe_symbol`, `bynk_get_documentation`,
  `bynk_get_architecture`, `bynk_get_wire_contract` and `bynk_check`.
- Each is a thin adapter over a request or command the extension already
  makes, so the language server is unchanged.
- **Read-only.** No tool edits files or starts anything long-lived, so none asks
  for confirmation. Tools that change state (running tests, formatting,
  applying a fix, the dev server, deploying) are left for a follow-up, where
  each can be weighed for its side effects.
- **Markdown output.** Results are compact Markdown, not the raw JSON models: an
  agent reads prose and short lists more reliably.
  - Locations are workspace-relative `path:line:column`, 1-based in both input
    and output, as an agent quotes them.
  - Each result is capped (about 12,000 characters, less when the caller gives a
    token budget) and says when it was cut and how to narrow the call.
  - The renderers are a pure module, unit-tested without VS Code.
- **Web.** On the web, only the two tools that need nothing but the VS Code API
  are registered (`bynk_get_diagnostics`, `bynk_describe_symbol`). The rest are
  `when: "!isWeb"`, because the wasm language server doesn't serve the custom
  requests and there is no `bynk` process to run.
- **Guidance.** Each tool's `modelDescription` opens by telling the model that
  Bynk is young and not to rely on remembered syntax. This puts the guidance on
  every tool without depending on a separate chat-instructions contribution.
- **Book search.** Searching the Bynk Book is not in this set. Where its text
  should come from (the installed toolchain, the extension, or the website) is
  open in #2463 and left to a follow-up.

**Consequences.** An agent in VS Code can ground its Bynk in the installed
toolchain, quoting the same explanations and locations a person sees.

The tool names and input schemas are a surface agents and prompts may come to
depend on, so they change as deliberately as a command id. Adding a field is
safe; renaming a tool or a field is a break.

The tools reuse the view requests, so a change to a model's wire shape now also
reaches `lmRender.ts`. The hand-mirrored types in `webview/*Types.ts` keep both
in step.
