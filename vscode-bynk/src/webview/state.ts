// #2196: the agent state view's bundle entry (`out/state.js`). Runs in the
// webview's browser context.
//
// Renders the `AgentStatePayload` the host embeds as
// `window.__BYNK_AGENT_STATE__`: for the selected state field, a Mermaid
// `stateDiagram-v2` (`state-gen.ts`) and the handler x state table, which is
// the exact record (track §3.4). Everything links to source: table cells to
// their edge, handler names to the handler, diagram states and transitions
// through `state-map.ts`. The host reports the handler under the cursor as
// the view's `focus`; its row and transitions are highlighted without
// re-requesting the model.

import mermaid from "mermaid";

import { startLiveView, type ViewState, type VsCodeApi } from "./live";
import { cellEdges, cellText, toMermaidState, type TransitionGroup } from "./state-gen";
import { stateNode, transitionLabels } from "./state-map";
import type { AgentStatePayload, StateMachine } from "./stateTypes";
import type { Range, RevealMessage } from "./types";

declare function acquireVsCodeApi(): VsCodeApi;

const vscodeApi = acquireVsCodeApi();

// Kept across live updates (not across a reload): which field is shown,
// whether unchanged outcomes are drawn, and the handler under the cursor.
let selectedField: string | undefined;
let showUnchanged = false;
let focus: string | undefined;
let mermaidReady = false;
let renderSeq = 0;
let current: AgentStatePayload | null = null;

function reveal(uri: string, range: Range): void {
  const message: RevealMessage = { type: "reveal", uri, range };
  vscodeApi.postMessage(message);
}

function clickable(el: Element, uri: string, range: Range): void {
  (el as HTMLElement).style.cursor = "pointer";
  el.addEventListener("click", (ev) => {
    ev.stopPropagation();
    reveal(uri, range);
  });
}

function vscodeCssVar(name: string, fallback: string): string {
  const value = getComputedStyle(document.body).getPropertyValue(name).trim();
  return value || fallback;
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
    labelBackgroundColor: editorBg,
    noteBkgColor: editorBg,
    noteTextColor: editorFg,
    noteBorderColor: panelBorder,
  };
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, text?: string, cls?: string): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (text !== undefined) e.textContent = text;
  if (cls) e.className = cls;
  return e;
}

function renderEmpty(root: HTMLElement, payload: AgentStatePayload): void {
  const { model, uri } = payload;
  const p = el("p");
  p.append("No state field: no ");
  p.append(el("code", "store"));
  p.append(" Cell of ");
  p.append(el("strong", model.agent));
  p.append(" holds an enum or sum type, so it has no state machine to draw.");
  root.append(p);
  if (model.handlers.length > 0) {
    const list = el("ul");
    for (const h of model.handlers) {
      const item = el("li");
      const link = el("a", `${h.name}()`);
      clickable(link, uri, h.range);
      item.append(link);
      list.append(item);
    }
    root.append(el("p", "Its handlers:"), list);
  }
}

function controls(root: HTMLElement, payload: AgentStatePayload, machine: StateMachine): void {
  const bar = el("div", undefined, "controls");
  const title = el("h2");
  const agent = el("a", payload.model.agent);
  clickable(agent, payload.uri, payload.model.range);
  title.append(agent, document.createTextNode("."));
  if (payload.model.machines.length > 1) {
    const select = el("select");
    select.id = "bynk-state-field";
    select.setAttribute("aria-label", "State field");
    for (const m of payload.model.machines) {
      const opt = el("option", m.field);
      opt.value = m.field;
      opt.selected = m.field === machine.field;
      select.append(opt);
    }
    select.addEventListener("change", () => {
      selectedField = select.value;
      void renderPayload(root, current);
    });
    title.append(select);
  } else {
    const field = el("a", machine.field);
    clickable(field, payload.uri, machine.fieldRange);
    title.append(field);
  }
  bar.append(title);

  const label = el("label");
  const box = el("input");
  box.type = "checkbox";
  box.id = "bynk-state-unchanged";
  box.checked = showUnchanged;
  box.addEventListener("change", () => {
    showUnchanged = box.checked;
    void renderPayload(root, current);
  });
  label.append(box, document.createTextNode(" Show unchanged outcomes in the diagram"));
  bar.append(label);
  root.append(bar);
}

async function diagram(root: HTMLElement, payload: AgentStatePayload, machine: StateMachine): Promise<HTMLElement[]> {
  const holder = el("div", undefined, "diagram");
  root.append(holder);
  const { text, edgeOrder, stateIds } = toMermaidState(machine, { showUnchanged });
  let svg: string;
  try {
    ({ svg } = await mermaid.render(`bynk-agent-state-${renderSeq++}`, text));
  } catch (err) {
    holder.textContent = `Failed to render diagram: ${String(err)}`;
    return [];
  }
  holder.innerHTML = svg;

  // States link to the field's declaration (a state has no single source
  // location of its own); `?` stands for computed writes and links nowhere.
  const nodes = [...holder.querySelectorAll("g.node")];
  for (const [id, name] of stateIds) {
    const node = stateNode(nodes, id);
    if (node && name !== "?") clickable(node, payload.uri, machine.fieldRange);
  }

  // Transitions link to their first edge's write.
  const labels = transitionLabels([...holder.querySelectorAll(".edgeLabel .label[data-id]")]);
  const marked: HTMLElement[] = [];
  edgeOrder.forEach((group: TransitionGroup, i) => {
    const label = labels[i] as HTMLElement | undefined;
    if (!label) return;
    clickable(label, payload.uri, group.edges[0].range);
    label.dataset.handlers = group.handlers.join(" ");
    label.title = group.edges
      .map((e) => [e.handler + "()", ...e.guards.map((g) => `when ${g}`)].join(" "))
      .join("\n");
    marked.push(label);
  });
  return marked;
}

function table(root: HTMLElement, payload: AgentStatePayload, machine: StateMachine): void {
  const t = el("table", undefined, "states");
  const head = el("tr");
  head.append(el("th", "Handler \\ state"));
  for (const s of machine.states) {
    const th = el("th", s);
    if (!machine.reachable.includes(s)) {
      th.classList.add("unreachable");
      th.title = "Unreachable from the initial state";
    }
    if (s === machine.initial) th.title = "Initial state";
    head.append(th);
  }
  t.append(el("thead"));
  t.tHead?.append(head);
  const body = el("tbody");
  for (const h of payload.model.handlers) {
    const row = el("tr");
    row.dataset.handler = h.name;
    const name = el("th");
    const link = el("a", `${h.name}()`);
    clickable(link, payload.uri, h.range);
    name.append(link);
    row.append(name);
    const truncated = machine.truncated.includes(h.name);
    for (const s of machine.states) {
      const cell = el("td");
      if (truncated) {
        cell.textContent = "too many paths";
        cell.classList.add("muted");
      } else {
        const edges = cellEdges(machine, h.name, s);
        if (edges.length === 0) {
          cell.textContent = "—";
          cell.classList.add("muted");
          cell.title = "No path of this handler starts in this state";
        }
        for (const e of edges) {
          const entry = el("div", cellText(e), "entry");
          if (e.to.kind !== "Unchanged") entry.classList.add("moves");
          if (e.guards.length > 0) entry.title = e.guards.map((g) => `when ${g}`).join("\n");
          clickable(entry, payload.uri, e.range);
          cell.append(entry);
        }
      }
      row.append(cell);
    }
    body.append(row);
  }
  t.append(body);
  root.append(t);
}

let transitions: HTMLElement[] = [];

function applyFocus(root: HTMLElement): void {
  for (const row of root.querySelectorAll<HTMLElement>("tr[data-handler]")) {
    row.classList.toggle("focus", row.dataset.handler === focus);
  }
  for (const label of transitions) {
    const hs = (label.dataset.handlers ?? "").split(" ");
    label.classList.toggle("focus", focus !== undefined && hs.includes(focus));
  }
}

async function renderPayload(root: HTMLElement, payload: AgentStatePayload | null): Promise<void> {
  current = payload;
  root.textContent = "";
  if (!payload) {
    root.textContent = "No agent at the cursor.";
    return;
  }
  const machines = payload.model.machines;
  if (machines.length === 0) {
    renderEmpty(root, payload);
    return;
  }
  const machine = machines.find((m) => m.field === selectedField) ?? machines[0];
  selectedField = machine.field;

  if (!mermaidReady) {
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      theme: "base",
      themeVariables: mermaidThemeVariables(),
    });
    mermaidReady = true;
  }

  controls(root, payload, machine);
  transitions = await diagram(root, payload, machine);
  table(root, payload, machine);
  applyFocus(root);
}

startLiveView<AgentStatePayload>(vscodeApi, "__BYNK_AGENT_STATE__", renderPayload, (view: ViewState) => {
  focus = view.focus;
  const root = document.getElementById("root");
  if (root) applyFocus(root);
});
