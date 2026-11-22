---
title: Set up Neovim, Helix or Zed
---
**Goal:** run the `bynk-lsp` language server in Neovim, Helix or Zed, so `.bynk`
files get diagnostics, hover, go-to-definition, completion and formatting.

Each editor needs the same three things: the `bynk-lsp` binary, a rule that
treats `.bynk` files as Bynk, and `bynk.toml` as the marker for the project root.
For VS Code, see [Set up editor support](/reference/tooling/editor-and-tooling/editor-support/).

## Install `bynk-lsp`

`bynk-lsp` ships in every release archive next to `bynk`. Download the archive
for your platform and put both binaries on your `PATH`, as
[Install](/book/introduction/install/#download-a-release-binary) describes. To
build from source instead, from a clone of the repository:

```sh
cargo install --path bynk-lsp
```

Check that your editor will find it:

```sh
bynk-lsp --version
```

The server speaks LSP over stdin and stdout and takes no arguments: the command
is just `bynk-lsp`. (`--version` is the only flag it reads.) It finds the
project itself by looking for a `bynk.toml` at or above the open files; with
none, it checks each file on its own. It logs to `~/.bynk-lsp.log`; set
`BYNK_LSP_LOG=debug` in the editor's environment for more detail. See the
[`bynk-lsp` reference](/reference/tooling/bynk-lsp/) for its features.

## Neovim

Tested with Neovim 0.11.5: the server attaches to a `.bynk` buffer, takes the
directory holding `bynk.toml` as its root, and answers requests.

This uses the built-in `vim.lsp.config` and `vim.lsp.enable`, so it needs
Neovim **0.11 or newer** and no plugins. Neovim doesn't know the `.bynk`
extension, so the config registers it as the `bynk` filetype first. Add this to
your `init.lua`:

```lua
vim.filetype.add({ extension = { bynk = "bynk" } })

vim.lsp.config("bynk_lsp", {
  cmd = { "bynk-lsp" },
  filetypes = { "bynk" },
  root_markers = { "bynk.toml" },
})
vim.lsp.enable("bynk_lsp")
```

Open a `.bynk` file and run `:checkhealth vim.lsp` to see the client attached.
Neovim walks up from the file to the nearest `bynk.toml` and uses that directory
as the root. A file with no `bynk.toml` above it still gets the server, in
single-file mode.

`nvim-lspconfig` has no `bynk-lsp` entry, and the config above doesn't need it.
If you prefer one file per server, put the table passed to `vim.lsp.config` in
`lsp/bynk_lsp.lua` on your `runtimepath` (for example
`~/.config/nvim/lsp/bynk_lsp.lua`) as `return { … }`, keeping the
`vim.filetype.add` and `vim.lsp.enable` lines in `init.lua`.

For colour, the server sends semantic tokens, which Neovim applies by default,
so standard token types such as types, functions and keywords are highlighted
without a grammar. Bynk's own token types (`capability`, `service`, `agent` and
others) show as `@lsp.type.capability` and so on; link them to a highlight group
with `vim.api.nvim_set_hl` if your colour scheme leaves them plain. The
[`tree-sitter-bynk`](/reference/tooling/tree-sitter-bynk/) grammar can also be wired
into Neovim's tree-sitter support, which this page doesn't cover.

## Helix

Tested with Helix 25.07.1: the grammar fetches and builds, `hx --health bynk`
finds the server and the highlight queries, and opening a `.bynk` file starts
the server with the `bynk.toml` directory as its root.

Helix reads `languages.toml` from its config directory (`~/.config/helix/` on
Linux and macOS, `%AppData%\helix\` on Windows). Add:

```toml
[language-server.bynk-lsp]
command = "bynk-lsp"

[[language]]
name = "bynk"
scope = "source.bynk"
file-types = ["bynk"]
roots = ["bynk.toml"]
comment-tokens = "//"
indent = { tab-width = 4, unit = "\t" }
language-servers = ["bynk-lsp"]

[[grammar]]
name = "bynk"
# The grammar lives in the tree-sitter-bynk/ directory of the bynk repository.
# rev is the commit the v0.639.1 release tag points to.
source = { git = "https://github.com/bynk-lang/bynk", rev = "e43b4487940e3e8549ce62233d067b65ef7a33b5", subpath = "tree-sitter-bynk" }
```

The `[language-server]` table names the command; the `[[language]]` stanza
matches `.bynk` files, sets `bynk.toml` as the root marker, and attaches the
server. `scope` is required. `indent` matches `bynk fmt`'s default of tabs.

The `[[grammar]]` section is only needed for syntax highlighting; the language
server works without it. `rev` must be a full commit hash: Helix 25.07.1
rejects a tag name there. To pin the grammar to the release you installed,
look up the commit its tag points to:

```sh
git ls-remote https://github.com/bynk-lang/bynk 'refs/tags/v0.639.1^{}'
```

Then fetch and build the grammar, and copy the highlight queries from the
fetched source into your runtime directory, where Helix looks for them by
language name:

```sh
hx --grammar fetch
hx --grammar build
mkdir -p ~/.config/helix/runtime/queries/bynk
cp ~/.config/helix/runtime/grammars/sources/bynk/tree-sitter-bynk/queries/*.scm \
  ~/.config/helix/runtime/queries/bynk/
```

That copies `highlights.scm` and `injections.scm`, which are in the repository
at `tree-sitter-bynk/queries/`. Check the result with:

```sh
hx --health bynk
```

It should show `bynk-lsp` on your `PATH`, the tree-sitter parser, and the
highlight queries. Helix has no indent or text-object queries for Bynk.

`hx --grammar fetch` also fetches every grammar Helix ships, so it can take a
while the first time. Helix picks the topmost directory holding a `bynk.toml`,
stopping at the workspace root (the nearest `.git`), as the server's root.

## Zed

**Not yet tested in the editor.** The extension below follows Zed's extension
documentation, and its Rust code compiles for `wasm32-wasip2`, but it hasn't been
loaded into Zed.

Zed can't add a new language from `settings.json`: its `file_types` setting only
maps file names to languages Zed already has. A new language, and a language
server for it, come from an extension. There is no Bynk extension for Zed yet,
so to use `bynk-lsp` in Zed you build a small one yourself and install it as a
dev extension. It has this layout:

```text
zed-bynk/
  extension.toml
  Cargo.toml
  src/lib.rs
  languages/bynk/
    config.toml
    highlights.scm
    injections.scm
```

`extension.toml` declares the extension, the grammar and the language server:

```toml
id = "bynk"
name = "Bynk"
version = "0.0.1"
schema_version = 1
authors = ["Your Name <you@example.com>"]
description = "Bynk language support"
repository = "https://github.com/your-name/zed-bynk"

[grammars.bynk]
repository = "https://github.com/bynk-lang/bynk"
# The commit the v0.639.1 release tag points to.
rev = "e43b4487940e3e8549ce62233d067b65ef7a33b5"
path = "tree-sitter-bynk"

[language_servers.bynk-lsp]
name = "bynk-lsp"
languages = ["Bynk"]
```

`path` points Zed at the grammar's directory inside the repository. Use the same
commit as the queries you copy below.

`languages/bynk/config.toml` defines the language. Its `name` must match the
entry in `languages` above, and `path_suffixes` associates `.bynk` files:

```toml
name = "Bynk"
grammar = "bynk"
path_suffixes = ["bynk"]
line_comments = ["// "]
hard_tabs = true
```

Copy `highlights.scm` and `injections.scm` from `tree-sitter-bynk/queries/` in
the bynk repository (at the same commit) into `languages/bynk/`. Zed's themes
name a [different set of captures](https://zed.dev/docs/extensions/languages#syntax-highlighting)
from the ones the grammar uses, so some tokens may show uncoloured until you
rename their captures (for example `@comment.documentation` to `@comment.doc`).

Zed only starts a language server whose command the extension's Rust code
returns, so the extension needs a small crate. `Cargo.toml`:

```toml
[package]
name = "zed-bynk"
version = "0.0.1"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
zed_extension_api = "0.7.0"
```

`src/lib.rs` finds `bynk-lsp` on the `PATH` and starts it with no arguments:

```rust
use zed_extension_api::{self as zed, LanguageServerId, Result};

struct BynkExtension;

impl zed::Extension for BynkExtension {
    fn new() -> Self {
        BynkExtension
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let command = worktree
            .which("bynk-lsp")
            .ok_or_else(|| "bynk-lsp is not on your PATH".to_string())?;
        Ok(zed::Command {
            command,
            args: vec![],
            env: worktree.shell_env(),
        })
    }
}

zed::register_extension!(BynkExtension);
```

Zed builds the extension itself, so you need Rust installed through rustup. In
Zed, open the Extensions page, choose **Install Dev Extension**, and select the
`zed-bynk` directory. Zed then compiles the grammar and the crate.

Zed starts the server for the folder you open, so open the directory that holds
`bynk.toml` (or one above it). For a `bynk.toml` schema in Zed, see the
manifest's [Editor support](/reference/tooling/manifest/#editor-support).

## Related

- [Set up editor support](/reference/tooling/editor-and-tooling/editor-support/) for VS Code.
- Reference: [`bynk-lsp`](/reference/tooling/bynk-lsp/) and
  [`tree-sitter-bynk`](/reference/tooling/tree-sitter-bynk/).
