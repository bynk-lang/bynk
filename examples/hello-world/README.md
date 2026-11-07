# Hello, World!

A complete Bynk project in two source files and a test — run it locally,
then deploy it to Cloudflare, from the same build.

It is small, but it is not a toy: it shows the things Bynk is *for*.

- **A refined type** — `Subject` is a `String` that is provably non-empty
  and at most 40 characters. `greeting` takes a `Subject`, so it never
  validates input: invalid subjects cannot exist.
- **Honest effects and capabilities** — the HTTP handlers declare
  `given Logger`; the dependency is visible in the signature, supplied by
  the platform (`consumes bynk { Logger }`), and mockable in tests.
- **Typed HTTP** — `on http` handlers return `HttpResult`; the compiler
  generates the router, boundary validation, and the Worker entry point.
- **Tests in the language** — `bynk test` fabricates a pinned
  `Val[Subject]` and asserts behaviour, no harness code.

## Layout

```text
hello-world/
├── bynk.toml               # project manifest ([paths])
├── src/
│   └── hello/
│       ├── text.bynk       # commons hello.text — Subject + greeting
│       └── web.bynk        # context hello.web — the HTTP service
└── tests/
    └── hello/
        └── text.bynk       # tests targeting hello.text
```

A unit's dotted name mirrors its path: `commons hello.text` lives at
`src/hello/text.bynk`. The `context` is the unit of deployment — it
becomes one Cloudflare Worker.

## Prerequisites

Run `bynk doctor` to check these for you (see the book's install page):

```sh
bynk doctor
```

- `bynk` on your `PATH` (see the book's install page; from this
  repository: `cargo build --release -p bynk` →
  `target/release/bynk`).
- Node.js (for `bynk test` and for Wrangler).

## Check and test

From this directory:

```sh
bynk check src
bynk test .
```

```text
hello.text:
  ✓ greets the world
  ✓ greets any valid subject
  ✓ rejects an empty subject

3 passed, 0 failed.
```

## Build the Worker

```sh
bynk compile src --output out --target workers
```

This emits a complete, standard Cloudflare Worker under
`out/workers/hello-web/` (entry point, router, dependency wiring, and
`wrangler.toml`), plus the shared runtime and the platform binding for
the `bynk` surface.

## Run it locally

```sh
bynk dev
```

From anywhere inside the project, `bynk dev` compiles, picks the `hello-web`
worker, and serves it on `http://localhost:8787` — no need to build first or
`cd` anywhere. (Under the hood it runs the `bynk compile` above, then `cd
out/workers/hello-web && npx wrangler dev`.)

Then, in another terminal:

```sh
curl http://localhost:8787/
# "Hello, World!"

curl http://localhost:8787/hello/Bynk
# "Hello, Bynk!"

curl http://localhost:8787/hello/this-name-is-way-too-long-to-be-a-valid-subject
# {"error":"a name must be non-empty and at most 40 characters"}  (HTTP 400)
```

The `Logger.info` lines appear in the `wrangler dev` console — that is
the `bynk { Logger }` capability, bound by the toolchain for the
Cloudflare platform.

## Deploy it

From the same directory (a free Cloudflare account is enough;
`wrangler` will prompt you to log in the first time):

```sh
cd out/workers/hello-web
npx wrangler deploy
```

Wrangler prints the deployed URL — your greeting is live at
`https://hello-web.<your-subdomain>.workers.dev/hello/World`.

## Where to go next

- [Tutorials](https://bynk-lang.org/book/tutorials/01-first-program/) — the URL
  shortener series grows these same ideas into a stateful service.
- [How a Bynk program is shaped](https://bynk-lang.org/book/guides/program-structure/how-a-program-is-shaped/)
  — why contexts, capabilities, and boundaries look the way they do.
