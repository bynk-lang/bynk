// #846: unit coverage for `toMermaid` (src/webview/mermaid-gen.ts) — the one
// piece of the sequence-diagram feature that is a pure function, testable
// directly against hand-built `SequenceModel` fixtures without a live
// webview/Mermaid render. Rendered-diagram correctness (does Mermaid accept
// this text, does the SVG's `.actor`/`.messageText` order match what
// main.ts zips against) needs a real browser and is out of reach here —
// this only pins the generated Mermaid *text*.

import * as assert from "assert";

import { toMermaid } from "../../src/webview/mermaid-gen";
import type { SequenceModel } from "../../src/webview/types";

const ZERO_RANGE = { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } };

describe("toMermaid", () => {
  it("renders a degenerate (entry-only) model as just the participant line", () => {
    const model: SequenceModel = {
      participants: [{ id: 0, kind: "Entry", name: "api call", range: null }],
      messages: [],
      blocks: [],
    };
    const { text } = toMermaid(model);
    assert.strictEqual(text, "sequenceDiagram\n    participant P0 as api call");
  });

  it("renders a Call+Return pair and a fire-and-forget Send with distinct arrows", () => {
    const model: SequenceModel = {
      participants: [
        { id: 0, kind: "Entry", name: "api call", range: null },
        { id: 1, kind: "Capability", name: "Clock", range: ZERO_RANGE },
      ],
      messages: [
        { from: 0, to: 1, kind: "Call", label: "now()", range: ZERO_RANGE, block: null },
        { from: 1, to: 0, kind: "Return", label: "", range: ZERO_RANGE, block: null },
        { from: 0, to: 1, kind: "Send", label: "info(\"hi\")", range: ZERO_RANGE, block: null },
      ],
      blocks: [],
    };
    const { text, messageOrder } = toMermaid(model);
    assert.ok(text.includes("P0->>P1: now()"), text);
    assert.ok(text.includes("P1-->>P0: reply"), text);
    assert.ok(text.includes("P0-)P1: info(#58;\"hi\")") || text.includes('P0-)P1: info("hi")'), text);
    assert.strictEqual(messageOrder.length, 3);
  });

  it("renders a return-gating if/else's branch outcomes as notes when both branches carry no messages", () => {
    // The issue's own regression case: rate-limiter's
    // `if view.allowed { Ok(view) } else { TooManyRequests(...) }` produces
    // an AltBlock whose branches are message-free. The block must not only
    // render — each branch's reply must render as a note, or Mermaid draws an
    // empty `alt` as a mangled zero-width box (the reported symptom).
    const model: SequenceModel = {
      participants: [{ id: 0, kind: "Entry", name: "api call", range: null }],
      messages: [],
      blocks: [
        {
          id: 0,
          kind: "If",
          branches: [
            { label: "view.allowed", messageIds: [], reply: "Ok(view)" },
            { label: "otherwise", messageIds: [], reply: "TooManyRequests(...)" },
          ],
          range: ZERO_RANGE,
          parent: null,
          parentBranch: null,
        },
      ],
    };
    const { text, noteOrder } = toMermaid(model);
    assert.ok(text.includes("alt view.allowed"), text);
    assert.ok(text.includes("else otherwise"), text);
    assert.ok(text.includes("note over P0: Ok(view)"), text);
    assert.ok(text.includes("note over P0: TooManyRequests(...)"), text);
    assert.ok(text.includes("end"), text);
    // One note per branch reply, each linking back to the block for click-to-code.
    assert.strictEqual(noteOrder.length, 2);
    assert.ok(noteOrder.every((n) => n.block?.id === 0));
  });

  it("anchors a message-free, reply-free branch with a placeholder note so the box never collapses", () => {
    // Defensive floor: a branch that yields nothing renderable (an explicit
    // `{ () }` — no messages, no nested block, no reply) would still collapse
    // the `alt`. A placeholder note keeps it legible.
    const model: SequenceModel = {
      participants: [{ id: 0, kind: "Entry", name: "api call", range: null }],
      messages: [],
      blocks: [
        {
          id: 0,
          kind: "If",
          branches: [
            { label: "a", messageIds: [], reply: null },
            { label: "b", messageIds: [], reply: null },
          ],
          range: ZERO_RANGE,
          parent: null,
          parentBranch: null,
        },
      ],
    };
    const { text, noteOrder } = toMermaid(model);
    assert.strictEqual((text.match(/note over P0: …/g) ?? []).length, 2, text);
    assert.strictEqual(noteOrder.length, 2);
  });

  it("nests a child block under the correct parent branch, not just the parent block", () => {
    // Two sibling blocks under the same parent id but different branches
    // must not be conflated — this is the regression `parent`/
    // `parentBranch` (rather than `parent` alone) exists to prevent.
    const model: SequenceModel = {
      participants: [
        { id: 0, kind: "Entry", name: "api call", range: null },
        { id: 1, kind: "Capability", name: "Clock", range: ZERO_RANGE },
      ],
      messages: [
        {
          from: 0,
          to: 1,
          kind: "Call",
          label: "now()",
          range: { start: { line: 2, character: 0 }, end: { line: 2, character: 1 } },
          block: 1,
        },
        {
          from: 1,
          to: 0,
          kind: "Return",
          label: "",
          range: { start: { line: 2, character: 1 }, end: { line: 2, character: 2 } },
          block: 1,
        },
      ],
      blocks: [
        {
          id: 0,
          kind: "If",
          branches: [
            { label: "then", messageIds: [], reply: null },
            { label: "else", messageIds: [], reply: null },
          ],
          range: { start: { line: 0, character: 0 }, end: { line: 5, character: 0 } },
          parent: null,
          parentBranch: null,
        },
        {
          id: 1,
          kind: "If",
          branches: [
            { label: "then", messageIds: [0, 1], reply: null },
            { label: "else", messageIds: [], reply: null },
          ],
          range: { start: { line: 1, character: 0 }, end: { line: 3, character: 0 } },
          parent: 0,
          parentBranch: 0, // nested in block 0's "then" branch
        },
      ],
    };
    const { text } = toMermaid(model);
    // The nested `alt` (block 1) must appear between block 0's `alt then`
    // and its `else` — i.e. inside the "then" branch, not after `end`.
    const outerThen = text.indexOf("alt then");
    const outerEnd = text.lastIndexOf("end");
    const innerAlt = text.indexOf("alt then", outerThen + 1);
    assert.ok(outerThen >= 0 && innerAlt > outerThen && innerAlt < outerEnd, text);
    assert.ok(text.includes("P0->>P1: now()"), text);
  });

  it("renders a Collapsed block as a note, not an alt/opt", () => {
    const model: SequenceModel = {
      participants: [{ id: 0, kind: "Entry", name: "api call", range: null }],
      messages: [],
      blocks: [
        {
          id: 0,
          kind: "Collapsed",
          branches: [],
          range: ZERO_RANGE,
          parent: null,
          parentBranch: null,
        },
      ],
    };
    const { text, noteOrder } = toMermaid(model);
    assert.ok(text.includes("note over P0"), text);
    assert.ok(!text.includes("alt "), text);
    assert.strictEqual(noteOrder.length, 1);
  });

  it("renders an Actor participant with the `actor` keyword and routes replies to it", () => {
    // With a principal, the actor originates the request (a Call in) and
    // receives the handler's outcomes as Return messages (not notes).
    const model: SequenceModel = {
      participants: [
        { id: 1, kind: "Actor", name: "Visitor", range: ZERO_RANGE },
        { id: 0, kind: "Entry", name: "api", range: null },
      ],
      messages: [
        { from: 1, to: 0, kind: "Call", label: "GET /check/:client", range: ZERO_RANGE, block: null },
        { from: 0, to: 1, kind: "Return", label: "Ok(view)", range: ZERO_RANGE, block: 0 },
        { from: 0, to: 1, kind: "Return", label: "TooManyRequests(...)", range: ZERO_RANGE, block: 0 },
      ],
      blocks: [
        {
          id: 0,
          kind: "If",
          branches: [
            { label: "view.allowed", messageIds: [1], reply: null },
            { label: "otherwise", messageIds: [2], reply: null },
          ],
          range: ZERO_RANGE,
          parent: null,
          parentBranch: null,
        },
      ],
    };
    const { text, noteOrder } = toMermaid(model);
    assert.ok(text.includes("actor P1 as Visitor"), text);
    assert.ok(text.includes("participant P0 as api"), text);
    assert.ok(text.includes("P1->>P0: GET /check/#58;client"), text);
    // The outcomes are dashed return arrows to the actor, inside the alt.
    assert.ok(text.includes("P0-->>P1: Ok(view)"), text);
    assert.ok(text.includes("P0-->>P1: TooManyRequests(...)"), text);
    // No reply notes and no empty-branch placeholders — the arrows are content.
    assert.strictEqual(noteOrder.length, 0, text);
    assert.ok(!text.includes("note over"), text);
  });

  it("anchors notes over the entry, not the actor, when a principal is present", () => {
    // Regression: `entryAnchor` must be found by kind, not array position — the
    // actor sits at participants[0] (id 1) ahead of the entry (id 0), so a
    // placeholder/collapsed note anchored by position would draw over the actor.
    const model: SequenceModel = {
      participants: [
        { id: 1, kind: "Actor", name: "Visitor", range: ZERO_RANGE },
        { id: 0, kind: "Entry", name: "api", range: null },
      ],
      messages: [],
      blocks: [
        {
          id: 0,
          kind: "Collapsed",
          branches: [],
          range: ZERO_RANGE,
          parent: null,
          parentBranch: null,
        },
      ],
    };
    const { text, noteOrder } = toMermaid(model);
    assert.ok(text.includes("note over P0:"), text);
    assert.ok(!text.includes("note over P1:"), text);
    assert.strictEqual(noteOrder.length, 1);
  });

  it("renders opt (not alt) for a single-branch block", () => {
    const model: SequenceModel = {
      participants: [{ id: 0, kind: "Entry", name: "api call", range: null }],
      messages: [],
      blocks: [
        {
          id: 0,
          kind: "Match",
          branches: [{ label: "Some(x)", messageIds: [], reply: null }],
          range: ZERO_RANGE,
          parent: null,
          parentBranch: null,
        },
      ],
    };
    const { text } = toMermaid(model);
    assert.ok(text.includes("opt Some(x)"), text);
  });

  it("renders an agent's state writes as notes, interleaved with its calls in source order (#2194)", () => {
    // `Order.add`: a call to another agent, then a store write, inside the
    // `Pending` branch, then the branch's reply. The write must render as a
    // note (not an arrow), in source order, and zip with the notes — not the
    // messages — so click-to-code stays aligned.
    const at = (line: number) => ({ start: { line, character: 0 }, end: { line, character: 1 } });
    const model: SequenceModel = {
      participants: [
        { id: 0, kind: "Entry", name: "Order.add", range: null },
        { id: 1, kind: "Agent", name: "Counter", range: at(1) },
      ],
      messages: [
        { from: 0, to: 1, kind: "Call", label: "bump()", range: at(2), block: 0 },
        { from: 1, to: 0, kind: "Return", label: "", range: at(2), block: 0 },
        { from: 0, to: 0, kind: "StateWrite", label: "items.put(sku, …)", range: at(3), block: 0 },
        { from: 0, to: 0, kind: "StateWrite", label: "status := Placed", range: at(4), block: 0 },
      ],
      blocks: [
        {
          id: 0,
          kind: "Match",
          branches: [{ label: "Pending", messageIds: [0, 1, 2, 3], reply: "Ok(())" }],
          range: at(1),
          parent: null,
          parentBranch: null,
        },
      ],
    };
    const { text, messageOrder, noteOrder } = toMermaid(model);
    const body = text.split("\n").slice(3).map((l) => l.trim());
    assert.deepStrictEqual(body, [
      "opt Pending",
      "P0->>P1: bump()",
      "P1-->>P0: reply",
      "note over P0: items.put(sku, …)",
      "note over P0: status #58;= Placed",
      "note over P0: Ok(())",
      "end",
    ]);
    // Writes are notes, not messages: two arrows, three notes.
    assert.strictEqual(messageOrder.length, 2);
    assert.strictEqual(noteOrder.length, 3);
    // Each write links to itself; the reply links to its block.
    assert.deepStrictEqual(noteOrder[0], { range: at(3), block: null });
    assert.deepStrictEqual(noteOrder[1], { range: at(4), block: null });
    assert.strictEqual(noteOrder[2].block?.id, 0);
  });
});
