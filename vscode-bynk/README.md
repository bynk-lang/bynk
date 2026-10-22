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
  assertion to its `.bynk` line. Runs `bynk test --format json`; compile
  failures land in the Problems panel. **Run with Coverage** runs
  `bynk test --coverage`: the Test Coverage view shows each `.bynk` file's line
  percentage and the gutter marks covered and uncovered lines (whole-run, line
  level; it needs `tsc` and `node`). A **`▷ Run Test | Debug Test`** CodeLens
  (and native gutter run/debug glyphs) sit at each test in the editor — disable
  the lens with `bynk.testCodeLens`.
  What you select is what runs and what is reported: a handful of cases run as one
  `bynk test --case` process each, and a suite, or more cases than that, runs the
  project once and reports only the ones you picked; excluded tests are left out.
  A test a remote run skipped (`bynk test --against`) shows as skipped, with the
  driver's reason in the run output, not as a failure. A suite whose tests are
  all declared in one file opens that file, and **Bynk: Run Tests in This File**
  (also in the editor's context menu) runs every test declared in it. **Debug**
  on one test debugs just that case; on a suite or several tests it debugs the
  whole project and says so, because the debugger filters by a single case.
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
- **Plan-first deploy** — **Bynk: Deploy…** picks an environment and a scope,
  shows the `bynk deploy --dry-run` plan (upload order, what each context
  creates or sets, required secrets, orphans), and only then runs `bynk deploy`
  in a terminal, where the driver asks you to confirm. The extension never passes
  `--yes` or `--prune`, and never handles a secret value.
- **Doctor** — **Bynk: Doctor** shows `bynk doctor`'s readiness checklist
  (compile, test, dev and deploy, editor) with each gap's remedy as buttons: run
  it in a terminal after you confirm, open a link or setting, or copy it. A test,
  debug or dev-server failure on a missing tool offers **Run Doctor**.
- **Live model views** — the sequence diagram, documentation page, architecture
  map, wire contract, and agent state view (see Commands) open beside the editor and stay current:
  they refresh when you save (or as you type, with `bynk.views.refresh`), the
  sequence and wire-contract views follow the cursor from handler to handler
  (pin one to freeze it), and open views come back after a window reload. Open
  them from the editor title bar, the Command Palette, the `Show Sequence`
  CodeLens above each handler, the `Show State` CodeLens on each agent, or an
  optional `Wire contract` CodeLens beside a handler's (`bynk.viewsCodeLens`).
  The agent state view follows the cursor from agent to agent.
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
sidebar, the sequence, documentation, architecture and wire-contract views, and
diagnostic explanations. Their settings still show in the Settings editor (VS
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
| **Bynk: Run Tests** | Run the project's tests via `bynk test --format json` and report them in the Testing view. |
| **Bynk: Run Tests in This File** | Run every test declared in the active `.bynk` file (or the file you right-click). |
| **Bynk: Debug Tests** | Run the tests under the inspector, so breakpoints in `.bynk` sources pause. |
| **Bynk: Show Sequence Diagram** | A sequence diagram of the handler at the cursor: its capability, context, and agent calls, and — in an agent's handler — its writes to the agent's own state, as notes. Follows the cursor into other handlers unless pinned. |
| **Bynk: Show Documentation** | The active file's declarations, rendered as a reference page. |
| **Bynk: Show Architecture Map** | A whole-project map of contexts and adapters, their `consumes` edges, and what each one binds. |
| **Bynk: Show Wire Contract** | The JSON wire shape of the service handler at the cursor. Follows the cursor into other handlers unless pinned. |
| **Bynk: Show Agent State** | The state machines of the agent at the cursor: for each `store` Cell of an enum or sum type, a state diagram generated from its handlers and a handler × state table of what each handler does from each state, with moves a `transition` would refuse at commit marked ⚠, and each handler's effects — what it writes, reads and calls on each path (shown instead for an agent with no state field). Follows the cursor from agent to agent, highlighting the handler under it. |
| **Bynk: Explain Diagnostic Code…** | The installed compiler's explanation of a diagnostic code, in a panel, offline: pick a code from the Problems panel or type one. The **Explain** quick fix on a diagnostic opens the same panel. |
| **Bynk: Start Dev Server** | Serve the project with `bynk dev` in a terminal. In a multi-context project it asks which context to serve (**All contexts** first). |
| **Bynk: Stop Dev Server** | Stop the dev server and every worker it started. |
| **Bynk: Restart Dev Server** | Stop the dev server and start it again with the same contexts. |
| **Bynk: Deploy…** | Pick an environment and a scope, review the dry-run plan, then deploy from the plan in a terminal. `bynk` asks you to confirm before it changes anything. |
| **Bynk: Show Deploy Plan** | The same plan for review only. It never deploys. |
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
| `bynk.debug.semanticValues` | `true` | Show values in the debugger in Bynk's vocabulary (`Ok(42)`, not `{tag: "Ok", value: 42}`). |
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
