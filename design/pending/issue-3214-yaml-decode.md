---
level: minor
changelog: A `Yaml.decode[T](text)` static reads a safe YAML 1.2 subset through the same typed decode as `Json.decode`, with positioned refusals and `DecodeError` paths
---

## ADR: yaml-decode
title: Yaml.decode[T] reads YAML through the typed JSON decode path
summary: A compiler-known Yaml.decode static; YAML parses to the JSON-shaped tree, then the identical deserialise_<T>; core-schema resolution, distinct refusals, depth and size caps

**Context.** Settings people keep in a repository are mostly YAML, and a Worker reading one
needs the refusals `Json.decode[T]` gives: field defaults, `Option`, refinements and an
error that names its path. `decode[T]` must be compiler-known (ADR 0045), so a library
cannot offer it. Proposed in #3215 for #3214. ADR 0047's `JsonError` was first renamed
`DecodeError` so both decoders share one type.

**Decision.**

- **The surface.** `Yaml.decode[T](text: String) -> Result[T, DecodeError]`, a compiler-backed
  static on a built-in `Yaml` module beside `Json`. `T` is any type `Json.decode` accepts,
  explicit or inferred from an expected `Result[T, DecodeError]`; the checks and refusals are
  `Json.decode`'s (`bynk.types.json_uncodable`, `bynk.generics.uninferable_type_arg`). It
  lowers to the `Json.decode` wrapper with the runtime's `__bynkYamlParse` in place of
  `JSON.parse`, so YAML reaches the identical `__deserialise_<T>` and the identical error
  mapping. There is no `Yaml.encode` (a style policy and a round-trip contract the issue did
  not ask for) and no `Yaml.decodeSql`.
- **Core-schema resolution, by shape, plain scalars only.** Null, booleans, integers
  (decimal, `0o`, `0x`) and floats as YAML 1.2 defines them; everything else is a string, so
  `on`, `off`, `yes`, `no`, timestamps and `1_000` stay strings. A quoted scalar is a string.
  A mapping key is its text. Integers beyond 2^53 read as `JSON.parse` reads them, so the
  typed step decides, with no new rule.
- **Refusals are distinct, positioned `DecodeError`s, never a partial value.** `kind` is
  `Malformed`, `UnsupportedAnchor`, `UnsupportedAlias`, `UnsupportedMergeKey`,
  `UnsupportedTag`, `MultipleDocuments`, `UnsupportedSyntax`, `UnsupportedValue` (`.inf`,
  `.nan`: the JSON-shaped tree cannot hold them, as the codec already refuses a non-finite
  `Float`), `DuplicateKey` (ADR 0701's rule, not last-wins) or `LimitExceeded`. `kind` is
  free text on the existing record, so no type changes. `path` is the `$.a.b[2]` path where
  the refusal was found, and `message` ends `(line L, column C)`. Tags are refused even
  when they agree with the resolved type, because resolution is by shape.
- **Limits.** Input over 1,048,576 UTF-8 bytes (the HTTP body cap) and nesting over 256 levels
  (the recursive-codec bound of #2656, counted over block and flow collections) are refused
  while parsing, so a hostile document cannot exhaust the stack. `Json.decode` has no size
  cap of its own, because the host's `JSON.parse` bounds its own cost; a hand-written parser
  is not covered by that.

**Consequences.** A settings file decodes into a typed record with `$.nudges.after_days`-style
errors and a refusal a maintainer can act on. A file that relies on anchors, aliases, merge
keys, tags or several documents is refused with a stable, documented reason, and the subset
can only widen later. The `Yaml` name is shadowable by a user type, as `Json` is. The editor
surface lists `Yaml` and its one static; no new diagnostic code is added.

## ADR: yaml-subset-parser
title: The YAML subset is parsed by a hand-written runtime module, tested against a corpus
summary: A pure ~15 KB runtime parser instead of an npm dependency; yaml-test-suite subset as the conformance corpus and `yaml` as a test-only oracle

**Context.** #3215 weighed a hand-written subset parser (a) against vendoring or depending
on an npm YAML library (b). The runtime has no npm runtime dependency (ADR 0615: modules are
embedded and emitted per project), `deny.toml` covers Rust crates only, and the issue wants
a subset that refuses anchors, aliases, merge keys, tags and multiple documents.

**Decision.** (a): `bynk-emit/runtime/src/yaml.ts`, a pure parser with no dependency, emitted
only by a project that calls `Yaml.decode`. It is 14.8 KB minified (5.0 KB gzip), measured
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
rather than guessed.

**Consequences.** No third-party code ships in an emitted project and the runtime's
no-runtime-dependency contract holds. The cost is ownership of a parser: an accepted
construct that is misread is a bug in Bynk, and the corpus is the guard. The corpus found and
fixed, before release, mis-chomped block scalars, escaped whitespace trimmed at a fold, and
a crash on an unterminated escape. The 11 `lenient` cases are about indentation and comment
placement in invalid documents; none yields a wrong value for a valid one.
