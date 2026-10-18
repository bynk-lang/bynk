---
title: "`vscode-bynk`"
---
The Visual Studio Code extension for Bynk. It provides syntax highlighting plus
the full language-server experience by launching [`bynkc-lsp`](/docs/tooling/bynk-lsp/). For
step-by-step setup, see the how-to
[Set up editor support](/docs/editor-and-tooling/editor-support/); this page is the
reference.

## Features

- Syntax highlighting (a TextMate grammar, mirrored from
  [`tree-sitter-bynk`](/docs/tooling/tree-sitter-bynk/)).
- Live diagnostics, hover with type signatures and doc blocks, and
  go-to-definition — all from the bundled `bynkc-lsp`.
- Format-on-save via the shared formatter (honours `editor.formatOnSave`).
- Status-bar items showing the project name and compiler version.
- **Snippets** for every construct — type a prefix (`context`, `commons`,
  `type`, `enum`, `fn`, `capability`, `provides`, `service`, `on http`,
  `on cron`, `agent`, `actor`, `suite`, `property`, `uses`, `consumes`) and press
  <kbd>Tab</kbd> to scaffold it, then tab through the placeholders.
- **Scaffolding commands** — **Bynk: New Project** (scaffolds `bynk.toml` +
  `src/<name>.bynk`) and **Bynk: New Context** (adds a `context` file). Both
  refuse to overwrite an existing file.
- **Run/Debug Tests from the editor** — run and debug buttons in the editor
  title bar and right-click menu on a `.bynk` file, plus a
  <kbd>Ctrl/Cmd</kbd>+<kbd>;</kbd> chord (<kbd>;</kbd> then <kbd>t</kbd> to run,
  <kbd>;</kbd> then <kbd>d</kbd> to debug). Bynk's Command Palette and explorer
  entries are scoped to Bynk projects, so they stay out of the way elsewhere.
- **Per-case run/debug lenses** — a `▷ Run Test` / `Debug Test` CodeLens above
  each `case` runs (or debugs) *that* case, not the whole project — it shells
  `bynkc test --case "<name>"`, the CLI's exact-name run filter.
- **Provider CodeLens on capabilities** — a `capability` declaration shows an
  `N providers` lens listing the services that `provides` it, alongside the
  `N references` lens; click to peek the provider definitions.
- **Refinement-family CodeLens** — a refined/opaque `type` (or plain alias) over
  a builtin base shows an `N refinements of String` lens listing every type over
  the same base across the project; click to peek the family. (A lone refinement
  gets no lens — there is nothing to navigate to.)
- **Inline doc-comment rendering** — doc comments (`--- … ---`) render in place
  with light Markdown affordances while you read the source: heading lines are
  coloured (a themeable `bynk.docHeadingForeground`, paired with bold so the cue
  is not colour-only), `**bold**` renders bold, and `*italic*` renders italic.
  Markers stay visible and the font size is never touched, so line height is
  stable (style-in-place, not conceal-and-reveal). The `[Name]` links inside a
  doc comment are clickable via [document links](/docs/tooling/bynk-lsp/#capabilities).
  Toggle with `bynk.inlineDocRendering.enable`.
- **Show Documentation** — the **Bynk: Show Documentation** command opens a
  webview rendering the current unit's declarations (every file of a multi-file
  unit, under its module doc) as a reference page ("live
  rustdoc for Bynk"): each declaration's heading, signature, and doc comment
  rendered as Markdown, in outline order and hierarchy, each heading clickable
  back to its source. Undocumented declarations appear with a *No documentation*
  note so the page doubles as a doc-coverage view (a toggle hides them for a
  clean read). Doc-comment Markdown is rendered with HTML disabled; the page
  reads the source's own doc comments, distinct from this documentation site.
  (Served by the language server's
  [`bynk/documentationModel`](/docs/tooling/bynk-lsp/#capabilities) request.)
- **Show Architecture Map** — the **Bynk: Show Architecture Map** command opens
  a webview mapping the active file's whole project: one box per context/
  adapter, an arrow per `consumes` (labelled with its selected capabilities
  where braced), and each box's capabilities/providers/services/agents —
  collapsed by default, click the `+`/`−` toggle next to a box to expand it to
  its declarations. Every box and declaration is clickable back to its source
  — a whole-project map, complementing the language server's per-handler
  sequence-diagram request. (Served by the language server's
  [`bynk/architectureModel`](/docs/tooling/bynk-lsp/#capabilities) request.)
- **Live model views** — the documentation page, the architecture map, and the
  **Show Sequence Diagram** and **Show Wire Contract** views behave like a live
  preview. While a view is visible it re-requests its model after you save a
  `.bynk` file or `bynk.toml` (`bynk.views.refresh: "onSave"`, the default), as
  you type (`"onType"`, about 500 ms after the last edit), or only from its
  **Refresh** button (`"manual"`), and updates in place, keeping your scroll
  position and expanded boxes. The documentation page follows its own file and
  the map the whole project. The sequence and wire-contract views follow the
  cursor: move into another handler and they retarget to it, unless you click
  **Following cursor** to pin the current handler. Open views come back after
  **Developer: Reload Window**. All four are buttons on a `.bynk` editor's
  title bar, every service and agent handler carries a `Show Sequence`
  CodeLens, and `bynk.viewsCodeLens` adds a `Wire contract` lens beside it
  (off by default).
- A **Get Started with Bynk** walkthrough (Welcome page → Help → walkthroughs)
  that sets up a project and a first context.
- A **`bynkc: check` build task** (Terminal → Run Task) that type-checks the
  whole project and routes errors — including in files you haven't opened — into
  the Problems panel via the `$bynkc` problem-matcher.

The extension activates on opening a `.bynk` file or any workspace containing a
`bynk.toml`.

## Build and install

From the `vscode-bynk/` directory:

```sh
npm install
npm run build           # tsc -p .
npx vsce package        # produces a .vsix
code --install-extension bynk-vscode-*.vsix
```

The extension needs `bynkc-lsp` available — build it with
`cargo build --release -p bynk-lsp` and put it on `PATH`, or set
`bynk.executablePath`.

## Settings

| Setting | Default | Purpose |
|---|---|---|
| `bynk.executablePath` | `bynkc-lsp` | Path to the language-server binary. |
| `bynk.compilerPath` | `bynkc` | Path to the `bynkc` compiler used by the `bynkc: check` build task. |
| `bynk.trace.server` | `off` | Trace LSP traffic (`off` / `messages` / `verbose`) in the "Bynk LSP" output channel. |
| `bynk.inlineDocRendering.enable` | `true` | Render doc comments in place with heading colour, bold, and italic. |

## Layout

| Path | What it is |
|---|---|
| `src/extension.ts` | Entry point: resolves and launches `bynkc-lsp` over stdio. |
| `src/inlineDoc.ts` | Pure doc-comment Markdown tokenizer → heading/bold/italic ranges. |
| `src/inlineDocRendering.ts` | Applies those ranges as in-editor decorations (the `inlineDoc.ts` consumer). |
| `src/scaffold.ts` | The **New Project** / **New Context** command handlers. |
| `src/webviewHost.ts` | The shared webview substrate: CSP + per-render nonce, the payload-embedding HTML shell, and `postMessage`→reveal click-to-code — built once, consumed by both webviews below. |
| `src/sequenceDiagram.ts` | The **Show Sequence Diagram** command + panel (`bynk/sequenceModel`). |
| `src/documentationView.ts` | The **Show Documentation** command + panel (`bynk/documentationModel`). |
| `src/architectureMap.ts` | The **Show Architecture Map** command + panel (`bynk/architectureModel`). |
| `src/webview/` | The browser-context webview bundles: `main.ts`/`mermaid-gen.ts` (sequence, vendored Mermaid) and `docview.ts`/`doc-render.ts` (documentation, vendored markdown-it). |
| `src/tasks.ts` | The `bynkc: check` build-task provider. |
| `snippets/bynk.json` | Construct scaffolds, wired via `contributes.snippets`. |
| `walkthroughs/*.md` | The getting-started walkthrough steps. |
| `syntaxes/bynk.tmLanguage.json` | TextMate grammar (highlighting fallback). |
| `language-configuration.json` | Brackets, comments, and editor behaviour. |
| `package.json` | Manifest: activation events, settings, commands, build scripts. |
