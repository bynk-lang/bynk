---
title: Read a YAML settings file into a typed value
---
**Goal:** turn the text of a YAML file (a repository's settings, say) into a
fully-typed, validated Bynk value, and refuse a bad file with a reason a person can
act on.

`Yaml.decode[T](text)` parses a YAML document and checks it against `T`, returning
a `Result[T, DecodeError]`. It is [`Json.decode`](/book/guides/type-system/decode-json/)
with a different front end: the YAML becomes the same tree of objects, lists,
strings, numbers, booleans and `null` that JSON does, and then goes through the
identical check. Field defaults, `Option`, refinements and the `$.path` in an error
all behave exactly as they do for JSON.

## Decode a settings file

A bot reads a `.github/settings.yml` that turns each of its features on per
repository:

```yaml
# Which features the App runs here, and how.
triage:
  enabled: true
  labels: [bug, enhancement, documentation]   # the only labels it may suggest
nudges:
  enabled: true
  after_days: 3
```

Declare the shape, and put the rules in the types:

```bynk
commons settings {
	type AfterDays = Int where InRange(1, 30)

	type Triage = {
		enabled: Bool,
		labels: List[String],
	}

	type Nudges = {
		enabled: Bool = false,
		after_days: AfterDays = 3,
	}

	type Settings = {
		triage: Triage,
		nudges: Option[Nudges] = None,
	}

	fn load(text: String) -> Result[Settings, DecodeError] {
		Yaml.decode[Settings](text)
	}
}
```

A `nudges` block without `after_days` takes the default `3`, an absent `nudges` is
`None`, and a file that sets `after_days: 90` is refused with the field named:

```text
RefinementViolation at $.nudges.after_days: …
```

so the message can say *which* setting is wrong, not just "invalid".

## Handle the result

`DecodeError` is the same type `Json.decode` returns, with the same three fields
(`kind`, `path`, `message`), so one `match` handles either decoder:

```bynk
commons settings {
	type Settings = { mode: String = "auto" }

	fn describe(text: String) -> String {
		match Yaml.decode[Settings](text) {
			Ok(s) => "mode is \(s.mode)",
			Err(e) => "\(e.kind) at \(e.path): \(e.message)",
		}
	}
}
```

## What is read, and how

Bynk reads a **safe subset of YAML 1.2**: block and flow mappings and sequences,
plain, single-quoted and double-quoted scalars, block scalars (`|` and `>`), and
comments, in a single document.

Plain scalars are typed by the YAML 1.2 *core schema*, so `true`, `false`, `null`,
`~`, integers (`12`, `0o17`, `0x1F`) and floats (`1.5`, `1e3`) are what they look
like. Everything else is a string, and in particular **`on`, `off`, `yes` and `no`
stay strings**, as do `2001-12-14` and `1_000`. A quoted scalar is always a string,
so `"1"` is the text `1`, not a number. If a field is a `Bool`, write `true`.

## What is refused

A file that needs more than that subset is refused with its own `kind` and the line
and column in the message, never half-read:

| Kind | Cause |
|---|---|
| `UnsupportedAnchor` | an anchor (`&name`) |
| `UnsupportedAlias` | an alias (`*name`) |
| `UnsupportedMergeKey` | a merge key (`<<`) |
| `UnsupportedTag` | a tag (`!!str`, `!custom`) |
| `MultipleDocuments` | a second document after `---` |
| `UnsupportedSyntax` | a construct outside the subset (a complex `? key`, a directive) |
| `UnsupportedValue` | `.inf` or `.nan`, which a decoded value cannot hold |
| `DuplicateKey` | the same key twice in one mapping |
| `LimitExceeded` | input over 1 MiB, or nested deeper than 256 levels |
| `Malformed` | a syntax error |

Anchors and aliases are refused on purpose: they let a small file expand into an
enormous one, and a settings file should read the same top to bottom.

**See also:** [Reference: `Yaml.decode`](/book/reference/types/#yaml-decode),
[Decode untrusted JSON into a typed value](/book/guides/type-system/decode-json/).
