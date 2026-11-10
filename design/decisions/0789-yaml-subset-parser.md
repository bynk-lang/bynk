# 0789 — The YAML subset is parsed by a hand-written runtime module, tested against a corpus

- **Status:** Accepted (v0.538)

**Context.** #3215 weighed a hand-written subset parser (a) against vendoring or depending
on an npm YAML library (b). The runtime has no npm runtime dependency (ADR 0615: modules are
embedded and emitted per project), `deny.toml` covers Rust crates only, and the issue wants
a subset that refuses anchors, aliases, merge keys, tags and multiple documents.

**Decision.** (a): `bynk-emit/runtime/src/yaml.ts`, a pure parser with no dependency, emitted
only by a project that calls `Yaml.decode`. It is 15.1 KB minified (5.1 KB gzip), measured
with esbuild; the libraries measured the same way were `yaml@2.8.1` at 97 KB (30 KB gzip)
and `js-yaml@4.1.0` at 40 KB (13 KB gzip), and `js-yaml` cannot refuse anchors by
configuration. Correctness is held by tests, not by the parser's size: a conformance corpus
drawn from the yaml-test-suite (`test/yaml-corpus/`, MIT, data tag `data-2022-01-17`) whose
`manifest.json` classifies each case as `pass` (the result equals the suite's `in.json`
exactly), `refused` (a valid case outside the subset, which must stay refused with its
recorded kind) or `lenient` (an invalid case still accepted, a ratchet that may only
shrink); a differential test against the `yaml` package, a dev dependency of the runtime
package and never shipped; the refusal, resolution, position and limit tests; and a seeded
mutation fuzz asserting that nothing throws or hangs. Anything outside the subset is refused
rather than guessed. Whitespace is a space or a tab only, scanned without regular expressions, so a long run of whitespace costs one pass and a quoted scalar folded over many lines costs time linear in its length (a review of #3275 measured and fixed a quadratic fold).

**Consequences.** No third-party code ships in an emitted project and the runtime's
no-runtime-dependency contract holds. The cost is ownership of a parser: an accepted
construct that is misread is a bug in Bynk, and the corpus is the guard. The corpus found and
fixed, before release, mis-chomped block scalars, escaped whitespace trimmed at a fold, and
a crash on an unterminated escape. The 11 `lenient` cases are about indentation and comment
placement in invalid documents; none yields a wrong value for a valid one.
