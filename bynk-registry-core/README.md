# bynk-registry-core

**The [Bynk](https://github.com/bynk-lang/bynk) registry's admission rules, with no I/O.**
`unpack`, `pack` and `admit` work on an archive held in memory: the hardened
archive reader, the canonical tree checksum, and the manifest, name, version and
dependency rules a package must meet.

- Natively, `bynk registry add` and `bynk registry serve` link it.
- It also builds to a standalone `wasm32-unknown-unknown` module with a plain C
  ABI (`alloc`, `dealloc`, `admit`), via `scripts/build-registry-core.sh`, so the
  hosted registry applies the very same rules.

It has no `std::fs`, no `ring` and no network. Signing and the index stay with
the caller.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.
