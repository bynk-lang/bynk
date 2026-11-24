---
level: minor
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: `String` gains `isEmpty`, `trimStart`, `trimEnd`, `padStart`, `padEnd`, `repeat`, `lines`, `lastIndexOf` and `replaceFirst`, `List[String]` gains `join(sep)`, and `bynk.string` is deprecated
---

## ADR: string-methods-and-list-join
title: String method completion, List[String].join, and bynk.string deprecated
summary: Adds nine String kernel methods and join on a List[String], and deprecates the one-function bynk.string commons

**Context.** The `String` kernel lacked everyday operations, and joining a list was the free function `join(parts, sep)` in `bynk.string`, a commons that held nothing else, the same half-finished move ADR 0116 left for `List`.

**Decision.** Nine `String` methods: `isEmpty`, `trimStart`, `trimEnd` (the whitespace `trim` defines), `padStart`/`padEnd`, `repeat`, `lines`, `lastIndexOf` and `replaceFirst`. Padding widths count code points (consistent with the length predicates, #3949) through the runtime helper `__bynkPad`; the last repeat of a multi-character `fill` is cut to fit; an empty `fill` literal is the error `bynk.string.empty_pad_fill` and a computed empty one faults. `repeat` with a negative count faults. `lines` splits on `\n` and `\r\n`, gives no empty last line for a trailing break, and `[]` for `""`. `lastIndexOf` returns a UTF-16 code-unit index like `indexOf`, `slice` and `length`. `replaceFirst` inserts `to` literally, as `replace` does. `join(sep)` is an arity-one form of the existing `List.join` (which takes three arguments), on a `List[String]` only: any other element type is `bynk.types.method_not_found` with a note to map to `String` first, and there is no implicit `toString`. `bynk.string` is deprecated: `join(parts, sep)` warns with a machine-applicable fix to `parts.join(sep)`. The warning code is `bynk.string.deprecated_function`, not the issue's generic `bynk.deprecated`, for consistency with `bynk.list.deprecated_function` (#3962) and the `bynk.map` and `bynk.set` ones (#3961); each deprecated module carries its own code, registry entry and docs route.

**Consequences.** Additive for programs not using `bynk.string`; a program that does gets a warning with a fix, nothing fails. `bynk.locale` (the one repository unit that used `bynk.string`) now calls `parts.join(", ")`. Fixture 211 keeps using `bynk.string.join`, deliberately, to cover the deprecation. Completion lists `join` on any `List` receiver (completion reads the kernel table by receiver kind, not element type), and a non-`String` receiver gets the `method_not_found` note when the call is checked.
