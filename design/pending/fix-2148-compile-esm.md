---
level: minor
changelog: '`bynkc compile` always writes `out/package.json` declaring `"type": "module"` (beside any adapter npm dependencies), so `tsc -p out/tsconfig.json` with emit on builds the output as ES modules rather than CommonJS: a declaration named `require` or (under TypeScript 7) `Object` no longer fails with TS2441, and a hand-written binding is checked under the ES-module rules Node runs it with, so an extensionless relative import is a `tsc` error (TS2835) instead of a run-time `ERR_MODULE_NOT_FOUND` (#2148)'
---

## ADR: output-declares-es-modules
title: The build output declares itself ES modules with a generated package.json
summary: out/ always carries package.json with "type": "module", so NodeNext checks and builds it as the ESM it is

**Context.** The emitter writes ES-module TypeScript (`.js`-suffixed imports,
`export`) and a `tsconfig.json` with `module: NodeNext`. Under `NodeNext`,
`tsc` takes each file's format from the nearest `package.json`. The output
carried one only when an adapter declared npm dependencies, and that one did
not declare `"type"`. So a user's `tsc -p out/tsconfig.json` with emit on
down-levelled the output to CommonJS. That transform reserves the top-level
names `require` (TypeScript 5 and 7) and `Object` (TypeScript 7), so a Bynk
`fn require` or `type Object` failed with TS2441 (#2148). #2146 fixed the
`bynk test` runner by passing `--module ES2022 --moduleResolution Bundler`.

`Bundler` resolution would also have fixed `compile` output from the tsconfig
alone, with no new file. But a hand-written adapter binding is copied verbatim
into the output and runs under Node's ESM loader (in `bynk test`) or a
bundler (in deploy). `Bundler` checks it more loosely than Node runs it: an
extensionless relative import, a CommonJS idiom (`require`, `module.exports`,
`__dirname`), or a named import from a CommonJS npm package would pass `tsc`
and then fail at run time. Before, those were `tsc` errors or worked as
CommonJS.

**Decision.** The output root always carries a generated `package.json`
declaring `"type": "module"`. When adapters declare npm dependencies they go
in the same file, beside the marker. The emitted tsconfig keeps
`module: NodeNext` and `moduleResolution: NodeNext`, so `tsc` sees the model
Node runs: ES modules with Node's resolution rules. The `bynk test` runner
drops its `--module`/`--moduleResolution`/`--moduleDetection` overrides and
uses the emitted config as written. It still marks its own `out-js` tree as
ES modules, because that tree lies outside the output root.

`out/package.json` is a compiler-owned output file, like `tsconfig.json`.
Output reconciliation already deleted any `.json` file in the output tree that
the build did not produce, and the adapter-dependency `package.json` was
already regenerated on every build. A user's own `package.json` above `out/`
is never read or written, and it no longer decides how `out/` is built.

**Consequences.**
- Every output tree gains `package.json`. With no adapter dependencies it is
  `{"type": "module"}` alone. Fixture goldens skip that bare marker as
  project-wide boilerplate, as they skip `runtime.ts` and `tsconfig.json`. The
  five adapter fixtures' snapshots gain the `"type"` line.
- A binding must be an ES module that names its relative imports' `.js` files.
  An extensionless import is TS2835 at `tsc` time, and CommonJS idioms do not
  type-check. The adapters reference says so.
- `--emit js` output keeps the marker, so Node runs its `.js` as ES modules.
  Deploys are unaffected: wrangler bundles `index.ts` with esbuild, which
  ignores `"type"`.
- The JavaScript a user builds with `tsc` lands in `out-js`, outside the output
  root. Running it with plain `node` still needs `"type": "module"` in scope
  there (Node 22+ also detects ESM syntax).
