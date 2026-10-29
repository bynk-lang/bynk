# bynk

[![crates.io](https://img.shields.io/crates/v/bynk.svg)](https://crates.io/crates/bynk)
[![docs.rs](https://img.shields.io/docsrs/bynk)](https://docs.rs/bynk)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The **Bynk developer front-end** — the `bynk` driver. It **links the compiler
pipeline in-process** and orchestrates the Node toolchain; `bynk` is to
[`bynkc`](https://crates.io/crates/bynkc) what `cargo` is to `rustc`. A fresh
`cargo install bynk` is self-contained: it compiles, scaffolds, and serves
projects with no separately-installed `bynkc`. Environment orchestration — *is
my machine ready?*, *scaffold me a project*, *build and serve it locally*, *ship
it* — is the driver's job.

See the [Bynk Book](https://bynk-lang.org/book/) for the full guide and
reference, and the [`bynk` CLI reference](https://bynk-lang.org/docs/bynk-cli/)
for every argument and exit code.

## The project-lifecycle arc

The driver's commands form a four-step arc, **`doctor → new → dev → deploy`**:

```sh
bynk doctor       # is my machine ready to compile, test, and deploy?
bynk new hello    # scaffold a complete, runnable project
cd hello
bynk dev          # build it and serve it locally on http://localhost:8787
bynk deploy       # provision Cloudflare resources and deploy every Worker
```

| Command | What it does |
|---|---|
| `bynk doctor` | Survey the toolchain grouped by capability (compile · test · dev/deploy · editor · build-from-source), reporting presence + version + provenance, and print the exact remedy for anything missing. |
| `bynk new <path>` | Scaffold a complete, runnable single-context HTTP service (`bynk.toml`, `.gitignore`, `src/<name>.bynk`) that `bynk dev` serves unmodified. Pure offline file-writing — no toolchain required. |
| `bynk dev` | Build the project and serve it locally with `wrangler dev` in local mode (Miniflare), rebuilding on save — every context served, Service Bindings wired (`--context` narrows it). One step in place of the manual `bynkc compile` + `cd` + `wrangler dev` recipe. No provisioning needed. |
| `bynk deploy` | Provision each context's Cloudflare resources and deploy its Worker, in Service-Binding dependency order. Cloudflare ids live in the committed `bynk.deploy.lock`; `--dry-run` prints the plan, `--env` targets an environment. |

Alongside the arc, the everyday compiler commands run through the driver with
`bynkc`'s exact semantics (the command bodies are shared, in `bynk-driver`):

| Command | What it does |
|---|---|
| `bynk check` | Type-check a `.bynk` file or project without writing output. |
| `bynk fmt` | Format `.bynk` sources in place (`-` for stdin → stdout). |
| `bynk test` | Discover and run a project's `suite`/`case` tests. |
| `bynk explain <code>` | Explain a diagnostic code — what the rule is, why it exists, and how to fix it. |

Each command has a pinned argument/exit contract; run `bynk <command> --help`
for the flags.

## Install

```sh
curl -fsSL https://github.com/bynk-lang/bynk/releases/latest/download/install.sh | sh   # prebuilt
cargo install bynk                                                                      # or from source
```

On Windows, use `irm https://github.com/bynk-lang/bynk/releases/latest/download/install.ps1 | iex`.
`cargo binstall bynk` also fetches the prebuilt binary.

Or build from the workspace:

```sh
cargo build --release -p bynk   # → target/release/bynk
```

Requires a stable Rust toolchain, 2024 edition (MSRV 1.95). The compiler is
linked in — `bynk dev` compiles a project in-process — so no separate `bynkc` is
needed; `dev` and `deploy` additionally shell Node + `wrangler`, and `bynk new`
needs neither (it only writes files). Power users can point `bynk` at an external
compiler with `$BYNK_BYNKC` (e.g. to pin a version); `bynk doctor` reports that
override and any driver↔compiler skew.

## Design

- **Links the pipeline** — the driver links the compiler library crates
  (`bynk-driver` / `bynk-emit` / `bynk-syntax` / `bynk-render` / `bynk-fmt`) and
  compiles in-process, rather than shelling the `bynkc` binary, so it is
  self-contained
  ([ADR 0101](https://github.com/bynk-lang/bynk/blob/main/design/decisions/0101-front-end-links-pipeline-binary-topology.md),
  building on [ADR 0083](https://github.com/bynk-lang/bynk/blob/main/design/decisions/0083-bynk-driver-thin-orchestrator.md)).
  A `$BYNK_BYNKC` override is kept as a power-user escape hatch.
- **Single-concern modules** — one per command (`doctor`, `new`, `dev`,
  `deploy`, `check`, `fmt`, `test`, `explain`), plus `compiler` (locate an
  external `bynkc` and report driver↔compiler version skew), `workers` (the
  in-process compile and `wrangler` plumbing `dev` and `deploy` share), and
  `report`. Tool detection (`probe`: presence + version + provenance, via the
  `which` crate) lives in `bynk-driver`, shared with `bynkc`.
- **Deterministic surface** — the human-facing output of each command is pinned
  by goldens in `tests/` (blessed with `BYNK_BLESS=1 cargo test -p bynk`), and
  `new`'s embedded starter template is compile-tested so the scaffold can't rot.

The decisions behind each command are recorded in
[`design/decisions/`](https://github.com/bynk-lang/bynk/tree/main/design/decisions)
(ADRs 0083–0084 for `doctor`, 0096 and 0192 for `dev`, 0097 for `new`, 0179,
0193, and 0254 among those for `deploy`, 0264 for `explain`).

## Tests

```sh
cargo test -p bynk
```

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
