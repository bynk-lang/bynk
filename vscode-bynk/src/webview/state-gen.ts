// #2196: pure `StateMachine` -> Mermaid `stateDiagram-v2` text, and the
// handler x state table's cells. Kept free of the DOM, like `mermaid-gen.ts`
// and `architecture-gen.ts`, so it is unit-testable.
//
// The diagram (track §3.4, #2196 DECISION C):
//   - `[*] --> initial`, then one transition per (from, to) pair a handler can
//     produce, with the handlers that produce it joined on its label;
//   - a computed write (`Unknown`) goes to a `?` pseudo-state;
//   - an unchanged path is a self-loop, hidden unless `showUnchanged`;
//   - a state outside `reachable` is dimmed; a truncated handler is a note.
// States are addressed as `s<index>` (a state name could clash with Mermaid
// syntax), labelled with their real names.
//
// Click-to-code (DECISION D): no Mermaid `click` — in `stateDiagram-v2` it
// only takes a URL, on states only, opened in a new tab. Instead:
//   - states map by DOM id: Mermaid gives each the id `state-<id>-<n>`
//     (`stateDomId`), so `s3`'s node is found by its `state-s3-` prefix;
//   - transition labels map by `edgeOrder`: Mermaid names each transition
//     `edge<n>`, with `n` increasing in statement order, so sorting the label
//     elements by that number recovers this module's emission order whatever
//     order the layout put them in the DOM (`state-map.ts`).

import type { StateEdge, StateMachine } from "./stateTypes";

export interface StateMermaidOptions {
  /** Draw unchanged (refused) outcomes as self-loops. Off by default. */
  showUnchanged: boolean;
}

/** One rendered transition: every model edge it stands for. */
export interface TransitionGroup {
  from: string;
  /** A state name, or `"?"` for a computed write. */
  to: string;
  handlers: string[];
  edges: StateEdge[];
  /** #2197: the strongest `transition` violation on this transition, if any. */
  flag: "Known" | "Possible" | null;
  /** The violated `transition`s' names. */
  violated: string[];
}

export interface StateMermaidResult {
  text: string;
  /** Emission order of the transitions, `[*] --> initial` excluded. */
  edgeOrder: TransitionGroup[];
  /** Whether `[*] --> initial` was emitted (it is the first transition). */
  hasInitial: boolean;
  /** Mermaid state id (`s0`, …, `unknown`) -> state name (`?` for unknown). */
  stateIds: Map<string, string>;
}

export const UNKNOWN_ID = "unknown";

function stateId(i: number): string {
  return `s${i}`;
}

/** Mermaid's state description and transition labels split on `:` and `;`. */
function esc(label: string): string {
  return label.replace(/\s+/g, " ").replace(/[:;]/g, " ").trim();
}

export function toMermaidState(machine: StateMachine, opts: StateMermaidOptions): StateMermaidResult {
  const lines = ["stateDiagram-v2"];
  const stateIds = new Map<string, string>();
  const index = new Map(machine.states.map((s, i) => [s, i]));
  machine.states.forEach((s, i) => {
    stateIds.set(stateId(i), s);
    lines.push(`    state "${esc(s)}" as ${stateId(i)}`);
  });

  // Group the model's edges into one transition per (from, to).
  const groups: TransitionGroup[] = [];
  const add = (from: string, to: string, edge: StateEdge): void => {
    let g = groups.find((x) => x.from === from && x.to === to);
    if (!g) {
      g = { from, to, handlers: [], edges: [], flag: null, violated: [] };
      groups.push(g);
    }
    if (!g.handlers.includes(edge.handler)) g.handlers.push(edge.handler);
    g.edges.push(edge);
    for (const v of violationsFrom(edge, from)) {
      if (!g.violated.includes(v.transition)) g.violated.push(v.transition);
      if (g.flag !== "Known") g.flag = v.certainty;
    }
  };
  for (const e of machine.edges) {
    for (const from of e.from) {
      if (e.to.kind === "State") add(from, e.to.state, e);
      else if (e.to.kind === "Unknown") add(from, "?", e);
      // An unchanged outcome a `transition` refuses is always drawn: a
      // flagged edge is never hidden (track §3.5).
      else if (opts.showUnchanged || violationsFrom(e, from).length > 0) add(from, from, e);
    }
  }

  if (groups.some((g) => g.to === "?")) {
    stateIds.set(UNKNOWN_ID, "?");
    lines.push(`    state "?" as ${UNKNOWN_ID}`);
  }

  const idOf = (name: string): string =>
    name === "?" ? UNKNOWN_ID : stateId(index.get(name) ?? 0);

  const hasInitial = machine.initial !== null && index.has(machine.initial);
  if (hasInitial && machine.initial !== null) lines.push(`    [*] --> ${idOf(machine.initial)}`);
  for (const g of groups) {
    const handlers = g.handlers.map((h) => `${h}()`).join(", ");
    const label = esc(g.flag ? `⚠ ${handlers} (${g.violated.join(", ")})` : handlers);
    lines.push(`    ${idOf(g.from)} --> ${idOf(g.to)} : ${label}`);
  }

  if (machine.truncated.length > 0 && machine.states.length > 0) {
    const names = machine.truncated.map((h) => `${h}()`).join(", ");
    lines.push(`    note right of ${stateId(0)} : ${esc(`${names} too many paths to analyse`)}`);
  }

  const dimmed = machine.states
    .map((s, i) => [s, i] as const)
    .filter(([s]) => !machine.reachable.includes(s))
    .map(([, i]) => stateId(i));
  lines.push("    classDef unreachable opacity:0.45,stroke-dasharray:4 3");
  lines.push("    classDef computed stroke-dasharray:4 3");
  if (dimmed.length > 0) lines.push(`    class ${dimmed.join(",")} unreachable`);
  if (stateIds.has(UNKNOWN_ID)) lines.push(`    class ${UNKNOWN_ID} computed`);

  return { text: lines.join("\n"), edgeOrder: groups, hasInitial, stateIds };
}

/** #2197: the violations of `edge` that apply when it starts in `from`. */
export function violationsFrom(edge: StateEdge, from: string): StateEdge["violations"] {
  return (edge.violations ?? []).filter((v) => v.from.includes(from));
}

/** One cell of the handler x state table: every path of `handler` that can
 *  start in `state`. Empty when the handler has no path from it (a path the
 *  extractor pruned as infeasible). */
export function cellEdges(machine: StateMachine, handler: string, state: string): StateEdge[] {
  return machine.edges.filter((e) => e.handler === handler && e.from.includes(state));
}

/** A cell entry's text: `→ Placed · Ok(())`, `Err(AlreadyPlaced)`, `→ ? · …`. */
export function cellText(edge: StateEdge): string {
  const target =
    edge.to.kind === "State" ? `→ ${edge.to.state}` : edge.to.kind === "Unknown" ? "→ ?" : "";
  const outcome = edge.outcome ?? "";
  return [target, outcome].filter((p) => p !== "").join(" · ") || "()";
}
