# 0788 — Yaml.decode[T] reads YAML through the typed JSON decode path

- **Status:** Accepted (v0.538)

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
