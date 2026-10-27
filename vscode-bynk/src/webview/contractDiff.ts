// #2467: the contract-diff view's bundle entry (`out/contractDiff.js`). Runs in
// the webview's browser context: only `acquireVsCodeApi()`, no `vscode` module.
// Reads the `CdPayload` the host embeds as `window.__BYNK_CONTRACT_DIFF__`.

import { renderContractDiff } from "./contract-diff-render";
import type { CdPayload, CdRevealMessage, Range } from "./contractDiffTypes";
import { startLiveView, type VsCodeApi } from "./live";

declare function acquireVsCodeApi(): VsCodeApi;

const vscodeApi = acquireVsCodeApi();

function reveal(uri: string, range: Range): void {
  const message: CdRevealMessage = { type: "reveal", uri, range };
  vscodeApi.postMessage(message);
}

function render(root: HTMLElement, payload: CdPayload | null): void {
  if (!payload) {
    root.textContent = "No contract diff to show.";
    return;
  }
  renderContractDiff(root, payload, reveal);
}

startLiveView<CdPayload>(vscodeApi, "__BYNK_CONTRACT_DIFF__", render);
