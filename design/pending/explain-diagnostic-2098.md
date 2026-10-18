---
level: minor
changelog: A diagnostic with a curated explanation offers an **Explain** quick fix in VS Code that shows the installed compiler's own explanation in a panel, offline, served by the new `bynk/explain` language-server request; **Bynk: Explain Diagnostic Code…** looks up any code
---

## ADR: lsp-explain-request
title: The language server serves diagnostic explanations over a custom `bynk/explain` request
summary: `bynk/explain { code } → { title, markdown, href } | null`, advertised as `experimental.explain`, plus a client-gated Explain quick fix

**Context.** Diagnostics carry a `codeDescription` link to the Book when the
compiler curates an explanation for their code (#853). Following it needs a
network, leaves the editor, and can show text for a different compiler version
than the installed one. `bynk explain <code>` prints the same curated blurb,
example and link offline, but the editor did not use it. The explanation table
(`bynk_syntax::diagnostics::EXPLANATIONS`) is compiled into the language server
already, so the server holds the exact text for every code it emits (#2098).

This is a `minor` increment because it adds author-facing editor surface: a new
LSP request, a code action and a VS Code command.

**Decision.** Transport (DECISION A): a custom request,
`bynk/explain { code } → { title, markdown, href } | null`, advertised under
`ServerCapabilities.experimental.explain` like the other custom requests. It is
keyed by the code alone (no document, no analysis round), so a client can also
look up a code seen in CI output. `title` is the code, `markdown` is the blurb
then the example as a fenced `bynk` block with no link and no raw HTML, and
`href` is the hosted-Book URL the `codeDescription` link already uses. `null`
for any code with no curated explanation; the client words the fallback. The
alternative, shelling `bynk explain` from the extension, adds a process and
needs `bynk` on `PATH` even where only the server is installed.

Surface (DECISION B): a code action plus a command, leaving hover and the
`codeDescription` link as they are. For each distinct curated code among the
diagnostics in the requested range, `codeAction` appends one `quickfix` titled
``Explain `<code>` ``, after every real fix and never preferred. It carries no
edit, only the client-side command `bynk.explainDiagnostic` with the code as its
argument. Because the client executes that command, the action is offered only
to a client that advertises `experimental.explainDiagnosticCommand: true` in its
`ClientCapabilities`; the VS Code extension does. The extension renders the
result in a reusable "Bynk: Explanation" webview through the documentation
view's markdown-it renderer and CSP, and its "Bynk: Explain Diagnostic Code…"
command looks up a code picked from the Problems panel or typed in.

**Consequences.** The in-editor explanation is offline and matches the installed
compiler by construction, and it cannot drift from `bynk explain` or the
`codeDescription` link, since all three read one table. A client other than VS
Code sees no extra lightbulb entry unless it opts in through the client
capability and implements the command. An uncurated code offers no action, and
the command reports it with a pointer to the diagnostic index. The example is
shown as a plain code block; it is not syntax-highlighted, the same as the
documentation view's signatures.
