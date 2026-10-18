// #2098: the "Bynk: Explanation" webview's bundle entry (`out/explainview.js`,
// a separate esbuild target, like `docview.js`). Runs in the webview's browser
// context: no `vscode` module, only `acquireVsCodeApi()`. Renders the payload
// the host embeds as `window.__BYNK_EXPLAIN__` and routes link clicks to the
// host, which opens only http(s) URLs.

import { createMarkdownRenderer, isExternalHttpLink } from "./doc-render";
import { renderExplanationHtml } from "./explain-render";
import type { ExplainPayload } from "./explainTypes";

declare function acquireVsCodeApi(): {
  postMessage(message: unknown): void;
};

const vscodeApi = acquireVsCodeApi();

function main(): void {
  const root = document.getElementById("root");
  const payload = (window as unknown as { __BYNK_EXPLAIN__?: ExplainPayload }).__BYNK_EXPLAIN__;
  if (!root) return;
  if (!payload) {
    root.textContent = "No explanation to render.";
    return;
  }
  root.innerHTML = renderExplanationHtml(createMarkdownRenderer(), payload.explanation);
  for (const a of Array.from(root.querySelectorAll("a"))) {
    const href = a.getAttribute("href") ?? "";
    a.addEventListener("click", (ev) => {
      ev.preventDefault();
      if (isExternalHttpLink(href)) vscodeApi.postMessage({ type: "openExternal", href });
    });
  }
}

main();
