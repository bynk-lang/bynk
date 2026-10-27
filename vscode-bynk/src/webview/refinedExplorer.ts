// #2470: the "Refined Type" explorer's bundle entry (`out/refinedExplorer.js`,
// a separate esbuild target). Runs in the webview's browser context: no
// `vscode` module, only `acquireVsCodeApi()`. Renders the model the host embeds
// as `window.__BYNK_REFINED__`, and asks the host to check each candidate (the
// predicates run there, not here). Built with DOM calls and `textContent` only —
// a candidate or a pattern is data, never markup.

import type { CandidateVerdict, RefinedPayload, VerdictMessage } from "./refinedTypes";

declare function acquireVsCodeApi(): { postMessage(message: unknown): void };

const vscodeApi = acquireVsCodeApi();

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** How a candidate text is shown in a chip: a string is quoted. */
function show(base: string, text: string): string {
  return base === "String" ? JSON.stringify(text) : text;
}

function renderVerdict(into: HTMLElement, verdict: CandidateVerdict): void {
  into.replaceChildren();
  into.className = "rx-verdict";
  switch (verdict.kind) {
    case "valid":
      into.classList.add("rx-ok");
      into.append(el("strong", undefined, "✓ valid"));
      return;
    case "unreadable":
      into.classList.add("rx-bad");
      into.append(el("strong", undefined, "✗ "), document.createTextNode(verdict.message));
      return;
    case "timeout":
      into.classList.add("rx-bad");
      into.append(el("strong", undefined, "✗ "), document.createTextNode("Checking took too long and was stopped."));
      return;
    case "invalid": {
      into.classList.add("rx-bad");
      into.append(el("strong", undefined, "✗ invalid"));
      const list = el("ul");
      for (const f of verdict.failures) {
        const li = el("li");
        li.append(el("code", undefined, f.source), document.createTextNode(` — ${f.message}`));
        list.append(li);
      }
      into.append(list);
    }
  }
}

function main(): void {
  const root = document.getElementById("root");
  const payload = (window as unknown as { __BYNK_REFINED__?: RefinedPayload }).__BYNK_REFINED__;
  if (!root) return;
  if (!payload) {
    root.textContent = "No refined type to explore.";
    return;
  }
  const { model } = payload;
  root.replaceChildren();

  const title = el("h1");
  title.append(el("code", undefined, model.name), el("span", "rx-kind", model.opaque ? " opaque refined type" : " refined type"));
  root.append(title, el("pre", "rx-decl", model.declaration));

  root.append(el("h2", undefined, "A value is valid when"));
  const clauses = el("ul", "rx-clauses");
  for (const c of model.clauses) {
    const li = el("li");
    li.append(el("code", undefined, c.source), document.createTextNode(` — ${c.description}`));
    clauses.append(li);
  }
  root.append(clauses);

  const input = el("input");
  input.type = "text";
  input.id = "rx-input";
  input.placeholder = model.base === "String" ? "Type a string…" : `Type ${model.base === "Int" ? "an Int" : "a Float"}…`;
  input.spellcheck = false;
  input.setAttribute("aria-label", `Candidate ${model.name} value`);
  const verdict = el("div", "rx-verdict");
  verdict.setAttribute("role", "status");
  verdict.setAttribute("aria-live", "polite");

  let pending = "";
  const check = (text: string): void => {
    pending = text;
    vscodeApi.postMessage({ type: "check", text });
  };

  root.append(el("h2", undefined, "Examples"));
  if (model.examples.length > 0) {
    const chips = el("div", "rx-chips");
    for (const ex of model.examples) {
      const chip = el("button", "rx-chip", show(model.base, ex));
      chip.type = "button";
      chip.addEventListener("click", () => {
        input.value = ex;
        check(ex);
      });
      chips.append(chip);
    }
    root.append(chips);
  }
  if (model.examplesNote) root.append(el("p", "rx-note", model.examplesNote));
  root.append(el("p", "rx-note", `Chosen with a fixed seed, so this list is the same each time you open it.`));

  root.append(el("h2", undefined, "Try a value"));
  root.append(input, verdict);
  input.addEventListener("input", () => check(input.value));

  window.addEventListener("message", (ev: MessageEvent<VerdictMessage>) => {
    const msg = ev.data;
    // A late answer to an older keystroke must not overwrite a newer one.
    if (msg?.type === "verdict" && msg.text === pending) renderVerdict(verdict, msg.verdict);
  });
}

main();
