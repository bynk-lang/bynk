// #847: the documentation webview's bundle entry (`out/docview.js`, a separate
// esbuild target from the extension host's `out/extension.js` and #846's
// `out/webview.js` — see `package.json`'s `build` script). Runs inside the
// webview's browser context: no `vscode` module, only `acquireVsCodeApi()`.
//
// Reads the `DocPayload` the host embeds as `window.__BYNK_DOC_MODEL__` and
// renders it as a reference page — for each declaration a heading, its rendered
// signature+doc Markdown, and (Decision B) a "no documentation" placeholder for
// the undocumented, with a toggle to hide them for a clean reading page. Unlike
// #846's sequence view, the DOM is built here element-by-element, so each
// heading/signature holds a direct reference to its `{uri, range}` — no
// fragile DOM-order zip against a rendered SVG. Doc Markdown is rendered
// HTML-disabled (`doc-render`); links are gated through the host's http(s)
// allow-list.

import {
  createMarkdownRenderer,
  docLinkIndex,
  headingTag,
  isExternalHttpLink,
  renderDocMarkdown,
} from "./doc-render";
import type { DocEntry, DocLink, DocPayload, Range } from "./docTypes";
import { startLiveView, type VsCodeApi } from "./live";

declare function acquireVsCodeApi(): VsCodeApi;

const vscodeApi = acquireVsCodeApi();

function reveal(uri: string, range: Range): void {
  vscodeApi.postMessage({ type: "reveal", uri, range });
}

function makeClickable(el: HTMLElement, uri: string, range: Range): void {
  el.style.cursor = "pointer";
  el.addEventListener("click", () => reveal(uri, range));
}

/** Scroll to entry `index`'s section, un-hiding it first if the reader has
 *  hidden undocumented entries. */
function goToEntry(index: number): void {
  const target = document.getElementById(`entry-${index}`);
  if (!target) return;
  const root = document.getElementById("root");
  if (root && target.dataset.documented === "false" && root.dataset.hideUndocumented === "true") {
    root.dataset.hideUndocumented = "false";
    const box = root.querySelector<HTMLInputElement>(".toggle input");
    if (box) box.checked = false;
  }
  target.scrollIntoView({ block: "start" });
}

/** Route a resolved intra-doc link (#2462): the server decided where it goes. */
function followLink(link: DocLink): void {
  switch (link.kind) {
    case "entry":
      goToEntry(link.entry);
      break;
    case "source":
      reveal(link.uri, link.range);
      break;
    case "external":
      if (isExternalHttpLink(link.href)) {
        vscodeApi.postMessage({ type: "openExternal", href: link.href });
      }
      break;
  }
}

/** Wire every `<a>` in a rendered doc block: a server-resolved intra-doc link
 *  (`links`, #2462) is routed by its resolved target; an http(s) link asks the
 *  host to open it externally (the host re-checks the scheme and shows its own
 *  trust prompt); anything else is inert. */
function wireDocLinks(container: HTMLElement, links: DocLink[]): void {
  for (const a of Array.from(container.querySelectorAll("a"))) {
    const href = a.getAttribute("href") ?? "";
    const resolved = docLinkIndex(href);
    const link = resolved === null ? undefined : links[resolved];
    a.addEventListener("click", (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      if (link) {
        followLink(link);
      } else if (isExternalHttpLink(href)) {
        vscodeApi.postMessage({ type: "openExternal", href });
      }
    });
  }
}

function renderEntry(
  md: ReturnType<typeof createMarkdownRenderer>,
  entry: DocEntry,
  index: number,
): HTMLElement {
  const section = document.createElement("section");
  section.className = "entry";
  section.id = `entry-${index}`;
  section.dataset.documented = String(entry.documented);
  section.style.marginLeft = `${entry.depth * 16}px`;

  const heading = document.createElement(headingTag(entry.depth));
  const badge = document.createElement("span");
  badge.className = "kind";
  badge.textContent = entry.kind;
  const name = document.createElement("span");
  name.className = "name";
  name.textContent = entry.name;
  heading.append(badge, name);
  // #1885: the page merges a unit's files, so each entry names the file
  // its range is in, which may not be the requested document.
  const entryUri = entry.uri;
  makeClickable(heading, entryUri, entry.range);
  section.append(heading);

  // The entry Markdown is a fenced `bynk` signature plus (when documented) the
  // doc prose — rendered HTML-disabled.
  const body = document.createElement("div");
  body.className = "body";
  body.innerHTML = renderDocMarkdown(md, entry.markdown);
  wireDocLinks(body, entry.links);
  // The signature code block is also a click-to-code target (issue: "headings
  // and signatures are clickable").
  const sig = body.querySelector("pre");
  if (sig instanceof HTMLElement) makeClickable(sig, entryUri, entry.range);
  section.append(body);

  if (!entry.documented) {
    const note = document.createElement("p");
    note.className = "no-doc";
    note.textContent = "No documentation";
    section.append(note);
  }
  return section;
}

function render(root: HTMLElement, payload: DocPayload | null): void {
  if (!payload) {
    root.textContent = "No documentation to render.";
    return;
  }
  const { model, uri } = payload;
  const md = createMarkdownRenderer();
  root.textContent = "";

  // Page header: the unit's kind + name (click-to-code to the header), an
  // undocumented-toggle, then the unit's own doc.
  const header = document.createElement("header");
  const title = document.createElement("h1");
  const kind = document.createElement("span");
  kind.className = "kind";
  kind.textContent = model.unitKind;
  const uname = document.createElement("span");
  uname.className = "name";
  uname.textContent = model.unitName;
  title.append(kind, uname);
  makeClickable(title, uri, model.unitRange);
  header.append(title);

  const toggleLabel = document.createElement("label");
  toggleLabel.className = "toggle";
  const toggle = document.createElement("input");
  toggle.type = "checkbox";
  // #2093: a live update re-renders into the same root, whose data attribute
  // still holds the reader's choice — keep the checkbox in step with it.
  toggle.checked = root.dataset.hideUndocumented === "true";
  toggle.addEventListener("change", () => {
    root.dataset.hideUndocumented = String(toggle.checked);
  });
  toggleLabel.append(toggle, document.createTextNode(" Hide undocumented"));
  header.append(toggleLabel);

  if (model.unitDoc) {
    const lede = document.createElement("div");
    lede.className = "body";
    lede.innerHTML = renderDocMarkdown(md, model.unitDoc);
    wireDocLinks(lede, model.unitLinks);
    header.append(lede);
  }
  root.append(header);

  if (model.entries.length === 0) {
    const empty = document.createElement("p");
    empty.className = "no-doc";
    empty.textContent = "This unit declares nothing to document.";
    root.append(empty);
    return;
  }

  model.entries.forEach((entry, i) => root.append(renderEntry(md, entry, i)));
}

startLiveView<DocPayload>(vscodeApi, "__BYNK_DOC_MODEL__", render);
