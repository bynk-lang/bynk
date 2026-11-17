# Model diagrams — a first-party UML renderer for the sequence, architecture and component views

- **Status:** All six slices shipped (§12); retirement pending. The doc
  merged in #3587 (v0.626.2) with its two front-loaded ADRs,
  [0857](../decisions/0857-model-views-first-party-uml-renderer.md) and
  [0858](../decisions/0858-diagram-models-are-complete.md). Both are still
  **Proposed**: the work answered most settling questions (§13) by building,
  and accepting the ADRs is the maintainer's call. Every model view now draws
  through the first-party engine, and `mermaid` is no longer a dependency of
  `vscode-bynk`. What stands between this and retirement is listed in §16.
  The spine issue was never opened; the retirement PR can close the theme
  without one, or open and close it for the record.
- **Realises:** `design/bynk-lsp-spec.md` §3.23 (sequence diagram, #846) and
  §3.25 (architecture map, #851), and the "architecture in the language" claim
  in `design/bynk-design-notes.md`. If the architecture is written in the
  language, the tooling should be able to *show* it faithfully, in a notation
  readers already know.
- **Posture:** Feature track per
  [ADR 0076](../decisions/0076-feature-track-posture.md). It qualifies on two
  axes (§1.2).
- **Governing records:**
  - [ADR 0258](../decisions/0258-sequence-diagram-lifeline-classifier.md):
    what is a lifeline. The classifier stands; its Mermaid rendering is
    superseded (§10).
  - [ADR 0261](../decisions/0261-sequence-diagram-nested-block-branch-tracking.md),
    [ADR 0274](../decisions/0274-sequence-diagram-branch-outcomes.md) and
    [ADR 0275](../decisions/0275-sequence-diagram-principal-actor.md): the
    sequence model's blocks, branch outcomes and principal actor. The *model*
    decisions stand; their Mermaid-specific consequences are superseded (§10).
  - [ADR 0302](../decisions/0302-architecture-map-node-granularity.md): the
    architecture map's context-level nodes with expansion on demand. The
    granularity stands (§7.3); its Mermaid mechanics are superseded (§10).
  - [ADR 0536](../decisions/0536-agent-state-view.md): the agent state view
    and its mapping back to source without Mermaid `click`.
  - [ADR 0269](../decisions/0269-documentation-view-webview-substrate-and-links.md):
    the shared webview substrate and its `default-src 'none'` CSP.
  - [ADR 0504](../decisions/0504-live-model-views.md): live model views. These
    refresh on edit, follow the cursor and survive a reload.

## 1. The theme

### 1.1 What this track settles

The VS Code extension draws three diagrams from structured models the language
server sends: the **sequence diagram** of a handler, the **architecture map** of
a project, and the **agent state view**. All three turn the model into
**Mermaid** text, let Mermaid lay it out and draw it, and then reverse-engineer
the resulting SVG to make it clickable.

This track replaces that with a **first-party diagram engine** in the
extension. The engine is driven by the same models, draws in **UML 2.5 notation
with a Bynk profile**, and treats click-to-code, theming and live updates as
part of its design rather than workarounds. It then uses the engine to do three
things:

1. **Make the sequence diagram substantially better** (§6). It is the
   most-used view, and the one Mermaid constrains most.
2. **Overhaul the architecture map** (§7) into a proper system view with the
   outside world on it.
3. **Add a component diagram** (§8): one context opened up, with its parts,
   provided and required interfaces, and how they are wired. Mermaid has no
   such diagram.

When the track retires, the extension no longer bundles Mermaid. The Book keeps
Mermaid for its hand-written conceptual diagrams, which is what Mermaid is good
at (§10).

### 1.2 Why a track (the ADR 0076 trigger)

- **Multi-increment: yes.** The engine core, three views, and server-side model
  growth for each view cannot ship as one increment. Each slice (§12) is useful
  on its own.
- **Surface not yet settled: yes.** The notation (§5), the model deltas (§6.3,
  §7.3, §8.3), where layout runs, and build-versus-adopt (§11) are all open.
- **Security or safety boundary: marginal.** The webviews render text taken
  from user source (labels, names) into a page under a strict CSP. The engine
  must never produce markup from that text (§4.5). That is a constraint on the
  design, not a reason for the track.

## 2. What's true today

These are measured facts, not impressions.

- **Click-to-code is reverse-engineered from Mermaid's output.** Mermaid's
  `sequenceDiagram` cannot carry our ids, so `vscode-bynk/src/webview/main.ts`
  matches messages and notes to the model by counting rendered elements in
  emission order. Branch headers are found by matching either of two CSS class
  names, because Mermaid renamed one between versions.
  `src/webview/participant-map.ts` recovers one element per participant by
  sorting label nodes by their `x` coordinate. The 30-line comment above it
  documents Mermaid 11 emitting several `.actor` nodes per participant, grouped
  and in reverse order. The state view (`state-map.ts`) depends on Mermaid's
  internal `state-<id>-<n>` ids and `data-id="edge<n>"` numbering. A Mermaid
  upgrade that changes any of this breaks clicks silently. Tests exist
  specifically to notice that.
- **The model is bent to fit Mermaid's text syntax.** `mermaid-gen.ts` escapes
  `:` and `;`, writes the word `reply` when a return has no label, and inserts
  `…` placeholder notes so an empty branch does not collapse. ADR 0274 exists
  because Mermaid draws an empty `alt` as a zero-width box with its labels
  wrapped one character per line.
- **Theming is copied in, not linked.** Mermaid bakes colours into the SVG. Each
  view therefore reads `--vscode-*` custom properties into Mermaid
  `themeVariables`. The architecture map also converts them to hex
  (`classdef-color.ts`), because the commas in `rgba(...)` break a `classDef`.
  The sequence and state views read the theme once, at first render, so a
  theme change made while a panel is open is likely to be missed (unverified).
- **Render ids must be unique per render** (`renderSeq`). Otherwise Mermaid
  finds the previous SVG in the live DOM.
- **Bundle weight.** A local build of `vscode-bynk` at 0.614.1 gives
  `out/webview.js` (sequence), `out/architecture.js` and `out/state.js` at
  **≈5.3 MB each, ≈16 MB together** (≈1.5 MB each gzipped). Each is an IIFE
  carrying its own full copy of Mermaid. By comparison, `out/docview.js` is
  0.1 MB.
- **Interaction stops at "click to jump to source".** There is no hover detail,
  no folding of a single fragment, no keyboard traversal, and no
  screen-reader structure. The sequence view cannot highlight the handler
  under the cursor, because each update replaces the whole SVG.
- **The sequence model is pre-shaped for the renderer.** `bynk-ide`'s
  `sequence.rs` collapses nesting deeper than `MAX_BLOCK_DEPTH = 2` into a
  single `Collapsed` marker on the *server*. That is a presentation decision
  made in the model, because Mermaid offers no folding.

What is right, and stays, is the split recorded in LSP spec §3.23. The server
sends structured, presentation-free models (`bynk/sequenceModel`,
`bynk/architectureModel`, `bynk/agentStateModel`), and the extension owns
presentation. This track keeps that boundary and moves presentation onto
something we control.

## 3. Goals and non-goals

**Goals**

- **G1. Identity by construction.** Every drawn element carries its model
  identity (`data-bynk-ref`). Click, hover and focus are lookups, never zips or
  coordinate heuristics.
- **G2. Live theming.** Styling is CSS over `var(--vscode-*)`, so light, dark,
  high-contrast and custom themes, and theme switches, need no code.
- **G3. Stable under live update.** A re-render of a changed model keeps
  scroll position, zoom, folds and selection. Unchanged elements keep their
  positions where the layout allows.
- **G4. Notation readers already know:** UML 2.5, with a small Bynk profile
  (§5).
- **G5. Small.** The engine plus the views come to well under 1 MB minified,
  ELK included. The extension's diagram webviews share one bundle or one
  vendor chunk.
- **G6. Testable without a browser.** Model → diagram IR → layout are pure
  functions with snapshot tests. Drawing is exercised against jsdom, which is
  already a devDependency.
- **G7. Accessible.** Every diagram supports keyboard traversal and exposes an
  ARIA structure, so a screen reader can navigate the diagram rather than only
  reading its text.

**Non-goals**

- **A text language for diagrams.** We are not writing a Mermaid or PlantUML
  competitor. Diagrams are derived from Bynk source. If the Book later wants
  hand-authored UML, the IR (§4.2) can grow a syntax then.
- **Diagram editing.** The views are read-only projections of the source. The
  source is the single place you edit.
- **Our own graph layout algorithms** (§11, D2).
- **Replacing the Book's Mermaid.** See §10.

## 4. The engine

### 4.1 Pipeline

```text
LSP model ──► view adapter ──► diagram IR ──► layout ──► drawing (SVG) ──► interaction
(server,       (one per view;   (shared;      (grid for   (DOM APIs, CSS    (shared: reveal,
 presentation   pure)            pure)         sequence;   classes, data-    hover, focus,
 free)                                         ELK for     bynk-ref; no      fold, keyboard,
                                               structure)  innerHTML)        cursor sync)
```

It lives in `vscode-bynk` under `src/diagram/`: `ir.ts`, `layout/`, `draw/`,
`interact/` and `notation/`. The views (`src/webview/sequence.ts`,
`architecture.ts`, `component.ts`) become thin adapters. `live.ts`'s render
queue (ADR 0504) is unchanged. The engine plugs into it as the `render`
callback.

### 4.2 The diagram IR

One intermediate representation serves every view. A view adapter produces it,
and layout and drawing consume it. Its parts:

- **Nodes:** classifier boxes with a stereotype, a name and optional
  compartments. Nodes may nest, which gives composite structure and
  deployment-style grouping.
- **Ports** on node edges, each with an optional provided interface (ball) or
  required interface (socket).
- **Edges:** dependency, assembly connector, delegation connector,
  association, and messages. Each has an arrowhead kind, an optional label, and
  a source and target, either node or port.
- **Lifelines, messages and combined fragments** for sequence diagrams. A
  fragment has an operator (`alt`, `opt`, `break`, `ref`, `par`, `loop`) and
  operands, each with a guard and contents.
- **Notes and annotations.**

Every IR element carries a `ref`: an opaque id, its source location (`uri` and
`range`), and a fold state when it is foldable. The IR holds no coordinates.
Layout returns a separate positioned scene keyed by the same ids, so a layout
change never touches model identity.

### 4.3 Layout

There are two strategies, chosen by diagram kind:

- **Sequence diagrams: our own grid layout.** Lifelines are columns and events
  are rows. Fragments are rectangles spanning the columns their contents touch,
  inset by nesting depth. This layout is simple, deterministic and stable under
  edit: adding a message shifts the rows below it, and nothing else moves.
- **Structure diagrams (architecture and component): ELK.js** (Eclipse Layout
  Kernel) with the `layered` algorithm. It provides hierarchical (nested)
  nodes, **ports with side constraints**, orthogonal edge routing, and
  edge-label placement. Ports are what ball-and-socket notation needs. Dagre,
  which Mermaid uses, has no ports. ELK takes a JSON graph in and returns
  positions, so it sits cleanly behind the IR.

**Text measurement** is behind an interface. In the webview it uses a hidden
canvas's `measureText` with the editor font. A fixed-metric implementation
exists for tests (and later for headless rendering, §10). Layout takes the
measurer as a parameter, so it stays pure.

### 4.4 Stability under live update

ADR 0504's live views re-render on every edit. To meet G3:

- Folds, the selection, zoom and the scroll anchor are page state keyed by
  `ref` id. They survive a re-render, and an id that no longer exists simply
  matches nothing (the same rule the architecture map's `expanded` set uses
  today).
- The sequence grid is incremental by construction (§4.3).
- For ELK views, the previous layout's node order is passed back as a
  constraint (ELK's interactive and `considerModelOrder` options), so an
  unrelated edit does not reshuffle the picture. Whether that is enough is
  question Q3 (§13).
- A changed element can be briefly highlighted, so the reader sees what the
  edit did. This is opt-in and respects `prefers-reduced-motion`.

### 4.5 Drawing, CSP and safety

- SVG is built with `createElementNS` and `textContent`. **Source-derived text
  never goes through `innerHTML`.** That removes the class of escaping bug that
  `escapeLabel` patches over today.
- Styling is a stylesheet under the existing nonce CSP: classes such as
  `.bynk-lifeline` and `.bynk-msg--async` over `var(--vscode-*)`. Nothing is
  baked in. High-contrast themes get `--vscode-contrastBorder` outlines for
  free.
- ELK runs on the page's main thread through `elk.bundled.js`, so the CSP
  needs no `worker-src`. Moving it to a worker later is a CSP change that needs
  its own review.

### 4.6 Interaction layer (shared)

The same behaviours apply in every view:

- **Reveal.** Click an element to post the `reveal` message the host already
  handles.
- **Hover.** A card shows details the drawing omits: types, effects, the
  `given` set, or a route.
- **Focus from the editor.** The host already reports the cursor's handler
  (ADR 0536's `focus`). The engine highlights the matching element *and*
  reveals it on scroll. For the sequence view, the cursor's statement
  highlights its message (§6.2).
- **Focus to the editor.** Hovering an element can optionally decorate its
  source range in the editor through a new `highlight` message.
- **Fold and unfold** for fragments, composite nodes and nested contexts.
- **Pan and zoom,** zoom-to-fit, and a minimap for large diagrams.
- **Keyboard.** Arrow keys walk the structure (next message, parent fragment,
  neighbouring node). Enter reveals and Space folds. Every focusable element
  has an ARIA label built from the same data as its hover card.
- **Export.** Save as SVG, copy as PNG, and **copy as Mermaid**: the current
  `mermaid-gen.ts` and `architecture-gen.ts` are kept as *exporters* with no
  runtime dependency on Mermaid, so a diagram can still be pasted into a pull
  request or a README.

### 4.7 Testing

- Adapters and layout get snapshot tests of IR and of positioned scenes for
  fixture models. The existing `mermaid-gen.unit.test.ts` fixtures port
  directly.
- Drawing gets jsdom tests: every IR element with a `ref` produces exactly one
  element with that `data-bynk-ref`. This replaces the DOM-shape pinning that
  `participant-map.ts` and `state-map.ts` need today.
- Visual regression uses Playwright screenshots of the fixture gallery in light,
  dark and high-contrast themes.
- `scripts/check-bundle.mjs` gains a budget for the diagram bundle (G5).

## 5. Notation: UML 2.5 with a Bynk profile

Bynk's architecture maps onto UML closely enough that a **profile** (UML's own
mechanism for domain stereotypes) is enough. Nothing needs inventing.

| Bynk | UML element | Stereotype | Shown as |
|---|---|---|---|
| `context` | Component | «context» | Component box (with the component icon) |
| adapter | Component | «adapter» | Component box, dashed border |
| `commons` | Package | «commons» | Package tab |
| `service` | Part (property) of a context | «service» | Box inside the context |
| `agent` | Part, multiplicity `*` (keyed) | «agent» | Box inside the context, `key` shown as a qualifier, e.g. `Book[channel]` |
| handler (`on call` or HTTP route) | Operation; port on the service | n/a | Operation in a compartment; HTTP routes on an «http» port |
| `capability` | Interface | «capability» | Socket where it is required (`given`); ball where it is provided |
| `provides` | Interface realisation | n/a | Ball on the provider, with an assembly connector to the socket |
| `consumes` | Usage dependency or assembly | «consumes» | Required interface on the consumer, provided interface on the consumed context's service |
| `by <Actor>` | Actor | «actor» (stereotype dropped when it adds nothing) | Stick figure, outside the system boundary |
| Platform built-ins (`bynk`) | Component | «platform» | Greyed box at the edge |
| A Worker or Durable Object (workers target) | Node or artefact | «worker», «durable object» | Optional deployment overlay (§7.2) |

Sequence-diagram notation follows UML exactly:

| Message | Arrow |
|---|---|
| Synchronous call (awaited `<-`) | Solid line, filled arrowhead |
| Reply | Dashed line, open arrowhead |
| Asynchronous send to an agent | Solid line, open arrowhead |
| Found message (a trigger with no modelled sender: a schedule or an event) | Starts from a filled dot |

An execution specification (activation bar) is drawn on each lifeline while a
call is outstanding. A state write is a **state invariant or action on the
agent's lifeline**, not a free-floating note.

## 6. The sequence diagram

This is the most-used view and the one with the most to gain.

### 6.1 What the reader gets

1. **Proper UML arrows and activations** (§5): a reader can tell an awaited
   call from a fire-and-forget send at a glance.
2. **Lifeline headers that say what each lifeline is:** a stereotype, a name,
   and for an agent its key expression (`«agent» Book[channel]`). Hovering
   shows the capability's or context's declared surface.
3. **Sticky lifeline headers.** On a long handler the headers stay pinned while
   you scroll, which is the single most common complaint about tall sequence
   diagrams.
4. **Foldable fragments, folded by default past depth 2.** This replaces the
   server's hard `Collapsed` cut (§6.3). A folded fragment shows its operator,
   guard and a message count, and expands in place.
5. **Guards in UML form** (`[view.allowed]`). Each operand's outcome is drawn as
   the reply message it actually is, rather than as a note.
6. **`break` fragments for early exits.** A `?` that can propagate an `Err`
   becomes a `break` whose operand is the error path. Today the error path is
   invisible.
7. **`ref` fragments for boundary calls, with drill-down.** A cross-context or
   agent call is boundary-stop today (ADR 0258), one Call and one Return.
   Instead it renders as a `ref` naming the callee handler. Clicking it either
   opens that handler's own diagram, or **expands it inline** by requesting the
   callee's `sequenceModel` and nesting it. This turns "who does this talk to"
   into "what actually happens".
8. **Found messages for triggers.** A handler with no principal (`on schedule`,
   an event subscriber, an agent call from elsewhere) starts from a found
   message labelled with its trigger. That replaces the combined entry label.
9. **Two-way cursor sync.** Moving the editor cursor onto a statement
   highlights its message, and selecting a message reveals its statement. This
   uses the message `range`s the model already carries.
10. **Hover detail.** For a message: argument and result types, and the effect
    it performs. For a state write: the field and its new value.
11. **Copy as Mermaid** for sharing (§4.6).

### 6.2 Interaction specifics

- **Cursor-to-message lookup:** the smallest message range containing the
  cursor, falling back to the enclosing fragment.
- **Fold state** is keyed by block id plus branch, kept across live updates
  (G3) and dropped on retarget.
- **Inline `ref` expansion** is lazy (one extra request per expansion) and
  cycle-guarded: a handler already open in the chain renders as a plain `ref`.

### 6.3 Model deltas (`bynk-ide::sequence`, `bynk/sequenceModel`)

All of these are additive. The wire shape grows, and older extensions ignore
the new fields. Every new field below is optional or defaults, so the change
stays compatible with `bynkServerMinVersion`.

| Delta | Why | Notes |
|---|---|---|
| Ship the **full block tree**; record `depth` on each block instead of collapsing at `MAX_BLOCK_DEPTH` | Folding moves to the client (6.1.4) | `Collapsed` remains for genuinely unanalysable bodies. Needs a size guard for pathological handlers (a cap far above 2, for example 16) |
| **`break` blocks** for `?` sites that can propagate `Err`, with the error operand | 6.1.6 | `ExprKind::Question` is already walked. It also needs the propagated error type for the label |
| **Callee identity** on boundary messages: the target handler's location | 6.1.7 | Lets the client request the callee's `sequenceModel` without guessing. **Follow-up for packaging slice 6:** callees are located among the project's own units only, since a package never shares a context (ADR 0542). Once behavioural library units let a dependency's agent be mixed into a consumer's context, `locate_callee_in` (`bynk-lsp`) must also search that package's files for the agent's handler, or its `location` stays `null` |
| A **trigger** on the model: `principal` (exists), `schedule`, `event` or `call` | 6.1.8 | Drives the found message |
| **Message detail**: argument types, result type and effect, as display strings | 6.1.10 | Hover only. Strings, not types, so the wire stays presentation-free but self-describing. **Deferred from slice 3:** the walk is syntactic and has no types |
| **Agent key expression** on agent participants | 6.1.2 | |

Already landed, outside this track: [ADR 0853](../decisions/0853-sequence-branch-span.md)
(v0.625.2) gives each branch its arm's source span (`WireBranch.range`).
Slice 1 links a guard to its whole block, because a branch had no span of its
own. With 0853, a guard (and a folded operand, 6.1.4) can link to its own arm.

Loops and parallel composition (`loop`, `par`) enter the model only when the
language has a construct that warrants them. The IR supports them from day one.

## 7. The architecture map, overhauled

### 7.1 What is wrong with it now

It is a Mermaid `flowchart LR`. Contexts are boxes and `consumes` edges are
arrows. Expanding a context hangs its members off it as a fan of loose nodes
joined by `---` lines, through a synthetic `+`/`−` node (ADR 0302). It has no
notion of the outside world (who calls the system), no capability wiring
between contexts, no grouping, and nested content is not drawn *inside* its
context. Dagre's layout also reshuffles freely between renders.

### 7.2 What it becomes: the system view

- Contexts and adapters are **«context» and «adapter» components** (§5), with
  members drawn **inside** the component when it is expanded (true nesting,
  laid out by ELK). Expanding is in place, by a fold control on the box, with
  no synthetic node.
- **The outside world is drawn around a system boundary:**
  - actors from `by` clauses, connected to the contexts whose handlers they
    call;
  - HTTP ingress, shown as an «http» port on each context that has `from http`
    services;
  - schedules and event sources.
- **`consumes` edges are labelled with what is consumed** (the braced
  selection, or the whole unit). Hovering an edge lists the handlers that
  actually call across it.
- **Capabilities are drawn as interfaces at the boundary** when expanded:
  required (`given`) sockets wired to the providing adapter or platform
  component.
- **Layering and grouping.** Optional grouping by package or Worker group
  (`[deploy.workers]`). A `consumes` cycle is a checker error
  (`phase_detect_consumes_cycles`). While one exists, the map highlights it,
  so the diagram shows the error rather than hiding it.
- **Optional deployment overlay** (workers target): Worker boundaries, agents
  as Durable Objects, Service Bindings on cross-context edges, and bound D1
  and KV resources. This is a toggle, not a separate diagram.
- **Search and filter**, plus a **path highlight**: select two contexts to
  highlight every dependency path between them.
- **Semantic zoom into the component view** (§8): double-click a context
  (or use its *Open component view* action) to open its component diagram.

### 7.3 Model deltas (`bynk-ide::architecture`, `bynk/architectureModel`)

All additive:

- **Actors and ingress:** for each context, the actors its handlers name, and
  whether it has HTTP, schedule or event entry points.
- **Per-edge detail:** the handlers that use each `consumes` edge (for hover)
  and their locations.
- **Capability wiring:** for each consumed capability, which provider or
  adapter satisfies it. The model's `providers` and `capabilities` lists carry
  most of this already. What is missing is the link between them.
- **Grouping and deployment** (optional, behind the overlay): the Worker group,
  the agents' Durable Object classes, and declared bindings, taken from the
  same data as `bynk deploy --dry-run`'s plan.
- **Cycles:** the members of any `consumes` cycle the checker reports.

Already landed, outside this track: [ADR 0854](../decisions/0854-architecture-model-events-uses.md)
(v0.625.3) adds each event's emitters and subscribers, and each handler's
capability use (`uses`, with `stubbedIn`), to `bynk/architectureModel`. That
covers the event entry points and sources above, and the `given` half of
capability wiring. What still remains is the link from a capability to the
provider or adapter that satisfies it.

ADR 0302's decision (context-level nodes by default, expansion on demand)
**stands**. Only its Mermaid mechanics (the synthetic toggle node, id lookups
of the form `flowchart-<id>-<n>`) are superseded.

## 8. The component diagram (new)

### 8.1 What it shows

It shows **one context opened up**, as a UML component diagram with composite
structure.

**Inside the context:**

- Its services and agents as parts. Each service lists its handlers as
  operations (HTTP routes on an «http» port, `on call` handlers on the
  service's provided interface). Agents show their key and, collapsed, their
  state fields.

**On the context's boundary:**

- **Provided interfaces (balls):** the services other contexts may consume, and
  the HTTP surface.
- **Required interfaces (sockets):** every capability in a handler's `given`,
  and every consumed service of another context.
- **Delegation connectors** from each boundary port to the parts that
  realise or use it. That answers "which service needs `Logger`?" at a glance.

**Outside the context**, in grey and one level deep:

- The components that satisfy its required interfaces (adapters, platform,
  other contexts) and the components that consume what it provides, joined by
  **assembly connectors** (ball-in-socket).

Every element links to source. A handler links to its sequence diagram
(§9).

### 8.2 Scope

The view is per context and follows the cursor's context (`followScope:
"owner"` per ADR 0536, at context level). It can be pinned. A multi-context
selection (from the architecture map) shows several contexts, each opened,
wired to one another.

### 8.3 Model

**[DECISION D4]** (§11): this is a projection of an enriched
`bynk/architectureModel`, not a new request. ADR 0854 took the same line for
events and capability use ("extend `bynk/architectureModel` rather than add a
request"), so D4 now has a precedent. Its additional content:

- each handler's `given` set, resolved to capability identity (**landed**:
  ADR 0854's per-handler `uses`);
- each handler's outbound `consumes` calls;
- the context's provided surface (consumable services and HTTP routes);
- agent keys and state field names.

All of this is per-context data the checker already has. The per-handler
`given` and calls are what `agent_effects.rs` and `sequence.rs` already
compute, so the same project-analysis tables can be reused (as #846 did with
`ProjectAnalysis.sequence_info`).

## 9. Cross-view navigation

The three views form one connected set, at three levels of detail:

| From | Gesture | Opens |
|---|---|---|
| Architecture map | Open a context | Component view of that context |
| Component view | Activate a handler (operation) | Sequence diagram of that handler |
| Sequence diagram | Activate a lifeline header | Component view of the lifeline's context, or the architecture map for a capability |
| Sequence diagram | Activate a `ref` | The callee's sequence diagram, or inline expansion |
| Any view | Activate an element | Its source |

These are host commands (`bynk.showComponent`, `bynk.showSequence` and the
like) with locations as arguments, so the code lenses and the views share one
entry path.

## 10. Migration, coexistence and the Book

- **During the track,** views not yet ported stay on Mermaid. Slice 0 (§12)
  makes that cheaper by sharing one Mermaid bundle across them.
- **The agent state view** is ported last (slice 6). Its states and transitions
  map onto the ELK path (UML state machine notation, plus the dashed `?` state
  and unreachable-state dimming that ADR 0536 specifies). When it lands,
  Mermaid leaves `vscode-bynk`'s dependencies and the copy-as-Mermaid exporters
  are the only code that remains of it.
- **The Book** (`site/`, `rehype-mermaid`) is unaffected. Its diagrams are
  hand-written and conceptual, which is Mermaid's sweet spot. A later,
  separate increment *may* use the engine headlessly, with the fixed-metric
  measurer from §4.3, to put generated diagrams of real handlers into
  `bynk doc` output or the Book. That is out of this track's scope (Q6).
- **The superseded consequences** are the Mermaid-rendering parts of six ADRs:
  0258, 0261, 0274, 0275, 0302 and 0536. That covers rendering as Mermaid
  text, the DOM-order zip, `noteOrder`, the `actor` keyword, the participant
  `x` sort, the `flowchart-<id>` lookup, `state-<id>-<n>` and `edge<n>`. Their
  *model* decisions are untouched. LSP spec §3.23 and §3.25 are updated in place, by
  the slices that change them.

## 11. Decisions

Each fork is marked `[DECISION]`, with a recommendation.

- **[DECISION D1] Build, adopt or stay.**
  - **(a) Stay on Mermaid** and harden it. Cheapest, but it fixes none of §2's
    structural problems and cannot draw §8 at all.
  - **(b) Adopt Eclipse Sprotty** (TypeScript, ELK-based, with VS Code webview
    integration). It provides selection, expand and hover. But it is built for
    graphs, so the sequence view, which is the main beneficiary, would still
    be ours. It also brings InversifyJS dependency injection and its own
    architecture.
  - **(c) PlantUML.** It covers the UML set, but needs a Java runtime or a heavy
    experimental WebAssembly build. Its text-in, SVG-out model repeats the
    clickability problem, and it is hard to theme.
  - **(d) A first-party engine over ELK.**
  - **Recommend (d).** Spend a 2–3 day **Sprotty spike against the component
    view** during settling. If it makes structure views markedly cheaper,
    reconsider (b) for §7 and §8 only.
- **[DECISION D2] Structure layout.** ELK `layered` (ports, nesting, orthogonal
  routing), dagre (small, but no ports or real nesting), or our own.
  **Recommend ELK.** Writing our own layout is explicitly a non-goal.
- **[DECISION D3] Where layout runs.** In the webview (TypeScript, real text
  metrics) or on the server (Rust in `bynk-ide`, shared with the CLI, but with
  guessed text metrics and presentation leaking into the models).
  **Recommend the webview.** This keeps the boundary in LSP spec §3.23.
- **[DECISION D4] Component model.** Enrich `bynk/architectureModel` and
  project from it, or add a new `bynk/componentModel`. **Recommend enriching.**
  It is one project-scoped analysis and one cache, and the two views must agree
  by construction. Per-context payload size is the counter-argument (Q4).
- **[DECISION D5] Nesting depth.** The server ships the full tree and the client
  folds, or the server keeps cutting. **Recommend the full tree,** with a
  generous safety cap.
- **[DECISION D6] Notation.** Strict UML 2.5 plus the Bynk profile (§5), or a
  bespoke notation. **Recommend UML plus the profile.** Readers already know
  it, and stereotypes are UML's own extension mechanism.
- **[DECISION D7] Drawing technology.** Hand-built SVG through the DOM, or a UI
  framework (Preact, Lit). **Recommend hand-built.** The drawing layer is
  small, the IR already isolates it, and it keeps the bundle and the CSP
  simple.
- **[DECISION D8] Engine home.** `vscode-bynk/src/diagram/` for now, or a
  separate package from the start. **Recommend in-tree.** Extract it only when
  a second consumer (Q6) is real.

## 12. Candidate slice decomposition

| # | Slice | Repos | Depends on | Status |
|---|---|---|---|---|
| 0 | **Mermaid hygiene.** One shared Mermaid bundle for the three views, and re-theme on theme change. Standalone and useful even if the track stalls | vscode-bynk | n/a | **Shipped** (vscode-bynk #170). Compare Model's diagrams, which landed meanwhile, shared the bundle too. Superseded by slice 6, which removed Mermaid and shares ELK the same way |
| 1 | **Engine core plus sequence parity.** IR, grid layout, drawing, the interaction basics (reveal, hover, keyboard), notation from §5, and copy as Mermaid. No server change. Deletes `participant-map.ts` and the zip | vscode-bynk | n/a | **Shipped** (vscode-bynk #170) |
| 2 | **Sequence interaction.** Sticky headers, folding, two-way cursor sync, pan and zoom, minimap, export | vscode-bynk | 1 | **Shipped** (vscode-bynk #174) |
| 3 | **Sequence model deltas** (§6.3): full tree, `break`, triggers and found messages, callee identity with `ref` drill-down, hover detail | bynk (ide, lsp), vscode-bynk | 1 | **Shipped** (bynk #3651, vscode-bynk #175): the full tree with depths, `break` for `?`, triggers as found messages, agent keys, and located callees with `ref` drill-down. **Deferred:** hover detail, which needs the checker's types; and locating a callee in a dependency package (§6.3's packaging follow-up) |
| 4 | **ELK path plus architecture overhaul** (§7) with its model deltas. The deployment overlay may split off as 4b | bynk, vscode-bynk | 1 | **Shipped** (bynk #3654, vscode-bynk #176): the ELK path; units as components in a system boundary, opened in place; `consumes` cycles marked; actors calling in, labelled with the protocol; platform wiring from each handler's `uses`; events. **Deferred:** per-edge caller detail on hover, grouping by package or Worker group, the deployment overlay (4b), search, filter and path highlight |
| 5 | **Component view** (§8): model enrichment, view, and cross-view navigation (§9) | bynk, vscode-bynk | 4 | **Shipped** (bynk #3657, vscode-bynk #177): each handler's `calls` and each agent's `key` and `state` on `architectureModel` (D4); the view, with parts, compartments, provided and required ports, and neighbours; navigation from the map, from an operation, and from the sequence diagram's **Components** button. **Deferred:** a sequence lifeline header opening its context's component view, events in the component view, and a multi-context selection from the map (§8.2) |
| 6 | **Agent state view port.** Mermaid leaves the extension, and the bundle budget is enforced | vscode-bynk | 4 | **Shipped** (vscode-bynk #178): the agent state machine in UML notation on the ELK path, and Compare Model's map and machines on the engine. Mermaid left the dependencies; ELK ships once (`out/elk.js`) and `check-bundle.mjs` enforces per-page, ELK, extension and total budgets. Desktop bundles went from 9.1 MiB to 2.6 MiB |

Slices 2 and 3 can run in parallel, as can 3 and 4.

## 13. Settling-review questions

- **Q1.** The Sprotty spike (D1). Does it change the recommendation for
  structure views? **Overtaken:** the spike was not run. Slices 4–6 built the
  structure views on the first-party engine over ELK (D1 (d), D2), and they
  needed none of what Sprotty would have added.
- **Q2.** The ELK bundle size in practice, with `elk.bundled.js` minified. Does
  G5 hold? If not, is a lazily loaded vendor chunk acceptable under the
  webview CSP? **Resolved in slices 4 and 6:** minified, ELK is about
  1.45 MiB (as against Mermaid's 5.1 MiB). It ships once, as a vendor script
  each ELK page loads under the existing nonce CSP, and every page stays under
  110 KiB. `check-bundle.mjs` enforces the budgets.
- **Q3.** Is ELK's model-order constraint enough for G3 on the architecture
  map, or do we need to persist node order across sessions? **Answered for
  now:** the views use `considerModelOrder` over the server's deterministic
  order, and no persisted order has been needed. Revisit if layout churn is
  reported on large projects.
- **Q4.** D4 at scale: what is the payload size of an enriched
  `architectureModel` on the largest example, and is it worth a per-context
  request? **Still open:** D4 shipped (slice 5), but the payload has not been
  measured at scale.
- **Q5.** The `break` rendering for `?`. Should every `?` site be a fragment,
  or only those whose `Err` actually reaches the handler's result (fewer, more
  honest fragments)? **Resolved in slice 3:** every `?` the walk sees. A `?`
  in a lambda returns from the lambda, and lambdas are not walked, so each one
  shown does reach the handler's result.
- **Q6.** Headless rendering for `bynk doc` and the Book. Is it in scope for a
  later slice of this track, or a separate track? **Out of this track:** no
  slice took it on, so it is a separate increment if wanted.
- **Q7.** Should the web extension (vscode.dev) get the same views? The engine
  is browser-only, so probably yes, at no extra cost. Confirm against
  `check-bundle.mjs --web` budgets. **Still open:** the views remain
  desktop-only (`!isWeb`), as before the track.

## 14. Front-loaded ADRs

These were stamped on the merge of this doc (#3587, v0.626.2) with status
**Proposed**, because Q1 can still change D1. The slice that resolves the
settling questions moves them to Accepted:

- [**ADR 0857**](../decisions/0857-model-views-first-party-uml-renderer.md): the
  model views render through a first-party engine in UML notation. This is D1,
  D3 and D6. It supersedes the Mermaid-mechanics consequences listed in §10.
  Slice 1 put the sequence view on it; slices 4–6 put the rest there.
- [**ADR 0858**](../decisions/0858-diagram-models-are-complete.md): the server's
  diagram models are complete, and the client decides how much to show. This
  is D5. It retires `MAX_BLOCK_DEPTH` as a presentation cut (slice 3).

D2, D4, D7 and D8 are implementation-level and are recorded by the slices that
make them: D2 (ELK) in slice 4, D4 (enrich `architectureModel`) in slice 5,
D7 (hand-built SVG) from slice 1, and D8 (in-tree, `vscode-bynk/src/diagram/`)
throughout.

Both ADRs are still **Proposed**. Every slice has shipped on them, and Q1 is
overtaken (§13), so nothing is left that could change D1. Moving them to
Accepted is the retirement PR's to do, once the maintainer agrees.

## 15. Risks

- **Scope creep into a general diagramming tool.** Mitigated by the non-goals
  (§3) and by every IR feature being justified by a view.
- **The long tail of layout edge cases:** long labels, very wide diagrams,
  deep nesting. Mitigated by the fixture gallery and visual regression tests
  (§4.7), seeded from real examples.
- **ELK quality on dense graphs.** Mitigated by folding and filtering (§7.2);
  the D1 fallback is Sprotty for structure views.
- **Two renderers during migration.** This is bounded: slice 0 shares the
  Mermaid cost, and slice 6 removes it.
- **Model growth leaks presentation into the server.** Guarded by the rule
  that every delta is a *fact about the program* (types, locations, nesting),
  never a position, colour or fold state.

## 16. Done when

Each condition, with where it stands after slice 6:

- The sequence, architecture and component views render through the engine,
  with every element linked to source by identity. **Met** (slices 1, 4
  and 5).
- The agent state view is ported, and `mermaid` is no longer a dependency of
  `vscode-bynk`. **Met** (slice 6).
- The diagram bundle is within its budget (G5), and the visual regression
  gallery passes in light, dark and high-contrast themes. **Half met:** the
  budgets hold and CI enforces them (slice 6). There is no visual regression
  gallery. The views are covered by pure layout tests and jsdom tests, and
  were checked by eye in Chromium, but no screenshots are compared in CI.
- LSP spec §3.23 and §3.25 (and a new section for the component view) describe
  the enriched models. **Met, with one change of plan:** both sections were
  updated in place by slices 3–5. The component view needs no section of its
  own, because D4 made it a projection of §3.25's `bynk/architectureModel`.
- The Book's tooling page (`docs/tooling/vscode-bynk.md`) documents the three
  views and their navigation. **Not met:** the page predates the track. It
  does not mention the component view or the navigation between views, and it
  still describes the Mermaid files slice 6 removed.

Left before the retirement PR: the Book's tooling page, a decision on the
visual regression gallery (build it, or drop the condition), and accepting
ADRs 0857 and 0858. The deferred items in §12 are follow-ups, not conditions.
