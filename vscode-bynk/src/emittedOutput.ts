// #2471: "Bynk: Show Compiled Output" — the TypeScript the compiler emits for
// the current file, read-only, beside it.
//
// The text comes from the language server's `bynk/emittedOutput` custom
// request (DECISION A): it compiles the analysed buffers in memory, so an
// unsaved edit is what is shown and no `bynk build` output on disk can be
// stale. The view is a `bynk-emitted:` virtual document served by a
// `TextDocumentContentProvider`. It is re-requested while the view is
// open (per `bynk.views.refresh`, on save by default) and dropped when the
// document closes, so emission costs nothing until
// asked for. Moving the cursor in either editor highlights the matching lines
// in the other, through the emitter's line-level source map (DECISION B: a
// highlight, not locked scrolling); the lookups are in `emittedMap.ts`.

import * as vscode from "vscode";
import type { LanguageClient } from "vscode-languageclient/node";

import {
  generatedRangeForSource,
  sourceRangeForGenerated,
  type EmittedFile,
  type LineRange,
} from "./emittedMap";
import type { ViewRefresh } from "./viewRefresh";

export const EMITTED_SCHEME = "bynk-emitted";
export const SHOW_COMPILED_OUTPUT = "bynk.showCompiledOutput";

interface EmittedOutputResult {
  files: EmittedFile[];
}

interface View {
  /** The `.bynk` document the output is for. */
  source: vscode.Uri;
  file: EmittedFile;
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
): void {
  const getClient = hub.getClient;
  const views = new Map<string, View>();
  const changed = new vscode.EventEmitter<vscode.Uri>();
  const highlight = vscode.window.createTextEditorDecorationType({
    isWholeLine: true,
    backgroundColor: new vscode.ThemeColor("editor.findMatchHighlightBackground"),
    overviewRulerColor: new vscode.ThemeColor("editorOverviewRuler.findMatchForeground"),
    overviewRulerLane: vscode.OverviewRulerLane.Center,
  });

  async function request(source: vscode.Uri): Promise<EmittedFile[] | null> {
    const client = getClient();
    if (!client) return null;
    try {
      const result = await client.sendRequest<EmittedOutputResult | null>("bynk/emittedOutput", {
        textDocument: { uri: source.toString() },
      });
      return result ? result.files : null;
    } catch {
      return null;
    }
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
    const files = await request(source);
    if (!files) {
      void vscode.window.showWarningMessage(
        "Bynk: no compiled output — the project doesn't compile yet, or the language server isn't ready.",
      );
      return undefined;
    }
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
    views.set(uri.toString(), { source, file });
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

  /** Re-request every open view whose project may have changed. */
  async function refreshAll(): Promise<void> {
    for (const [key, view] of [...views]) {
      const files = await request(view.source);
      // A project that stops compiling keeps the last good output on screen.
      const file = files?.find((f) => f.path === view.file.path);
      if (!file) continue;
      views.set(key, { source: view.source, file });
      changed.fire(vscode.Uri.parse(key));
    }
    const active = vscode.window.activeTextEditor;
    if (active) syncFrom(active);
  }

  context.subscriptions.push(
    highlight,
    changed,
    vscode.workspace.registerTextDocumentContentProvider(EMITTED_SCHEME, provider),
    vscode.commands.registerCommand(SHOW_COMPILED_OUTPUT, show),
    vscode.window.onDidChangeTextEditorSelection((e) => syncFrom(e.textEditor)),
    // The hub fires once the server has committed the round for a save (or
    // per `bynk.views.refresh`), so the request sees the saved text.
    hub.onDidRefresh(() => {
      if (views.size > 0) void refreshAll();
    }),
    vscode.workspace.onDidCloseTextDocument((doc) => {
      if (doc.uri.scheme !== EMITTED_SCHEME) return;
      const key = doc.uri.toString();
      const view = views.get(key);
      if (view) clearHighlights(doc.uri, view);
      views.delete(key);
    }),
  );
}
