# 0465 — The standard library has four layers, each with an admission rule, and a unit joins it through the bynk-std registry

- **Status:** Accepted (v0.330.12)

**Context.** The standard library existed only in miniature, and nothing said what may join it or
where. ADR 0034 promised that "the stdlib can grow without compiler changes", but each first-party
unit was a hand-written block in `phase_parse`. Each block had its own cache and a hand-picked
`ExprId` base, and the dependencies between units were hand-written `||` cascades. One edge, the
`bynk` adapter's own `uses bynk.locale.types`, was caught only because that adapter happened to be
pushed into `parsed` before the `uses` scan ran. The standard-library track (spine #1936,
`design/archive/retired-tracks.md` §2, §4, §8 D5–D6) settled the layering and admission rules;
slice S1 (#1945) makes admission a matter of data.

**Decision.**

- **D1 — Four layers.**
  - **L0, the kernel:** the compiler's built-in types, kernel methods and compiler-backed statics.
  - **L1, the commons:** everything derivable from L0, written in Bynk.
  - **L2, the `bynk` surface:** capabilities whose guarantee holds on every supported platform.
  - **L3, platform adapters:** honest, platform-shaped infrastructure, which locks a deployment unit
    (ADRs 0016, 0017, 0026).

  Every stdlib item names its layer.
- **D2 — The admission rules.**
  - An item is L0 only if it cannot be written in Bynk over the existing kernel; otherwise it is L1
    (ADRs 0034, 0036).
  - An operation is **pure only if Bynk can implement it synchronously on every platform without
    third-party code**. `Effect[T]` lowers to `Promise<T>` and pure functions lower synchronously,
    so an operation whose platform implementation is async is a capability. Hashing, HMAC and
    signing over Web Crypto are therefore effectful; hex, base64url, URL and ISO-8601 stay pure.
  - An item is L2 only if its guarantee holds, or is weakened until it holds, on Cloudflare, Node
    and the browser. Infrastructure never joins L2 (ADR 0012).
  - Anything that needs syntax, a compile-time-checked payload, handler-transaction participation
    or a compiler-synthesised argument is not a capability, and is routed to language work
    (`bynk-greenfield-compiler.md` Part 14).
- **D3 — New capabilities are flat sibling units.** A new capability family ships as its own
  `bynk.<name>` unit (e.g. `bynk.crypto`), not inside `adapter bynk`. ADR 0012's list for the
  `bynk` surface stays closed, and each dependency remains one greppable `consumes` line.
  Extending an existing capability stays where it is (e.g. `Random.bytes`).
- **D4 — Admission is a registry entry plus its files.** A first-party unit is one entry in
  `bynk-std`'s `UNITS` table: name, kind (adapter, injected when consumed; commons, injected when
  used), synthetic identity path, source, binding (none, per platform, or a single fixed one),
  permanent `id_block`, platform nativity, and `env`-taking providers. `bynk-check` injects from
  the table and names no unit. `platform_of`, `provider_takes_env` and `FIRSTPARTY_SOURCES` are
  views over it, the last kept in its observable order (hover takes the first match; completion
  lists in order).
- **D5 — Dependencies come from the sources.** Injection follows each injected unit's own
  `uses`/`consumes` lines until nothing new is reached, so the edges between first-party units are
  never listed by hand, and the order-dependent edge above is now correct by construction. For
  every one of the 128 combinations of the seven references a project can make, the result equals
  the previous cascade.
- **D6 — Ids and order are permanent.** Each unit's `id_block` is the block it already had. It is
  never reused or moved, and a new unit takes the next free one. Injected units are pushed in
  ascending `id_block` order, which is the order the hand-written blocks used, so emitted output is
  byte-identical.
- **D7 — Unit-specific language behaviour stays named.** Secrets' deploy wiring, Locale message
  bundles, the `bynk`-unit special cases in the checker and emitter, and KV's Worker binding still
  name their unit. They are features of those units, not facts about units in general. Wrangler
  stanza derivation stays in the emitter until the derived-capabilities slice reshapes it.

**Consequences.** Adding a first-party unit no longer touches `bynk-check`. A structural test fails
if `phase_parse` or the injection loop names a unit or a per-unit constant again. The #901 drift
guard now checks that every `*_SRC` constant is the `source` of a `UNITS` entry. The registry's
id blocks and `FIRSTPARTY_SOURCES`' order are pinned by tests in `bynk-std`.
