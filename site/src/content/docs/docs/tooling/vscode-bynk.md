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
  clean read). An intra-doc link (`[Name]`) in a doc comment is clickable: it
  scrolls to the target's section when the page documents it, and otherwise
  opens the declaration in the editor. Doc-comment Markdown is rendered with HTML disabled; the page
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
- **Show Compiled Output** — **Bynk: Show Compiled Output** (a button on a
  `.bynk` editor's title bar) opens the TypeScript the compiler emits for the
  current file in a read-only editor beside it. It compiles the project to a
  scratch directory with
  [`bynk compile --sources-stdin`](/docs/bynk-cli/#bynk-compile), handing it your
  **unsaved buffers**, so what you see is what is on screen, and the view
  follows your edits (about half a second after you stop typing) as well as
  saves. Each update is a full compile in a fresh process, a few tens of
  milliseconds on the example projects, and happens only while the view is open.
  Moving the cursor in either editor highlights the matching lines in the other
  (it does not lock scrolling), through the emitter's source map, which is
  line-level: a generated line maps to the statement it came from. Between an
  edit and the rebuild the highlight is paused, as the lines no longer line up.
  If the project stops compiling the view keeps the last good output. A file
  that compiles to several modules (a context's worker entry and its handlers
  in a Workers build) asks which to show. The shared runtime
  (`__bynk_runtime/…`) belongs to no one file, so it is not part of a file's
  output. Needs `bynk` and `bynkc` (the extension's managed toolchain
  provides them); the output is a preview build and never touches
  `bynk.schema.lock`.
- **Explore a refined type** — hovering a refined type (`type Age = Int where
  InRange(0, 150)`) lists its base and predicates in words, with a **Try
  values…** link. The link opens a **Bynk: Refined Type** panel: a few example
  values (the same list every time, from a fixed seed) and a box that checks a
  value you type against the predicate, naming every clause that fails. Click an
  example to check it. The check runs in the extension host over the very
  condition the compiled `.of` uses, with a time limit, and runs none of your
  project's code. A `Matches` type lists no examples unless a generated one
  happens to match, as `Val[T]` needs a pin there. (Served by the language
  server's [`bynk/refinedTypeModel`](/docs/tooling/bynk-lsp/#capabilities)
  request.)
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
- **Explorer file decorations** — a two-letter badge on each context's
  files in the file Explorer, with the full context name in the tooltip
  (`order.billing` → `OB`, `billing` → `BI`), and a red tint on a file, and the
  folders above it, whose tests failed in the last run. A run that passes the
  failing tests clears the tint. A test file carries the badge of the context
  it tests; commons and adapters carry none. The badge shows by default only in
  a project with more than one context; `bynk.fileDecorations.contextBadge`
  (`auto`, `always`, `never`) and `bynk.fileDecorations.failedTests` switch each
  decoration, and the tint's colour is `bynk.testFailedForeground`.
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
- **Worker logs** while the dev server runs: **Bynk: Show Worker Logs** opens
  the *Bynk Logs* output channel with every context's output, each line tagged
  with its context. A stack frame that named generated TypeScript is already at
  its `.bynk` location (the driver rewrites it through the compiler's source
  maps, see [Worker logs](/docs/bynk-cli/#worker-logs)), so **Ctrl/Cmd-click**
  opens the line that threw. **Bynk: Filter Worker Logs** narrows the view by
  level and by context, and **Bynk: Clear Worker Logs** empties it. The view keeps
  the last 5,000 lines in memory only: nothing is written to disk, and a
  closed window forgets them. A restart adds a marker line rather than clearing.
- **Plan-first deploy**: **Bynk: Deploy…** picks an environment (from
  `bynk.deploy.lock`, or a new one) and a scope, then shows the
  [`bynk deploy --dry-run --format json`](/docs/bynk-cli/#bynk-deploy) plan: the
  upload order, each context's KV namespaces, queues, Durable Objects and
  secrets (required ones highlighted), and orphans. Its Deploy button runs the
  same `bynk deploy` in a terminal without `--yes`, so the driver's confirmation
  is the final gate. `--prune` is never offered, and secret values never pass
  through the editor; only a `--secrets-file` path does. **Bynk: Show Deploy
  Plan** is the preview alone.
- **Dependencies and the registry**: every registry question goes through
  [`bynk`](/docs/bynk-cli/), never the editor's own HTTP, so routing, trust and
  tokens stay in one place.
  - The Project sidebar gets a **Dependencies** node (when `bynk.toml` has any)
    listing each dependency with the version `bynk.lock` pins and where it comes
    from, from [`bynk deps`](/docs/bynk-cli/#bynk-deps).
  - **Bynk: Add Dependency…** takes an `org/package`, looks it up with
    [`bynk search`](/docs/bynk-cli/#bynk-search), offers its usable versions
    (newest first; yanked and withdrawn ones are left out) and runs
    [`bynk add`](/docs/bynk-cli/#bynk-add). A registry's index has no free-text
    search, so the name must be exact. **Bynk: Fetch Dependencies** runs
    [`bynk fetch`](/docs/bynk-cli/#bynk-fetch).
  - **Upgrade lenses** in `bynk.toml` (`0.3.1 → 0.4.0 available`) appear above a
    registry dependency whose registry offers a newer version; clicking one
    rewrites its range to `^0.4.0` and fetches. They are on by default
    (`bynk.dependencies.upgradeLenses`), look each package up at most once per
    session, in the background, and show nothing for a registry that can't be
    reached.
  - **Bynk: Publish…** runs `bynk publish --dry-run --format json` first and
    shows the package, version, registry, checksum and file list in a
    confirmation; only its **Publish** button runs the real
    [`bynk publish`](/docs/bynk-cli/#bynk-publish). The first-party registry
    isn't live yet, so these work today against registries routed in
    `[registries]` (see [run a private registry](/book/guides/projects-build-and-deployment/run-a-private-registry/)).
- **State migrations**: the **Bynk** sidebar's **State Migrations** node lists
  each agent class from [`bynk migrate status --format json`](/docs/bynk-cli/#bynk-migrate-status)
  (offline; the extension never calls Cloudflare): its version and floor, its
  `migrate` steps (those the floor has passed are marked as retirable), any
  version with no path, and the environments it is deployed to. Selecting a
  class opens its [agent state view](#features). **Bynk: Sweep State
  Migrations…** picks an environment and a scope and shows the sweep plan; its
  Sweep button runs [`bynk migrate sweep`](/docs/bynk-cli/#bynk-migrate-sweep)
  in a terminal without `--yes`, where `bynk` lists the instances and asks you
  to confirm. A sweep that touches `default`, `prod`, `production` or `live` (or
  all environments, when one of those is recorded) first asks you to type the
  environment's name. **Bynk: Refresh State Migrations** reruns the status.
- **Doctor**: **Bynk: Doctor** renders [`bynk doctor`](/docs/editor-and-tooling/doctor/)'s
  JSON report as a checklist grouped by capability, with each gap's remedy as
  buttons: run its command in a visible terminal after a confirmation, open a
  link or the matching setting, or copy it. It never runs on activation. A test,
  debug or dev-server failure on a missing tool offers **Run Doctor**, scoped
  with `--only`, and the walkthrough has a **Check your setup** step.
- **Playground links**: **Bynk: Copy Playground Link** puts a
  [playground](https://playground.bynk-lang.dev) URL for the selection on the
  clipboard, and **Bynk: Open in Playground** opens it in a browser. Both are in
  the editor context menu and the Command Palette, and work on the web too. With
  no selection, a single-file program (one with no `bynk.toml` above it) is shared
  whole; a file in a project asks you to select instead, because it depends on
  its sibling units, and a selection that leaves out the file's `uses` clauses gets a
  warning, since names from those units won't resolve in the playground. The code is compressed into the URL's fragment
  (`#base64url(deflate-raw(utf8))`, the format By Example's links use), which a
  browser never sends to a server. A snippet whose link would pass 16,384
  characters is refused with a message, never truncated. The link is made only
  when you run the command.
- **Review pending npm requirements**: when a `check`, `dev` or deploy-plan run
  reports `bynk.requires.unapproved`, the extension reads
  [`bynk approve --list --format json`](/docs/bynk-cli/#bynk-approve) (it never
  polls) and shows what awaits approval as a Problems entry on `bynk.lock`, a
  badge on the **Bynk** project view, and one notification with **Review…**.
  **Bynk: Review Pending npm Requirements…** opens the review: each package, its
  range, the chain of dependencies that requires it, and its npm page, with the
  new or range-changed ones first and what is already approved folded away.
  **Approve** is per package; **Approve all** is secondary and asks for an
  explicit confirmation, because an approved package runs on your next
  `npm install`. The approval is `bynk approve <package>`, recorded in
  `bynk.lock`.
- **Tools for AI agents**: the extension registers language-model tools, so a
  chat agent in VS Code (Copilot's agent mode, or any extension calling
  `vscode.lm.invokeTool`) can ask the installed toolchain about Bynk instead of
  relying on what it remembers. (Agents outside VS Code get the same tools from
  [`bynk mcp`](/docs/tooling/bynk-mcp/).) Every tool is read-only. Paths in their input
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

- **`@bynk` in the Chat view**: a chat participant for learning and working in
  Bynk. It answers with the model you picked in chat, from what the tools above
  return, and ends each answer with its sources (a Book link, a diagnostic code
  or a `path:line` location), or says it has none. It tells the model to call
  `bynk_check` before claiming code compiles.

  | Command | What it does |
  |---|---|
  | `@bynk /explain` | Explains the diagnostics on the cursor's line (or the file's), a code you give (`@bynk /explain bynk.resolve.unknown_type`), or the selected symbol, quoting the toolchain's explanation and linking the Book. |
  | `@bynk /learn <topic>` | A short lesson from the Book. It needs the `bynk_search_book` tool; without it, `/learn` says so and links the Book rather than teaching from memory. |
  | `@bynk /architecture` | Walks through the project's contexts, services and agents, from `bynk_get_architecture`. |
  | `@bynk /fix` | Proposes an edit for the current error as a diff, with an **Apply fix** button. Nothing changes until you click it, and the button refuses if the lines have changed since. |

  Desktop only: it needs the same language server and CLI as those tools.

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
  inline doc-comment rendering. The two playground-link commands work too.
- **Desktop-only, hidden on the web:** tests, debugging, the dev server,
  deploy, **Bynk: Doctor**, the getting-started walkthrough, tasks, scaffolding, the server download, the **Bynk** project sidebar, the Explorer file decorations,
  the sequence, documentation, architecture and wire-contract views,
  **Explain Diagnostic**, the `@bynk` chat participant, and the AI-agent tools
  other than `bynk_get_diagnostics` and `bynk_describe_symbol`. Semantic tokens, inlay hints, go-to-definition,
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
| `bynk.fileDecorations.contextBadge` | `auto` | Explorer badge naming a file's context: `auto` (only in a project with several contexts), `always`, or `never`. |
| `bynk.fileDecorations.failedTests` | `true` | Tint a file red in the Explorer when its tests failed in the last run. |
| `bynk.dev.basePort` | empty (8787) | First port the dev server allocates (`bynk dev --base-port`). |
| `bynk.dev.args` | `[]` | Extra arguments forwarded to `wrangler dev` after `--`. |
| `bynk.dependencies.upgradeLenses` | `true` | Show `0.3.1 → 0.4.0 available` lenses above registry dependencies in `bynk.toml`. |

## Layout

| Path | What it is |
|---|---|
| `src/extension.ts` | Entry point: resolves and launches `bynkc-lsp` over stdio. |
| `src/inlineDoc.ts` | Pure doc-comment Markdown tokenizer → heading/bold/italic ranges. |
| `src/inlineDocRendering.ts` | Applies those ranges as in-editor decorations (the `inlineDoc.ts` consumer). |
| `src/playgroundLink.ts`, `src/playgroundCommands.ts` | The playground deep link (the encoding and its size cap, pure) and the **Copy Playground Link** / **Open in Playground** commands, shared by both entry points. |
| `src/scaffold.ts` | The **New Project** / **New Context** command handlers. |
| `src/webviewHost.ts` | The shared webview substrate: CSP + per-render nonce, the payload-embedding HTML shell, and `postMessage`→reveal click-to-code — built once, consumed by both webviews below. |
| `src/sequenceDiagram.ts` | The **Show Sequence Diagram** command + panel (`bynk/sequenceModel`). |
| `src/agentStateView.ts` | The **Show Agent State** command + panel (`bynk/agentStateModel`); its page is `src/webview/state.ts`, with `state-gen.ts` (model → Mermaid `stateDiagram-v2`) and `state-map.ts` (rendered SVG → model, for click-to-code). |
| `src/documentationView.ts` | The **Show Documentation** command + panel (`bynk/documentationModel`). |
| `src/explainView.ts` | The **Explain Diagnostic Code…** command, the **Explain** quick fix's target, and its panel (`bynk/explain`). |
| `src/refinedExplorer.ts` | The **Explore Refined Type** command (the hover's **Try values…** target), the hover-trust middleware and its panel (`bynk/refinedTypeModel`); `refinedEval.ts` checks a candidate against the clauses. |
| `src/architectureMap.ts` | The **Show Architecture Map** command + panel (`bynk/architectureModel`). |
| `src/webview/` | The browser-context webview bundles: `main.ts`/`mermaid-gen.ts` (sequence, vendored Mermaid) and `docview.ts`/`doc-render.ts` (documentation, vendored markdown-it). |
| `src/tasks.ts` | The `bynkc: check` build-task provider. |
| `src/cli.ts`, `src/cliCore.ts` | The shared `bynk` CLI runner: spawning with the extension's settings, one-shot and streamed runs, process-tree stop, project-root lookup. |
| `src/deploy.ts`, `src/deployPlan.ts` | Plan-first deploy: the pickers, the dry-run plan view, and the terminal deploy. |
| `src/migrationsView.ts`, `src/migrationsCore.ts` | The State Migrations node's status store, the plan-first sweep, and its pure core. |
| `src/doctor.ts`, `src/doctorReport.ts` | **Bynk: Doctor**: the report view, its remedy actions, and the "Run Doctor" offer other failures use. |
| `src/approveRequirements.ts`, `src/approveReport.ts` | **Bynk: Review Pending npm Requirements…**: finding pending requirements in driver output, the Problems entry and badge, and the per-package review. |
| `src/cliView.ts` | The host-rendered webview the CLI views share: CSP and nonce, and buttons that post back. |
| `src/devServer.ts`, `src/devEvents.ts` | The dev-server controller: the `bynk: dev` task, its commands and status-bar item, and the `bynk dev --format json` event stream it reads. |
| `src/devLogs.ts`, `src/devLogsView.ts` | The Bynk Logs view: the bounded in-memory buffer of `log` events, its level and context filter, and the output channel that shows it. |
| `snippets/bynk.json` | Construct scaffolds, wired via `contributes.snippets`. |
| `walkthroughs/*.md` | The getting-started walkthrough steps. |
| `syntaxes/bynk.tmLanguage.json` | TextMate grammar (highlighting fallback). |
| `language-configuration.json` | Brackets, comments, and editor behaviour. |
| `package.json` | Manifest: activation events, settings, commands, build scripts. |
