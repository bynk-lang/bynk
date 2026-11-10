# Run-time benchmarks

What compiled Bynk costs at run time, against hand-written TypeScript doing the
same work. The results are published on the book's
[comparison page](../../site/src/content/docs/book/about/bynk-compared-to-the-alternatives.md)
and recorded per release in [`design/benchmarks/runtime/`](../../design/benchmarks/runtime/) as
`v<version>.json`. Compiler and editor speed are a separate matter (#3168).

```sh
cargo xtask bench-runtime                 # measure; print, write nothing
cargo xtask bench-runtime --record        # and write design/benchmarks/runtime/v<version>.json
cargo xtask bench-runtime --sizes-only    # bundle sizes only: deterministic, no workerd
```

`--runs`, `--requests` and `--startups` set the sample sizes (defaults 5, 1000
and 15). It builds a release `bynk`, installs this directory's pinned npm
dependencies (no lockfile, nothing committed) and runs `bench.mjs`. Needs Node 22
or later. Nothing is deployed: everything runs locally under workerd, through
Miniflare. The npm install pins the top-level packages only (transitive
dependencies float) and runs install scripts, since workerd downloads its
binary; that is fine for a manual developer tool, and worth revisiting if this
ever runs in CI.

Bundle sizes (bundled, minified and gzipped, for each example and its twin) are
recorded per release in that file. They are tracked, not gated: there is no size
threshold and no CI check yet, so a regression shows up as a diff in the record.

## What is measured

For each of `examples/hello-world`, `examples/orders` and `examples/rate-limiter`:

- **Bundle size.** `bynk compile --target workers`, then esbuild bundles the
  worker's entry point (the same options for the twin): the bundled size, the
  minified size, and the minified size gzipped at level 9. These are
  deterministic.
- **Start-up.** A fresh Miniflare instance, timed from construction to the first
  answered request. It includes spawning workerd, so an empty worker is measured
  beside it as the floor. Variants run in alternation, after two discarded
  rounds.
- **Request latency.** Each run is a fresh instance, seeded (`orders` gets 100
  orders and 500 lines), warmed with 200 requests, then timed one request at a
  time. We keep p50 and p99 per run and report the median across runs with the
  range. Runs alternate between the variants.
- **Write scaling.** `POST /lines` against an `orders` book holding 0, 500 and
  2000 lines, three runs each, because Bynk loads and commits an agent's whole
  state on every call.
- **Growth.** A synthetic single-context project of K groups, each a refined
  type, a record, an agent with a stored cell and an HTTP route, bundled the same
  way, to show how bundle size grows with the program.

## The twins

[`twins/`](twins/) holds a hand-written TypeScript version of each example. They
are meant to be what a careful TypeScript author would write for the same job,
not a strawman and not a golfed minimum:

- No libraries. Routing, validation and JSON handling are written by hand, the
  way the rest of a small Worker is. A real project would often add a validation
  library, which would make the twin larger; the twin is therefore a floor on
  size, not a typical figure.
- The same observable behaviour. Before measuring, the harness sends each route's
  request to both variants and stops if the status codes differ or the bodies
  differ (compared sorted where row order is unspecified; a rate-limited route is
  primed into the limit first, and compares status only because its body carries
  a time). During measuring it stops if any timed request gets a status other
  than the one its route names, so a row never silently times an error path. The twins validate what the Bynk types
  validate: a name of 1 to 40 characters, a non-empty client, string ids, a
  positive integer quantity.
- Idiomatic Durable Objects. State is stored the way a hand-written object
  usually stores it: `orders` keeps one storage key per record, with a small
  index key per order for the equality lookup; `rate-limiter` keeps its two
  numbers under one key. Bynk currently loads and commits an agent's whole state
  on each call, which is part of what the numbers show.
- What the twin leaves out, Bynk does: structured boundary errors, a security
  header policy, `HEAD` handling, fault reporting through `waitUntil`, and the
  cross-context call and migration endpoints. Those are part of the size.

If you change a twin, run the harness: it fails if the behaviour no longer
matches.

## Reading the numbers

- Bundle sizes are exact for this toolchain. Start-up and latency are not: they
  are medians of several runs, shown with their range, from one machine. Differences
  of a few tenths of a millisecond are inside the noise.
- Latency is the time for `dispatchFetch` to return and the body to be read, so it
  includes Miniflare's own proxying (about one and a half milliseconds even for
  `hello-world`'s smallest route). It is the same for both variants, so compare the two columns, not the
  absolute figure with a deployed Worker.
- Nothing here is a deployed measurement. Network, cold isolates on Cloudflare's
  fleet and storage latency are not modelled.
