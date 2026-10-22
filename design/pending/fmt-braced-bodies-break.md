---
level: minor
changelog: "`bynk fmt` always breaks braced declaration bodies: record types, `fn`/handler bodies and multi-field record literals go one item per line, however short; `match` arms and `if` branches stay compact; a `suite` header gets a blank line and `case`/`property` keep their order"
---

## ADR: fmt-braced-bodies-break
title: The formatter always breaks braced declaration bodies
summary: Record types, fn and handler bodies and multi-field record literals break regardless of width

**Context.** The formatter packed any braced body that fitted the line width onto
one line, so `type P = { x: Int, y: Int }` and `fn f() -> Int { 1 }` stayed inline
while a slightly longer sibling wrapped. Whether a declaration was one line or
five depended on its width rather than its shape, and it did not honour the
layout the author had written.

**Decision.** A braced declaration body always breaks: a record type (and an
`event` body) puts one field per line, an `fn`, handler or provider operation
body goes on its own indented lines, and a record literal with two or more
fields (or a spread) puts one field per line. An empty body prints as `{}`.
`match` arms, `if` branches and lambda bodies are unchanged: they stay compact
when they fit. A record literal nested inside another expression (a list
element, a call argument) still follows the enclosing expression's width.

A fragment-form `suite` header is followed by a blank line, as a `commons` header
is, and `case` and `property` blocks keep their source order instead of every
`property` moving after the last `case`.

**Consequences.** Formatted output changes for most existing sources, so the
first-party `bynk-std/lib` sources are reformatted here and downstream projects
will see a one-off diff from `bynk fmt`. No emitted output changes: formatting
is whitespace only. `--max-line-width` still governs call arguments, list
literals and chains.
