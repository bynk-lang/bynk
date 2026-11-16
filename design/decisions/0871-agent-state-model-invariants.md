# 0871 — `bynk/agentStateModel` carries an agent's declared `invariant`s and `transition`s as one list of source-text predicates

- **Status:** Accepted (v0.633.5)

**Context.** The agent state model carried an agent's step `transition`s only
indirectly, as per-edge `violations` and a machine's `not_evaluated` list, and
its snapshot `invariant`s not at all. A client could not list them, show them
beside the state diagram, or tell that one changed between two revisions
(bynk-lang/vscode-bynk#154). Both are in the checked AST, so the language, not the
client, should supply them (#3608). This adds a field to the wire shape recorded
in ADRs 0422, 0535, 0536 and 0547; it changes nothing else.

**Decision.**

- **One list, discriminated by `kind`.** `AgentStateModel.invariants` (in
  `bynk-check`) and the wire's `invariants` hold `{ kind: "invariant" | "transition",
  name, predicate, range, doc? }`: the agent's snapshot `invariant`s, then its
  step `transition`s, each in declaration order. They share one predicate
  surface (ADR 0144), and a client presents them together.
- **The predicate is source text, printed by the expression printer.** It is
  `bynk_syntax::print::expr_to_string`, the printer `bynk-fmt` re-exports, so
  whitespace, line breaks and `//` comments in the declaration do not change it and a
  client can compare two revisions by `(kind, name, predicate)`. A structured
  predicate (as in ADR 0638's refined-type explorer) is more than a listing or a
  diff needs.
- **`range` runs from the `invariant`/`transition` keyword to the end of the
  predicate**; the leading documentation block is not in it. **`doc` is that
  documentation's text with trailing whitespace trimmed**, when there is any, and
  is omitted, not `null`, when absent.
- **Omitted when empty.** An agent that declares neither kind has no
  `invariants` key, so an older client ignores the field and a newer one treats
  its absence as "none".
- **A split agent has nothing to merge.** An agent is one declaration in one file
  (a second is `bynk.resolve.duplicate_agent`); what is split across files is the
  context. The list is that declaration's own, and a request for one agent never
  lists another's, which a test over a two-file context pins.

**Consequences.** Responses grow by a few short strings per agent. Both kinds are
listed whether or not the overlay can evaluate them; `violations` and
`not_evaluated` are unchanged. The predicate's text is the printer's, so a printer
change is a one-time diff for a client comparing across versions of the tool.
