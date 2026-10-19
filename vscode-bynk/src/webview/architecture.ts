// #851: the architecture-map webview's own bundle entry (`out/architecture.js`,
// a fourth esbuild target alongside `out/webview.js` (#846) and `out/docview.js`
// (#847) — see `package.json`'s `build` script). Runs inside the webview's
// browser context, never the extension host.
//
// Reads the `ArchPayload` the extension host embeds as
// `window.__BYNK_ARCH_MODEL__`, renders it via vendored Mermaid `flowchart`
// (no CDN — the page's CSP forbids one), and wires two click behaviours,
// looked up by the Mermaid node id `architecture-gen.ts` assigned (NOT a
// DOM-order zip — see that module's doc for why a flowchart doesn't need
// the sequence view's `participant-map.ts`-style workaround):
//   - a context/adapter box or a declaration-level member -> `reveal`,
//     posted to the extension host exactly like the sequence/doc views;
//   - the small +/− toggle next to each box -> DECISION A's expand-on-demand,
//     handled entirely client-side (flips membership in `expanded`, re-renders).

import mermaid from "mermaid";

import { type ClickTarget, toMermaidArch } from "./architecture-gen";
import { toClassDefColor } from "./classdef-color";
import type { ArchPayload, ArchRevealMessage } from "./architectureTypes";
import { startLiveView, type VsCodeApi } from "./live";

declare function acquireVsCodeApi(): VsCodeApi;

const vscodeApi = acquireVsCodeApi();

function reveal(uri: string, range: ArchRevealMessage["range"]): void {
  const message: ArchRevealMessage = { type: "reveal", uri, range };
  vscodeApi.postMessage(message);
}

/** Same convention as `main.ts`'s `vscodeCssVar` — read the live VS Code
 *  theme's palette off `--vscode-*` custom properties, snapshotted once per
 *  render rather than left as raw `var(...)` in the Mermaid `classDef` text
 *  (Mermaid bakes `classDef` into a `<style>` block it generates itself, and
 *  snapshotting here — not depending on CSS cascade into that block — is the
 *  same posture `mermaidThemeVariables()` already established). */
function vscodeCssVar(name: string, fallback: string): string {
  const value = getComputedStyle(document.body).getPropertyValue(name).trim();
  return value || fallback;
}

/** `vscodeCssVar`, but safe inside a `classDef`: resolved through the browser
 *  to hex, because a theme's `rgba(38, 79, 120, 0.5)` has commas, and a
 *  `classDef` splits its properties on them (the parse error that blanked the
 *  map). */
function classDefVar(name: string, fallback: string): string {
  const probe = document.createElement("span");
  probe.style.color = vscodeCssVar(name, fallback);
  document.body.appendChild(probe);
  const resolved = getComputedStyle(probe).color;
  probe.remove();
  return toClassDefColor(resolved) ?? toClassDefColor(fallback) ?? fallback;
}

function mermaidThemeVariables(): Record<string, string> {
  const editorBg = vscodeCssVar("--vscode-editor-background", "#1e1e1e");
  const editorFg = vscodeCssVar("--vscode-editor-foreground", "#cccccc");
  const panelBorder = vscodeCssVar("--vscode-panel-border", editorFg);
  const selectionBg = vscodeCssVar("--vscode-editor-inactiveSelectionBackground", editorBg);

  return {
    background: editorBg,
    primaryColor: selectionBg,
    primaryTextColor: editorFg,
    primaryBorderColor: panelBorder,
    lineColor: editorFg,
    textColor: editorFg,
    edgeLabelBackground: editorBg,
    clusterBkg: editorBg,
    clusterBorder: panelBorder,
  };
}

function classDefBlock(): string {
  const editorBg = classDefVar("--vscode-editor-background", "#1e1e1e");
  const editorFg = classDefVar("--vscode-editor-foreground", "#cccccc");
  const panelBorder = classDefVar("--vscode-panel-border", editorFg);
  const selectionBg = classDefVar("--vscode-editor-inactiveSelectionBackground", editorBg);
  const codeBg = classDefVar("--vscode-textCodeBlock-background", editorBg);
  const descFg = classDefVar("--vscode-descriptionForeground", editorFg);
  return [
    `classDef contextNode fill:${selectionBg},stroke:${panelBorder},color:${editorFg};`,
    `classDef adapterNode fill:${codeBg},stroke:${panelBorder},color:${editorFg},stroke-dasharray: 4 2;`,
    `classDef memberNode fill:${editorBg},stroke:${panelBorder},color:${descFg};`,
    `classDef toggleNode fill:transparent,stroke:${descFg},color:${descFg};`,
  ].join("\n");
}

function wireClicks(root: ParentNode, clickTargets: Map<string, ClickTarget>, onToggle: (name: string) => void): void {
  for (const [mermaidId, target] of clickTargets) {
    // Mermaid 12.2 prefixes node ids with the SVG's id (`<svg id>-flowchart-…`),
    // so match anywhere, not at the start.
    const el = root.querySelector(`[id*="flowchart-${mermaidId}-"]`);
    if (!el) continue;
    (el as HTMLElement).style.cursor = "pointer";
    el.addEventListener("click", () => {
      if (target.kind === "reveal") reveal(target.loc.uri, target.loc.range);
      else onToggle(target.name);
    });
  }
}

// `mermaid.render` looks up its temp container by this id and, on a second
// call with the *same* id, finds the previous render's SVG already sitting in
// the live DOM (`root.innerHTML = svg` below put it there) instead of a fresh
// container — DECISION A's expand/collapse toggle re-renders in place, so a
// fixed id broke on exactly the second render (the first click). The
// sequence/documentation views never hit this: each of their invocations
// reassigns `panel.webview.html`, giving Mermaid a fresh document every time.
let renderSeq = 0;

async function render(root: HTMLElement, payload: ArchPayload, expanded: Set<string>): Promise<void> {
  const { text, clickTargets } = toMermaidArch(payload.model, expanded);
  const fullText = `${text}\n${classDefBlock()}`;
  let svg: string;
  try {
    ({ svg } = await mermaid.render(`bynk-architecture-map-${renderSeq++}`, fullText));
  } catch (err) {
    root.textContent = `Failed to render diagram: ${String(err)}`;
    return;
  }
  root.innerHTML = svg;
  wireClicks(root, clickTargets, (name) => {
    if (expanded.has(name)) expanded.delete(name);
    else expanded.add(name);
    void render(root, payload, expanded);
  });
}

// #2093: which boxes the reader expanded, kept across live updates so a
// refresh redraws the map they were looking at (names that no longer exist
// simply match nothing).
const expanded = new Set<string>();
let mermaidReady = false;

async function renderPayload(root: HTMLElement, payload: ArchPayload | null): Promise<void> {
  if (!payload || payload.model.nodes.length === 0) {
    root.textContent = "No contexts found in this project.";
    return;
  }

  if (!mermaidReady) {
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      theme: "base",
      themeVariables: mermaidThemeVariables(),
      flowchart: { htmlLabels: false },
    });
    mermaidReady = true;
  }

  await render(root, payload, expanded);
}

startLiveView<ArchPayload>(vscodeApi, "__BYNK_ARCH_MODEL__", renderPayload);
