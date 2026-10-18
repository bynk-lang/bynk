---
level: patch
changelog: The VS Code extension runs in vscode.dev and github.dev. A `browser` entry starts a web-worker language server over `bynk-wasm`'s `bynk_analyze`, `bynk_hover` and `bynk_complete`, giving diagnostics, hover and completion for each open `.bynk` file on its own (status bar **Bynk (web, single-file)**). Node-only commands carry `!isWeb`. The three wasm entry points take an optional platform name, and the web worker passes `cloudflare`, as `bynkc-lsp` analyses (#2102).
---

## ADR: vscode-web-extension-thin-wasm-adapter
title: The VS Code web extension is a thin LSP adapter over the playground's wasm entry points
summary: A web worker answers LSP with `bynk_analyze`/`bynk_hover`/`bynk_complete`; `bynk-lsp` on wasm is a follow-up

**Context.** The extension was Node-only. It provisions a native `bynkc-lsp`, so in vscode.dev and github.dev, where people land when they press `.` on a GitHub repository, it didn't load at all. The compiler already runs in the browser for the playground through `bynk-wasm`, which exports single-source `bynk_analyze`, `bynk_hover` and `bynk_complete` returning JSON (#2102).

**Decision.** `package.json` gains `"browser": "./out/web/extension.js"`. In a web host that entry starts a `vscode-languageclient/browser` client whose server is a web worker (`out/web/server.js`). The worker is a small TypeScript LSP loop over `vscode-languageserver/browser`, with full document sync. It calls the same three wasm functions the playground calls, so there is one wasm surface, not a second one. It maps UTF-16 LSP positions to the UTF-8 byte offsets `bynk-wasm` speaks and back, snapping to character boundaries, because `bynk_complete` slices the source at the offset it is given. Diagnostics take their ranges from the byte `from`/`to`, never from the byte-counted `line`/`col`. Hover is the type in a `bynk` code fence and completion uses `bynk-lsp`'s kind table, so web and desktop look the same. After a wasm trap (`RuntimeError`), the worker stops calling that instance and answers every query with its empty fallback. It also notifies the client (`bynk/wasmTrapped`), which restarts the worker for a fresh instance, because the wasm-bindgen glue can't re-initialise an instance in place. Restarts happen at most once a minute: a trap inside that window defers the restart to the window's end rather than dropping it, so a poisoned instance never keeps serving.

Grammar, snippets, language configuration and inline doc rendering are client-side and work unchanged. Commands that need a process or a filesystem, or `bynk-lsp` custom requests, carry `!isWeb` in every menu and keybinding: tests, debugging, tasks, scaffolding, server download and restart, and the sequence, documentation, architecture and wire-contract views. The getting-started walkthrough carries it too. `bynk.showServerOutput` is the one command the web entry registers.

**Consequences.** Opening a Bynk repository in vscode.dev gives highlighting, diagnostics, hover and completion with no install. Semantic tokens, inlay hints, navigation, rename and signature help stay desktop-only until `bynk-lsp` itself compiles to `wasm32-wasip1` with a virtual filesystem, which is the long-term path to parity and is noted as a follow-up. A manifest unit test fails if a contributed command the web entry doesn't register shows anywhere without `!isWeb`.

## ADR: vscode-web-extension-single-file-mode
title: The web extension analyses each open file on its own, hides the cross-unit cascade, and says so
summary: Single-file analysis in slice 1; cross-unit roots, single-file artefacts and the cascade they cause are dropped; status bar reads "Bynk (web, single-file)"

**Context.** The wasm entry points analyse one in-memory source. On the repository's own examples, a context that `uses` a sibling commons gets `bynk.uses.unknown_commons` and then a `bynk.resolve.unknown_type`/`unknown_function`/`unknown_name` for every name the import would have provided. A test file gets `bynk.suite.unknown_target`. None of these are errors in the project.

**Decision.** Slice 1 analyses each open file on its own. The filter's rules come from analysing every `bynkc/tests/fixtures/positive` source file alone. Those files all check clean as projects, so whatever the single-file analysis reports for them is an artefact. Before publishing, the worker drops:

- **The cross-unit roots, always:** `bynk.uses.unknown_commons`, `bynk.consumes.unknown_context` and `bynk.suite.unknown_target`. Each names a unit in another file.
- **The single-file artefacts, always.** `bynk.tier.system_needs_wire` needs the suite's target context. `bynk.stability.experimental` needs the `bynk.toml` `[unstable] allow` list. `bynk.project.inconsistent_commons_name` needs the real path, and the in-memory path is synthesised.
- **An unreadable binding module (`bynk.adapter.no_binding`, "could not be read"), always.** The worker reads no files. An adapter missing its `binding` clause is still reported.
- **The cascade, in a file with a root, or in one file of a unit split across several** (its `context`/`commons`/`adapter` name doesn't end in its file name). The cascade is `bynk.resolve.*`, `bynk.consumes.*`, `bynk.given.unknown_capability`, `bynk.event.unknown_subscription`, `bynk.generics.type_arg_mismatch`, `bynk.types.cannot_infer_*` and `bynk.types.uninferable_element_type`.

In a self-contained file these are real errors and are kept. With these rules no positive fixture shows an error on its own. A status bar item reads **Bynk (web, single-file)**, and its tooltip says that names from other units aren't resolved and their errors are hidden. Slice 2 loads the project through `vscode.workspace.fs`, and the filter goes away there.

**Consequences.** A file that uses or consumes another unit shows no false errors on the web. A genuine misspelled name or inference failure in that same file is hidden too, until project loading lands. Other type errors and parse errors are always reported.

## ADR: vscode-web-extension-lazy-wasm-and-budget
title: The web worker fetches the wasm module lazily; the web bundle has its own size budget
summary: `initializationOptions.wasmUrl` names `out/web/bynk_wasm_bg.wasm`, fetched on first use; `check-bundle.mjs --web` enforces self-containment and budgets

**Context.** The release `bynk-wasm` module is about 9 MiB (about 2.8 MiB gzipped) without `wasm-opt`. Instantiating it at activation would slow every vscode.dev session that merely opens a Bynk repository.

**Decision.** The client passes the module's URL in `initializationOptions.wasmUrl`, built from `context.extensionUri`, because the worker script isn't served from the extension's path in vscode.dev. The worker fetches and instantiates the module on the first request that needs it. `scripts/check-bundle.mjs --web` fails if `out/web/extension.js` requires anything but `vscode`, if `out/web/server.js` requires anything at all, or if either bundle or the module exceeds its budget (600 KiB, 400 KiB, 12 MiB). The module is built by the playground's `build-wasm.sh` (now taking `BYNK_WASM_OUT_DIR`) into the gitignored `vscode-bynk/wasm/`. It is never committed, and `vscode:prepublish` bundles it. CI's extension job builds it in release mode with the pinned `wasm-bindgen-cli`, then runs the `@vscode/test-web` smoke suite headless.

**Consequences.** Packaging the VSIX now needs the Rust `wasm32-unknown-unknown` target and `wasm-bindgen-cli`, and the VSIX carries the wasm module for desktop users too (about 6 MB packaged). A budget is raised deliberately, in the PR that needs it.

## ADR: bynk-wasm-editor-platform-argument
title: `bynk-wasm`'s editor entry points take an optional platform name
summary: `bynk_analyze`/`bynk_hover`/`bynk_complete` take `platform?: string`; omitted means `browser` (the playground), and the web extension passes `cloudflare`

**Context.** The three editor entry points were hard-wired to `Platform::Browser`, the playground's platform. Under it, a unit that consumes `bynk.cloudflare` (KV, for instance) draws `bynk.target.vendor_required`. On the desktop, `bynk-lsp` analyses a project for `Platform::default()` (Cloudflare), where the same file checks clean, so the web extension would have shown false errors on ordinary Workers code.

**Decision.** Each of the three exports takes a trailing `platform: Option<String>`, which wasm-bindgen exposes as an optional `platform?: string | null`. `platform_named` maps `"cloudflare"` and `"node"` to their platforms, and anything else, or nothing, to `browser`. The playground's calls are unchanged. The web worker passes `"cloudflare"`. `bynk_compile` keeps its single argument.

**Consequences.** The playground and the web extension share one wasm surface, each analysing for the platform its users target. A project-level platform setting, if one is added, can flow through the same argument.
