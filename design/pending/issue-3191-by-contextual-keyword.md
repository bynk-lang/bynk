---
level: minor
changelog: "`by` is a contextual keyword: an ordinary name (a parameter, a binding, a field) everywhere except its clause positions, so `on call bump(by: Int)` parses"
probes: diagnostic_coverage
---

## ADR: by-contextual-keyword
title: `by` is a contextual keyword, read by text in its clause positions
summary: `by` lexes as an identifier; its four clause positions recognise it by text, with a same-line-and-identifier rule for the call-site clause

**Context.** `by` was one of the 61 hard keywords, so `on call bump(by: Int)` was
`bynk.parse.reserved_keyword` and its only advice was "rename the identifier".
`by` is a short, common word, and the natural name for a step, an increment or a
sort key. ADR 0716 already admits it as a *label* (a field before `:`, a member
after `.`), so `by` was legal as a record field and illegal as a parameter, a
`let`, a lambda binding or a bare reference. The language already has a
contextual tier (`advance`, `par`, `key`, `store`, …) for exactly this. Calls are
positional (there are no named arguments), so `bump(by: 2)` is not call syntax;
what has to be admitted is the name in the parameter list and the body.

**Decision.**

- **A. How.** `by` is lexed as an ordinary identifier (`TokenKind::By` is
  removed), and each clause position recognises it by text, as `advance` already
  does: a handler's or service header's actor clause, a call's actor clause, the
  amount of `advance <Cap> by <Duration>`, and the label of `@indexed(by: …)`. It
  moves from the hard keywords to the contextual words in the keyword registry (60
  hard keywords), and every identifier lookahead (record construction, `:=`,
  lambda, record patterns, `store` fields) admits it with no special case.
- **B. The call-site clause.** A block has no statement terminators, so after an
  effect-let value (or an `expect` subject) `by` is either the clause or the first
  token of the next statement. It is the clause only when it is on the same line
  as the end of the value **and** an identifier (the actor) follows it, the shape
  of the same-line rule ADR 0716 uses for members. The formatter always writes the
  clause on the value's own line, so formatted code is never misread. A `by` on a
  later line, or not followed by an identifier (`{ let a <- f() by }`), is the next
  statement. The residual, two statements on one line with `by` second and an
  identifier after it, reads as a clause and reports the clause's own error.
- **C. Field shorthand.** `Step { by }` and `Step { by, .. }` become ordinary
  shorthand, with no exception. `bynk.parse.keyword_field_shorthand` stays for the
  words that remain hard keywords.
- **D. Scope.** Everywhere an identifier goes: parameters, `let`, lambda and
  pattern bindings, bare references, `store` and `key` field names, record
  shorthand. A partial rule would leave the gap `event` had.
- **E. Agent operation names.** `on call by(…)` is admitted with the rest (the
  next token is `(`, so there is no ambiguity), rather than carrying a refusal of
  its own.

**Consequences.** `bump(by: Int)` and `xs.map((x) => x + by)` parse. The
checker, emitter and runtime are untouched: `by` is a legal TypeScript
identifier and the emitter's reserved names do not include it. The editor
surfaces follow: the keywords reference and `llms.txt` move `by` to the
contextual tier, the editor contract drops it from the hard keywords, the
TextMate grammar colours `by` only before an actor (or after `advance <Cap>`),
and the tree-sitter grammar reads `by` as a contextual identifier with a declared
conflict at the call-site clause settled for the clause by dynamic precedence.
Tree-sitter has no newline awareness, so in the contrived one-line layout its
reading can differ from the compiler's, which is authoritative. Hover shows the
keyword doc only where `by` is a clause word and the ordinary name hover
elsewhere, and completion offers actor names only in clause positions. A reader
can now write `-> T by by: Visitor`, which is legal and ugly; the formatter does
not rename.
