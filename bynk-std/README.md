# bynk-std

[![crates.io](https://img.shields.io/crates/v/bynk-std.svg)](https://crates.io/crates/bynk-std)
[![docs.rs](https://img.shields.io/docsrs/bynk-std)](https://docs.rs/bynk-std)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The **first-party sources of the [Bynk](https://github.com/bynk-lang/bynk)
standard library, as data**: the `.bynk` sources and TypeScript bindings the
toolchain ships, embedded with `include_str!`, plus the metadata it reads about
them.

It holds:

- the `bynk` surface adapter (`Clock`, `Random`, `Logger`, `Fetch`, `Secrets`,
  `Locale`, `Idempotency`, `Events`) and its per-platform bindings for
  Cloudflare, Node and the browser;
- the Bynk-written commons: `bynk.list`, `bynk.map`, `bynk.string`,
  `bynk.locale` and `bynk.locale.types`;
- the `bynk.cloudflare` platform adapter (`Kv`, and the experimental `Service`)
  and its binding;
- the experimental `bynk.crypto` adapter (`Crypto`: digests, HMAC, constant-time
  comparison, sign and verify over Web Crypto) and its binding for each platform;
- `UNITS`, the registry of first-party units: each one's name, kind, source,
  binding, permanent id block, platform nativity and `env`-taking providers.
  `bynk-check` injects from it, so adding a unit is an entry plus its files;
- `Platform`, and the views over the registry: `FIRSTPARTY_SOURCES`,
  `platform_of` and `provider_takes_env`.

The sources are in `lib/`: the portable layers in `lib/core/`, the platform
adapter in `lib/cloudflare/` and the cryptography adapter in `lib/crypto/`.
Nothing in `lib/core/` depends on `lib/cloudflare/` or `lib/crypto/`.

The crate holds no logic. [`bynk-check`](https://crates.io/crates/bynk-check)
parses and injects these sources when a project `uses` or `consumes` them, and
re-exports this crate unchanged as `bynk_check::firstparty`.

## Where it sits

`bynk-std` has no dependencies. It is a second root beside the syntax leaf, read
by [`bynk-check`](https://crates.io/crates/bynk-check):

```text
bynk-syntax                lexer, parser, AST, CompileError, diagnostic codes
├── bynk-project           project model: discovery, unit graph, paths
├── bynk-ts                the TypeScript tree and its printer
├── bynk-fmt               the formatter
└── bynk-check             name resolution and type checking   + project, std, ts
    ├── bynk-model         the system model and its builder
    ├── bynk-emit          TS emission from a checked program + ts, model, project
    │   └── bynk-driver    shared CLI command bodies, build sequencing + fmt, project, ts, model, strip
    └── bynk-ide           non-bailing editor analysis         + project, fmt, model
bynk-std                   first-party sources and bindings, as data
bynk-strip                 TS → JS type-stripping (text level), on oxc alone
```

Each crate depends on its parent in the tree, plus any crates listed after
its `+`. The front-ends sit on top: the `bynk` CLI over
`bynk-driver`, the `bynk-lsp` language server (`bynk-lsp`) over `bynk-ide`, and
the unpublished `bynk-wasm` playground module over `bynk-driver`
and `bynk-ide`.

Most users compile Bynk through the
[`bynk`](https://crates.io/crates/bynk) CLI rather than depending on this crate
directly.

## Use

```toml
[dependencies]
bynk-std = "0.555"
```

```rust
use bynk_std::{FIRSTPARTY_SOURCES, Platform};

for (unit, source) in FIRSTPARTY_SOURCES {
    println!("{unit}: {} bytes", source.len());
}
let binding = Platform::Node.bynk_binding_source();
```

See the [API docs](https://docs.rs/bynk-std) for the full surface.

## License

Licensed under either of [Apache-2.0](https://github.com/bynk-lang/bynk/blob/main/LICENSE-APACHE) or
[MIT](https://github.com/bynk-lang/bynk/blob/main/LICENSE-MIT) at your option.
