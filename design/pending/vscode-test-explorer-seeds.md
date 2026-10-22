---
level: patch
changelog: The VS Code Test Explorer shows a failed property's seed and counterexample as data, re-runs it with that seed, and can pin the seed per test (held back when the property tests before it change); `bynk test --format json` gives a failed property or history property `kind`, `seed` and `counterexample`, and `--no-run` marks generative cases with `kind`
---

## ADR: test-json-generative-case-fields
title: A failed generative case reports its kind, seed and counterexample as data
summary: Appends kind, seed and counterexample to the test JSON Case, after tier, and marks generative cases in discovery

**Context.** A failed `property` or history property reached tooling as one
sentence: the case count, the root seed, the shrunk counterexample and a
`reproduce:` line, handed over as `message` (ADR 0098). An editor could not offer
to reproduce the failure, keep the seed, or show the counterexample as anything
but prose, short of parsing the runner's wording and so freezing it as a
contract. A property that threw something other than a failed `expect` returned
a bare `String(e)` with no seed, although its draws depend on the seed just the
same. ADR 0567 let the document gain optional trailing `Case` fields.

**Decision.** `Case` gains three optional fields after `tier`, in this order:
`kind`, `seed` and `counterexample`; the order is part of the document's
contract, and each is omitted when absent. The runner supplies them, because the
seed and the shrunk values are already in hand where the failure is built, and
the message wording stays free to change: `__bynkRunProperty` and
`__bynkRunHistory` return them beside `error`, and the `case` event carries them
for a failed case. `kind` is `property` or `history` and is set, with `seed`, for
every failed generative case, including a non-assertion throw; `seed` is the
run's root seed as the `0x…` text `--seed` takes; `counterexample` is the
runner's rendering of the shrunk inputs (or of the shrunk history sequence) and
is set only when shrinking produced one. The fields ride the `case` event only
when the build has a module that inlines the generative runtime, so every other
runner, and every passing case, is unchanged. Discovery (`--no-run`) also sets
`kind` on each generative case (a `property`, a history property, a contract
attack), so a tool can tell which cases a seed's stream depends on without
running. The extension pins a seed as the pair of the seed and the ordered names
of the suite's generative cases, and holds a pin back when that list differs,
since a property's stream is `mix(rootSeed, ordinal)` and the ordinal is its
position among those cases. A name-stable derivation was rejected as the default:
it would change the stream for every seed already in a log or in CI.

**Consequences.** A consumer must ignore fields it does not know and must not
assume a property's `kind` is present on a passing run. A pin is editor-local
state, never `bynk.toml` or source. A later case field is appended after
`counterexample`.
