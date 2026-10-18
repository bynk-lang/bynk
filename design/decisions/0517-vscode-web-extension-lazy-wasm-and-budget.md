# 0517 — The web worker fetches the wasm module lazily; the web bundle has its own size budget

- **Status:** Accepted (v0.359.1)

**Context.** The release `bynk-wasm` module is about 9 MiB (about 2.8 MiB gzipped) without `wasm-opt`. Instantiating it at activation would slow every vscode.dev session that merely opens a Bynk repository.

**Decision.** The client passes the module's URL in `initializationOptions.wasmUrl`, built from `context.extensionUri`, because the worker script isn't served from the extension's path in vscode.dev. The worker fetches and instantiates the module on the first request that needs it. `scripts/check-bundle.mjs --web` fails if `out/web/extension.js` requires anything but `vscode`, if `out/web/server.js` requires anything at all, or if either bundle or the module exceeds its budget (600 KiB, 400 KiB, 12 MiB). The module is built by the playground's `build-wasm.sh` (now taking `BYNK_WASM_OUT_DIR`) into the gitignored `vscode-bynk/wasm/`. It is never committed, and `vscode:prepublish` bundles it. CI's extension job builds it in release mode with the pinned `wasm-bindgen-cli`, then runs the `@vscode/test-web` smoke suite headless.

**Consequences.** Packaging the VSIX now needs the Rust `wasm32-unknown-unknown` target and `wasm-bindgen-cli`, and the VSIX carries the wasm module for desktop users too (about 6 MB packaged). A budget is raised deliberately, in the PR that needs it.
