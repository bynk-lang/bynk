# @bynk/wire-client

The runtime half of the typed TypeScript client `bynkc` generates for a Bynk
context's foreign callers (`out/workers/<context>/bynk-client.ts`). A Worker
that is not written in Bynk imports the generated client; the client imports
this package for the codec helpers and for `callService`, which signs each
request and decodes each answer.

Reference: <https://bynk-lang.org/reference/typescript-client/>.

## Versioning

This package renders the published cross-context wire format
(`reference/wire-format.md`), so it follows that format's version rather than
the compiler's: **its major version is the wire version it speaks**. A
generated client states the wire version it was generated for
(`export const WIRE_VERSION: WireVersion = "1"`), so pairing it with a package
for another wire version is a `tsc` error in the caller's build.

It is deliberately separate from the bynk runtime (`bynk-emit/runtime`, ADR
0310's emit ABI), which is versioned with the compiler and embedded in every
Bynk build (`design/archive/retired-tracks.md`, interop track Q7).

## Not a copy

`index.ts` is generated, never edited by hand. `bynk-emit/runtime/scripts/bundle.mjs`
concatenates the runtime's own `result`, `bytes`, `errors` and `boundary`
modules with this package's `src/surface.ts`, exactly as it builds the runtime
every Bynk Worker embeds. So a foreign caller encodes, signs and decodes with
the code a Bynk caller runs. After changing either, run `npm run bundle` in
`bynk-emit/runtime`; its `npm run check` (CI's `runtime` job) fails while this
file is stale.

## Shape

The package ships TypeScript source (`index.ts`), which a Workers bundler
(`wrangler`, esbuild) compiles with the caller's own code. It has no
dependencies; it uses the Workers runtime's `fetch` types and Web Crypto.
