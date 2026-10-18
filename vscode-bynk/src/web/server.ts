// The web extension's language server (#2102): a web worker that answers LSP
// over postMessage, backed by `bynk-wasm`, the compiler the playground runs.
//
// DECISION A: a thin LSP adapter over the wasm entry points, not `bynk-lsp`
// compiled to wasm. DECISION B: a file inside a Bynk project (a workspace
// folder with `bynk.toml`, sent by the client as `bynk/projects`) is analysed
// with the whole project (`bynk_*_project`, slice 2). A file outside any project
// is analysed on its own (`bynk_analyze`/`bynk_hover`/`bynk_complete`), with the
// single-file filter (slice 1). DECISION C: the wasm module is fetched lazily, on
// the first request that needs it, from `initializationOptions.wasmUrl`.

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
import initWasm, {
  bynk_analyze,
  bynk_analyze_project,
  bynk_complete,
  bynk_complete_project,
  bynk_hover,
  bynk_hover_project,
} from "bynk-wasm";

import {
  hoverMarkdown,
  publishableDiagnostics,
  toLspCompletionItems,
  toLspDiagnostics,
  type WasmCompletion,
  type WasmDiagnostic,
} from "./convert";
import { LineIndex } from "./positions";
import { diagnosticsByPath, Project, projectFor } from "./project";
import {
  PROJECT_FILE,
  PROJECTS,
  WASM_TRAPPED,
  type InitializationOptions,
  type ProjectFileParams,
  type ProjectsParams,
} from "./protocol";
import { WasmCaller } from "./wasmCall";

/**
 * The platform the worker analyses for. `bynk-lsp` analyses a project for
 * `Platform::default()` (Cloudflare); the wasm default (`browser`) is the
 * playground's, and it would flag every `consumes bynk.cloudflare` as
 * `bynk.target.vendor_required`.
 */
const PLATFORM = "cloudflare";

/** Delay before re-analysing after an edit. */
const DEBOUNCE_MS = 200;

// The worker's own global scope. The project compiles against the DOM lib (not
// WebWorker, which conflicts with it), so name it by the reader's parameter type.
const scope = self as unknown as ConstructorParameters<typeof BrowserMessageReader>[0];

const connection = createConnection(new BrowserMessageReader(scope), new BrowserMessageWriter(scope));
const documents = new TextDocuments(TextDocument);

let wasmUrl: string | undefined;
let ready: Promise<void> | undefined;

/** The workspace's projects, by root URI. */
const projects = new Map<string, Project>();
/** The URIs each project last published diagnostics for, to clear stale ones. */
const published = new Map<string, Set<string>>();

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
    serverInfo: { name: "bynk-web" },
  };
});

const pending = new Map<string, ReturnType<typeof setTimeout>>();

/** Debounce `run` under `key` (a document URI, or a project root). */
function debounce(key: string, run: () => Promise<void>): void {
  const prior = pending.get(key);
  if (prior) clearTimeout(prior);
  pending.set(
    key,
    setTimeout(() => {
      pending.delete(key);
      void run();
    }, DEBOUNCE_MS),
  );
}

/** The project warnings last shown, by root, so each set is shown once. */
const shownWarnings = new Map<string, string>();

/**
 * Show a project's warnings (an ignored absolute `[paths]` entry) in the
 * output channel and as a notification, once per distinct set.
 */
function warnOnce(root: string, warnings: string[]): void {
  const key = warnings.join("\n");
  if (shownWarnings.get(root) === key) return;
  shownWarnings.set(root, key);
  for (const w of warnings) {
    connection.console.warn(w);
    void connection.window.showWarningMessage(`Bynk (web): ${w}`);
  }
}

/** Re-analyse whatever `uri` belongs to: its project, or the file alone. */
function schedule(uri: string): void {
  const project = projectFor(projects.values(), uri);
  if (project) debounce(project.root, () => validateProject(project));
  else debounce(uri, () => validateFile(uri));
}

function openDocuments(): { uri: string; text: string }[] {
  return documents.all().map((d) => ({ uri: d.uri, text: d.getText() }));
}

/** Analyse a file outside any project on its own (slice 1). */
async function validateFile(uri: string): Promise<void> {
  const doc = documents.get(uri);
  if (!doc) return;
  const version = doc.version;
  const text = doc.getText();
  const outcome = await wasm.call<{ diagnostics: WasmDiagnostic[] }>(() =>
    bynk_analyze(text, PLATFORM),
  );
  // A newer edit (or a close, or the file joining a project) superseded this run.
  const current = documents.get(uri);
  if (!current || current.version !== version || projectFor(projects.values(), uri)) return;
  // A failed analysis publishes nothing, so the last good diagnostics stay.
  const diagnostics = publishableDiagnostics(outcome, text, uri);
  if (diagnostics === undefined) return;
  await connection.sendDiagnostics({ uri, version, diagnostics: diagnostics as Diagnostic[] });
}

/**
 * Analyse a whole project and publish every file's diagnostics, open or not,
 * clearing files that had diagnostics last time and have none now.
 */
async function validateProject(project: Project): Promise<void> {
  const { files, uris } = project.sources(openDocuments());
  const json = JSON.stringify(files);
  const outcome = await wasm.call<{ diagnostics: WasmDiagnostic[]; warnings?: string[] }>(() =>
    bynk_analyze_project(json, PLATFORM),
  );
  // The project was replaced (a `bynk/projects` resend) while this ran, or the
  // analysis failed: publish nothing, so the last good diagnostics stay.
  if (projects.get(project.root) !== project || !outcome.ok) return;
  warnOnce(project.root, outcome.value.warnings ?? []);
  const byPath = diagnosticsByPath(outcome.value.diagnostics);
  const now = new Set<string>();
  for (const [path, diags] of byPath) {
    const uri = uris.get(path);
    const text = files[path];
    if (uri === undefined || text === undefined) continue;
    now.add(uri);
    await connection.sendDiagnostics({ uri, diagnostics: toLspDiagnostics(text, diags) as Diagnostic[] });
  }
  // Clear what this project published last time and no longer reports, and any
  // open file's single-file diagnostics from before the project arrived.
  const stale = new Set([
    ...(published.get(project.root) ?? []),
    ...documents.all().map((d) => d.uri).filter((uri) => project.contains(uri)),
  ]);
  for (const uri of stale) {
    if (!now.has(uri)) await connection.sendDiagnostics({ uri, diagnostics: [] });
  }
  published.set(project.root, now);
}

connection.onNotification(PROJECTS, (params: ProjectsParams) => {
  // Clear what the old projects published; the new ones re-publish below.
  for (const [root, uris] of published) {
    if (!params.projects.some((p) => p.root.replace(/\/+$/, "") === root)) {
      for (const uri of uris) void connection.sendDiagnostics({ uri, diagnostics: [] });
      published.delete(root);
    }
  }
  projects.clear();
  for (const snapshot of params.projects) {
    const project = new Project(snapshot);
    projects.set(project.root, project);
    debounce(project.root, () => validateProject(project));
  }
  // Files outside every project go back to (or stay in) single-file mode.
  for (const doc of documents.all()) {
    if (!projectFor(projects.values(), doc.uri)) schedule(doc.uri);
  }
});

connection.onNotification(PROJECT_FILE, (params: ProjectFileParams) => {
  const project = projects.get(params.root.replace(/\/+$/, ""));
  if (!project) return;
  project.update(params);
  debounce(project.root, () => validateProject(project));
});

documents.onDidOpen((e) => schedule(e.document.uri));
documents.onDidChangeContent((e) => schedule(e.document.uri));
documents.onDidClose((e) => {
  const uri = e.document.uri;
  const project = projectFor(projects.values(), uri);
  if (project) {
    // Its saved text is the project's again.
    debounce(project.root, () => validateProject(project));
    return;
  }
  const prior = pending.get(uri);
  if (prior) clearTimeout(prior);
  pending.delete(uri);
  void connection.sendDiagnostics({ uri, diagnostics: [] });
});

connection.onHover(async (params): Promise<Hover | null> => {
  const doc = documents.get(params.textDocument.uri);
  if (!doc) return null;
  const text = doc.getText();
  const offset = new LineIndex(text).byteOffsetAt(params.position);
  const project = projectFor(projects.values(), doc.uri);
  const path = project?.pathOf(doc.uri);
  const result = await callWasm<{ ty: string | null }>({ ty: null }, () => {
    if (project && path !== undefined) {
      const json = JSON.stringify(project.sources(openDocuments()).files);
      return bynk_hover_project(json, path, offset, PLATFORM);
    }
    return bynk_hover(text, offset, PLATFORM);
  });
  const value = hoverMarkdown(result.ty);
  return value ? { contents: { kind: "markdown", value } } : null;
});

connection.onCompletion(async (params): Promise<CompletionItem[]> => {
  const doc = documents.get(params.textDocument.uri);
  if (!doc) return [];
  const text = doc.getText();
  const offset = new LineIndex(text).byteOffsetAt(params.position);
  const project = projectFor(projects.values(), doc.uri);
  const path = project?.pathOf(doc.uri);
  const result = await callWasm<{ items: WasmCompletion[] }>({ items: [] }, () => {
    if (project && path !== undefined) {
      const json = JSON.stringify(project.sources(openDocuments()).files);
      return bynk_complete_project(json, path, offset, PLATFORM);
    }
    return bynk_complete(text, offset, PLATFORM);
  });
  return toLspCompletionItems(result.items) as CompletionItem[];
});

documents.listen(connection);
connection.listen();
