# Semver

Parse, order and match [semantic versions](https://semver.org) — the part of a
package manager that decides *which release does this range mean*. A pure
library, plus a small HTTP service that exposes it.

What it shows:

- **A real algorithm in a `commons`.** `parse`, `compare` and `satisfies` are
  ordinary functions over a record, so the whole thing is unit-tested with no
  context, no platform and no stubs.
- **Errors as values.** A malformed version or range is a `Result`, never an
  exception; the service turns an `Err` into a `400`.
- **Recursion over a list** (`matchesAll`, `satisfiesSets`, `comparePreFrom`)
  where a loop with an early exit would be written elsewhere, and `fold` where
  the walk visits everything (`maxSatisfying`).
- **Strings as the input format.** `split`, `slice`, `indexOf`, `startsWith`
  and `Int.parse` take a version and a range apart.

The semantics follow semver 2.0 and the npm range dialect:

| Range | Means |
|---|---|
| `1.2.3`, `=1.2.3` | exactly `1.2.3` |
| `>1.2.3`, `>=1.2.3`, `<1.2.3`, `<=1.2.3` | a comparison |
| `^1.2.3` | `>=1.2.3 <2.0.0`; `^0.2.3` is `>=0.2.3 <0.3.0`; `^0.0.3` is `>=0.0.3 <0.0.4` |
| `~1.2.3` | `>=1.2.3 <1.3.0` |
| `>=1.2.3 <2.0.0` | every comparator must hold |
| `^1.0.0 \|\| ^3.0.0` | either set may hold |
| `*` | anything (but not a pre-release) |

Not supported, and reported as an error rather than guessed at: a space after
an operator (`>= 1.2.3`), partial versions (`^1.2`, `1.x`) and hyphen ranges
(`1.0.0 - 2.0.0`). Pre-release tags are validated (`1.0.0-01` and `1.0.0-a..b`
are rejected).

The service bounds its input: a version is at most 64 characters, a range 256
and a `/resolve` list 1000 versions, so an oversized body is a `400` and the
recursion in `satisfies` stays shallow.

Pre-releases order below their release (`1.0.0-rc.1 < 1.0.0`) and are opt-in:
`1.3.0-beta.1` satisfies a range only if a comparator in it names a
pre-release of `1.3.0`. Build metadata (`+build`) is accepted and ignored.

## Layout

```text
semver/
├── bynk.toml
├── src/
│   ├── semver.bynk    # commons semver — Version, parse, compare, satisfies, maxSatisfying
│   └── api.bynk       # context api — the HTTP service
└── tests/
    ├── semver.bynk    # the library: parsing, precedence, every range form
    └── api.bynk       # the service: statuses and bodies
```

## Check and test

```sh
bynkc check .
bynkc test .
```

## Run it

```sh
bynk dev
```

```sh
curl -XPOST localhost:8787/compare -d '{"a":"1.2.0","b":"1.10.0"}'
# {"order":-1}

curl -XPOST localhost:8787/satisfies -d '{"version":"1.4.2","range":"^1.2.0"}'
# {"satisfies":true}

curl -XPOST localhost:8787/resolve \
  -d '{"versions":["1.0.0","1.9.0","2.0.0"],"range":"^1.0.0"}'
# {"version":"1.9.0"}

curl -XPOST localhost:8787/satisfies -d '{"version":"1.0.0","range":"^banana"}'
# (HTTP 400)
```
