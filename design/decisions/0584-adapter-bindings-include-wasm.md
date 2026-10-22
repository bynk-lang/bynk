# 0584 — Adapter bindings include WebAssembly modules

- **Status:** Accepted (v0.389)

**Context.** A binding was copied as one TypeScript file; nothing beside it
reached `out/`, the emitted `wrangler.toml` had no module rules, and `tsc` copies
only TypeScript and JSON, so a binding could not load a WebAssembly module. The
first-party registry needs vetted Rust (its admission rules) behind an ordinary
adapter capability (track `first-party-registry`, slice L2).

**Decision.**

- **A.** `includes [ "./a.wasm", … ]` follows the module in the binding clause.
  `includes` is a contextual word, not a keyword. Each path is relative to the
  adapter's file, inside its directory, a `.wasm`, and unique once normalised;
  `bynk.adapter.include_missing`, `include_outside`, `include_kind` and
  `include_duplicate` report the failures. The file comes from beside the
  adapter, including in a dependency package, so its bytes are covered by the
  package checksum.
- **B.** The compiler generates a loader shim per module, `x.wasm.ts` beside it,
  whose default export is the compiled `WebAssembly.Module`, and the runtime
  gains `loadWasm(module, imports?)`. The Workers form re-exports `./x.wasm`;
  the form `bynk test` writes inlines the bytes (base64) and compiles them at
  load. Inlining replaces reading a file beside the output: `tsc` does not copy
  non-TypeScript files into `out-js`, and a file read would need Node typings in
  the emitted project.
- **C.** Included files are a new artefact class, `Artefacts.modules`: raw bytes
  beside the binding, never served and unrelated to assets. A Worker whose
  adapter closure reaches an included module gets an explicit
  `[[rules]] type = "CompiledWasm"` entry in `wrangler.toml`. A build with no
  `includes` is byte-identical to before.
- **D.** An included module is code from the package, like its binding: no
  approval of its own (the npm gate governs what npm brings in), and it runs with
  the binding's reach, sandboxed by the wasm instance with only the imports the
  binding passes.

**Consequences.** Rejected: scanning a binding's imports for siblings (implicit,
breaks on dynamic imports) and an `[assets]`-style table (assets are public
static files). Only `.wasm` is includable; other binary files, wasm components,
WASI and threads are out of scope. The unit tier stubs the adapter as before;
integration runs the real binding on Node. Verified against Wrangler 4.149
(`deploy --dry-run` bundles the module through the rule).
