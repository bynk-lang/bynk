---
title: "Format your code with `bynk-fmt`"
---
**Goal:** format Bynk source to the canonical style.

Bynk's formatter is built into the compiler as `bynk fmt`.

## Format files in place

```sh
bynk fmt src/counters.bynk
bynk fmt src/*.bynk
bynk fmt .
```

A directory formats the `.bynk` files `bynk check` reads for it: a project
root's `[paths] include` trees minus `exclude`, or any other directory walked
recursively.

This rewrites the named files to canonical form (tab indentation, normalised
spacing). For example:

```bynk
commons demo {
type Id=Int
fn add(a:Int,b:Int)->Int{a+b}
}
```

becomes:

```bynk
commons demo {
	type Id = Int

	fn add(a: Int, b: Int) -> Int { a + b }
}
```

## Format via stdin

Pass `-` to read from stdin and write to stdout — handy for editor integrations:

```sh
cat src/counters.bynk | bynk fmt -
```

## Check formatting in CI

`--check` verifies formatting without writing, exiting non-zero if any file is
not already canonical:

```sh
bynk fmt --check src/*.bynk
```

## Override the style for a run

Bynk has one canonical style, and the default output is it. When a house style
or a narrower terminal genuinely calls for something else, four flags override
the style for that invocation:

```sh
bynk fmt --indent spaces --indent-width 4 src/*.bynk
bynk fmt --max-line-width 120 src/*.bynk
bynk fmt --no-trailing-comma src/*.bynk
```

| Flag | Default | Effect |
|---|---|---|
| `--indent tab\|spaces` | the project's, else `tab` | Tabs are the default so each reader picks their own width in their editor. |
| `--indent-width N` | the project's, else `2` | Spaces per nesting level. Passing it when the run resolves to tabs is an error rather than a silent no-op. |
| `--max-line-width COLUMNS` | the project's, else `100` | The soft target a construct wraps to fit. A line with no break point in it (a long string literal) is left long. |
| `--no-trailing-comma` | off | Drops the trailing comma from multi-line records, sums, list literals and `exports` clauses. `--trailing-comma` is its opposite. |
| `--no-config` | off | Ignore the project's `[fmt]` section for this run. |

Nothing is written to `bynk.toml` — a flag applies to that run only.

## Set a style for the whole project

Put it in the project's [`bynk.toml`](/docs/manifest/) and every `fmt` run picks
it up, along with format-on-save in the editor:

```toml
[fmt]
indent = "spaces"
indent_width = 4
max_line_width = 120
```

The manifest is found by walking up from each file being formatted, so a command
that spans two projects gives each its own style. A flag on the command line
overrides the field it names and leaves the rest of the section in force; the
resolution order is defaults → `[fmt]` → flags.

`--check` judges files against those resolved options, so a project on a
non-default style has a CI gate that can actually pass:

```sh
bynk fmt --check src/*.bynk
```

To format to the canonical style whatever project you are standing in — a
release script, say — add `--no-config` to skip the `[fmt]` layer.

## Related

- [Set up editor support](/docs/editor-and-tooling/editor-support/) for format-on-save.
- Reference: [`bynk-fmt`](/docs/tooling/bynk-fmt/).
