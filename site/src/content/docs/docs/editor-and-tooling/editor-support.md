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

### In the browser

The same extension runs in [vscode.dev](https://vscode.dev) and
[github.dev](https://github.dev) with nothing to install: press <kbd>.</kbd> on a
Bynk repository on GitHub. In the browser the language features come from the
Bynk compiler built to WebAssembly rather than from `bynkc-lsp`, and they cover
each open file on its own: syntax highlighting, diagnostics, hover and
completion. The status bar reads **Bynk (web, single-file)**. Errors about names
from another unit are hidden, because the other unit isn't loaded. Tests,
debugging, tasks, scaffolding and the project views are desktop-only.

## Settings

| Setting | Default | Purpose |
|---|---|---|
| `bynk.executablePath` | `bynkc-lsp` | Path to the language-server binary. |
| `bynk.trace.server` | `off` | Trace LSP traffic (`off` / `messages` / `verbose`) in the "Bynk LSP" output channel. |

## Other editors

Any editor with a generic LSP client can use `bynkc-lsp`. Syntax highlighting is
also available through the [`tree-sitter-bynk`](/docs/tooling/tree-sitter-bynk/)
grammar.

## Related

- [Format your code with `bynk-fmt`](/docs/editor-and-tooling/format/).
- Reference: [`bynk-lsp`](/docs/tooling/bynk-lsp/).
