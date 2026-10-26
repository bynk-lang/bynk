// #2472: dependencies and the registry in the editor.
//
// - **Bynk: Add Dependency…** looks an `org/package` up with `bynk search`,
//   offers its versions, then runs `bynk add`.
// - **Bynk: Fetch Dependencies** runs `bynk fetch`.
// - **Upgrade code lenses** in `bynk.toml` (`0.3.1 → 0.4.0 available`).
// - **Bynk: Publish…** runs `bynk publish --dry-run`, shows the package, version,
//   registry and file list, and publishes only after a confirmation.
//
// DECISION A: every registry query goes through the driver. DECISION B: the
// lenses are on by default (`bynk.dependencies.upgradeLenses`), look a package
// up at most once per session, and tolerate being offline (a failed lookup
// shows nothing and is not retried). DECISION C: publish is plan-first.

import * as vscode from "vscode";

import { BynkNotFoundError, type BynkResult, findProjectRoot, runBynk } from "./cli";
import {
  addArgs,
  currentVersion,
  type DepRow,
  depsArgs,
  fetchArgs,
  type ManifestDependency,
  pickableVersions,
  parseDeps,
  parsePublishPlan,
  parseSearch,
  publishArgs,
  publishFilesText,
  publishPlanArgs,
  publishPlanText,
  rangeFor,
  scanManifestDependencies,
  type SearchResult,
  searchArgs,
  upgradeTarget,
  upgradeTitle,
  validPackageName,
} from "./dependenciesCore";
import { reportBynkNotFound } from "./doctor";
import { isManifestPath } from "./manifestSchema";

let output: vscode.OutputChannel | undefined;

function channel(): vscode.OutputChannel {
  output ??= vscode.window.createOutputChannel("Bynk Dependencies");
  return output;
}

/** `bynk deps` for the project at `root`: `undefined` when the driver is
 *  missing or does not know the command (an older toolchain). Never throws. */
export async function loadDependencies(root: string): Promise<DepRow[] | undefined> {
  try {
    const r = await runBynk(depsArgs(), root);
    return r.code === 0 ? parseDeps(r.stdout) : undefined;
  } catch {
    return undefined;
  }
}

/** One `bynk search` per package per session (DECISION B): a failure — offline,
 *  an unrouted organisation, an older driver — is remembered as "no answer" and
 *  not retried until the window reloads. */
export class SearchCache {
  private readonly lookups = new Map<string, Promise<SearchResult | undefined>>();
  private readonly settled = new Map<string, SearchResult | undefined>();
  /** Test seam: how many lookups actually ran. */
  runs = 0;

  constructor(
    private readonly run: (pkg: string, root: string) => Promise<SearchResult | undefined> = searchPackage,
  ) {}

  /** Keyed by project as well as package: the project's `[registries]`
   *  decide where an organisation routes. */
  private static key(pkg: string, root: string): string {
    return `${root}\u0000${pkg}`;
  }

  get(pkg: string, root: string): Promise<SearchResult | undefined> {
    const key = SearchCache.key(pkg, root);
    let hit = this.lookups.get(key);
    if (!hit) {
      this.runs++;
      hit = this.run(pkg, root).catch(() => undefined);
      this.lookups.set(key, hit);
    }
    return hit;
  }

  /** The answer if the lookup has finished, without starting one. */
  peek(pkg: string, root: string): SearchResult | undefined {
    return this.settled.get(SearchCache.key(pkg, root));
  }

  /** Look up, and remember the answer for `peek`. */
  async fill(pkg: string, root: string): Promise<SearchResult | undefined> {
    const found = await this.get(pkg, root);
    this.settled.set(SearchCache.key(pkg, root), found);
    return found;
  }
}

async function searchPackage(pkg: string, root: string): Promise<SearchResult | undefined> {
  const r = await runBynk(searchArgs(pkg), root);
  return r.code === 0 ? parseSearch(r.stdout) : undefined;
}

function enabled(): boolean {
  return vscode.workspace.getConfiguration("bynk").get<boolean>("dependencies.upgradeLenses", true);
}

/** The directory holding `document`'s `bynk.toml`. */
function manifestDir(document: vscode.TextDocument): string {
  return vscode.Uri.joinPath(document.uri, "..").fsPath;
}

class UpgradeLensProvider implements vscode.CodeLensProvider, vscode.Disposable {
  private readonly emitter = new vscode.EventEmitter<void>();
  readonly onDidChangeCodeLenses = this.emitter.event;
  private readonly sub: vscode.Disposable;
  /** The locked versions `bynk deps` reported, by manifest directory. */
  private readonly locks = new Map<string, Promise<DepRow[] | undefined>>();
  private readonly pending = new Set<string>();

  constructor(readonly cache: SearchCache) {
    this.sub = vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("bynk.dependencies.upgradeLenses")) this.emitter.fire();
    });
  }

  /** Forget the locked versions (after an add, fetch or upgrade). */
  invalidate(): void {
    this.locks.clear();
    this.emitter.fire();
  }

  async provideCodeLenses(document: vscode.TextDocument): Promise<vscode.CodeLens[]> {
    if (!enabled() || !isManifestPath(document.fileName) || document.uri.scheme !== "file") return [];
    const dir = manifestDir(document);
    let rows = this.locks.get(dir);
    if (!rows) {
      rows = loadDependencies(dir);
      this.locks.set(dir, rows);
    }
    const locked = await rows;
    const lenses: vscode.CodeLens[] = [];
    for (const dep of scanManifestDependencies(document.getText())) {
      const row = locked?.find((r) => r.alias === dep.alias && r.dev === dep.dev);
      const pkg = dep.package ?? row?.package;
      if (!pkg || !validPackageName(pkg)) continue;
      const found = this.cache.peek(pkg, dir);
      const asked = `${dir}\u0000${pkg}`;
      if (found === undefined && !this.pending.has(asked)) {
        // Not asked yet: ask in the background; the lens appears when it answers.
        this.pending.add(asked);
        void this.cache.fill(pkg, dir).then(() => this.emitter.fire());
      }
      const current = currentVersion(row?.locked, dep.range);
      const target = upgradeTarget(current, found);
      if (!current || !target) continue;
      const at = new vscode.Position(dep.line, 0);
      lenses.push(
        new vscode.CodeLens(new vscode.Range(at, at), {
          title: upgradeTitle(current, target),
          tooltip: `Set ${dep.alias} to ${rangeFor(target)} and fetch it`,
          command: "bynk.dependencies.upgrade",
          arguments: [document.uri.toString(), dep, target],
        }),
      );
    }
    return lenses;
  }

  dispose(): void {
    this.sub.dispose();
    this.emitter.dispose();
  }
}

export function registerDependencies(context: vscode.ExtensionContext): SearchCache {
  const cache = new SearchCache();
  const lenses = new UpgradeLensProvider(cache);
  context.subscriptions.push(
    lenses,
    { dispose: () => output?.dispose() },
    vscode.languages.registerCodeLensProvider({ pattern: "**/bynk.toml" }, lenses),
    vscode.commands.registerCommand("bynk.addDependency", (preset?: { package?: string }) =>
      addDependency(lenses, preset),
    ),
    vscode.commands.registerCommand("bynk.fetchDependencies", () => fetchDependencies(lenses)),
    vscode.commands.registerCommand("bynk.publish", () => publish(lenses)),
    vscode.commands.registerCommand("bynk.dependencies.upgrade", (uri: string, dep: ManifestDependency, target: string) =>
      upgrade(lenses, uri, dep, target),
    ),
    vscode.commands.registerCommand("bynk.dependencies.openManifest", async (root: vscode.Uri) => {
      await vscode.commands.executeCommand("vscode.open", vscode.Uri.joinPath(root, "bynk.toml"));
    }),
  );
  return cache;
}

/** Report a failed `bynk` run in the output channel with a way to read it. */
async function reportFailure(what: string, args: string[], r: BynkResult): Promise<void> {
  const out = channel();
  out.clear();
  out.appendLine(`$ bynk ${args.join(" ")}`);
  out.append(r.stderr || r.stdout);
  const choice = await vscode.window.showErrorMessage(`Bynk: ${what}.`, "Show Output");
  if (choice) out.show();
}

/** Run `args` in `root` under a progress notification; `undefined` (after
 *  telling the user) when `bynk` could not be started. */
async function run(root: string, title: string, args: string[]): Promise<BynkResult | undefined> {
  try {
    return await vscode.window.withProgress(
      { location: vscode.ProgressLocation.Notification, title, cancellable: true },
      (_progress, token) => runBynk(args, root, token),
    );
  } catch (e) {
    if (e instanceof BynkNotFoundError) await reportBynkNotFound();
    else void vscode.window.showErrorMessage(`Bynk: ${String(e)}`);
    return undefined;
  }
}

async function refreshViews(lenses: UpgradeLensProvider): Promise<void> {
  lenses.invalidate();
  await vscode.commands.executeCommand("bynk.project.refresh");
}

async function addDependency(lenses: UpgradeLensProvider, preset?: { package?: string }): Promise<void> {
  const root = await findProjectRoot();
  if (!root) {
    void vscode.window.showErrorMessage("Bynk: no bynk.toml found in the workspace.");
    return;
  }
  const pkg =
    typeof preset?.package === "string" && validPackageName(preset.package)
      ? preset.package
      : await vscode.window.showInputBox({
          title: "Add Dependency",
          prompt: "The package to add, as `org/package`",
          placeHolder: "acme/money",
          ignoreFocusOut: true,
          validateInput: (v) => (validPackageName(v.trim()) ? undefined : "Use `org/package`, e.g. `acme/money`."),
        });
  if (!pkg) return;
  const name = pkg.trim();

  const searched = await run(root.fsPath, `Bynk: looking up ${name}…`, searchArgs(name));
  if (!searched) return;
  const found = searched.code === 0 ? parseSearch(searched.stdout) : undefined;
  if (!found) {
    // A registry's index has no free-text search, so a miss here means the
    // name is wrong, the organisation isn't routed in `[registries]`, or the
    // registry can't be reached: the driver's message says which.
    await reportFailure(`could not look up ${name} on its registry`, searchArgs(name), searched);
    return;
  }
  const versions = pickableVersions(found);
  if (versions.length === 0) {
    void vscode.window.showErrorMessage(`Bynk: ${name} has no version that can be added (all yanked or withdrawn).`);
    return;
  }
  const picked = await vscode.window.showQuickPick(
    versions.map((v, i) => ({
      label: v.version,
      description: i === 0 ? `latest · ${found.registry}` : undefined,
      range: rangeFor(v.version),
    })),
    { title: `Add ${name}`, placeHolder: "Which version?", ignoreFocusOut: true },
  );
  if (!picked) return;

  const args = addArgs(name, picked.range);
  const added = await run(root.fsPath, `Bynk: adding ${name}…`, args);
  if (!added) return;
  await refreshViews(lenses);
  if (added.code !== 0) {
    await reportFailure(`\`bynk add ${name}\` failed`, args, added);
    return;
  }
  void vscode.window.showInformationMessage(`Bynk: added ${name} ${picked.range} to bynk.toml.`);
}

async function fetchDependencies(lenses: UpgradeLensProvider): Promise<void> {
  const root = await findProjectRoot();
  if (!root) {
    void vscode.window.showErrorMessage("Bynk: no bynk.toml found in the workspace.");
    return;
  }
  const args = fetchArgs();
  const r = await run(root.fsPath, "Bynk: fetching dependencies…", args);
  if (!r) return;
  await refreshViews(lenses);
  if (r.code !== 0) {
    await reportFailure("`bynk fetch` failed", args, r);
    return;
  }
  let fetched = 0;
  try {
    fetched = (JSON.parse(r.stdout) as { fetched?: unknown[] }).fetched?.length ?? 0;
  } catch {
    /* an older driver prints no document: the success still stands */
  }
  void vscode.window.showInformationMessage(
    fetched > 0 ? `Bynk: fetched ${fetched} package${fetched === 1 ? "" : "s"}.` : "Bynk: dependencies are up to date.",
  );
}

/** Rewrite one dependency's version range in its `bynk.toml`, then fetch. */
async function upgrade(
  lenses: UpgradeLensProvider,
  uri: string,
  dep: ManifestDependency,
  target: string,
): Promise<void> {
  const document = await vscode.workspace.openTextDocument(vscode.Uri.parse(uri));
  const line = document.lineAt(dep.rangeLine).text;
  // The text may have moved since the lens was drawn: only rewrite what is still there.
  if (line.slice(dep.rangeStart, dep.rangeEnd) !== dep.range) {
    void vscode.window.showWarningMessage("Bynk: bynk.toml changed since this lens was drawn; try again.");
    lenses.invalidate();
    return;
  }
  const edit = new vscode.WorkspaceEdit();
  edit.replace(
    document.uri,
    new vscode.Range(dep.rangeLine, dep.rangeStart, dep.rangeLine, dep.rangeEnd),
    rangeFor(target),
  );
  if (!(await vscode.workspace.applyEdit(edit))) return;
  await document.save();
  await fetchDependencies(lenses);
}

async function publish(lenses: UpgradeLensProvider): Promise<void> {
  const root = await findProjectRoot();
  if (!root) {
    void vscode.window.showErrorMessage("Bynk: no bynk.toml found in the workspace.");
    return;
  }
  // DECISION C: plan first, always.
  const planArgs = publishPlanArgs();
  const planned = await run(root.fsPath, "Bynk: planning a publish…", planArgs);
  if (!planned) return;
  const plan = planned.code === 0 ? parsePublishPlan(planned.stdout) : undefined;
  if (!plan) {
    await reportFailure("`bynk publish --dry-run` could not plan a publish", planArgs, planned);
    return;
  }
  const out = channel();
  out.clear();
  out.appendLine(publishPlanText({ ...plan, files: [] }));
  out.appendLine("\nFiles:");
  out.appendLine(publishFilesText(plan));

  // "Show Full File List" opens the channel and asks again, so looking at the
  // files never costs a second dry run.
  let choice: string | undefined;
  do {
    choice = await vscode.window.showWarningMessage(
      `Publish ${plan.package} ${plan.version} to ${plan.registry}?`,
      { modal: true, detail: `${publishPlanText(plan)}\n\nA published version is permanent: it can be yanked, never replaced.` },
      "Publish",
      "Show Full File List",
    );
    if (choice === "Show Full File List") out.show();
  } while (choice === "Show Full File List");
  if (choice !== "Publish") return;

  // Bound to the plan just confirmed: a tree that changed since is refused.
  const args = publishArgs(plan.checksum);
  const r = await run(root.fsPath, `Bynk: publishing ${plan.package} ${plan.version}…`, args);
  if (!r) return;
  if (r.code !== 0) {
    await reportFailure(`\`bynk publish\` failed for ${plan.package} ${plan.version}`, args, r);
    return;
  }
  await refreshViews(lenses);
  void vscode.window.showInformationMessage(`Bynk: published ${plan.package} ${plan.version} to ${plan.registry}.`);
}
