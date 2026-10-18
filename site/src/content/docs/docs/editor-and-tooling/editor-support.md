---
title: Set up editor support
---
**Goal:** get syntax highlighting, diagnostics, hover, and go-to-definition in
your editor.

## VS Code

The `vscode-bynk` extension provides syntax highlighting plus live language
features backed by the `bynkc-lsp` language server.

**1. Build the language server** from the workspace root:

```sh
cargo build --release -p bynk-lsp
```

This produces `target/release/bynkc-lsp`. Put it on your `PATH`, or point the
extension at it with the `bynk.executablePath` setting.

**2. Build and install the extension** from the `vscode-bynk/` directory:

```sh
cd vscode-bynk
npm install
npm run package
code --install-extension bynk-vscode-*.vsix
```

The extension activates on `.bynk` files and on any workspace containing a
`bynk.toml`. You then get syntax highlighting, live diagnostics, hover with type
signatures, go-to-definition, and format-on-save (honouring
`editor.formatOnSave`).

When a diagnostic's code has a curated explanation, the lightbulb offers
**Explain `<code>`**. It opens the explanation in a panel beside the editor,
from the installed toolchain and without a network: the same text
`bynk explain <code>` prints. **Bynk: Explain Diagnostic Code…** looks up any
code, for example one from CI output.

Renaming or moving a `.bynk` file, or a folder of them, in the Explorer updates
the unit names it declares and every `uses`/`consumes` that refers to them. VS
Code shows the edit for review first. Moving units into a different project is
left for you to fix by hand.

To serve the project while you edit, run **Bynk: Start Dev Server** from the
Command Palette. It runs `bynk dev` in a terminal, and a status-bar item shows
each context's URL. The [extension reference](/docs/tooling/vscode-bynk/) lists
every command. If something is missing from your toolchain, **Bynk: Doctor**
lists what and how to fix it.

For `bynk.toml`, install a TOML extension that reads JSON Schemas, such as
[Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml).
The Bynk extension associates the manifest's schema with every `bynk.toml`, so
you get completion, hover and validation for its tables and keys (see
[Editor support](/docs/manifest/#editor-support) in the manifest reference).

**3. Find your way around the project.** Open the **Bynk** view in the activity
bar for a tree of the project's contexts, commons, adapters, and tests. Expand a
unit to see what it consumes, what consumes it, and its declarations, and click
any node to jump to its source. A service handler's inline buttons open its
sequence diagram and wire contract; these views, the documentation page, and
the architecture map stay current as you save (see `bynk.views.refresh`).

### In the browser

The same extension runs in [vscode.dev](https://vscode.dev) and
[github.dev](https://github.dev) with nothing to install: press <kbd>.</kbd> on a
Bynk repository on GitHub. In the browser the language features come from the
Bynk compiler built to WebAssembly rather than from `bynkc-lsp`, and they cover
each open file on its own: syntax highlighting, diagnostics, hover and
completion. The status bar reads **Bynk (web, single-file)**. Errors about names
from another unit are hidden, because the other unit isn't loaded. Tests,
debugging, the dev server, deploy, Doctor, tasks, scaffolding, the **Bynk** sidebar and
the project views are desktop-only.

## Settings

| Setting | Default | Purpose |
|---|---|---|
| `bynk.executablePath` | `bynkc-lsp` | Path to the language-server binary. |
| `bynk.trace.server` | `off` | Trace LSP traffic (`off` / `messages` / `verbose`) in the "Bynk LSP" output channel. |
| `bynk.views.refresh` | `onSave` | When the project sidebar and open model views refresh: `onSave`, `onType`, or `manual`. |

## Other editors

Any editor with a generic LSP client can use `bynkc-lsp`. Syntax highlighting is
also available through the [`tree-sitter-bynk`](/docs/tooling/tree-sitter-bynk/)
grammar. A TOML language server that reads JSON Schemas (taplo, in Zed, Helix
and others) checks `bynk.toml` once the file names the schema; see
[Editor support](/docs/manifest/#editor-support).

## Related

- [Format your code with `bynk-fmt`](/docs/editor-and-tooling/format/).
- Reference: [`bynk-lsp`](/docs/tooling/bynk-lsp/).
