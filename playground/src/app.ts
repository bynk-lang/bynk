// The Bynk playground app (in-browser track, slice 4). Runs on the **app origin**:
// edits Bynk, compiles it to JS in wasm (`bynk_compile`), shows diagnostics, and
// dispatches a successful compile to the cross-origin **sandbox** iframe to run.

import { Compartment, EditorState } from "@codemirror/state";
import {
  EditorView,
  keymap,
  lineNumbers,
  highlightActiveLine,
  hoverTooltip,
} from "@codemirror/view";
import type { Tooltip } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { linter, lintGutter, forceLinting } from "@codemirror/lint";
import type { Diagnostic as CmDiagnostic } from "@codemirror/lint";
import { autocompletion } from "@codemirror/autocomplete";
import type { CompletionContext, CompletionResult } from "@codemirror/autocomplete";
import { bynkHighlighting } from "./highlight";
import { bynkTreeSitterHighlighting } from "./tshighlight";
import { encodeSnippet, decodeSnippet } from "./deeplink";
import { SANDBOX_ORIGIN } from "./shared";
import type { CompileResult, Diagnostic, RunReply } from "./shared";
import { EXAMPLES } from "./examples";
import { byteToUtf16Offset, utf16ToByteOffset } from "./positions";
import init, { bynk_compile, bynk_analyze, bynk_hover, bynk_complete } from "./vendor/bynk_wasm.js";

// The default program when there is no shared snippet in the URL: the first gallery
// example (Hello, world).
const DEFAULT_SOURCE = EXAMPLES[0].source;

const RUN_TIMEOUT_MS = 3000;
const PLATFORM_LOCK = new Set(["bynk.target.vendor_required", "bynk.target.browser_bundle_only"]);

const $ = (id: string) => document.getElementById(id)!;

let view: EditorView;
// Highlighting starts on the synchronous stream highlighter and swaps to
// web-tree-sitter once its wasm has loaded (slice 5b).
const highlightCompartment = new Compartment();
let sandboxReady = false;
// The linter no-ops until the wasm compiler has loaded (slice 5d).
let wasmReady = false;

// A CodeMirror lint source: on each (debounced) change, ask the wasm analyser for
// diagnostics and render them inline (squiggles + gutter). Non-bailing, so type
// errors in a context show up live — not only on Run.
const bynkLinter = linter(
  (view): CmDiagnostic[] => {
    if (!wasmReady) return [];
    const text = view.state.doc.toString();
    let diags: Diagnostic[];
    try {
      diags = (JSON.parse(bynk_analyze(text)) as { diagnostics: Diagnostic[] }).diagnostics;
    } catch {
      return [];
    }
    // `from`/`to` are UTF-8 byte offsets; CodeMirror ranges are UTF-16 (#2126).
    // The conversion clamps to the document, so no further clamping is needed.
    return diags
      .map((d) => ({
        from: byteToUtf16Offset(text, d.from),
        to: byteToUtf16Offset(text, Math.max(d.to, d.from)),
        severity: d.severity === "error" ? ("error" as const) : ("warning" as const),
        message: d.message,
      }))
      .filter((d) => d.from <= d.to);
  },
  { delay: 300 },
);

// A CodeMirror hover tooltip: ask the wasm analyser for the inferred type at
// the cursor position and show it, if any (#397). `null` — no tooltip — when
// the wasm compiler isn't loaded yet, the position has no recorded expression,
// or the buffer doesn't currently check clean (the analyser's "clean-file
// ceiling", ADR 0063).
const bynkHover = hoverTooltip((view, pos): Tooltip | null => {
  if (!wasmReady) return null;
  let result: { ty: string | null };
  try {
    // `pos` is a UTF-16 offset; `bynk_hover` takes a UTF-8 byte offset (#2126).
    const text = view.state.doc.toString();
    result = JSON.parse(bynk_hover(text, utf16ToByteOffset(text, pos))) as { ty: string | null };
  } catch {
    return null;
  }
  if (!result.ty) return null;
  const ty = result.ty;
  return {
    pos,
    end: pos,
    above: true,
    create: () => {
      const dom = document.createElement("div");
      dom.className = "cm-tooltip-bynk-hover";
      dom.textContent = ty;
      return { dom };
    },
  };
});

interface CompletionCandidate {
  label: string;
  kind: string;
  detail: string | null;
  insert_text: string | null;
}

// `bynk_complete`'s `CompletionKind` strings mapped to CodeMirror's own
// completion `type`, which drives the icon in the completion list. `insert_text`
// is not used yet — it only carries LSP snippet syntax (`${1:label}`) for
// `Snippet` items, a different placeholder grammar from CodeMirror's own
// `snippet()` syntax, so every candidate inserts its bare label for now.
const KIND_TO_CM_TYPE: Record<string, string> = {
  unit: "namespace",
  capability: "interface",
  type: "type",
  keyword: "keyword",
  snippet: "text",
  variant: "enum",
  member: "method",
  field: "property",
  constructor: "function",
  function: "function",
  constant: "constant",
  local: "variable",
};

// A CodeMirror completion source: ask the wasm analyser for context-aware
// candidates (capability methods, types, keywords, in-scope locals, value-receiver
// members) at the cursor position (#808). `null` when the wasm compiler isn't
// loaded yet or there is nothing to offer.
const bynkCompletions = (context: CompletionContext): CompletionResult | null => {
  if (!wasmReady) return null;
  const word = context.matchBefore(/[\w.]*/);
  if (!word || (word.from === word.to && !context.explicit)) return null;
  let items: CompletionCandidate[];
  try {
    // `context.pos` is a UTF-16 offset; `bynk_complete` takes a UTF-8 byte
    // offset (#2126). The returned range is `word.from` — already UTF-16.
    const text = context.state.doc.toString();
    items = (
      JSON.parse(bynk_complete(text, utf16ToByteOffset(text, context.pos))) as { items: CompletionCandidate[] }
    ).items;
  } catch {
    return null;
  }
  if (items.length === 0) return null;
  return {
    from: word.from,
    options: items.map((c) => ({
      label: c.label,
      type: KIND_TO_CM_TYPE[c.kind] ?? "text",
      detail: c.detail ?? undefined,
      apply: c.label,
    })),
    validFor: /^[\w.]*$/,
  };
};

let runSeq = 0;
const pending = new Map<number, (r: RunReply) => void>();

function source(): string {
  return view.state.doc.toString();
}

function setStatus(text: string, kind: "idle" | "busy" | "ok" | "error" = "idle"): void {
  const el = $("status");
  el.textContent = text;
  el.dataset.kind = kind;
}

function renderDiagnostics(diags: Diagnostic[]): void {
  const panel = $("diagnostics");
  panel.innerHTML = "";
  if (diags.length === 0) {
    panel.classList.add("empty");
    return;
  }
  panel.classList.remove("empty");
  for (const d of diags) {
    const row = document.createElement("div");
    row.className = `diag diag-${d.severity}`;
    const loc = d.line ? `${d.line}:${d.col}` : "—";
    // `category` is a fixed compiler constant today; escape it too so this stays
    // injection-proof if a future diagnostic ever derives a category from source.
    row.innerHTML = `<span class="diag-loc">${loc}</span> <span class="diag-cat">${escapeHtml(d.category)}</span> ${escapeHtml(d.message)}`;
    panel.appendChild(row);
  }
}

function escapeHtml(s: string): string {
  return s.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c] as string);
}

function renderOutput(reply: RunReply): void {
  const out = $("output");
  out.innerHTML = "";
  const add = (cls: string, text: string) => {
    const line = document.createElement("div");
    line.className = cls;
    line.textContent = text;
    out.appendChild(line);
  };
  if (reply.timedOut) {
    add("out-error", `⏱ execution exceeded ${RUN_TIMEOUT_MS} ms and was terminated.`);
    return;
  }
  if (reply.noEntry) {
    add("out-note", "Compiled. No zero-argument service handler to run — add `on call() -> …` to see output.");
    return;
  }
  for (const log of reply.logs) {
    add(log.level === "error" ? "out-error" : "out-log", log.message);
  }
  if (reply.error) {
    add("out-error", reply.error);
  } else if (reply.value !== undefined) {
    add("out-value", `⇒ ${reply.value}`);
  }
}

function showUnsupported(diags: Diagnostic[]): void {
  const lock = diags.find((d) => PLATFORM_LOCK.has(d.category));
  const banner = $("unsupported");
  if (lock) {
    banner.textContent = `Not runnable in-browser: ${lock.message}`;
    banner.hidden = false;
  } else {
    banner.hidden = true;
  }
}

async function compileAndRun(): Promise<void> {
  setStatus("compiling…", "busy");
  $("output").innerHTML = "";
  let result: CompileResult;
  try {
    result = JSON.parse(bynk_compile(source())) as CompileResult;
  } catch (e) {
    setStatus("compiler error", "error");
    renderDiagnostics([
      { path: null, line: 0, col: 0, from: 0, to: 0, severity: "error", category: "bynk.wasm", message: String(e) },
    ]);
    return;
  }
  renderDiagnostics(result.diagnostics);
  showUnsupported(result.diagnostics);

  if (!result.ok) {
    setStatus(`${result.diagnostics.filter((d) => d.severity === "error").length} error(s)`, "error");
    return;
  }
  if (!sandboxReady) {
    setStatus("sandbox not ready", "error");
    return;
  }

  setStatus("running…", "busy");
  const id = ++runSeq;
  const reply = await new Promise<RunReply>((resolve) => {
    pending.set(id, resolve);
    const iframe = $("sandbox") as HTMLIFrameElement;
    iframe.contentWindow!.postMessage(
      { kind: "bynk-run", id, files: result.files, timeoutMs: RUN_TIMEOUT_MS },
      // Opaque sandbox origin can't be named; the sandbox validates by app origin.
      "*",
    );
  });
  renderOutput(reply);
  setStatus(reply.error || reply.timedOut ? "run failed" : "ran", reply.error || reply.timedOut ? "error" : "ok");
}

async function copyAndReport(url: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(url);
    setStatus("link copied", "ok");
  } catch {
    setStatus("link in address bar", "ok");
  }
}

async function share(): Promise<void> {
  const src = source();
  // Prefer a short `?s=<id>` link via the same-origin share service (slice 5c);
  // fall back to the self-contained `#hash` link if the service is unavailable.
  try {
    const res = await fetch("/api/snippets", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ source: src }),
    });
    if (res.ok) {
      const { id } = (await res.json()) as { id: string };
      const url = `${location.origin}${location.pathname}?s=${encodeURIComponent(id)}`;
      window.history.replaceState(null, "", url);
      await copyAndReport(url);
      return;
    }
  } catch {
    // network/Worker error — fall through to the hash form
  }
  const url = `${location.origin}${location.pathname}#${await encodeSnippet(src)}`;
  window.history.replaceState(null, "", url);
  await copyAndReport(url);
}

// The initial program: a `?s=<id>` shared snippet (via the service), else a `#hash`
// snippet, else the default example.
async function loadInitialSource(): Promise<string> {
  const id = new URLSearchParams(location.search).get("s");
  if (id) {
    try {
      const res = await fetch(`/api/snippets/${encodeURIComponent(id)}`);
      if (res.ok) return ((await res.json()) as { source: string }).source;
    } catch {
      // fall through to the hash / default
    }
  }
  return (await decodeSnippet(location.hash)) ?? DEFAULT_SOURCE;
}

function setEditorContent(src: string): void {
  view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: src } });
  view.focus();
}

function mountExamples(): void {
  const sel = $("examples") as HTMLSelectElement;
  for (const ex of EXAMPLES) {
    const opt = document.createElement("option");
    opt.value = ex.id;
    opt.textContent = ex.title;
    sel.appendChild(opt);
  }
  sel.addEventListener("change", () => {
    const ex = EXAMPLES.find((e) => e.id === sel.value);
    if (ex) {
      setEditorContent(ex.source);
      // The chosen example drops any shared-snippet URL (?s= / #hash) so a reshare
      // is clean. (`window.history`; the bare `history` is CodeMirror's editor history.)
      window.history.replaceState(null, "", location.pathname);
    }
  });
}

function makeEditor(doc: string): void {
  view = new EditorView({
    parent: $("editor"),
    state: EditorState.create({
      doc,
      extensions: [
        lineNumbers(),
        highlightActiveLine(),
        history(),
        keymap.of([
          { key: "Mod-Enter", run: () => (void compileAndRun(), true) },
          ...defaultKeymap,
          ...historyKeymap,
        ]),
        highlightCompartment.of(bynkHighlighting()),
        bynkLinter,
        lintGutter(),
        bynkHover,
        autocompletion({ override: [bynkCompletions] }),
        EditorView.theme({ "&": { height: "100%" }, ".cm-scroller": { overflow: "auto" } }, { dark: true }),
      ],
    }),
  });
}

function mountSandbox(): void {
  const iframe = document.createElement("iframe");
  iframe.id = "sandbox";
  // Distinct origin + opaque sandbox: even a sandbox escape lands on a bare origin.
  iframe.setAttribute("sandbox", "allow-scripts");
  iframe.src = `${SANDBOX_ORIGIN}/sandbox.html`;
  iframe.hidden = true;
  document.body.appendChild(iframe);

  window.addEventListener("message", (e: MessageEvent) => {
    const iframeWin = iframe.contentWindow;
    // Replies come from the opaque-origin iframe; identify it by window, not origin.
    if (e.source !== iframeWin) return;
    const data = e.data as { kind?: string; id?: number };
    if (data?.kind === "bynk-sandbox-ready") {
      sandboxReady = true;
      setStatus("ready", "ok");
      return;
    }
    if (data?.kind === "bynk-result" && typeof data.id === "number") {
      const resolve = pending.get(data.id);
      if (resolve) {
        pending.delete(data.id);
        resolve(data as RunReply);
      }
    }
  });
}

async function main(): Promise<void> {
  setStatus("loading…", "busy");
  mountSandbox();
  makeEditor(await loadInitialSource());
  mountExamples();
  $("run").addEventListener("click", () => void compileAndRun());
  $("share").addEventListener("click", () => void share());
  // Load the wasm compiler. Resolve the module against the page URL (not
  // `import.meta.url`) so esbuild leaves the path alone and the `.wasm` is fetched
  // from the deploy root.
  await init(new URL("bynk_wasm_bg.wasm", location.href));
  wasmReady = true;
  if (!sandboxReady) setStatus("compiler ready", "ok");
  // Lint the initial program now that the analyser is loaded.
  forceLinting(view);

  // Upgrade highlighting to web-tree-sitter once its wasm loads; on failure the
  // stream highlighter stays (slice 5b).
  const ts = await bynkTreeSitterHighlighting();
  if (ts) view.dispatch({ effects: highlightCompartment.reconfigure(ts) });
}

void main();
