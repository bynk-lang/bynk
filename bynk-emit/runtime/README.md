# @bynk/runtime

The bynk runtime: the shared types and helpers that every emitted bynk module
imports from `./runtime.js`. `Result`/`Option`, `Bytes`, the validation/JSON
error records, the Durable Object storage surface, the cross-Worker boundary
protocol, `HttpResult`, `QueueResult`, the agent lifecycle, the JWT / webhook
crypto seams, held `Connection`s, and the ICU message-formatting and
`Accept-Language` locale-negotiation helpers.

## Why this is a package

The runtime began life as a single `runtime.ts` string embedded into the
compiler via `include_str!`. It outgrew that: one flat file carrying seven
unrelated concerns, type-checked only as a side effect of a generated project
and never unit-tested. This package gives it first-class treatment — focused
modules, isolated `tsc`, and real tests — **without changing what ships**.

## The contract: one flat file, unchanged

`bynk-emit` embeds `../src/emitter/runtime.ts` via
`include_str!("emitter/runtime.ts")` and emits it verbatim as `out/runtime.ts`
at a project's root, where every module imports it by relative path. That file
must stay a single file with a flat public surface, and emitter↔runtime must
stay lockstep (same compiler binary ⇒ no version skew).

So this is **not** a published npm dependency and **not** a tree-shaking
bundler. `scripts/bundle.mjs` deterministically concatenates the modules below
(in dependency order, mirroring `src/index.ts`), strips their internal relative
imports, preserves every comment, and writes the single
`../src/emitter/runtime.ts`. That file is committed and is a build output:
**edit the modules in `src/`, never the bundled file.**

## The second artifact: `@bynk/wire-client`

`scripts/bundle.mjs` also writes `../wire-client/index.ts`, the runtime half of
the TypeScript client bynkc generates for a context's foreign callers (I4,
#1970). It is the `result`, `bytes`, `errors` and `boundary` modules plus that
package's own `src/surface.ts`, concatenated the same way, so a non-Bynk caller
runs the boundary code a Bynk caller runs. It is versioned with the wire
format, not with this runtime; `npm run check` guards both artifacts.

## Per-module form (ADR 0615)

`bundle.mjs` also writes each module to `../src/emitter/runtime/<module>.ts`
(with that module's `__` aliases appended) and the `runtime_modules.rs` table
bynk-emit embeds. Emission switches to this form in #2441; until then the flat
file above is what ships. `npm run check` guards both.

## Modules

`result` · `bytes` · `errors` · `storage` · `boundary` · `http` · `queue` ·
`agent` · `auth` · `connection` · `messages` · `locale` — re-exported in
dependency order by `src/index.ts`.

## Workflow

```sh
npm install          # or: npm ci
npm run typecheck    # tsc --noEmit over src + test
npm test             # node --test over test/**/*.test.ts (Node ≥ 22)
npm run bundle       # regenerate ../src/emitter/runtime.ts
npm run check        # drift guard: fail if the committed bundle is stale
npm run verify       # typecheck + test + check (what CI runs)
```

After changing anything in `src/`, run `npm run bundle` and commit the
regenerated `../src/emitter/runtime.ts`. CI's `runtime` job runs `npm run check`
and will fail a PR whose bundled file is out of date with the sources.

Tests run the `.ts` files directly under Node's TypeScript type-stripping — the
same execution model the emitted runtime uses under `bynkc test --inspect` — so
there is no separate compile step.
