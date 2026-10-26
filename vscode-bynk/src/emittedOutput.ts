// #2471: "Bynk: Show Compiled Output" — the TypeScript the compiler emits for
// the current file, read-only, beside it.
//
// The output comes from the CLI (DECISION A, as the maintainer settled it): the
// project is compiled to a scratch directory with `bynk compile --sources-stdin`,
// fed the project's unsaved buffers, so what is shown is what is on screen and
// the language server (which keeps no typed program and links no emitter) is not
// involved. The view is a `bynk-emitted:` virtual document served by a
// `TextDocumentContentProvider`. It is rebuilt when opened, on edits (debounced),
// on save and on external changes, only while the view is open, and dropped when
// its document closes. Each rebuild is a full compile of the project in a fresh
// process. Moving the cursor in either editor highlights the matching lines in the
// other, through the emitter's line-level source map (DECISION B: a highlight,
// not locked scrolling); the lookups are in `emittedMap.ts`.

import * as vscode from "vscode";
import * as fs from "node:fs";
import * as path from "node:path";

import { buildModules } from "./emittedBuild";
import {
  generatedRangeForSource,
  sourceRangeForGenerated,
  type EmittedFile,
  type LineRange,
} from "./emittedMap";
import type { ViewRefresh } from "./viewRefresh";

export const EMITTED_SCHEME = "bynk-emitted";
export const SHOW_COMPILED_OUTPUT = "bynk.showCompiledOutput";

/** How long to wait after the last edit before rebuilding. */
export const EDIT_DEBOUNCE_MS = 500;

interface View {
  /** The `.bynk` document the output is for. */
  source: vscode.Uri;
  file: EmittedFile;
  /** The source has been edited since `file` was built, so its lines no longer
   *  line up with the map: no highlighting until the rebuild lands. */
  stale: boolean;
}

/** The directory of the nearest `bynk.toml` above `file`, or `undefined`. */
export function projectRootOf(file: string): string | undefined {
  let dir = path.dirname(file);
  for (;;) {
    if (fs.existsSync(path.join(dir, "bynk.toml"))) return dir;
    const parent = path.dirname(dir);
    if (parent === dir) return undefined;
    dir = parent;
  }
}

/** The unsaved `.bynk` buffers under `root`: absolute path -> text. */
function dirtyBuffers(root: string): Record<string, string> {
  const buffers: Record<string, string> = {};
  for (const doc of vscode.workspace.textDocuments) {
    if (doc.languageId !== "bynk" || !doc.isDirty || doc.uri.scheme !== "file") continue;
    const rel = path.relative(root, doc.uri.fsPath);
    if (rel.startsWith("..") || path.isAbsolute(rel)) continue;
    buffers[doc.uri.fsPath] = doc.getText();
  }
  return buffers;
}

/** The virtual document for `file` of `source`. The query carries the source,
 *  so two files that emit the same output path don't share a document. */
export function emittedUri(source: vscode.Uri, outputPath: string): vscode.Uri {
  return vscode.Uri.from({
    scheme: EMITTED_SCHEME,
    path: `/${outputPath}`,
    query: encodeURIComponent(source.toString()),
  });
}

export function registerEmittedOutput(
  context: vscode.ExtensionContext,
  hub: ViewRefresh,
): void {  const views = new Map<string, View>();
  const changed = new vscode.EventEmitter<vscode.Uri>();
  const highlight = vscode.window.createTextEditorDecorationType({
    isWholeLine: true,
    backgroundColor: new vscode.ThemeColor("editor.findMatchHighlightBackground"),
    overviewRulerColor: new vscode.ThemeColor("editorOverviewRuler.findMatchForeground"),
    overviewRulerLane: vscode.OverviewRulerLane.Center,
  });

  /** Build the modules for `source` from the project on disk plus its
   *  unsaved buffers, or say why not. */
  let building: vscode.CancellationTokenSource | undefined;
  async function build(source: vscode.Uri): Promise<EmittedFile[] | string> {
    const root = projectRootOf(source.fsPath);
    if (!root) return "the file is not in a Bynk project (no bynk.toml above it)";
    building?.cancel();
    const mine = (building = new vscode.CancellationTokenSource());
    const result = await buildModules(root, source.fsPath, dirtyBuffers(root), mine.token);
    if (building === mine) building = undefined;
    mine.dispose();
    return result.ok ? result.files : result.message;
  }

  const provider: vscode.TextDocumentContentProvider = {
    onDidChange: changed.event,
    provideTextDocumentContent: (uri) => views.get(uri.toString())?.file.text ?? "",
  };

  function toRange(r: LineRange, doc: vscode.TextDocument): vscode.Range {
    const end = Math.min(r.end, doc.lineCount - 1);
    return new vscode.Range(r.start, 0, end, doc.lineAt(end).text.length);
  }

  function editorsFor(uri: vscode.Uri): vscode.TextEditor[] {
    const key = uri.toString();
    return vscode.window.visibleTextEditors.filter((e) => e.document.uri.toString() === key);
  }

  function clearHighlights(generated: vscode.Uri, view: View): void {
    for (const e of [...editorsFor(generated), ...editorsFor(view.source)]) {
      e.setDecorations(highlight, []);
    }
  }

  /** Highlight what `editor`'s cursor is on in the other side of each view. */
  function syncFrom(editor: vscode.TextEditor): void {
    const line = editor.selection.active.line;
    const key = editor.document.uri.toString();
    for (const [generatedKey, view] of views) {
      if (view.stale) continue;
      const generated = vscode.Uri.parse(generatedKey);
      if (key === generatedKey) {
        for (const target of editorsFor(view.source)) {
          const range = sourceRangeForGenerated(
            view.file,
            line,
            (l) => l < target.document.lineCount && target.document.lineAt(l).isEmptyOrWhitespace,
          );
          if (!range) {
            target.setDecorations(highlight, []);
            continue;
          }
          const r = toRange(range, target.document);
          target.setDecorations(highlight, [r]);
          target.revealRange(r, vscode.TextEditorRevealType.InCenterIfOutsideViewport);
        }
      } else if (key === view.source.toString()) {
        const range = generatedRangeForSource(view.file, line);
        for (const target of editorsFor(generated)) {
          if (!range) {
            target.setDecorations(highlight, []);
            continue;
          }
          const r = toRange(range, target.document);
          target.setDecorations(highlight, [r]);
          target.revealRange(r, vscode.TextEditorRevealType.InCenterIfOutsideViewport);
        }
      }
    }
  }

  /** Resolves to the virtual document it opened (what the integration suite
   *  reads), or `undefined` when there was nothing to show. */
  async function show(arg?: unknown): Promise<string | undefined> {
    const source =
      arg instanceof vscode.Uri ? arg : vscode.window.activeTextEditor?.document.uri;
    if (!source || !source.path.endsWith(".bynk")) {
      void vscode.window.showErrorMessage("Bynk: Show Compiled Output needs a .bynk file open.");
      return undefined;
    }
    const built = await build(source);
    if (typeof built === "string") {
      void vscode.window.showWarningMessage(`Bynk: no compiled output — ${built}`);
      return undefined;
    }
    const files = built;
    if (files.length === 0) {
      void vscode.window.showInformationMessage("Bynk: this file emits no TypeScript module.");
      return undefined;
    }
    let file = files[0];
    if (files.length > 1) {
      const pick = await vscode.window.showQuickPick(
        files.map((f) => ({ label: f.path, file: f })),
        { placeHolder: "This file compiles to several modules — which one?" },
      );
      if (!pick) return undefined;
      file = pick.file;
    }
    const uri = emittedUri(source, file.path);
    views.set(uri.toString(), { source, file, stale: false });
    changed.fire(uri);
    const doc = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(doc, {
      viewColumn: vscode.ViewColumn.Beside,
      preserveFocus: true,
      preview: false,
    });
    const active = vscode.window.activeTextEditor;
    if (active) syncFrom(active);
    return uri.toString();
  }

  /** Rebuild every open view. A project that stops compiling keeps the last
   *  good output on screen (and stays stale: its lines no longer line up). */
  async function refreshAll(): Promise<void> {
    for (const [key, view] of [...views]) {
      const built = await build(view.source);
      if (typeof built === "string") continue;
      const file = built.find((f) => f.path === view.file.path);
      if (!file || !views.has(key)) continue;
      views.set(key, { source: view.source, file, stale: false });
      changed.fire(vscode.Uri.parse(key));
    }
    const active = vscode.window.activeTextEditor;
    if (active) syncFrom(active);
  }

  let timer: NodeJS.Timeout | undefined;
  const scheduleRefresh = () => {
    if (views.size === 0) return;
    clearTimeout(timer);
    timer = setTimeout(() => void refreshAll(), EDIT_DEBOUNCE_MS);
  };

  context.subscriptions.push(
    highlight,
    changed,
    vscode.workspace.registerTextDocumentContentProvider(EMITTED_SCHEME, provider),
    vscode.commands.registerCommand(SHOW_COMPILED_OUTPUT, show),
    vscode.window.onDidChangeTextEditorSelection((e) => syncFrom(e.textEditor)),
    // An edit to a source file moves the lines its view maps from, so the
    // view stops highlighting until the debounced rebuild lands.
    vscode.workspace.onDidChangeTextDocument((e) => {
      if (e.document.languageId !== "bynk" || e.contentChanges.length === 0) return;
      const key = e.document.uri.toString();
      for (const [generatedKey, view] of views) {
        if (view.source.toString() === key) {
          views.set(generatedKey, { ...view, stale: true });
          clearHighlights(vscode.Uri.parse(generatedKey), view);
        }
      }
      scheduleRefresh();
    }),
    // Saves and changes made outside the editor (a git checkout, codegen).
    hub.onDidRefresh(scheduleRefresh),
    vscode.workspace.onDidCloseTextDocument((doc) => {
      if (doc.uri.scheme !== EMITTED_SCHEME) return;
      const key = doc.uri.toString();
      const view = views.get(key);
      if (view) clearHighlights(doc.uri, view);
      views.delete(key);
    }),
    { dispose: () => { clearTimeout(timer); building?.cancel(); } },
  );
}
