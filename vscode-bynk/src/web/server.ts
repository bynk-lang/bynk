// The web extension's language server (#2102): a web worker that answers LSP
// over postMessage, backed by `bynk-wasm` — the same `bynk_analyze` /
// `bynk_hover` / `bynk_complete` entry points the playground drives.
//
// DECISION A: a thin LSP adapter over the existing wasm functions, not
// `bynk-lsp` compiled to wasm. DECISION B: single-file analysis — each open
// document is analysed on its own (see `singleFileDiagnostics`). DECISION C: the
// wasm module is fetched lazily, on the first request that needs it, from the URL
// the client passes in `initializationOptions.wasmUrl`.

import {
  BrowserMessageReader,
  BrowserMessageWriter,
  createConnection,
  TextDocumentSyncKind,
  TextDocuments,
  type CompletionItem,
  type Diagnostic,
  type Hover,
  type InitializeResult,
} from "vscode-languageserver/browser";
import { TextDocument } from "vscode-languageserver-textdocument";

// Resolved by esbuild to the wasm-bindgen glue `npm run build:wasm` stages in
// `wasm/` (see src/web/bynk-wasm.d.ts and the `build:web` script).
import initWasm, { bynk_analyze, bynk_complete, bynk_hover } from "bynk-wasm";

import {
  hoverMarkdown,
  publishableDiagnostics,
  toLspCompletionItems,
  type WasmCompletion,
  type WasmDiagnostic,
} from "./convert";
import { LineIndex } from "./positions";
import { WASM_TRAPPED, type InitializationOptions } from "./protocol";
import { WasmCaller } from "./wasmCall";

/**
 * The platform the worker analyses for. `bynk-lsp` analyses a project for
 * `Platform::default()` (Cloudflare); the wasm default (`browser`) is the
 * playground's, and it would flag every `consumes bynk.cloudflare` as
 * `bynk.target.vendor_required`.
 */
const PLATFORM = "cloudflare";

/** Delay before re-analysing a document after an edit. */
const DEBOUNCE_MS = 200;

// The worker's own global scope. The project compiles against the DOM lib (not
// WebWorker, which conflicts with it), so name it by the reader's parameter type.
const scope = self as unknown as ConstructorParameters<typeof BrowserMessageReader>[0];

const connection = createConnection(new BrowserMessageReader(scope), new BrowserMessageWriter(scope));
const documents = new TextDocuments(TextDocument);

let wasmUrl: string | undefined;
let ready: Promise<void> | undefined;

/**
 * Fetch and instantiate the module on first use (DECISION C), so activation
 * costs nothing until a request needs the compiler. A failed load is retried
 * by the next request.
 */
function loadWasm(): Promise<void> {
  if (!ready) {
    const url = wasmUrl;
    if (!url) return Promise.reject(new Error("no wasmUrl in initializationOptions"));
    const loading = fetch(url)
      .then((r) => {
        if (!r.ok) throw new Error(`fetching ${url}: HTTP ${r.status}`);
        return r.arrayBuffer();
      })
      .then((bytes) => initWasm({ module_or_path: bytes }))
      .then(() => undefined);
    ready = loading;
    loading.catch(() => {
      if (ready === loading) ready = undefined;
    });
  }
  return ready;
}

/**
 * Calls into the module. A failure (a failed load, a throw, a trap) is
 * reported as such rather than as an empty answer. After the first trap the
 * instance is never called again, and the client is told to replace the worker.
 */
const wasm = new WasmCaller({
  load: loadWasm,
  onTrap: () => void connection.sendNotification(WASM_TRAPPED),
  log: (message) => connection.console.error(message),
  isTrap: (err) => err instanceof WebAssembly.RuntimeError,
});

/** Run one wasm call, returning `fallback` if it fails (hover, completion). */
async function callWasm<T>(fallback: T, call: () => string): Promise<T> {
  const outcome = await wasm.call<T>(call);
  return outcome.ok ? outcome.value : fallback;
}

connection.onInitialize((params): InitializeResult => {
  const options = params.initializationOptions as Partial<InitializationOptions> | undefined;
  wasmUrl = options?.wasmUrl;
  return {
    capabilities: {
      textDocumentSync: TextDocumentSyncKind.Full,
      hoverProvider: true,
      // The same trigger characters as bynk-lsp, so web and desktop agree.
      completionProvider: { triggerCharacters: [" ", "{", ",", "."] },
    },
    serverInfo: { name: "bynk-web (single-file)" },
  };
});

const pending = new Map<string, ReturnType<typeof setTimeout>>();

function schedule(doc: TextDocument): void {
  const prior = pending.get(doc.uri);
  if (prior) clearTimeout(prior);
  pending.set(
    doc.uri,
    setTimeout(() => {
      pending.delete(doc.uri);
      void validate(doc.uri);
    }, DEBOUNCE_MS),
  );
}

async function validate(uri: string): Promise<void> {
  const doc = documents.get(uri);
  if (!doc) return;
  const version = doc.version;
  const text = doc.getText();
  const outcome = await wasm.call<{ diagnostics: WasmDiagnostic[] }>(() =>
    bynk_analyze(text, PLATFORM),
  );
  // A newer edit (or a close) superseded this run.
  const current = documents.get(uri);
  if (!current || current.version !== version) return;
  // A failed analysis publishes nothing, so the last good diagnostics stay.
  const diagnostics = publishableDiagnostics(outcome, text, uri);
  if (diagnostics === undefined) return;
  await connection.sendDiagnostics({ uri, version, diagnostics: diagnostics as Diagnostic[] });
}

documents.onDidOpen((e) => schedule(e.document));
documents.onDidChangeContent((e) => schedule(e.document));
documents.onDidClose((e) => {
  const prior = pending.get(e.document.uri);
  if (prior) clearTimeout(prior);
  pending.delete(e.document.uri);
  void connection.sendDiagnostics({ uri: e.document.uri, diagnostics: [] });
});

connection.onHover(async (params): Promise<Hover | null> => {
  const doc = documents.get(params.textDocument.uri);
  if (!doc) return null;
  const text = doc.getText();
  const offset = new LineIndex(text).byteOffsetAt(params.position);
  const result = await callWasm<{ ty: string | null }>({ ty: null }, () =>
    bynk_hover(text, offset, PLATFORM),
  );
  const value = hoverMarkdown(result.ty);
  return value ? { contents: { kind: "markdown", value } } : null;
});

connection.onCompletion(async (params): Promise<CompletionItem[]> => {
  const doc = documents.get(params.textDocument.uri);
  if (!doc) return [];
  const text = doc.getText();
  const offset = new LineIndex(text).byteOffsetAt(params.position);
  const result = await callWasm<{ items: WasmCompletion[] }>({ items: [] }, () =>
    bynk_complete(text, offset, PLATFORM),
  );
  return toLspCompletionItems(result.items) as CompletionItem[];
});

documents.listen(connection);
connection.listen();
