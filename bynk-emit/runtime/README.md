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
