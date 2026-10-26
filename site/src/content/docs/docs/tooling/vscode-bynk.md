---
title: "`vscode-bynk`"
---
The Visual Studio Code extension for Bynk. It provides syntax highlighting plus
the full language-server experience by launching [`bynkc-lsp`](/docs/tooling/bynk-lsp/). For
step-by-step setup, see the how-to
[Set up editor support](/docs/editor-and-tooling/editor-support/); this page is the
reference.

The extension also has a web entry point, so it loads in vscode.dev and
github.dev. There a web worker runs `bynk-wasm` (the compiler built to
WebAssembly) for diagnostics, hover and completion across each project; see
[In the browser](#in-the-browser).

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
- **`bynk.toml` schema** — the extension associates the manifest's
  [JSON Schema](/docs/manifest/#editor-support) with every `bynk.toml`
  (`tomlValidation`), so a TOML extension that reads it (such as Even Better
  TOML) completes, documents and validates every table and key. The TOML
  extension is recommended, not required; Bynk suggests it once, when you open a
  `bynk.toml` without one.
- **Run/Debug Tests from the editor** — run and debug buttons in the editor
  title bar and right-click menu on a `.bynk` file, plus a
  <kbd>Ctrl/Cmd</kbd>+<kbd>;</kbd> chord (<kbd>;</kbd> then <kbd>t</kbd> to run,
  <kbd>;</kbd> then <kbd>d</kbd> to debug). Bynk's Command Palette and explorer
  entries are scoped to Bynk projects, so they stay out of the way elsewhere.
- **Test coverage**: a **Run with Coverage** profile in the Testing view runs
  [`bynk test --coverage --format json`](/book/guides/testing/run-tests/)
  and feeds VS Code's coverage UI: per-file line percentages in the Test
  Coverage view, and covered and uncovered lines in the gutter, all in `.bynk`
  terms. Coverage is whole-run and line-level. When the environment can't
  measure it (only `tsx`), the run shows the driver's reason and offers **Run
  Doctor**.
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
- **Explain a diagnostic** — a diagnostic whose code has a curated explanation
  offers an **Explain `<code>`** quick fix (the last entry in the lightbulb,
  never preferred over a real fix). It opens a **Bynk: Explanation** panel with
  the rule, why it exists, and an example, plus a "Read more in the Book" link.
  The text comes from the installed toolchain, so it matches the compiler that
  reported the error and needs no network: it is what `bynk explain <code>`
  prints. **Bynk: Explain Diagnostic Code…** does the same for a code picked
  from the Problems panel or typed in. (Served by the language server's
  [`bynk/explain`](/docs/tooling/bynk-lsp/#capabilities) request.)
- **Show Architecture Map** — the **Bynk: Show Architecture Map** command opens
  a webview mapping the active file's whole project: one box per context/
  adapter, an arrow per `consumes` (labelled with its selected capabilities
  where braced), and each box's capabilities/providers/services/agents —
  collapsed by default, click the `+`/`−` toggle next to a box to expand it to
  its declarations. Every box and declaration is clickable back to its source
  — a whole-project map, complementing the language server's per-handler
  sequence-diagram request. (Served by the language server's
  [`bynk/architectureModel`](/docs/tooling/bynk-lsp/#capabilities) request.)
- **Project sidebar** — the **Bynk** activity-bar view lists the project's
  units, grouped as contexts, commons, adapters, and tests. Expanding a unit
  shows its **Consumes** and **Consumed by** edges (with the capabilities a
  braced `consumes` selects) and its declarations, read from the file's
  document symbols; a service or agent expands to its handlers. Every node
  opens its source. Handler nodes carry inline **Show Sequence Diagram** and
  **Show Wire Contract** actions, unit nodes **Show Documentation**, and the
  view's title bar has **Refresh** and **Show Architecture Map**. The structure
  comes from the [`bynk/architectureModel`](/docs/tooling/bynk-lsp/#capabilities)
  request, shared with the architecture map, and refreshes on the
  `bynk.views.refresh` trigger while the view is visible. In a workspace with
  several projects, each is a top-level node.
- **Agent state writes in the sequence diagram** — for an agent's `on call`
  handler, the sequence diagram shows each write to the agent's own state as a
  note over the handler, in order and inside the branch that makes it: a `:=`
  (`status := Placed`) and each mutating store operation (`items.put(id, …)`,
  `count.update(…)`), whether it is awaited with `<-` or is a branch's last
  expression. Reads (`get`, `contains`, `Log` queries) are not shown, and nor
  is a write inside a lambda (`orders.forEach((o) => …)`), which the diagram
  does not look into. Click a note to jump to the write.
- **Agent state view** — **Bynk: Show Agent State**, the `Show State` CodeLens
  on each `agent`, or the editor title-bar button opens the state machines of
  the agent at the cursor. For each `store` Cell that holds an enum or sum type
  it draws a state diagram generated from the agent's handlers — the initial
  state, and each handler that moves the field from one state to another — and
  a handler × state table, the exact record: one row per handler, one column
  per state, each cell what that handler does from that state (`→ Placed ·
  Ok(())`, `Err(AlreadyPlaced)`). A computed write (`status := next(x)`,
  `status.update(f)`) goes to a `?` state; states no handler can reach from the
  initial one are dimmed; unchanged outcomes are in the table and, with **Show
  unchanged outcomes**, the diagram. Every cell, transition and state links to
  source. The view follows the cursor from agent to agent and highlights the
  handler under it; an agent with several state fields gets a field selector.
  An agent with no state field — a map, a counter, a log — shows its
  **effects** instead, which every agent also has under a collapsed section:
  for each handler, one row per path with its conditions, the store fields it
  writes (`items.put(id, …)`, `lastSeq :=`, each linking to the write), the
  fields it only reads, the agents it calls, and what it yields. A move a `transition` step
  invariant would refuse at commit is marked ⚠ with the transition's name, red
  when it always would (`old.status is Paid implies new.status is
  Paid` against an unguarded `status := Placed`) and amber when a computed write
  only may; a refused outcome that leaves the field unchanged is drawn even with
  unchanged outcomes hidden. Only `transition`s built from `is`, `==` and `!=`
  tests of `old.<field>` and `new.<field>` (with `!`, `&&`, `||`, `implies`) are
  checked; any other `transition` on the field is listed as not evaluated. A
  move the agent can only make on its very first commit is not flagged: the
  runtime skips `transition`s on that commit, which has no `old` state. A red
  move also reaches the Problems panel as the `bynk.transition.may_violate`
  warning; an amber one stays in the view.
- **Live model views** — the documentation page, the architecture map, and the
  **Show Sequence Diagram**, **Show Wire Contract** and **Show Agent State** views behave like a live
  preview. While a view is visible it re-requests its model after you save a
  `.bynk` file or `bynk.toml` (`bynk.views.refresh: "onSave"`, the default), as
  you type (`"onType"`, about 500 ms after the last edit), or only from its
  **Refresh** button (`"manual"`), and updates in place, keeping your scroll
  position and expanded boxes. Any `.bynk` or `bynk.toml` edit counts, since a
  page can draw on other files (a multi-file unit, a consumed context). The
  sequence and wire-contract views follow the
  cursor: move into another handler and they retarget to it, unless you click
  **Following cursor** to pin the current handler. The agent state view follows
  from agent to agent instead, and only moves its highlight within one. Open
  views come back after **Developer: Reload Window**. All five are buttons on a
  `.bynk` editor's title bar, every service and agent handler carries a `Show
  Sequence` CodeLens, every agent a `Show State` one, and `bynk.viewsCodeLens`
  adds a `Wire contract` lens beside a handler's (off by default).
- A **Get Started with Bynk** walkthrough (Welcome page → Help → walkthroughs)
  that sets up a project and a first context.
- A **`bynkc: check` build task** (Terminal → Run Task) that type-checks the
  whole project and routes errors — including in files you haven't opened — into
  the Problems panel via the `$bynkc` problem-matcher.
- A **dev server** you start from the editor: **Bynk: Start Dev Server** (or
  the `bynk: dev` task) runs [`bynk dev`](/docs/bynk-cli/#bynk-dev) in a
  terminal, for the whole project or one context you pick. A status-bar item
  shows what is serving; click it to open or copy a context's URL, or to stop or
  restart. A failing rebuild lands in the Problems panel and clears on the next
  good one. The task works as a `preLaunchTask`, counting as started once every
  context answers. The extension reads `bynk dev --format json`'s
  [lifecycle events](/docs/bynk-cli/#lifecycle-events), so it never parses the
  human report. A `mode: "dev"` debug session binds the same ports, so starting
  one while the dev server runs asks to stop it first.
- **Plan-first deploy**: **Bynk: Deploy…** picks an environment (from
  `bynk.deploy.lock`, or a new one) and a scope, then shows the
  [`bynk deploy --dry-run --format json`](/docs/bynk-cli/#bynk-deploy) plan: the
  upload order, each context's KV namespaces, queues, Durable Objects and
  secrets (required ones highlighted), and orphans. Its Deploy button runs the
  same `bynk deploy` in a terminal without `--yes`, so the driver's confirmation
  is the final gate. `--prune` is never offered, and secret values never pass
  through the editor; only a `--secrets-file` path does. **Bynk: Show Deploy
  Plan** is the preview alone.
- **Doctor**: **Bynk: Doctor** renders [`bynk doctor`](/docs/editor-and-tooling/doctor/)'s
  JSON report as a checklist grouped by capability, with each gap's remedy as
  buttons: run its command in a visible terminal after a confirmation, open a
  link or the matching setting, or copy it. It never runs on activation. A test,
  debug or dev-server failure on a missing tool offers **Run Doctor**, scoped
  with `--only`, and the walkthrough has a **Check your setup** step.
- **Tools for AI agents**: the extension registers language-model tools, so a
  chat agent in VS Code (Copilot's agent mode, or any extension calling
  `vscode.lm.invokeTool`) can ask the installed toolchain about Bynk instead of
  relying on what it remembers. Every tool is read-only. Paths in their input
  and output are workspace-relative, and lines and columns are 1-based. Refer to
  one in chat with `#`:

  | Tool | `#` reference | What it returns |
  |---|---|---|
  | `bynk_get_diagnostics` | `#bynkDiagnostics` | The errors and warnings the language server reports, with their codes, for one `file` or the workspace. |
  | `bynk_explain_diagnostic` | `#bynkExplain` | A diagnostic `code`'s explanation, example and Book link, as **Explain Diagnostic** shows it. |
  | `bynk_describe_symbol` | `#bynkSymbol` | A symbol's type, doc comment and definition: by `file`, `line` and `column` or `name`, or by `name` alone. |
  | `bynk_get_documentation` | `#bynkDocs` | Every declaration in a `file`'s unit, with its signature and doc comment. |
  | `bynk_get_architecture` | `#bynkArchitecture` | The project's contexts and adapters, their services, agents and capabilities, and what each consumes. |
  | `bynk_get_wire_contract` | `#bynkWireContract` | What the handler at a `file` and `line` accepts and returns on the wire. |
  | `bynk_check` | `#bynkCheck` | Every diagnostic [`bynk check`](/docs/bynk-cli/#bynk-check) reports for the whole project. It writes nothing. |
  | `bynk_search_book` | `#bynkBook` | The sections of the [Book](/book/) that best match a `query`, with their page links. The text comes from the installed toolchain ([`bynk book`](/docs/bynk-cli/#bynk-book)), so it matches your `bynk` version, and every result names that version. A `bynk` built without the Book bundle falls back to the hosted copy, and the result says its version may differ. |

  Each result is capped at about 12,000 characters and says when it was cut.

The extension activates on opening a `.bynk` file or any workspace containing a
`bynk.toml`.

## In the browser

In vscode.dev and github.dev the extension loads its `browser` entry point
instead of launching `bynkc-lsp`.

- **Language features:** a web-worker language server over `bynk-wasm` (the
  compiler the playground runs) gives live diagnostics, hover and completion. It
  analyses for the `cloudflare` platform, as `bynkc-lsp` does.
- **Projects:** a workspace folder with a `bynk.toml` at its root is read
  through `vscode.workspace.fs` (the manifest and every `.bynk` file, skipping
  dot-directories, `node_modules`, `out` and `target`) and analysed as a whole
  with `bynk_analyze_project`, honouring `[paths] include`/`exclude`. Every
  file's diagnostics are published, open or not, and hover and completion are
  typed in the project's context. The status bar reads **Bynk (web)**. A project
  over 2000 `.bynk` files falls back to single-file analysis.
- **Single-file:** a file outside every project is analysed on its own with
  `bynk_analyze`, and the status bar reads **Bynk (web, single-file)**. The
  following aren't reported there:
  - an unresolved `uses` of another unit (`bynk.uses.unknown_commons`), a
    `consumes` of a sibling context (`bynk.consumes.unknown_context`), or a
    suite's unresolved target (`bynk.suite.unknown_target`);
  - in a file with one of those, or in one file of a unit split across
    several, the unresolved-name, consumed-service, capability, event and type
    inference errors they cause;
  - errors that need what a lone file can't see: the `bynk.toml`
    `[unstable] allow` list, a suite's target, an adapter's binding module, or
    the file's real path.
- **Unchanged:** syntax highlighting, snippets, language configuration and
  inline doc-comment rendering.
- **Desktop-only, hidden on the web:** tests, debugging, the dev server,
  deploy, **Bynk: Doctor**, the getting-started walkthrough, tasks, scaffolding, the server download, the **Bynk** project sidebar,
  the sequence, documentation, architecture and wire-contract views,
  **Explain Diagnostic**, and the AI-agent tools other than
  `bynk_get_diagnostics` and `bynk_describe_symbol`. Semantic tokens, inlay hints, go-to-definition,
  references, rename and signature help need `bynkc-lsp`. Their settings
  (`bynk.dev.*`, `bynk.views.*`, `bynk.viewsCodeLens`, `bynk.bynkPath`,
  `bynk.executablePath` and the rest) still appear in the Settings editor,
  because VS Code can't hide a setting per host, but they have no effect on the
  web.

The wasm module is fetched when the first file is analysed, not at activation.

## Build and install

The released extension is on the
[Visual Studio Marketplace](https://marketplace.visualstudio.com/items?itemName=bynk-lang.bynk-vscode) as `bynk-lang.bynk-vscode`. To build it
yourself instead, from the `vscode-bynk/` directory:

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
| `bynk.dev.basePort` | empty (8787) | First port the dev server allocates (`bynk dev --base-port`). |
| `bynk.dev.args` | `[]` | Extra arguments forwarded to `wrangler dev` after `--`. |

## Layout

| Path | What it is |
|---|---|
| `src/extension.ts` | Entry point: resolves and launches `bynkc-lsp` over stdio. |
| `src/inlineDoc.ts` | Pure doc-comment Markdown tokenizer → heading/bold/italic ranges. |
| `src/inlineDocRendering.ts` | Applies those ranges as in-editor decorations (the `inlineDoc.ts` consumer). |
| `src/scaffold.ts` | The **New Project** / **New Context** command handlers. |
| `src/webviewHost.ts` | The shared webview substrate: CSP + per-render nonce, the payload-embedding HTML shell, and `postMessage`→reveal click-to-code — built once, consumed by both webviews below. |
| `src/sequenceDiagram.ts` | The **Show Sequence Diagram** command + panel (`bynk/sequenceModel`). |
| `src/agentStateView.ts` | The **Show Agent State** command + panel (`bynk/agentStateModel`); its page is `src/webview/state.ts`, with `state-gen.ts` (model → Mermaid `stateDiagram-v2`) and `state-map.ts` (rendered SVG → model, for click-to-code). |
| `src/documentationView.ts` | The **Show Documentation** command + panel (`bynk/documentationModel`). |
| `src/explainView.ts` | The **Explain Diagnostic Code…** command, the **Explain** quick fix's target, and its panel (`bynk/explain`). |
| `src/architectureMap.ts` | The **Show Architecture Map** command + panel (`bynk/architectureModel`). |
| `src/webview/` | The browser-context webview bundles: `main.ts`/`mermaid-gen.ts` (sequence, vendored Mermaid) and `docview.ts`/`doc-render.ts` (documentation, vendored markdown-it). |
| `src/tasks.ts` | The `bynkc: check` build-task provider. |
| `src/cli.ts`, `src/cliCore.ts` | The shared `bynk` CLI runner: spawning with the extension's settings, one-shot and streamed runs, process-tree stop, project-root lookup. |
| `src/deploy.ts`, `src/deployPlan.ts` | Plan-first deploy: the pickers, the dry-run plan view, and the terminal deploy. |
| `src/doctor.ts`, `src/doctorReport.ts` | **Bynk: Doctor**: the report view, its remedy actions, and the "Run Doctor" offer other failures use. |
| `src/cliView.ts` | The host-rendered webview the CLI views share: CSP and nonce, and buttons that post back. |
| `src/devServer.ts`, `src/devEvents.ts` | The dev-server controller: the `bynk: dev` task, its commands and status-bar item, and the `bynk dev --format json` event stream it reads. |
| `snippets/bynk.json` | Construct scaffolds, wired via `contributes.snippets`. |
| `walkthroughs/*.md` | The getting-started walkthrough steps. |
| `syntaxes/bynk.tmLanguage.json` | TextMate grammar (highlighting fallback). |
| `language-configuration.json` | Brackets, comments, and editor behaviour. |
| `package.json` | Manifest: activation events, settings, commands, build scripts. |
