# @bynk/runtime

The bynk runtime: the shared types and helpers that emitted bynk modules import.
`Result`/`Option`, `Bytes`, the validation/JSON error records, the Durable
Object storage surface, the cross-Worker boundary protocol, `HttpResult`,
`QueueResult`, the agent lifecycle, the JWT / webhook crypto seams, held
`Connection`s, and the ICU message-formatting and `Accept-Language`
locale-negotiation helpers.

## The contract: the module is the unit (ADR 0615)

Each module under `src/` is embedded by `bynk-emit` as one `(name, source)` pair
and emitted as `__bynk_runtime/<module>.ts` at the root of a project's output.
Generated code imports each helper from the module that defines it (via its `__`
alias, #1653), and a project emits exactly the modules its emitted files import,
closed over the modules' own imports (type-only edges included). A module is in
or out; there is no bundler or dead-code analysis. Emitter and runtime stay
lockstep because the files come from the same compiler binary that wrote the
imports, so there is no version skew.

This is **not** a published npm dependency. `scripts/bundle.mjs` writes
`../src/emitter/runtime/<module>.ts` (the module with its `__` aliases appended,
from `src/aliases.ts`) and `../src/emitter/runtime_modules.rs` (the source table
and a name-to-module table of every export). Both are committed build outputs:
**edit the modules in `src/`, never the generated files.**

## The second artifact: `@bynk/wire-client`

`scripts/bundle.mjs` also writes `../wire-client/index.ts`, the runtime half of
the TypeScript client bynkc generates for a context's foreign callers (I4,
#1970). It is the `result`, `bytes`, `errors` and `boundary` modules plus that
package's own `src/surface.ts`, concatenated the same way, so a non-Bynk caller
runs the boundary code a Bynk caller runs. It is versioned with the wire
format, not with this runtime; `npm run check` guards both artifacts.

## The test runtime and the client runtime (1B.3a, #4047)

Two more artifact families are generated from sources under `src/`:

- **`src/testing/`** is the runtime a generated test module embeds. Each module is an ordinary
  module, type-checked with the rest of the runtime by `npm run typecheck` whether or not a fixture
  uses it, and a module's use of another is an `import`. `bundle.mjs` writes its **splice form**,
  the module with its relative `import`s dropped and its line-initial `export`s removed (a splice
  is a script, not a module), to `../src/testgen/test_runtime/<name>.ts`, and the table
  `../src/testgen/test_runtime_modules.rs` the emitter reads them through. A generated test module
  still *splices* that text rather than importing it, so each module keeps its own clock, snapshot
  state and timer queue. `src/testing/harness.ts` declares the names a generated module
  supplies (`__bynkSnapshotFiles`, `__bynkAssetFiles`); it is a contract for the type-check and is
  never spliced. A name a fragment must keep exporting when spliced is listed in `SPLICE_EXPORTS`
  in `bundle.mjs`.
- **A fragment's runtime names are a table column** (1B.5e, #4054 Decision C). Each row records
  the names the module imports from the runtime's own modules (`../*.ts`), beside its name, the
  sibling modules it imports and its splice text. `TestRuntimeModule::splice` carries them as the
  `raw_using` names of the spliced statement, so a generated test module — whose imports come
  from its tree since 1B.5e — imports what the spliced text spells. A `testgen` unit test holds
  each row's column against the runtime exports its artifact spells, so a fragment that starts
  using `Ok` cannot leave a module under-importing it.
- **The test doubles are imported, not spliced** (1B.3b, #4047). `src/testing/bindings.ts` holds
  every `makeIntegration*` factory a multi-Worker test binds as a participant's `env.*`, and
  `src/testing/pglite.ts` the opt-in PGlite `Postgres` double, which alone imports
  `@electric-sql/pglite`. They are ordinary per-module runtime modules (emitted as
  `__bynk_runtime/testing/<name>.ts`, with the `__` aliases), and no production module defines one:
  `test/testing_boundary.test.ts` fails if one does. `makeTestState` and `InMemoryStorage` stay in
  `storage.ts`, because the Bundle target's agent storage is built on them.
- **`src/client/http_client.ts`** is the runtime `bynk client` inlines. `bundle.mjs` writes it, with
  its `import` dropped, to `../src/emitter/http_client_runtime.ts`.

`npm run check` fails if an artifact is stale or has no source module. **Edit `src/testing/` and
`src/client/`, never the artifacts.**

## Per-module form (ADR 0615)

`bundle.mjs` also writes each module to `../src/emitter/runtime/<module>.ts`
(with that module's `__` aliases appended) and the `runtime_modules.rs` table
bynk-emit embeds. Emission switches to this form in #2441; until then the flat
file above is what ships. `npm run check` guards both.

## Modules

`result` · `bytes` · `errors` · `storage` · `boundary` · `httpresult` · `http` · `queue` ·
`agent` · `auth` · `connection` · `messages` · `locale` — re-exported in
dependency order by `src/index.ts`.

## Workflow

```sh
npm install          # or: npm ci
npm run typecheck    # tsc --noEmit over src + test
npm test             # node --test over test/**/*.test.ts (Node ≥ 22)
npm run bundle       # regenerate the embedded per-module runtime
npm run check        # drift guard: fail if the committed bundle is stale
npm run verify       # typecheck + test + check (what CI runs)
```

After changing anything in `src/`, run `npm run bundle` and commit the
regenerated `../src/emitter/runtime/` and `runtime_modules.rs`. CI's `runtime` job runs `npm run check`
and will fail a PR whose bundled file is out of date with the sources.

Tests run the `.ts` files directly under Node's TypeScript type-stripping — the
same execution model the emitted runtime uses under `bynkc test --inspect` — so
there is no separate compile step.
