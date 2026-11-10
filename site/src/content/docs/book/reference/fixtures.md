---
title: Test fixtures
---
A [suite](/book/reference/testing/) reads a project file's text with a
`fixture` declaration. The file's contents become a `String` constant for the
suite's cases, kept beside the tests and never compiled into deployable code.
See the [grammar for `fixture_decl`](/book/reference/grammar/#rule-fixture_decl).

## Declaring a fixture

```bynk,fragment
suite github.events

--- A delivery captured from the GitHub App's Advanced tab.
fixture issueOpened = "fixtures/github/issues-opened.json"

fixture pushToMain = "fixtures/github/push.json"

case "a real issues delivery decodes" {
	let r = read(Some("issues"), Some("d-1"), Bytes.fromUtf8(issueOpened))
	expect r is Ok(_)
}
```

`fixtures/github/issues-opened.json` is an ordinary file, so a real 12 KB
payload stays readable, diffable and highlighted by its own language, instead
of an escaped string in a case. `issueOpened` is a `String`; turn it into a
value with `Bytes.fromUtf8` or `Json.decode[T]`.

- **Path:** relative to the project root, and inside the project's `fixtures/`
  folder. Anything else is `bynk.fixture.outside_fixtures_dir`; a path that is
  empty, absolute or contains `..` is `bynk.fixture.unsafe_path`. The file must
  exist as a regular file (`bynk.fixture.missing_file`) and be readable
  (`bynk.fixture.unreadable_file`); symbolic links are not followed. Only the
  files a declaration names are read.
- **Name:** a `String` value in the case scope, so it may not repeat a fixture
  of its suite (`bynk.fixture.duplicate`), nor take a name already in scope
  there (`bynk.resolve.name_conflict`): a function, type, constant, embed, asset,
  service or agent of the target or of a commons it `uses`, or `deps`.
- **Scope:** the declaring suite only. Its `case`s, `property` bodies,
  [helper `fn`s](/book/reference/testing/#helpers), `stub` clauses (their
  argument patterns, `returns` values and `after`) and
  [`env` entries](#in-env) see it, at `unit`, `integration` and `system`. Another suite, even one of the same target,
  cannot name it, and may declare a fixture of the same name.
- **Unused:** a fixture nothing in its suite mentions is `bynk.fixture.unused`,
  a warning. Naming it only in a `stub` clause or an `env` entry counts as use.

## A fixture in `env` {#in-env}

A suite's or case's [`env`](/book/reference/testing/) entry may name one of the
suite's fixtures instead of a `String` literal. Its participants then read the
file's text as that variable or secret, at every tier:

```bynk,fragment
suite github.hooks as system {
	env {
		APP_CLIENT_ID = "Iv1.testclient"
		APP_PRIVATE_KEY = appKey
	}

	fixture appKey = "fixtures/test-app-key.b64"

	case "the webhook signs in as the app" {
		let r <- hooks.POST(
			"/github",
			RequestHeaders.of(["X-GitHub-Event: ping"]),
			Bytes.fromUtf8("{}")
		)
		expect r is Ok(_)
	}
}
```

The value is the text as described below, so a trailing newline in the file is
part of it; trim it in the code that reads the secret, or save the file without
one. Another suite's fixture is not in scope, so naming one is
`bynk.suite.env_not_literal`. Under `bynk dev` each value is passed to the
Workers on the command line, which on Linux limits one value to 128 KiB.

## What the text is

The rules are an [embed](/book/reference/embeds/)'s: the file's contents
verbatim, except that a leading byte-order mark is dropped and `\r\n` and a bare
`\r` become `\n`, so a test reads the same on every checkout. The file must be
UTF-8 text: invalid UTF-8 is `bynk.fixture.invalid_encoding` and a NUL byte is
`bynk.fixture.binary_content`.

The text is not interpreted: `\(` and `{{ }}` mean nothing to the language.

## Limits

One file is at most **4 MiB** (`bynk.fixture.too_large`). There is no total for
a project: a fixture is not deployed, so the 1 MiB budget that protects a
Worker's bundle does not apply. The text is written into the suite's test
module once, however many cases read it, so a test runs without touching the
disk.

## Never deployed

A deployable is built without suites, so a fixture's text is in no file of the
output, and the output is the same with or without a suite that reads one. The
text exists only in test output. That is the difference from an `embed`, which
ships in the Worker and is capped at 256 KiB.

`bynk dev` re-runs when a file in `fixtures/` changes, and the editor checks that
each `fixture` still names a file when one is added or removed.

## Fixture, embed or asset

| | `fixture` | `embed` | `asset` |
| --- | --- | --- | --- |
| Folder | `fixtures/` | `embeds/` | `assets/` |
| Declared in | a suite | a context or commons | a context or commons |
| Deployed | never | yes, part of the code | yes, served |
| Value | a `String` | a `String` | an `Asset`, with a `.url` |
| Content | UTF-8 text | UTF-8 text | any file |
| Size | 4 MiB a file | 256 KiB a file, 1 MiB in all | the platform's |

Capture a fixture from the real producer rather than writing it by hand, so it
cannot drift from what that producer sends.
