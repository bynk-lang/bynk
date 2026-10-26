// #2478: Explorer file decorations — a short badge naming each `.bynk` file's
// owning context, and a red tint on files whose tests failed in the last run.
// Each is switched by a setting (`bynk.fileDecorations.*`).
//
// Desktop only: the tint reads the Test Explorer's results, which the web
// entry does not have, and the web entry does not register this provider.
// The marker for contract breaks waits on the contract diagnostics (#2467).
//
// The owning context comes from the same header scan the project sidebar
// (#2097) uses (`parseUnitHeader`), kept per file and updated from a file
// watcher; it needs no language-server round trip, so the badges are there
// before the server is.

import * as vscode from "vscode";

import {
  type BadgeMode,
  type ScannedFile,
  contextBadge,
  testFailures,
} from "./fileDecorationsCore";
import { parseUnitHeader } from "./projectModel";
import { clearProjectRoots, projectRootFor } from "./projectRoots";

const SECTION = "bynk.fileDecorations";

const decoder = new TextDecoder();

async function readText(uri: vscode.Uri): Promise<string> {
  return decoder.decode(await vscode.workspace.fs.readFile(uri));
}

export class FileDecorations implements vscode.FileDecorationProvider, vscode.Disposable {
  private readonly changed = new vscode.EventEmitter<vscode.Uri | vscode.Uri[] | undefined>();
  readonly onDidChangeFileDecorations = this.changed.event;

  private readonly files = new Map<string, ScannedFile>();
  private readonly disposables: vscode.Disposable[] = [this.changed];
  /** Bumped by every full scan, so a slower earlier scan does not overwrite a newer one. */
  private scanGen = 0;

  constructor() {
    const watcher = vscode.workspace.createFileSystemWatcher("**/{*.bynk,bynk.toml}");
    this.disposables.push(
      vscode.window.registerFileDecorationProvider(this),
      watcher,
      watcher.onDidCreate((uri) => void this.fileChanged(uri)),
      watcher.onDidChange((uri) => void this.fileChanged(uri)),
      watcher.onDidDelete((uri) => void this.fileDeleted(uri)),
      vscode.workspace.onDidChangeWorkspaceFolders(() => void this.scan()),
      vscode.workspace.onDidChangeConfiguration((e) => {
        if (e.affectsConfiguration(SECTION)) this.changed.fire(undefined);
      }),
      testFailures.onDidChange((uris) => this.changed.fire(uris.map((u) => vscode.Uri.parse(u)))),
    );
    void this.scan();
  }

  provideFileDecoration(uri: vscode.Uri): vscode.FileDecoration | undefined {
    const config = vscode.workspace.getConfiguration(SECTION);
    const key = uri.toString();
    const badge = contextBadge(this.files, key, config.get<BadgeMode>("contextBadge", "auto"));
    const failed = config.get<boolean>("failedTests", true) && testFailures.hasFailures(key);
    if (!badge && !failed) return undefined;
    const tooltip = [
      badge && `Bynk context ${badge.context}`,
      failed && "Tests failed in the last run",
    ]
      .filter(Boolean)
      .join(" · ");
    const decoration = new vscode.FileDecoration(
      badge?.badge,
      tooltip,
      failed ? new vscode.ThemeColor("bynk.testFailedForeground") : undefined,
    );
    // A tinted folder shows where to look; a badge on a folder would not mean anything.
    decoration.propagate = failed;
    return decoration;
  }

  dispose(): void {
    for (const d of this.disposables) d.dispose();
  }

  /** The decoration state of a file, for the integration test. */
  snapshot(uri: vscode.Uri): { badge?: string; tooltip?: string; failed: boolean } {
    const d = this.provideFileDecoration(uri);
    return { badge: d?.badge, tooltip: d?.tooltip, failed: d?.color !== undefined };
  }

  private async scan(): Promise<void> {
    const gen = ++this.scanGen;
    const found = await vscode.workspace.findFiles("**/*.bynk", "**/node_modules/**");
    const next = new Map<string, ScannedFile>();
    await Promise.all(
      found.map(async (uri) => {
        const entry = await this.read(uri);
        if (entry) next.set(uri.toString(), entry);
      }),
    );
    if (gen !== this.scanGen) return;
    this.files.clear();
    for (const [k, v] of next) this.files.set(k, v);
    this.changed.fire(undefined);
  }

  private async read(uri: vscode.Uri): Promise<ScannedFile | undefined> {
    try {
      const root = await projectRootFor(uri);
      const header = root && parseUnitHeader(await readText(uri));
      return root && header ? { root: root.toString(), kind: header.kind, name: header.name } : undefined;
    } catch {
      return undefined; // unreadable or gone: no decoration
    }
  }

  private async fileChanged(uri: vscode.Uri): Promise<void> {
    if (uri.path.endsWith("/bynk.toml")) {
      clearProjectRoots();
      return this.scan();
    }
    const before = this.files.get(uri.toString());
    const after = await this.read(uri);
    if (after) this.files.set(uri.toString(), after);
    else this.files.delete(uri.toString());
    this.refreshAround(uri, before, after);
  }

  private async fileDeleted(uri: vscode.Uri): Promise<void> {
    if (uri.path.endsWith("/bynk.toml")) {
      clearProjectRoots();
      return this.scan();
    }
    const before = this.files.get(uri.toString());
    this.files.delete(uri.toString());
    this.refreshAround(uri, before, undefined);
  }

  /** Refresh `uri`, and its whole project when a context appeared, went, or was
   *  renamed (which can move the project across the single-context threshold,
   *  or change a suite's badge). */
  private refreshAround(uri: vscode.Uri, before?: ScannedFile, after?: ScannedFile): void {
    const contextChanged = before?.kind !== after?.kind || before?.name !== after?.name;
    const touchesContext = before?.kind === "context" || after?.kind === "context";
    if (!contextChanged || !touchesContext) {
      this.changed.fire(uri);
      return;
    }
    const roots = new Set([before?.root, after?.root]);
    const affected = [...this.files].filter(([, f]) => roots.has(f.root)).map(([k]) => vscode.Uri.parse(k));
    this.changed.fire([uri, ...affected]);
  }
}

export function registerFileDecorations(context: vscode.ExtensionContext): FileDecorations {
  const provider = new FileDecorations();
  context.subscriptions.push(
    provider,
    // Test seam for the integration suite (not contributed, so never in the palette).
    vscode.commands.registerCommand("bynk._fileDecoration", (uri: string) =>
      provider.snapshot(vscode.Uri.parse(uri)),
    ),
  );
  return provider;
}
