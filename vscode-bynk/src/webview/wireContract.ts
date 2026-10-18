// #855: the wire-contract peek's own bundle entry (`out/wireContract.js`, a
// fifth esbuild target alongside `out/webview.js` (#846), `out/docview.js`
// (#847) and `out/architecture.js` (#851) — see `package.json`'s `build`
// script). Runs inside the webview's browser context, never the extension
// host: only `acquireVsCodeApi()`, no `vscode` module.
//
// Reads the `WcPayload` the extension host embeds as
// `window.__BYNK_WIRE_CONTRACT__` and hands it to `wire-render.ts`'s pure
// DOM-tree renderer, wiring its `onReveal` callback to `postMessage`
// click-to-code — the same hydration posture `sequenceDiagram.ts`'s
// `showPanel` message handler expects on the other end.

import { renderWireContract } from "./wire-render";
import { startLiveView, type VsCodeApi } from "./live";
import type { Range, WcPayload, WcRevealMessage } from "./wireContractTypes";

declare function acquireVsCodeApi(): VsCodeApi;

const vscodeApi = acquireVsCodeApi();

function reveal(uri: string, range: Range): void {
  const message: WcRevealMessage = { type: "reveal", uri, range };
  vscodeApi.postMessage(message);
}

function render(root: HTMLElement, payload: WcPayload | null): void {
  if (!payload) {
    root.textContent = "No wire contract to render.";
    return;
  }
  renderWireContract(root, payload, reveal);
}

startLiveView<WcPayload>(vscodeApi, "__BYNK_WIRE_CONTRACT__", render);
