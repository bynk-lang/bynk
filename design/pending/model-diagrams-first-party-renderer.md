---
level: patch
changelog: Design: a plan to move the VS Code extension's diagram views from Mermaid to a first-party renderer in UML notation (model-diagrams track, settling)
---

## ADR: model-views-first-party-uml-renderer
title: The extension's model views render through a first-party diagram engine in UML notation, not Mermaid
status: Proposed
summary: Model → diagram IR → layout (own grid for sequence, ELK for structure) → SVG, in the webview, in UML 2.5 with a Bynk profile

**Context.** The VS Code extension draws the sequence diagram (#846), the
architecture map (#851) and the agent state view (#2196) by generating Mermaid
text from the server's structured models. Mermaid then lays the diagram out and
draws it, and the extension reverse-engineers the SVG for click-to-code:

- a DOM-order zip for messages, notes and branch headers;
- a coordinate sort for participants (`participant-map.ts`);
- Mermaid's internal `flowchart-<id>-<n>`, `state-<id>-<n>` and `edge<n>`
  naming for the other views.

A Mermaid upgrade can silently break any of these. The model is bent to
Mermaid's text syntax (label escaping, placeholder notes, ADR 0274's
empty-`alt` workaround). Theme colours are copied in at render time rather than
linked. Each of the three views bundles its own copy of Mermaid, about 5.3 MB
each and about 16 MB in total. Mermaid also has no component diagram, which is
the next view the architecture-first story needs. The `model-diagrams` track
(`design/tracks/model-diagrams.md`) weighs four options:

- staying on Mermaid;
- adopting Eclipse Sprotty;
- PlantUML;
- a first-party engine.

**Decision.** The extension renders its model views through a **first-party
diagram engine** in `vscode-bynk`. The pipeline is:

1. Each view adapter turns its LSP model into one shared **diagram IR**. The
   IR is identity-carrying, has no coordinates, and gives every element a
   `ref` with its source location.
2. The IR is **laid out** by one of two strategies:
   - our own grid layout for sequence diagrams;
   - **ELK.js** (`layered`, with ports and nesting) for structure diagrams.
3. It is **drawn** as SVG through DOM APIs, never through `innerHTML` of
   source-derived text. Styling is CSS over `var(--vscode-*)`, and each
   element carries `data-bynk-ref`.

One shared interaction layer provides reveal, hover, focus, folding, keyboard
navigation and export. Layout runs **in the webview**, so the server's models
stay presentation-free, as LSP spec §3.23 established. The notation is **UML 2.5
with a Bynk profile**, whose stereotypes are «context», «adapter», «service»,
«agent», «capability», «platform» and «commons».

Views migrate one at a time. Until the agent state view is ported, the views
that remain on Mermaid share one Mermaid bundle. After that, Mermaid survives
only as a *copy-as-Mermaid* exporter with no runtime dependency. The Book's
hand-written Mermaid diagrams (`site/`, `rehype-mermaid`) are out of scope and
unchanged.

**Consequences.**

- Click-to-code becomes a lookup by identity. `participant-map.ts`,
  `state-map.ts` and the DOM-order zips are deleted as their views migrate.
- The Mermaid-rendering consequences of ADRs 0258, 0261, 0274, 0275, 0302
  and 0536 are superseded. Their model decisions stand.
- `check-bundle.mjs` gains a diagram-bundle budget.
- New behaviour is possible because the engine owns the drawing: foldable
  fragments, sticky lifelines, two-way cursor sync, `ref` drill-down, and a
  component view.
- We take on the maintenance of drawing code for every diagram kind. Graph
  layout itself is not ours: it is delegated to ELK, and writing our own is an
  explicit non-goal.
- A Sprotty spike during settling may still move the *structure* views to
  Sprotty. That would not change this decision for the sequence view.

## ADR: diagram-models-are-complete
title: The server's diagram models are complete; the client decides how much to show
status: Proposed
summary: Retire `MAX_BLOCK_DEPTH` as a presentation cut — ship the full block tree with depth, fold in the client

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
