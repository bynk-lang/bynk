// #2196: unit coverage for the agent state view's pure halves — the Mermaid
// `stateDiagram-v2` text and table cells (`state-gen.ts`) and the rendered-SVG
// back-mapping (`state-map.ts`). The element ids the mapping tests use are the
// shapes a real Mermaid 12.2 render produces (`<svg-id>-state-s0-4`, a note's
// `…-state-s0----note-4`, transition labels carrying `data-id="edge<n>"`, and
// no label element for the label-less `[*] --> initial`).

import * as assert from "assert";

import { cellEdges, cellText, toMermaidState } from "../../src/webview/state-gen";
import { stateNode, transitionLabels } from "../../src/webview/state-map";
import type { StateEdge, StateMachine } from "../../src/webview/stateTypes";

const R = { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } };

function edge(handler: string, from: string[], to: StateEdge["to"], outcome: string | null, guards: string[] = []): StateEdge {
  return { handler, from, guards, to, outcome, range: R };
}

const ORDER: StateMachine = {
  field: "status",
  fieldRange: R,
  states: ["Pending", "Placed", "Cancelled"],
  initial: "Pending",
  edges: [
    edge("place", ["Pending"], { kind: "State", state: "Placed" }, "Ok(())"),
    edge("place", ["Placed"], { kind: "Unchanged" }, "Err(AlreadyPlaced)"),
    edge("place", ["Cancelled"], { kind: "Unchanged" }, "Err(AlreadyCancelled)"),
    edge("cancel", ["Pending"], { kind: "State", state: "Cancelled" }, "Ok(())"),
    edge("cancel", ["Placed"], { kind: "Unchanged" }, "Err(AlreadyPlaced)"),
    edge("cancel", ["Cancelled"], { kind: "Unchanged" }, "Err(AlreadyCancelled)"),
  ],
  reachable: ["Pending", "Placed", "Cancelled"],
  truncated: [],
};

function body(text: string): string[] {
  return text.split("\n").slice(1).map((l) => l.trim());
}

describe("toMermaidState", () => {
  it("draws the Order lifecycle: initial state and the two moves, unchanged outcomes hidden", () => {
    const { text, edgeOrder, hasInitial, stateIds } = toMermaidState(ORDER, { showUnchanged: false });
    assert.deepStrictEqual(body(text), [
      'state "Pending" as s0',
      'state "Placed" as s1',
      'state "Cancelled" as s2',
      "[*] --> s0",
      "s0 --> s1 : place()",
      "s0 --> s2 : cancel()",
      "classDef unreachable opacity:0.45,stroke-dasharray:4 3",
      "classDef computed stroke-dasharray:4 3",
    ]);
    assert.strictEqual(hasInitial, true);
    assert.deepStrictEqual(edgeOrder.map((g) => [g.from, g.to, g.handlers]), [
      ["Pending", "Placed", ["place"]],
      ["Pending", "Cancelled", ["cancel"]],
    ]);
    assert.strictEqual(stateIds.get("s1"), "Placed");
  });

  it("draws unchanged outcomes as self-loops when asked, merging handlers on one transition", () => {
    const { text, edgeOrder } = toMermaidState(ORDER, { showUnchanged: true });
    assert.ok(text.includes("s1 --> s1 : place(), cancel()"), text);
    assert.ok(text.includes("s2 --> s2 : place(), cancel()"), text);
    const loop = edgeOrder.find((g) => g.from === "Placed" && g.to === "Placed");
    assert.strictEqual(loop?.edges.length, 2, "one transition stands for both handlers' edges");
  });

  it("sends a computed write to a dashed `?` pseudo-state, from every state it can start in", () => {
    const m: StateMachine = {
      ...ORDER,
      edges: [edge("reset", ["Placed", "Cancelled"], { kind: "Unknown" }, null)],
    };
    const { text, stateIds } = toMermaidState(m, { showUnchanged: false });
    assert.ok(text.includes('state "?" as unknown'), text);
    assert.ok(text.includes("s1 --> unknown : reset()"), text);
    assert.ok(text.includes("s2 --> unknown : reset()"), text);
    assert.ok(text.includes("class unknown computed"), text);
    assert.strictEqual(stateIds.get("unknown"), "?");
  });

  it("dims unreachable states, notes truncated handlers, and omits [*] without a literal initialiser", () => {
    const m: StateMachine = {
      ...ORDER,
      initial: null,
      reachable: ["Pending", "Placed"],
      truncated: ["go"],
    };
    const { text, hasInitial } = toMermaidState(m, { showUnchanged: false });
    assert.strictEqual(hasInitial, false);
    assert.ok(!text.includes("[*]"), text);
    assert.ok(text.includes("class s2 unreachable"), text);
    assert.ok(text.includes("note right of s0 : go() too many paths to analyse"), text);
  });

  it("keeps Mermaid's separators out of state names and labels", () => {
    const m: StateMachine = {
      ...ORDER,
      states: ["A;B", "C"],
      initial: "A;B",
      reachable: ["A;B", "C"],
      edges: [edge("x:y", ["A;B"], { kind: "State", state: "C" }, null)],
    };
    const { text } = toMermaidState(m, { showUnchanged: false });
    assert.ok(text.includes('state "A B" as s0'), text);
    assert.ok(text.includes("s0 --> s1 : x y()"), text);
  });
});

describe("handler x state cells", () => {
  it("lists every path of a handler from a state, with target and outcome", () => {
    assert.deepStrictEqual(cellEdges(ORDER, "place", "Pending").map(cellText), ["→ Placed · Ok(())"]);
    assert.deepStrictEqual(cellEdges(ORDER, "place", "Placed").map(cellText), ["Err(AlreadyPlaced)"]);
    assert.deepStrictEqual(cellText(edge("r", ["A"], { kind: "Unknown" }, null)), "→ ?");
    assert.deepStrictEqual(cellText(edge("r", ["A"], { kind: "Unchanged" }, null)), "()");
  });

  it("is empty where no path of the handler starts in the state", () => {
    const m: StateMachine = { ...ORDER, edges: [edge("place", ["Pending"], { kind: "State", state: "Placed" }, null)] };
    assert.deepStrictEqual(cellEdges(m, "place", "Placed"), []);
  });
});

interface FakeEl {
  id: string;
  dataId: string | null;
  getAttribute(name: string): string | null;
}

function fake(id: string, dataId: string | null = null): FakeEl {
  return { id, dataId, getAttribute: (name) => (name === "data-id" ? dataId : null) };
}

describe("state-map", () => {
  it("finds a state's node by its Mermaid id, never a longer id or its note", () => {
    const nodes = [
      fake("bynk-agent-state-0-state-s0----note-4"),
      fake("bynk-agent-state-0-state-s10-2"),
      fake("bynk-agent-state-0-state-s1-3"),
      fake("bynk-agent-state-0-state-s0-4"),
      fake("bynk-agent-state-0-state-unknown-3"),
    ];
    assert.strictEqual(stateNode(nodes, "s0")?.id, "bynk-agent-state-0-state-s0-4");
    assert.strictEqual(stateNode(nodes, "s1")?.id, "bynk-agent-state-0-state-s1-3");
    assert.strictEqual(stateNode(nodes, "unknown")?.id, "bynk-agent-state-0-state-unknown-3");
    assert.strictEqual(stateNode(nodes, "s2"), undefined);
  });

  it("orders transition labels by their edge number, whatever the DOM order", () => {
    const labels = [fake("", "edge3"), fake("", "edge1"), fake("", null), fake("", "s0-s0----note"), fake("", "edge10")];
    assert.deepStrictEqual(transitionLabels(labels).map((l) => l.dataId), ["edge1", "edge3", "edge10"]);
  });
});
