# 0858 — The server's diagram models are complete; the client decides how much to show

- **Status:** Proposed (v0.626.2)

**Context.** `bynk-ide::sequence` stops classifying at `MAX_BLOCK_DEPTH = 2`
and emits a single `Collapsed` marker beyond it. That is a presentation
decision made in the model, because Mermaid could not fold a fragment. With a
renderer that can fold (the `model-views-first-party-uml-renderer` ADR), the
cut only hides information a reader could otherwise expand.

**Decision.** The server's diagram models (`bynk/sequenceModel`,
`bynk/architectureModel`, `bynk/agentStateModel`) describe the program
**completely**, up to a generous safety cap that only protects against
pathological input. Each nested element carries its `depth`. *How much to show*
(the default fold depth, which members are expanded) is the client's decision,
and is kept as page state. Model growth stays **facts about the program**:
nesting, locations, types as display strings, and triggers. It never includes
positions, colours or fold state. Every addition is additive on the wire, so
older extensions keep working against newer servers.

**Consequences.**

- `AltKind::Collapsed` narrows to bodies that genuinely cannot be analysed.
- Payloads grow for deeply nested handlers. The safety cap bounds this, and the
  `model-diagrams` track's Q4 measures it.
- The rule gives later model deltas (`break` fragments, callee identity, actors
  and ingress on the architecture model) a test to pass: *is this a fact about
  the program, or a choice about the picture?*
