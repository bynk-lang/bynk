// #846: pure `SequenceModel` -> Mermaid `sequenceDiagram` text generation.
// Kept separate from `main.ts`'s DOM/webview glue so it has no dependency on
// a browser or VS Code webview host — the whole point of a structured
// `SequenceModel` wire shape (rather than server-rendered Mermaid text) is
// that `bynk-check`/`bynk-ide`/`bynk-lsp` stay Mermaid-agnostic; this module
// is the one place that owns "how a `SequenceModel` becomes Mermaid syntax."
//
// Rendering is a recursive walk of the block tree (`AltBlock.parent` +
// `AltBlock.parentBranch`), not a single pass over the flat `messages`
// array: a branch can be entirely message-free (the rate-limiter's
// `if`/`else` gates only the final return) while still containing a nested
// block, so the flat array alone cannot place it. At each level, sibling
// messages and child blocks are merged and ordered by source position
// (`range.start`).
//
// Mermaid's `click` directive support for `sequenceDiagram` is inconsistent
// across versions and does not cover every element kind this renders (an
// `alt`/`opt` header, a `note`), so click-to-code is wired by zipping these
// orderings against the rendered SVG in `main.ts` rather than via embedded
// `click` statements. `messageOrder`/`noteOrder` are emission order and zip
// 1:1 against `.messageText`/`.noteText` (which appear in that order);
// `participantOrder` is declaration order but does NOT zip against raw
// `.actor` nodes (Mermaid emits several per participant, regrouped) — the
// caller recovers one element per participant via `participant-map.ts`.

import type { AltBlock, Message, Participant, Range, SequenceModel } from "./types";

export interface MermaidResult {
  text: string;
  /** `model.participants` in declaration order — the order this function
   *  writes the `participant P#` lines. The caller zips this against one
   *  element per participant, recovered by `participant-map.ts`'s
   *  `participantEls` (NOT a raw `.actor` walk — Mermaid emits several
   *  `.actor` nodes per participant, regrouped out of declaration order). */
  participantOrder: Participant[];
  /** Emission order (source-position order, following nesting) — the
   *  caller zips this 1:1 against the rendered SVG's message-line
   *  elements, which Mermaid emits in the order the diagram text names
   *  them, i.e. this order. */
  messageOrder: Message[];
  /** One entry per rendered `note` element, in emission order. The caller
   *  zips this 1:1 against the rendered SVG's `.noteText` elements and links
   *  each to its `range`, so every note is click-to-code. A block note (a
   *  `Collapsed` marker, a branch's reply outcome, or an empty-branch
   *  placeholder) carries its block and links to the block's span — a single
   *  block can contribute several notes, one reply per branch, so this is not
   *  1:1 with `model.blocks`. A state-write note (#2194) carries no block and
   *  links to the write itself. */
  noteOrder: NoteAnchor[];
}

/** What a rendered note links back to. */
export interface NoteAnchor {
  range: Range;
  /** The block a block note belongs to; `null` for a state-write note. */
  block: AltBlock | null;
}

const RESERVED_LABEL_CHARS: Record<string, string> = {
  ":": "#58;",
  ";": "#59;",
};

function escapeLabel(label: string): string {
  const collapsedWhitespace = label.replace(/\s+/g, " ").trim();
  return collapsedWhitespace.replace(/[:;]/g, (ch) => RESERVED_LABEL_CHARS[ch] ?? ch);
}

function rangeKey(range: Range): [number, number] {
  return [range.start.line, range.start.character];
}

function childBlocksOf(blocks: AltBlock[], parentId: number, parentBranch: number): AltBlock[] {
  return blocks.filter((b) => b.parent === parentId && b.parentBranch === parentBranch);
}

type ContentItem =
  | { start: [number, number]; kind: "message"; message: Message }
  | { start: [number, number]; kind: "block"; block: AltBlock };

function orderedContent(messages: Message[], blocks: AltBlock[]): ContentItem[] {
  const items: ContentItem[] = [
    ...messages.map((message) => ({ start: rangeKey(message.range), kind: "message" as const, message })),
    ...blocks.map((block) => ({ start: rangeKey(block.range), kind: "block" as const, block })),
  ];
  items.sort((a, b) => a.start[0] - b.start[0] || a.start[1] - b.start[1]);
  return items;
}

export function toMermaid(model: SequenceModel): MermaidResult {
  const lines: string[] = ["sequenceDiagram"];
  const messageOrder: Message[] = [];
  const noteOrder: NoteAnchor[] = [];

  // The entry lifeline — where every note (a collapsed marker, a branch reply,
  // an empty-branch placeholder) is anchored, since the handler *is* the entry.
  // Found by kind, not array position: with a principal the actor is inserted
  // at index 0 (ahead of the entry), so `participants[0]` is no longer the
  // entry — anchoring by position would draw notes over the actor.
  const entryAnchor = model.participants.find((p) => p.kind === "Entry")?.id ?? 0;

  for (const p of model.participants) {
    // The principal renders as an `actor` (stick figure); everyone else is a
    // `participant` box.
    const keyword = p.kind === "Actor" ? "actor" : "participant";
    lines.push(`    ${keyword} P${p.id} as ${escapeLabel(p.name)}`);
  }

  const emitMessage = (m: Message): void => {
    const label = escapeLabel(m.label);
    if (m.kind === "StateWrite") {
      // A write to the agent's own state is a note, not an arrow — it renders
      // a `.noteText`, not a `.messageText`, so it zips with the notes.
      noteOrder.push({ range: m.range, block: null });
      lines.push(`    note over P${entryAnchor}: ${label}`);
      return;
    }
    messageOrder.push(m);
    if (m.kind === "Send") {
      lines.push(`    P${m.from}-)P${m.to}: ${label}`);
    } else if (m.kind === "Call") {
      lines.push(`    P${m.from}->>P${m.to}: ${label}`);
    } else {
      // Return: label is empty (the extractor doesn't synthesise one) —
      // Mermaid requires *some* text after `:`, so fall back to the arrow
      // alone reading as "reply".
      lines.push(`    P${m.from}-->>P${m.to}: ${label || "reply"}`);
    }
  };

  const emitNote = (block: AltBlock, text: string): void => {
    noteOrder.push({ range: block.range, block });
    lines.push(`    note over P${entryAnchor}: ${text}`);
  };

  const emitBlock = (block: AltBlock): void => {
    if (block.kind === "Collapsed") {
      emitNote(block, "nested branching — click to view source");
      return;
    }
    const keyword = block.branches.length <= 1 ? "opt" : "alt";
    block.branches.forEach((branch, branchIndex) => {
      lines.push(`    ${branchIndex === 0 ? keyword : "else"} ${escapeLabel(branch.label)}`);
      const branchMessages = branch.messageIds
        .map((id) => model.messages[id])
        .filter((m): m is Message => m !== undefined);
      const children = childBlocksOf(model.blocks, block.id, branchIndex);
      const before = lines.length;
      renderContent(branchMessages, children);
      // The branch's own reply — the value the handler yields on this path —
      // as a note over the entry lifeline. This is the content that keeps a
      // return-gating block (both branches call no lifeline) from rendering
      // as an empty, mangled `alt` box.
      if (branch.reply) emitNote(block, escapeLabel(branch.reply));
      // A branch that emitted nothing at all (no messages, no nested block,
      // no reply — e.g. an explicit `{ () }`) would still collapse the box;
      // anchor it with a placeholder so the diagram stays legible.
      if (lines.length === before) emitNote(block, "…");
    });
    lines.push("    end");
  };

  function renderContent(messages: Message[], blocks: AltBlock[]): void {
    for (const item of orderedContent(messages, blocks)) {
      if (item.kind === "message") emitMessage(item.message);
      else emitBlock(item.block);
    }
  }

  const rootMessages = model.messages.filter((m) => m.block === null);
  const rootBlocks = model.blocks.filter((b) => b.parent === null);
  renderContent(rootMessages, rootBlocks);

  return {
    text: lines.join("\n"),
    participantOrder: model.participants,
    messageOrder,
    noteOrder,
  };
}
