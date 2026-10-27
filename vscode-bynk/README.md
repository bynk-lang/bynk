# Bynk for VS Code

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

Language support for the **[Bynk](https://github.com/bynk-lang/bynk) language**
(`.bynk` files) in Visual Studio Code: syntax highlighting plus full language
features backed by the
[`bynkc-lsp`](https://github.com/bynk-lang/bynk/tree/main/bynk-lsp) language
server, which the extension finds or downloads for you.

The extension activates on any `.bynk` file, or on any workspace containing a
`bynk.toml`.

Install it from the Visual Studio Marketplace (`bynk-lang.bynk-vscode`), or run
`code --install-extension bynk-lang.bynk-vscode`.

## Features

- **Syntax highlighting** — a TextMate grammar, mirrored from the
  [tree-sitter grammar](https://github.com/bynk-lang/bynk/tree/main/tree-sitter-bynk)
  (the source of truth).
- **Colour themes** — **Bynk Dark** and **Bynk Light**, one palette in two
  schemes (amber keywords, cyan types, teal capabilities), with colours for the
  language server's semantic tokens (capabilities, services, agents, providers,
  annotations) as well as the grammar's scopes. Pick one with *Preferences: Color
  Theme*. Every token colour holds 4.5:1 contrast on the editor background; the
  themes are the same palette as the [playground](https://playground.bynk-lang.dev).
- **Bynk in Markdown** — ` ```bynk ` and ` ~~~bynk ` fenced code blocks in
  Markdown files are highlighted in the editor with the same grammar. (VS
  Code's built-in Markdown *preview* highlights with highlight.js, not TextMate,
  so fences there stay plain.)
- **Markdown in doc comments** — the Markdown in `---` doc-blocks and `--- text`
  one-line docs is highlighted: headings, emphasis, inline code, links, list and
  quote markers, and ` ```bynk ` examples as Bynk.
- **Live diagnostics** — errors and warnings as you type, exactly as
  `bynkc check` reports them, each tagged with its dotted category.
- **Hover** — type signatures and doc blocks.
- **Go-to-definition** and **find references** for types, functions,
  capabilities, services, and agents.
- **Rename** — workspace-wide and validated.
- **Completion**, **inlay hints** (inferred types), and **semantic tokens**
  (type-aware highlighting).
- **Code actions** — quick fixes for diagnostics that carry a suggestion,
  **Fill match arms** for a non-exhaustive `match`, **Inline variable**, **Generate provider stub**,
  **Organise `uses`/`consumes`** (`source.organizeImports`) and **Fix all**
  (`source.fixAll.bynk`). Run them on save with `"editor.codeActionsOnSave":
  { "source.organizeImports": "explicit", "source.fixAll.bynk": "explicit" }`
  in a `"[bynk]"` settings block.
- **Formatting** and **range formatting** via `bynk-fmt` (honours
  `editor.formatOnSave`).
- **Document & workspace symbols** and **document highlights**.
- **Test Explorer** — run a project's `test` and `integration` blocks from the
  Testing view (or **Bynk: Run Tests**), with click-through from a failing
  assertion to its `.bynk` line. Runs `bynk test --format ndjson`, so each
  case's result appears as that case finishes, with its duration (a `bynk` that
  predates the stream is run with `--format json` instead); compile failures land in the Problems panel. **Run with Coverage** runs
  `bynk test --coverage`: the Test Coverage view shows each `.bynk` file's line
  percentage and the gutter marks covered and uncovered lines (line level; it
  needs `tsc` and `node`). On a single test case it measures that case alone and
  the run is named for it (`Coverage: <case>`); on anything else it measures the
  whole project and says so in the run output. Reports are never merged across
  runs, and a case's report is not a slice of the whole project's. A
  **`▷ Run Test | Debug Test`** CodeLens
  (and native gutter run/debug glyphs) sit at each test in the editor — disable
  the lens with `bynk.testCodeLens`.
  What you select is what runs and what is reported: any selection of cases and
  suites is one `bynk test` process, narrowed with `--suite` and a repeated
  `--case`, and only the tests you picked are reported; excluded tests are left
  out. (A `bynk` that predates the set filter is run as before: one process per
  case for a handful of cases, and the whole project for more.)
  The Run profile has a **Watch** toggle (continuous run). With it on for an
  item, saving a `.bynk` file or `bynk.toml`, or changing one outside the editor,
  reruns that item after a one-second quiet period; several saves in a burst are
  one rerun. A change during a run cancels it and starts a new one, so runs never
  overlap and a cancelled run reports nothing. Turning Watch on does not itself
  run the tests. A failure to run (a missing `bynk`, a crashed runner) is offered
  once per Watch session and noted in the run output after that, and a compile
  failure still reaches the Problems panel on each run. Debug, Coverage and
  **Run against…** are not continuous.
  A test a remote run skipped (`bynk test --against`) shows as skipped, with the
  driver's reason in the run output, not as a failure. A suite whose tests are
  all declared in one file opens that file, and **Bynk: Run Tests in This File**
  (also in the editor's context menu) runs every test declared in it. **Debug**
  on one test debugs just that case, and on a suite or several tests it debugs
  that selection as one run (a `bynk` that predates the set filter debugs the whole
  project and says so).
  Each test is tagged with its tier (`unit`, `integration` or `system`; a test's
  tier is set by an `as` clause on its `case` or `suite`), so the Testing view's
  filter selects by tier. The **Run against…** profile runs the `system` tests
  against a target: `dev` (a local `bynk dev` the driver starts and stops) or a
  target declared under `[test.targets]` in `bynk.toml`, with the last choice
  remembered. A deployed target keeps its state between runs, and the run output
  says which target ran. The extension only passes a target's name: the driver
  refuses a bare URL and a `production = true` target, and its message is shown.
  A test that uses what a deployed target cannot honour (`advance Clock`, say)
  refuses the run, with a **Retry, skipping unsupported cases** action. Run
  against a target is run-only, not offered for Debug or Coverage.
  A failed `property` or history property shows its seed and counterexample as
  parts of the failure. **Re-run with This Seed** (on the test, in the Testing
  view) runs that one test again with `--case` and `--seed` and reproduces it.
  **Pin This Seed** keeps the seed in the workspace (never in `bynk.toml` or
  source), shows it beside the test, and passes it to a run of that one test; a
  suite or whole-project run always draws a fresh seed. A pin is shown as stale,
  and not passed, once the property tests of its suite change (an added,
  removed or reordered one), because the seed could then draw a different stream; **Unpin Seed** clears it.
  A failure that an agent's `invariant` or `transition` caused names it: a line such
  as `Violated invariant Wallet.nonneg` that opens the declaration. A failed history
  gets one line for each step the agent refused (`Step 2 (Spend) was refused by
  invariant Wallet.nonneg`), because a refused step is recorded as data and would
  otherwise not show which declaration refused it; the line opens the declaration,
  and a second line opens the handler the step called when the language server can
  place it. A declaration line the driver could not place opens the failed test
  instead. A refusal in a `system` suite or a remote run is not reported, since the
  agent runs behind the service boundary and the case never sees it.
- **Debugging** — set a breakpoint in a `.bynk` file and press **Debug**: in the
  Test Explorer (Node, `bynk test --inspect`) or via a `bynk` `launch.json`
  config for the dev-server worker (workerd, `bynk dev --inspect`). Delegates to
  VS Code's JavaScript debugger; breakpoints resolve to the exact `.bynk`
  statement through the emitted source maps.
- **`bynkc: check` build task** — type-check the whole project on demand, errors
  routed to the Problems panel.
- **Project sidebar** — a **Bynk** view in the activity bar lists the project's
  contexts, commons, adapters, and test suites. Each unit expands to its
  declarations (services and agents expand to their handlers) and to
  **Consumes** / **Consumed by** groups for its `consumes` edges. Every node
  opens its source; handler nodes have inline **Sequence Diagram** and
  **Wire Contract** actions, and unit nodes **Show Documentation**. The view
  refreshes on the same trigger as the live views below.
- **Dev server** — **Bynk: Start Dev Server** (or the `bynk: dev` task) serves
  the project, or one context you pick, with `bynk dev`. A status-bar item shows
  what is serving (`dev: 3 contexts`); click it to open or copy a context's URL,
  or to stop or restart the server. A failing rebuild lands in the Problems panel
  and clears on the next good one. The task works as a `preLaunchTask`: it
  counts as started once every context answers.
- **Worker logs** — **Bynk: Show Worker Logs** opens the *Bynk Logs* output
  channel with the dev server's workers' output, tagged by context. A stack
  frame is rewritten by the driver to its `.bynk` line, so Ctrl/Cmd-click opens
  the source that threw. **Filter Worker Logs** narrows by level and context.
  The last 5,000 lines are kept in memory only; nothing is written to disk.
- **Plan-first deploy** — **Bynk: Deploy…** picks an environment and a scope,
  shows the `bynk deploy --dry-run` plan (upload order, what each context
  creates or sets, required secrets, orphans), and only then runs `bynk deploy`
  in a terminal, where the driver asks you to confirm. The extension never passes
  `--yes` or `--prune`, and never handles a secret value.
- **State migrations** — the sidebar's **State Migrations** node shows
  `bynk migrate status` for each agent class (version, floor, live and
  retirable `migrate` steps, environments) and links to its agent state view.
  **Bynk: Sweep State Migrations…** shows the sweep plan and runs `bynk migrate
  sweep` in a terminal for you to confirm there; production environments first
  ask you to type their name.
- **Doctor** — **Bynk: Doctor** shows `bynk doctor`'s readiness checklist
  (compile, test, dev and deploy, editor) with each gap's remedy as buttons: run
  it in a terminal after you confirm, open a link or setting, or copy it. A test,
  debug or dev-server failure on a missing tool offers **Run Doctor**.
- **Review pending npm requirements** — when a check, dev or deploy-plan run
  reports an npm package awaiting `bynk approve`, it appears in Problems, as a
  badge on the **Bynk** view, and in a notification with **Review…**. **Bynk:
  Review Pending npm Requirements…** shows each package, what asks for it and
  why, and approves one package at a time; **Approve all** asks you to confirm.
- **Live model views** — the sequence diagram, documentation page, architecture
  map, wire contract, and agent state view (see Commands) open beside the editor and stay current:
  they refresh when you save (or as you type, with `bynk.views.refresh`), the
  sequence and wire-contract views follow the cursor from handler to handler
  (pin one to freeze it), and open views come back after a window reload. Open
  them from the editor title bar, the Command Palette, the `Show Sequence`
  CodeLens above each handler, the `Show State` CodeLens on each agent, or an
  optional `Wire contract` CodeLens beside a handler's (`bynk.viewsCodeLens`).
  The agent state view follows the cursor from agent to agent.
- **Tools for AI agents** — language-model tools a chat agent in VS Code
  (Copilot's agent mode, or another extension) can call to ask the installed
  toolchain instead of guessing: `bynk_get_diagnostics`,
  `bynk_explain_diagnostic`, `bynk_describe_symbol`, `bynk_get_documentation`,
  `bynk_get_architecture`, `bynk_get_wire_contract` and `bynk_check`. All are
  read-only. Refer to one in chat with `#`, for example `#bynkExplain`.
- **`@bynk` chat participant** — `@bynk /explain`, `/learn`, `/architecture` and
  `/fix` in the Chat view, answering from those tools and citing their sources.
  `/fix` proposes an edit as a diff you apply with a button; it never applies one
  itself.
- **Status bar** — the active project name (click to open `bynk.toml`) and the
  language-server state (click to show its output).
- **`bynk.toml` editing** — completion, hover and validation for every table and
  key of the project manifest, from the
  [`bynk.toml` JSON Schema](https://bynk-lang.org/schemas/bynk.toml.json) this
  extension associates with it. A TOML extension that reads JSON Schemas does the
  editing: install [Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml)
  (recommended, not required; the extension suggests it once, the first time you
  open a `bynk.toml` without it).

All `.bynk` language features come from `bynkc-lsp`; the extension is the
client that provisions and launches it.

## In the browser (vscode.dev, github.dev)

The extension also runs as a web extension, so opening a Bynk repository in
[vscode.dev](https://vscode.dev) or [github.dev](https://github.dev) (press
<kbd>.</kbd> on a GitHub repository) needs no install. There is no `bynkc-lsp`
in a browser: a web worker runs the Bynk compiler built to WebAssembly
(`bynk-wasm`, the engine behind the playground) and provides:

- syntax highlighting, snippets and inline doc-comment rendering;
- live diagnostics;
- hover (the inferred type at the cursor);
- completion.

A workspace folder with a `bynk.toml` at its root is read as a **project**
(its `bynk.toml` and every `.bynk` file below it, honouring `[paths]`), and
analysed as a whole, so a `uses` of a sibling commons resolves, and every
file's errors show in the Problems panel, open or not. The status bar reads
**Bynk (web)**. A `.bynk` file outside every project is analysed **on its
own** (**Bynk (web, single-file)**): a name from another unit can't resolve
there, so those errors, and the unresolved-name errors they cause, are hidden
rather than shown as false errors. Everything that needs a process or a local
filesystem is desktop-only and hidden on the web: tests, debugging, the dev
server, deploy, **Bynk: Doctor**, the getting-started walkthrough, tasks, scaffolding, the server download, the **Bynk** project
sidebar, the sequence, documentation, architecture and wire-contract views,
diagnostic explanations, and the AI-agent tools other than
`bynk_get_diagnostics` and `bynk_describe_symbol`. Their settings still show in the Settings editor (VS
Code can't hide a setting per host) but do nothing on the web. Open the folder
in desktop VS Code for the full language server.

## The language server

The extension needs the `bynkc-lsp` binary and resolves it in this order, most
explicit first:

1. the `bynk.executablePath` setting, when set;
2. `bynkc-lsp` on your `PATH` (a dev or global install);
3. a copy previously downloaded by the extension;
4. otherwise it **downloads** the release matching this build for your platform
   from GitHub, verifies it against the release `SHA256SUMS`, and caches it.

So in the common case the extension just works — no manual server install. The
version it downloads is pinned per extension build (`bynkServerVersion` in
`package.json`); if a `bynkc-lsp` already on your `PATH` reports a different
version, the extension warns but still uses it.

If no server can be provisioned — an `executablePath` that doesn't resolve, an
unsupported platform, or a failed download — the failure is loud and actionable
(an error notification, a status-bar indicator, and commands to retry) rather
than silently degrading to grammar-only highlighting.

On a platform with no prebuilt server, build it from the workspace root and
point the setting at it:

```sh
cargo build --release -p bynk-lsp   # → target/release/bynkc-lsp
```

## The toolchain (`bynk`, `bynkc`)

Checking, testing, `bynk dev`, debugging and deploy run the `bynk` driver, which
runs `bynkc`. You don't need Rust or `cargo install` for them. The extension
resolves `bynk` the way it resolves the server:

1. the `bynk.bynkPath` setting, when set;
2. `bynk` on your `PATH`;
3. otherwise it **downloads** `bynk` and `bynkc` from the same release as the
   server, verifies them against `SHA256SUMS`, and caches them beside it.

The downloaded copies also go on the `PATH` of new integrated terminals (after
your own `PATH`, so an install of yours still wins), so `bynk dev` and
`bynk deploy` work there too. Run **Bynk: Install Toolchain** to fetch them on
demand, or set `bynk.toolchain.autoInstall` to `false` to manage them yourself.
Node, `tsc` and `wrangler` are separate prerequisites; **Bynk: Doctor** checks
them.

A release cut before these binaries were published has none to download; that
shows as "not installed" in the Bynk LSP output, and the install guide applies.

## Commands

Available from the Command Palette under **Bynk**:

| Command | What it does |
| ------- | ------------ |
| **Bynk: Restart Language Server** | Re-provision and restart the server. |
| **Bynk: Download Language Server** | Force a fresh download of the pinned server. |
| **Bynk: Show Language Server Output** | Open the "Bynk LSP" output channel. |
| **Bynk: Open Project Config (bynk.toml)** | Open the workspace's `bynk.toml`. |
| **Bynk: New Project** / **Bynk: New Context** | Scaffold a project or a context file. Works without the server, and never overwrites an existing file. |
| **Bynk: Run Tests** | Run the project's tests via `bynk test --format ndjson` and report them in the Testing view as each case finishes. |
| **Bynk: Run Tests in This File** | Run every test declared in the active `.bynk` file (or the file you right-click). |
| **Bynk: Run System Tests Against…** | Pick a target (`dev` or one from `[test.targets]`) and run the project's `system` tests against it (`bynk test --against`). |
| **Bynk: Debug Tests** | Run the tests under the inspector, so breakpoints in `.bynk` sources pause. |
| **Bynk: Show Sequence Diagram** | A sequence diagram of the handler at the cursor: its capability, context, and agent calls, and — in an agent's handler — its writes to the agent's own state, as notes. Follows the cursor into other handlers unless pinned. |
| **Bynk: Show Documentation** | The active file's declarations, rendered as a reference page. |
| **Bynk: Show Architecture Map** | A whole-project map of contexts and adapters, their `consumes` edges, and what each one binds. |
| **Bynk: Show Wire Contract** | The JSON wire shape of the service handler at the cursor. Follows the cursor into other handlers unless pinned. |
| **Bynk: Show Agent State** | The state machines of the agent at the cursor: for each `store` Cell of an enum or sum type, a state diagram generated from its handlers and a handler × state table of what each handler does from each state, with moves a `transition` would refuse at commit marked ⚠, and each handler's effects — what it writes, reads and calls on each path (shown instead for an agent with no state field). Follows the cursor from agent to agent, highlighting the handler under it. |
| **Bynk: Show Compiled Output** | The TypeScript the compiler emits for the active file, read-only, beside it. Compiled from your unsaved buffers through `bynk compile`; the cursor in either editor highlights the matching lines in the other, and the output follows your edits. |
| **Bynk: Explain Diagnostic Code…** | The installed compiler's explanation of a diagnostic code, in a panel, offline: pick a code from the Problems panel or type one. The **Explain** quick fix on a diagnostic opens the same panel. |
| **Bynk: Start Dev Server** | Serve the project with `bynk dev` in a terminal. In a multi-context project it asks which context to serve (**All contexts** first). |
| **Bynk: Stop Dev Server** | Stop the dev server and every worker it started. |
| **Bynk: Restart Dev Server** | Stop the dev server and start it again with the same contexts. |
| **Bynk: Show Worker Logs** | Open the *Bynk Logs* channel: the dev server's workers' output, with stack frames at their `.bynk` lines. |
| **Bynk: Filter Worker Logs** | Show only some levels (all, warnings and errors, errors) and some contexts. |
| **Bynk: Clear Worker Logs** | Empty the log buffer and the channel. |
| **Bynk: Deploy…** | Pick an environment and a scope, review the dry-run plan, then deploy from the plan in a terminal. `bynk` asks you to confirm before it changes anything. |
| **Bynk: Show Deploy Plan** | The same plan for review only. It never deploys. |
| **Bynk: Review Pending npm Requirements…** | Review the npm packages dependencies' adapters ask for and approve them per package (`bynk approve`). |
| **Bynk: Doctor** | Check whether this machine can compile, test, serve and deploy Bynk, with a remedy for each gap. |

The five model views (sequence diagram, documentation, architecture map, wire
contract, agent state) are also buttons on the editor title bar of a `.bynk` file. Each
refreshes on the trigger `bynk.views.refresh` sets, has a **Refresh** button of
its own, and is restored after **Developer: Reload Window**.

## Settings

| Setting | Default | Purpose |
| ------- | ------- | ------- |
| `bynk.executablePath` | `""` (auto-resolve) | Absolute path to a `bynkc-lsp` binary to use. When empty, the extension resolves the server automatically (see above). |
| `bynk.bynkPath` | `""` (`bynk` on `PATH`) | Path to the `bynk` driver used by the `bynkc: check` task, the Test Explorer, debugging, and the dev server. When empty, `bynk` is taken from `PATH`, then from the toolchain the extension downloaded. |
| `bynk.toolchain.autoInstall` | `true` | Download `bynk` and `bynkc` when neither `bynk.bynkPath` nor `PATH` has a `bynk`. |
| `bynk.compilerPath` | `""` (driver resolves `bynkc`) | Pin an exact `bynkc`, passed through as `BYNK_BYNKC` to `bynk`. When empty, `bynk` resolves `bynkc` itself (`PATH`, then a sibling of `bynk`) — richer than a bare `PATH` lookup, so a driver-first install (`bynkc` reachable only via `BYNK_BYNKC` or as a `bynk` sibling) still works. |
| `bynk.trace.server` | `off` | Trace LSP protocol traffic (`off` / `messages` / `verbose`) in the "Bynk LSP" output channel. |
| `bynk.inlayHints.enable` | `true` | Show Bynk inlay hints (both kinds below). A persistent, Bynk-only preference; takes effect on the next edit or scroll. |
| `bynk.inlayHints.types` | `true` | Inferred-type hints on `let` bindings and lambda parameters. |
| `bynk.inlayHints.parameterNames` | `true` | Parameter-name hints at call arguments. |
| `bynk.testCodeLens` | `true` | Show a `Run Test \| Debug Test` CodeLens above each test case: **Run Test** runs just that case (`bynk test --case <name>`), **Debug Test** runs it under the inspector. The native gutter icons appear regardless. |
| `bynk.debug.semanticValues` | `true` | Show values in the debugger in Bynk's vocabulary (`Ok(42)`, not `{tag: "Ok", value: 42}`), including the inline values beside the code while paused (VS Code's `debug.inlineValues`). |
| `bynk.inlineDocRendering.enable` | `true` | Render `--- … ---` doc comments in place with light Markdown styling while reading the source. |
| `bynk.views.refresh` | `onSave` | When open model views re-request their model: `onSave`, `onType` (about 500 ms after the last edit), or `manual` (the view's Refresh button). A hidden view catches up when shown. |
| `bynk.viewsCodeLens` | `false` | Show a `Wire contract` CodeLens above each service and agent handler, beside the `Show Sequence` lens. |
| `bynk.dev.basePort` | empty (8787) | First port the dev server allocates (`bynk dev --base-port`): context *i* is served on this port + *i*. |
| `bynk.dev.args` | `[]` | Extra arguments forwarded to `wrangler dev` after `--` (for example `["--var", "KEY:value"]`). |

A `bynk: dev` task in `tasks.json` can narrow the contexts and move the ports,
and serve as a debug configuration's `preLaunchTask`:

```json
{
  "label": "serve orders",
  "type": "bynk",
  "command": "dev",
  "contexts": ["commerce.orders"],
  "basePort": 9000
}
```

The dev server and a `mode: "dev"` debug session bind the same ports, so
starting that debug session while the dev server runs asks whether to stop it
first.

Two built-in VS Code settings also apply:

- **`editor.inlayHints.enabled`** — the instant, editor-wide on/off for inlay hints (toggles immediately). Use `bynk.inlayHints.enable` when you want hints off for Bynk specifically and left alone elsewhere.
- **`editor.semanticHighlighting.enabled`** — turns semantic tokens (the type-aware highlighting) on or off. The extension ships theme fallbacks for Bynk's `capability` / `service` / `agent` / `provider` / `actor` / `messages` / `asset` token types, so they colour out of the box.

## Build & install from source

From this directory:

```sh
npm install
npm run package                       # bundle, then package a .vsix
code --install-extension bynk-vscode-*.vsix
```

`npm run package` bundles the extension with [esbuild](https://esbuild.github.io/)
and packages it with [`@vscode/vsce`](https://github.com/microsoft/vscode-vsce)
(both dev dependencies). Use `npm run build` alone for a plain bundle, or
`npm run watch` while developing — it rebuilds `src/extension.ts` only, so
re-run `npm run build` after changing a webview under `src/webview/`.
`npm run check` type-checks and `npm test` runs the extension tests.

The web extension needs the `bynk-wasm` module, built from the repository with
the `wasm32-unknown-unknown` Rust target and
[`wasm-bindgen-cli`](https://crates.io/crates/wasm-bindgen-cli) (the version in
the workspace `Cargo.lock`): `npm run build:wasm` stages it in `wasm/`, and
`npm run build:web` bundles `out/web/`. `npm run package` runs both bundles, so
packaging needs the wasm staged first. `npm run test:web` runs the web smoke
tests in a headless Chromium with
[`@vscode/test-web`](https://github.com/microsoft/vscode-test-web) (first
`npx playwright install chromium`).

See also
[Set up editor support](https://bynk-lang.org/docs/editor-and-tooling/editor-support/)
for using `bynkc-lsp` with other editors.

## License

Licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at
your option.
