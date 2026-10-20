// #846: the sequence-diagram webview's own bundle entry (`out/webview.js`,
// a separate esbuild target from the extension host's `out/extension.js` —
// see `package.json`'s `build` script). Runs inside the webview's browser
// context, never the extension host: no `vscode` module, only
// `acquireVsCodeApi()`.
//
// Reads the `SequencePayload` the extension host embeds as
// `window.__BYNK_SEQUENCE_MODEL__`, renders it via vendored Mermaid (no
// CDN — the page's CSP forbids one), and wires click-to-code: Mermaid's
// `click` directive support for `sequenceDiagram` is inconsistent across
// versions and doesn't cover every element this renders (an `alt` header, a
// `note`), so instead this walks the rendered SVG's message (`.messageText`),
// header (`.loopText`) and note (`.noteText`) elements in DOM order — the same
// order `mermaid-gen.ts` emitted them in — plus the participant label nodes in
// declaration order (see `participant-map.ts`; a plain `.actor` walk does NOT
// line up 1:1 with participants), and zips each against the `{uri, range}` it
// corresponds to, posting a `reveal` message back to the extension host on
// click.

import mermaid from "mermaid";

import { startLiveView, type VsCodeApi } from "./live";
import { toMermaid } from "./mermaid-gen";
import { participantEls } from "./participant-map";
import type { RevealMessage, SequencePayload } from "./types";

declare function acquireVsCodeApi(): VsCodeApi;

const vscodeApi = acquireVsCodeApi();

function reveal(uri: string, range: SequencePayload["model"]["messages"][number]["range"]): void {
  const message: RevealMessage = { type: "reveal", uri, range };
  vscodeApi.postMessage(message);
}

function wireClickable(el: Element, uri: string, range: SequencePayload["model"]["messages"][number]["range"]): void {
  el.addEventListener("click", () => reveal(uri, range));
  (el as HTMLElement).style.cursor = "pointer";
}

/** Read a `--vscode-*` custom property off the live document — VS Code injects
 *  the current colour theme's palette this way (see `documentationView.ts` for
 *  the same convention used in plain CSS). Read fresh on every render rather
 *  than hardcoded, so this tracks whatever theme (light, dark, high-contrast,
 *  or a user's custom one) is actually active, not just a light/dark guess. */
function vscodeCssVar(name: string, fallback: string): string {
  const value = getComputedStyle(document.body).getPropertyValue(name).trim();
  return value || fallback;
}

/** Mermaid's built-in "default" theme is a light palette baked into the
 *  rendered SVG's inline styles (fixed hex fills/strokes, no CSS variables) —
 *  it never adapts to VS Code's theme, which is why this looked fine in a
 *  light theme and unreadable in a dark one (dark text on boxes that stayed
 *  light, or vice versa). `theme: "base"` lets every colour be overridden, so
 *  this maps the sequence-diagram-relevant variables onto the editor's own
 *  `--vscode-*` custom properties instead of a hardcoded palette. */
function mermaidThemeVariables(): Record<string, string> {
  const editorBg = vscodeCssVar("--vscode-editor-background", "#1e1e1e");
  const editorFg = vscodeCssVar("--vscode-editor-foreground", "#cccccc");
  const panelBorder = vscodeCssVar("--vscode-panel-border", editorFg);
  const selectionBg = vscodeCssVar("--vscode-editor-inactiveSelectionBackground", editorBg);
  const noteBg = vscodeCssVar("--vscode-textCodeBlock-background", editorBg);

  return {
    background: editorBg,
    primaryColor: selectionBg,
    primaryTextColor: editorFg,
    primaryBorderColor: panelBorder,
    lineColor: editorFg,
    textColor: editorFg,
    actorBkg: selectionBg,
    actorBorder: panelBorder,
    actorTextColor: editorFg,
    actorLineColor: panelBorder,
    signalColor: editorFg,
    signalTextColor: editorFg,
    labelBoxBkgColor: selectionBg,
    labelBoxBorderColor: panelBorder,
    labelTextColor: editorFg,
    loopTextColor: editorFg,
    noteBkgColor: noteBg,
    noteBorderColor: panelBorder,
    noteTextColor: editorFg,
    activationBorderColor: panelBorder,
    activationBkgColor: selectionBg,
    sequenceNumberColor: editorBg,
  };
}

// #2093: each render needs a fresh Mermaid id — on a second `mermaid.render`
// with the same id in one document, Mermaid finds the previous SVG instead of a
// fresh container (the trap `architecture.ts` documents). Live updates now
// re-render in place, so the sequence view hits it too.
let renderSeq = 0;
let mermaidReady = false;

async function render(root: HTMLElement, payload: SequencePayload | null): Promise<void> {
  if (!payload) {
    root.textContent = "No sequence model to render.";
    return;
  }

  if (!mermaidReady) {
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      theme: "base",
      themeVariables: mermaidThemeVariables(),
    });
    mermaidReady = true;
  }

  const { text, participantOrder, messageOrder, noteOrder } = toMermaid(payload.model);
  let svg: string;
  try {
    ({ svg } = await mermaid.render(`bynk-sequence-diagram-${renderSeq++}`, text));
  } catch (err) {
    root.textContent = `Failed to render diagram: ${String(err)}`;
    return;
  }
  root.innerHTML = svg;

  // Participants (skip `Entry`, which carries no span). One label element per
  // participant in declaration order — the same order as `participantOrder` —
  // so this is a true 1:1 zip (a raw `.actor` walk is not; see
  // `participant-map.ts`).
  const partEls = participantEls(root);
  participantOrder.forEach((p, i) => {
    const el = partEls[i];
    if (p.range && el) wireClickable(el, payload.uri, p.range);
  });

  // Messages, in the same order `mermaid-gen.ts` emitted them (`.messageText`).
  const messageEls = root.querySelectorAll(".messageText");
  messageOrder.forEach((m, i) => {
    const el = messageEls[i];
    if (el) wireClickable(el, payload.uri, m.range);
  });

  // `alt`/`opt` branch headers (`.labelText`/`.loopText` depending on
  // Mermaid version) link to their block's own span — every branch header
  // for a given block points at the same span, since the block (not the
  // branch) is what has a source location.
  const headerEls = root.querySelectorAll(".labelText, .loopText");
  let headerIndex = 0;
  for (const block of payload.model.blocks) {
    if (block.kind === "Collapsed") continue;
    for (let b = 0; b < block.branches.length; b++) {
      const el = headerEls[headerIndex++];
      if (el) wireClickable(el, payload.uri, block.range);
    }
  }

  // Notes (`.noteText`), in emission order — a collapsed-nesting marker, a
  // branch's reply outcome, an empty-branch placeholder, or (#2194) an agent's
  // state write. A block note links to its owning block's span (the
  // reply/placeholder have no separate source range of their own; jumping to
  // the `if`/`match` is the useful target); a state write links to itself.
  const noteEls = root.querySelectorAll(".noteText");
  noteOrder.forEach((note, i) => {
    const el = noteEls[i];
    if (el) wireClickable(el, payload.uri, note.range);
  });

  reportWiring(participantOrder, partEls);
}

/** Test seam (#852): when the host sets `window.__BYNK_SEQUENCE_DEBUG__`, post
 *  back the rendered label the participant zip landed on for each participant.
 *  The rendered SVG's DOM is unreachable from the extension-host integration
 *  test, so this is how it checks that participant `i`'s element really is
 *  participant `i`'s box (label matches its name) rather than some other
 *  `.actor` node. No-op in production — the flag is only ever set by the test. */
function reportWiring(participantOrder: SequencePayload["model"]["participants"], partEls: Element[]): void {
  if (!(window as unknown as { __BYNK_SEQUENCE_DEBUG__?: boolean }).__BYNK_SEQUENCE_DEBUG__) return;
  vscodeApi.postMessage({
    type: "wiring",
    participants: participantOrder.map((p, i) => ({
      name: p.name,
      label: partEls[i]?.textContent?.trim() ?? null,
    })),
  });
}

startLiveView<SequencePayload>(vscodeApi, "__BYNK_SEQUENCE_MODEL__", render);
