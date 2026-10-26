// #2097: the Bynk project sidebar — an activity-bar view listing the project's
// units (contexts, commons, adapters, tests), each expanding to its `consumes`
// edges in both directions (DECISION C) and its declarations, every node
// click-to-source. Handler nodes carry inline actions for the sequence-diagram
// and wire-contract views; unit nodes open the documentation view.
//
// DECISION A: the structure comes from `bynk/architectureModel` (through the
// shared `ModelCache`, so the architecture map and this tree share a request)
// plus a header scan of the `.bynk` files; declarations are fetched lazily
// from document symbols when a unit expands. It refreshes on the live views'
// trigger (#2093's `ViewRefresh`), only while visible.

import type { ApprovalState } from "./approveRequirements";
import * as path from "path";

import * as vscode from "vscode";

import { requestArchitectureModel } from "./architectureMap";
import {
  buildProjectModel,
  type EdgeRef,
  parseUnitHeader,
  type ProjectUnit,
  type UnitGroup,
  type UnitHeader,
} from "./projectModel";
import { clearProjectRoots, projectRootFor } from "./projectRoots";
import type { SymbolLike } from "./refreshScheduler";
import type { ViewRefresh } from "./viewRefresh";
import type { ArchLoc } from "./webview/architectureTypes";

type Node =
  | { type: "project"; root: vscode.Uri; groups: UnitGroup[] }
  | { type: "group"; root: string; group: UnitGroup }
  | { type: "unit"; root: string; unit: ProjectUnit }
  | { type: "edges"; root: string; dir: "consumes" | "consumedBy"; unit: ProjectUnit }
  | { type: "edge"; dir: "consumes" | "consumedBy"; edge: EdgeRef }
  | { type: "decl"; uri: string; symbol: SymbolLike; handler: boolean; label: string }
  | { type: "message"; label: string };

interface Project {
  root: vscode.Uri;
  groups: UnitGroup[];
  /** The model request came back empty (cold start: no round yet). */
  modelMissing: boolean;
}

const UNIT_ICONS: Record<string, string> = {
  context: "symbol-namespace",
  commons: "library",
  adapter: "plug",
  test: "beaker",
};

// `vscode.SymbolKind` → codicon (`symbol-*`).
const SYMBOL_ICONS: Record<number, string> = {
  1: "symbol-module",
  4: "symbol-class",
  5: "symbol-method",
  6: "symbol-property",
  7: "symbol-field",
  9: "symbol-enum",
  10: "symbol-interface",
  11: "symbol-function",
  13: "symbol-constant",
  18: "symbol-object",
  21: "symbol-enum-member",
  22: "symbol-struct",
  25: "symbol-type-parameter",
};
const SYMBOL_KIND_CLASS = 4;
const SYMBOL_KIND_METHOD = 5;

/** Cold start: re-ask for the model this often while it is missing. */
const MODEL_RETRY_MS = 3000;
const MODEL_RETRIES = 10;

function openCommand(loc: ArchLoc | { uri: string; range: vscode.Range }, title = "Open"): vscode.Command {
  const r = loc.range;
  return {
    command: "vscode.open",
    title,
    arguments: [
      vscode.Uri.parse(loc.uri),
      {
        selection: new vscode.Range(r.start.line, r.start.character, r.start.line, r.start.character),
      } satisfies vscode.TextDocumentShowOptions,
    ],
  };
}

/** A handler's symbol is named `call` (or `call x`); its first source line
 *  (`on GET("/a") ()`) says far more. */
function handlerLabel(doc: vscode.TextDocument, symbol: SymbolLike): string {
  const line = doc.lineAt(symbol.range.start.line).text.slice(symbol.range.start.character).trim();
  // Search a copy with string literals blanked (same length), so a route
  // like `"/sort by name"` doesn't cut the label short.
  const masked = line.replace(/"(?:[^"\\]|\\.)*"/g, (m) => `"${"_".repeat(m.length - 2)}"`);
  const cut = masked.search(/\s(->|by|given)\s|\s*\{\s*$/);
  return (cut > 0 ? line.slice(0, cut) : line) || symbol.name;
}

export class ProjectTreeProvider implements vscode.TreeDataProvider<Node>, vscode.Disposable {
  private readonly emitter = new vscode.EventEmitter<Node | undefined>();
  readonly onDidChangeTreeData = this.emitter.event;
  private projects: Project[] = [];
  private loading: Promise<void> | undefined;
  /** Each load's number; only the latest may publish its result. */
  private loadGen = 0;
  private loaded = false;
  private dirty = false;
  private retries = 0;
  private retryTimer: ReturnType<typeof setTimeout> | undefined;
  private visible = () => true;

  /** A (re)started language client gets a fresh set of cold-start retries. */
  resetRetries(): void {
    this.retries = 0;
  }
  /** Test seam counter. */
  reloads = 0;

  constructor(private readonly hub: ViewRefresh) {}

  /** Tie refreshes to the view's visibility. */
  attach(view: vscode.TreeView<Node>): vscode.Disposable {
    this.visible = () => view.visible;
    return view.onDidChangeVisibility((e) => {
      if (e.visible && this.dirty) void this.reload();
    });
  }

  /** Something changed: reload now if visible, else when next shown. */
  invalidate(): void {
    if (!this.loaded) return; // nothing shown yet; the first getChildren loads
    if (this.visible()) void this.reload();
    else this.dirty = true;
  }

  async reload(fresh = false): Promise<void> {
    this.dirty = false;
    this.reloads++;
    const run = this.load(fresh);
    this.loading = run;
    await run;
    if (this.loading === run) this.loading = undefined;
    this.emitter.fire(undefined);
    this.scheduleRetry();
  }

  getTreeItem(node: Node): vscode.TreeItem {
    switch (node.type) {
      case "project": {
        const item = new vscode.TreeItem(path.basename(node.root.fsPath), vscode.TreeItemCollapsibleState.Expanded);
        item.id = `project:${node.root.toString()}`;
        item.iconPath = new vscode.ThemeIcon("folder-library");
        item.resourceUri = node.root;
        item.contextValue = "bynkProject";
        return item;
      }
      case "group": {
        const item = new vscode.TreeItem(node.group.label, vscode.TreeItemCollapsibleState.Expanded);
        item.id = `group:${node.root}:${node.group.kind}`;
        item.description = String(node.group.units.length);
        return item;
      }
      case "unit": {
        const u = node.unit;
        const item = new vscode.TreeItem(u.name, vscode.TreeItemCollapsibleState.Collapsed);
        item.id = `unit:${node.root}:${u.kind}:${u.name}`;
        item.iconPath = new vscode.ThemeIcon(UNIT_ICONS[u.kind] ?? "symbol-namespace");
        if (u.files.length > 1) item.description = `${u.files.length} files`;
        item.tooltip = `${u.kind} ${u.name}`;
        item.contextValue = "bynkUnit";
        if (u.loc) item.command = openCommand(u.loc, "Open unit");
        return item;
      }
      case "edges": {
        const list = node.dir === "consumes" ? node.unit.consumes : node.unit.consumedBy;
        const item = new vscode.TreeItem(
          node.dir === "consumes" ? "Consumes" : "Consumed by",
          vscode.TreeItemCollapsibleState.Collapsed,
        );
        item.id = `edges:${node.root}:${node.unit.kind}:${node.unit.name}:${node.dir}`;
        item.description = String(list.length);
        item.iconPath = new vscode.ThemeIcon(node.dir === "consumes" ? "arrow-right" : "arrow-left");
        return item;
      }
      case "edge": {
        const e = node.edge;
        const item = new vscode.TreeItem(e.unit, vscode.TreeItemCollapsibleState.None);
        if (e.capabilities.length > 0) item.description = `{ ${e.capabilities.join(", ")} }`;
        item.tooltip =
          node.dir === "consumes" ? `consumes ${e.unit}` : `consumed by ${e.unit}`;
        item.iconPath = new vscode.ThemeIcon("references");
        item.command = openCommand(e.loc, "Open consumes clause");
        return item;
      }
      case "decl": {
        const s = node.symbol;
        const hasChildren = (s.children?.length ?? 0) > 0;
        const item = new vscode.TreeItem(
          node.label,
          hasChildren ? vscode.TreeItemCollapsibleState.Collapsed : vscode.TreeItemCollapsibleState.None,
        );
        item.iconPath = new vscode.ThemeIcon(SYMBOL_ICONS[s.kind] ?? "symbol-misc");
        item.contextValue = node.handler ? "bynkHandler" : "bynkDeclaration";
        const r = s.selectionRange;
        item.command = openCommand({
          uri: node.uri,
          range: new vscode.Range(r.start.line, r.start.character, r.end.line, r.end.character),
        });
        return item;
      }
      case "message":
        return new vscode.TreeItem(node.label, vscode.TreeItemCollapsibleState.None);
    }
  }

  async getChildren(node?: Node): Promise<Node[]> {
    if (!node) {
      if (!this.loaded) await (this.loading ?? this.reloadQuiet());
      if (this.projects.length === 1) {
        const p = this.projects[0];
        return p.groups.map((group) => ({ type: "group", root: p.root.toString(), group }));
      }
      return this.projects.map((p) => ({ type: "project", root: p.root, groups: p.groups }));
    }
    switch (node.type) {
      case "project":
        return node.groups.map((group) => ({ type: "group", root: node.root.toString(), group }));
      case "group":
        return node.group.units.map((unit) => ({ type: "unit", root: node.root, unit }));
      case "unit":
        return this.unitChildren(node.root, node.unit);
      case "edges": {
        const list = node.dir === "consumes" ? node.unit.consumes : node.unit.consumedBy;
        return list.map((edge) => ({ type: "edge", dir: node.dir, edge }));
      }
      case "decl":
        return this.declChildren(node);
      default:
        return [];
    }
  }

  /** Test seam: the tree's labels, `depth` levels deep (lazy levels load). */
  async snapshot(depth: number): Promise<unknown[]> {
    const walk = async (n: Node | undefined, d: number): Promise<unknown[]> => {
      const kids = await this.getChildren(n);
      const out: unknown[] = [];
      for (const k of kids) {
        const item = this.getTreeItem(k);
        const label = typeof item.label === "string" ? item.label : (item.label?.label ?? "");
        const entry: Record<string, unknown> = { label, contextValue: item.contextValue };
        if (item.description) entry.description = item.description;
        if (d > 1 && item.collapsibleState !== vscode.TreeItemCollapsibleState.None) {
          entry.children = await walk(k, d - 1);
        }
        out.push(entry);
      }
      return out;
    };
    return walk(undefined, depth);
  }

  /** The first file of the first project (or of `root`) — what the
   *  architecture map resolves the project from. */
  projectFile(root?: vscode.Uri): string | undefined {
    const p = root ? this.projects.find((x) => x.root.toString() === root.toString()) : this.projects[0];
    for (const g of p?.groups ?? []) for (const u of g.units) if (u.files[0]) return u.files[0];
    return undefined;
  }

  dispose(): void {
    if (this.retryTimer !== undefined) clearTimeout(this.retryTimer);
    this.emitter.dispose();
  }

  private async reloadQuiet(): Promise<void> {
    const run = this.load(false);
    this.loading = run;
    await run;
    if (this.loading === run) this.loading = undefined;
    this.scheduleRetry();
  }

  /** While a project's model is missing (the server has not committed a
   *  round yet), re-ask a few times, so the edges appear without an edit.
   *  The next load re-requests just the missing projects' models, fresh
   *  (`load`), leaving the shared cache alone. */
  private scheduleRetry(): void {
    if (this.retryTimer !== undefined) return;
    if (!this.projects.some((p) => p.modelMissing) || this.retries >= MODEL_RETRIES) return;
    this.retries++;
    this.retryTimer = setTimeout(() => {
      this.retryTimer = undefined;
      this.invalidate();
    }, MODEL_RETRY_MS);
  }

  private async load(fresh: boolean): Promise<void> {
    const gen = ++this.loadGen;
    // A project whose model was missing last time is asked again, bypassing
    // the cache (which holds that `null`), however this load was triggered.
    const wasMissing = new Set(this.projects.filter((p) => p.modelMissing).map((p) => p.root.toString()));
    const files = await vscode.workspace.findFiles("**/*.bynk", "**/node_modules/**");
    const byRoot = new Map<string, { root: vscode.Uri; headers: UnitHeader[] }>();
    await Promise.all(
      files.map(async (file) => {
        const root = await projectRootFor(file);
        if (!root) return;
        const header = parseUnitHeader(await readText(file));
        if (!header) return;
        const key = root.toString();
        let entry = byRoot.get(key);
        if (!entry) {
          entry = { root, headers: [] };
          byRoot.set(key, entry);
        }
        entry.headers.push({ ...header, uri: file.toString() });
      }),
    );

    const client = this.hub.getClient();
    const projects: Project[] = [];
    for (const { root, headers } of byRoot.values()) {
      headers.sort((a, b) => a.uri.localeCompare(b.uri));
      let model = null;
      if (client && headers.length > 0) {
        try {
          const again = fresh || wasMissing.has(root.toString());
          model = await requestArchitectureModel(client, this.hub, headers[0].uri, again);
        } catch {
          model = null;
        }
      }
      // Only a project with a context or adapter has a model to wait for: a
      // `null` for one of only commons and tests is the answer, not a cold start.
      const expectsModel = headers.some((h) => h.kind === "context" || h.kind === "adapter");
      projects.push({
        root,
        groups: buildProjectModel(headers, model),
        modelMissing: !!client && !model && expectsModel,
      });
    }
    // A newer load started while this one ran: its result wins.
    if (gen !== this.loadGen) return;
    projects.sort((a, b) => a.root.toString().localeCompare(b.root.toString()));
    if (projects.every((p) => !p.modelMissing)) this.retries = 0;
    this.projects = projects;
    this.loaded = true;
  }

  private async unitChildren(root: string, unit: ProjectUnit): Promise<Node[]> {
    const out: Node[] = [];
    if (unit.consumes.length > 0) out.push({ type: "edges", root, dir: "consumes", unit });
    if (unit.consumedBy.length > 0) out.push({ type: "edges", root, dir: "consumedBy", unit });
    for (const file of unit.files) {
      let doc: vscode.TextDocument;
      try {
        doc = await vscode.workspace.openTextDocument(vscode.Uri.parse(file));
      } catch {
        continue; // a platform adapter's file the workspace cannot open
      }
      const symbols = await this.hub.documentSymbols(doc);
      // The file's one top-level symbol is the unit; its children are the
      // declarations.
      const decls = symbols.length === 1 ? (symbols[0].children ?? []) : symbols;
      for (const s of decls) out.push(this.declNode(doc, s, false));
    }
    if (out.length === 0) out.push({ type: "message", label: "No declarations" });
    return out;
  }

  private async declChildren(node: Extract<Node, { type: "decl" }>): Promise<Node[]> {
    const children = node.symbol.children ?? [];
    if (children.length === 0) return [];
    const doc = await vscode.workspace.openTextDocument(vscode.Uri.parse(node.uri));
    const isOwner = node.symbol.kind === SYMBOL_KIND_CLASS;
    return children.map((c) => this.declNode(doc, c, isOwner && c.kind === SYMBOL_KIND_METHOD));
  }

  private declNode(doc: vscode.TextDocument, symbol: SymbolLike, handler: boolean): Node {
    return {
      type: "decl",
      uri: doc.uri.toString(),
      symbol,
      handler,
      label: handler ? handlerLabel(doc, symbol) : symbol.name,
    };
  }
}

/** A file's current text: the open document's buffer if any, else disk. */
async function readText(file: vscode.Uri): Promise<string> {
  const open = vscode.workspace.textDocuments.find((d) => d.uri.toString() === file.toString());
  if (open) return open.getText();
  try {
    return new TextDecoder().decode(await vscode.workspace.fs.readFile(file));
  } catch {
    return "";
  }
}

type DeclNode = Extract<Node, { type: "decl" }>;
type UnitNode = Extract<Node, { type: "unit" }>;

export function registerProjectTree(
  context: vscode.ExtensionContext,
  hub: ViewRefresh,
  approvals?: ApprovalState,
): ProjectTreeProvider {
  const provider = new ProjectTreeProvider(hub);
  const view = vscode.window.createTreeView("bynk.project", {
    treeDataProvider: provider,
    showCollapseAll: true,
  });

  // #2473: npm packages awaiting approval badge the view.
  const badge = () => {
    const value = approvals?.pendingCount() ?? 0;
    view.badge =
      value > 0
        ? { value, tooltip: `${value} npm package${value === 1 ? "" : "s"} awaiting approval (Bynk: Review Pending npm Requirements)` }
        : undefined;
  };
  badge();

  // A manifest created or deleted moves project roots. The reload itself
  // comes through the hub, which watches `.bynk`/`bynk.toml` changes made
  // outside the editor and respects `bynk.views.refresh`.
  const manifests = vscode.workspace.createFileSystemWatcher("**/bynk.toml");

  context.subscriptions.push(
    provider,
    view,
    provider.attach(view),
    manifests,
    ...(approvals ? [approvals.onDidChange(badge)] : []),
    // The live views' trigger (#2093): saves by default, `bynk.views.refresh`,
    // and changes made outside the editor.
    hub.onDidRefresh(() => provider.invalidate()),
    hub.onDidStartClient(() => {
      provider.resetRetries();
      provider.invalidate();
    }),
    manifests.onDidCreate(clearProjectRoots),
    manifests.onDidDelete(clearProjectRoots),
    vscode.commands.registerCommand("bynk.project.refresh", () => provider.reload(true)),
    vscode.commands.registerCommand("bynk.project.showArchitectureMap", (node?: Node) => {
      const root = node?.type === "project" ? node.root : undefined;
      const file = provider.projectFile(root);
      return vscode.commands.executeCommand("bynk.showArchitectureMap", file);
    }),
    vscode.commands.registerCommand("bynk.project.showDocumentation", (node?: UnitNode) => {
      const file = node?.unit.loc?.uri ?? node?.unit.files[0];
      if (file) return vscode.commands.executeCommand("bynk.showDocumentation", file);
    }),
    vscode.commands.registerCommand("bynk.project.sequenceDiagram", (node?: DeclNode) => {
      if (node) return vscode.commands.executeCommand("bynk.showSequenceDiagram", node.uri, node.symbol.selectionRange.start);
    }),
    vscode.commands.registerCommand("bynk.project.wireContract", (node?: DeclNode) => {
      if (node) return vscode.commands.executeCommand("bynk.showWireContract", node.uri, node.symbol.selectionRange.start);
    }),
    // Test seams (not contributed): the rendered tree, `depth` levels deep,
    // and the reload count / visibility.
    vscode.commands.registerCommand("bynk._projectTreeSnapshot", async (depth = 3, reload = false) => {
      if (reload) await provider.reload(true);
      return provider.snapshot(depth);
    }),
    vscode.commands.registerCommand("bynk._sawExternalChange", (uri: string) => hub.sawExternal(uri)),
    vscode.commands.registerCommand("bynk._projectTreeState", () => ({
      reloads: provider.reloads,
      visible: view.visible,
      hub: { ...hub.stats, mode: hub.mode },
    })),
  );
  return provider;
}
