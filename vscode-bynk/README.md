# Bynk for VS Code

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

Language support for the **[Bynk](https://github.com/bynk-lang/bynk) language**
(`.bynk` files) in Visual Studio Code: syntax highlighting plus full language
features backed by the
[`bynkc-lsp`](https://github.com/bynk-lang/bynk/tree/main/bynk-lsp) language
server, which the extension finds or downloads for you.

The extension activates on any `.bynk` file, or on any workspace containing a
`bynk.toml`.

## Features

- **Syntax highlighting** — a TextMate grammar, mirrored from the
  [tree-sitter grammar](https://github.com/bynk-lang/bynk/tree/main/tree-sitter-bynk)
  (the source of truth).
- **Bynk in Markdown** — ` ```bynk ` and ` ~~~bynk ` fenced code blocks in
  Markdown files are highlighted in the editor with the same grammar. (VS
  Code's built-in Markdown *preview* highlights with highlight.js, not TextMate,
  so fences there stay plain.)
- **Live diagnostics** — errors and warnings as you type, exactly as
  `bynkc check` reports them, each tagged with its dotted category.
- **Hover** — type signatures and doc blocks.
- **Go-to-definition** and **find references** for types, functions,
  capabilities, services, and agents.
- **Rename** — workspace-wide and validated.
- **Completion**, **inlay hints** (inferred types), and **semantic tokens**
  (type-aware highlighting).
- **Code actions** — quick fixes for diagnostics that carry a suggestion.
- **Formatting** and **range formatting** via `bynk-fmt` (honours
  `editor.formatOnSave`).
- **Document & workspace symbols** and **document highlights**.
- **Test Explorer** — run a project's `test` and `integration` blocks from the
  Testing view (or **Bynk: Run Tests**), with click-through from a failing
  assertion to its `.bynk` line. Runs `bynk test --format json`; compile
  failures land in the Problems panel. A **`▷ Run Test | Debug Test`** CodeLens
  (and native gutter run/debug glyphs) sit at each test in the editor — disable
  the lens with `bynk.testCodeLens`.
- **Debugging** — set a breakpoint in a `.bynk` file and press **Debug**: in the
  Test Explorer (Node, `bynk test --inspect`) or via a `bynk` `launch.json`
  config for the dev-server worker (workerd, `bynk dev --inspect`). Delegates to
  VS Code's JavaScript debugger; breakpoints resolve to the exact `.bynk`
  statement through the emitted source maps.
- **`bynkc: check` build task** — type-check the whole project on demand, errors
  routed to the Problems panel.
- **Status bar** — the active project name (click to open `bynk.toml`) and the
  language-server state (click to show its output).

All language features come from `bynkc-lsp`; the extension is the client that
provisions and launches it.

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
| **Bynk: Debug Tests** | Run the tests under the inspector, so breakpoints in `.bynk` sources pause. |
| **Bynk: Show Sequence Diagram** | A sequence diagram of the handler at the cursor: its capability, context, and agent calls. |
| **Bynk: Show Documentation** | The active file's declarations, rendered as a reference page. |
| **Bynk: Show Architecture Map** | A whole-project map of contexts and adapters, their `consumes` edges, and what each one binds. |
| **Bynk: Show Wire Contract** | The JSON wire shape of the service handler at the cursor. |
| **Bynk: Explain Diagnostic Code…** | The installed compiler's explanation of a diagnostic code, in a panel, offline: pick a code from the Problems panel or type one. The **Explain** quick fix on a diagnostic opens the same panel. |

## Settings

| Setting | Default | Purpose |
| ------- | ------- | ------- |
| `bynk.executablePath` | `""` (auto-resolve) | Absolute path to a `bynkc-lsp` binary to use. When empty, the extension resolves the server automatically (see above). |
| `bynk.bynkPath` | `""` (`bynk` on `PATH`) | Path to the `bynk` driver used by the `bynkc: check` task, the Test Explorer, and debugging. When empty, `bynk` is taken from `PATH`. |
| `bynk.compilerPath` | `""` (driver resolves `bynkc`) | Pin an exact `bynkc`, passed through as `BYNK_BYNKC` to `bynk`. When empty, `bynk` resolves `bynkc` itself (`PATH`, then a sibling of `bynk`) — richer than a bare `PATH` lookup, so a driver-first install (`bynkc` reachable only via `BYNK_BYNKC` or as a `bynk` sibling) still works. |
| `bynk.trace.server` | `off` | Trace LSP protocol traffic (`off` / `messages` / `verbose`) in the "Bynk LSP" output channel. |
| `bynk.inlayHints.enable` | `true` | Show Bynk inlay hints (both kinds below). A persistent, Bynk-only preference; takes effect on the next edit or scroll. |
| `bynk.inlayHints.types` | `true` | Inferred-type hints on `let` bindings and lambda parameters. |
| `bynk.inlayHints.parameterNames` | `true` | Parameter-name hints at call arguments. |
| `bynk.testCodeLens` | `true` | Show a `Run Test \| Debug Test` CodeLens above each test case: **Run Test** runs just that case (`bynk test --case <name>`), **Debug Test** runs it under the inspector. The native gutter icons appear regardless. |
| `bynk.debug.semanticValues` | `true` | Show values in the debugger in Bynk's vocabulary (`Ok(42)`, not `{tag: "Ok", value: 42}`). |
| `bynk.inlineDocRendering.enable` | `true` | Render `--- … ---` doc comments in place with light Markdown styling while reading the source. |

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

See also
[Set up editor support](https://bynk-lang.org/docs/editor-and-tooling/editor-support/)
for using `bynkc-lsp` with other editors.

## License

Licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at
your option.
