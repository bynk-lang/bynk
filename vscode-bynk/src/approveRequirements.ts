// #2473: review and approve the npm packages that dependencies' adapters ask
// for (`bynk approve`), from the editor.
//
// A pending requirement is found where the driver already says so (DECISION B):
// a `bynk check`, `dev` or `deploy` whose output reports
// `bynk.requires.unapproved` — never by polling. On a hit this asks the driver
// for its listing (`bynk approve --list --format json`) and surfaces what is
// pending as a Problems entry on `bynk.lock`, a badge on the Project view, and
// one notification with **Review…**. The review panel (host-rendered through
// `cliView.ts`, like the Doctor) approves per package (DECISION A); approve-all
// is a secondary button that asks for an explicit confirmation first, because
// approving runs someone else's code on the next `npm install`.

import * as vscode from "vscode";

import { BynkNotFoundError, findProjectRoot, runBynk } from "./cli";
import { isActionMessage, renderActionHtml } from "./cliView";
import { reportBynkNotFound } from "./doctor";
import {
  APPROVE_CSS,
  ApprovalListing,
  ApprovedMemory,
  annotateChanges,
  approveArgs,
  parseApprovalListing,
  pendingMessage,
  pendingPackages,
  type PendingPackage,
  pendingSignature,
  rememberApproved,
  renderApproveHtml,
  reportsUnapproved,
} from "./approveReport";

const MEMORY_KEY = "bynk.approvedNpmRanges";

/** What the editor shows pending requirements through. */
export interface ApprovalState {
  /** Fires when the pending count may have changed. */
  readonly onDidChange: vscode.Event<void>;
  /** The number of packages awaiting approval, per the last listing. */
  pendingCount(): number;
}

let panel: vscode.WebviewPanel | undefined;
let current: { listing: ApprovalListing; cwd: string } | undefined;
const lastNotified = new Map<string, string>();
const changed = new vscode.EventEmitter<void>();
let pending = 0;
let problems: vscode.DiagnosticCollection | undefined;
let memento: vscode.Memento | undefined;

export function registerApproveRequirements(context: vscode.ExtensionContext): ApprovalState {
  memento = context.workspaceState;
  problems = vscode.languages.createDiagnosticCollection("bynk-approve");
  context.subscriptions.push(
    changed,
    problems,
    vscode.commands.registerCommand("bynk.approveRequirements", () => openReview()),
    // A check run as a task prints to a terminal this cannot read; its failure
    // is the cue to ask the driver (the listing is a local read of the lock).
    vscode.tasks.onDidEndTaskProcess((e) => {
      if (e.execution.task.definition.type === "bynkc" && e.exitCode !== undefined && e.exitCode !== 0) {
        void noteDriverFailure();
      }
    }),
  );
  // A test seam (never registered outside the extension tests, since it would
  // let another extension approve without the modal): drive a panel action,
  // answering the approve-all confirmation with `confirm`.
  if (context.extensionMode === vscode.ExtensionMode.Test) {
    context.subscriptions.push(
      vscode.commands.registerCommand(
        "bynk._approveAction",
        (action: string, args: Record<string, string>, confirm: boolean) =>
          onAction(action, args ?? {}, async () => confirm === true),
      ),
    );
  }
  return { onDidChange: changed.event, pendingCount: () => pending };
}

/** A `bynk check`/`dev`/`deploy` run printed `output` in `cwd`: when it reports
 *  an unapproved requirement, read the driver's listing and surface it. */
export function noteDriverOutput(cwd: string, output: string): void {
  if (reportsUnapproved(output)) void refresh(cwd, true);
}

async function noteDriverFailure(): Promise<void> {
  const root = await findProjectRoot();
  if (root) await refresh(root.fsPath, true);
}

/** Ask the driver for its listing. `undefined` when it cannot give one (no
 *  `bynk.lock` yet, a `bynk` too old for `--format json`, no `bynk`). */
async function listing(cwd: string): Promise<ApprovalListing | undefined> {
  try {
    const result = await runBynk(["approve", "--list", "--format", "json"], cwd);
    if (result.code !== 0) return undefined;
    return parseApprovalListing(result.stdout);
  } catch {
    return undefined;
  }
}

/** Read the listing and publish it, notifying (once per distinct pending set)
 *  when asked. */
async function refresh(cwd: string, notify: boolean): Promise<void> {
  const raw = await listing(cwd);
  if (raw) await publish(cwd, raw, notify);
}

/** Annotate `raw` with what moved since it was last seen, then surface it:
 *  Problems, badge, and (when `notify`) one notification per pending set.
 *  Returns the annotated listing. */
async function publish(cwd: string, raw: ApprovalListing, notify: boolean): Promise<ApprovalListing> {
  const memory = memento?.get<ApprovedMemory>(`${MEMORY_KEY}:${cwd}`) ?? {};
  const annotated = annotateChanges(raw, memory);
  await memento?.update(`${MEMORY_KEY}:${cwd}`, rememberApproved(raw, memory));
  current = { listing: annotated, cwd };
  pending = pendingPackages(annotated).length;
  await publishProblems(cwd, annotated);
  changed.fire();
  const signature = pendingSignature(annotated);
  if (pending === 0) {
    lastNotified.delete(cwd);
  } else if (notify && signature !== lastNotified.get(cwd)) {
    lastNotified.set(cwd, signature);
    void vscode.window.showWarningMessage(pendingMessage(annotated), "Review…").then((choice) => {
      if (choice) void openReview();
    });
  }
  return annotated;
}

/** One Problems entry per pending package, on its row in `bynk.lock`. */
async function publishProblems(cwd: string, l: ApprovalListing): Promise<void> {
  if (!problems) return;
  problems.clear();
  const packages = pendingPackages(l);
  if (packages.length === 0) return;
  const uri = vscode.Uri.joinPath(vscode.Uri.file(cwd), "bynk.lock");
  let lines: string[] = [];
  try {
    lines = new TextDecoder().decode(await vscode.workspace.fs.readFile(uri)).split(/\r?\n/);
  } catch {
    /* no readable lock: the entry sits at the top of the file */
  }
  problems.set(
    uri,
    packages.map((p) => {
      const at = Math.max(0, lines.findIndex((x) => x.trim() === `package = "${p.package}"`));
      const ranges = p.rows.map((r) => r.range).join(", ");
      const d = new vscode.Diagnostic(
        new vscode.Range(at, 0, at, Math.max(1, lines[at]?.length ?? 1)),
        `npm package \`${p.package}\` ${ranges} awaits approval (a build reaching it fails until approved); review it with “Bynk: Review Pending npm Requirements”.`,
        vscode.DiagnosticSeverity.Warning,
      );
      d.code = "bynk.requires.unapproved";
      d.source = "bynk approve";
      return d;
    }),
  );
}

async function openReview(): Promise<void> {
  const root = await findProjectRoot();
  const cwd = root?.fsPath ?? vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? process.cwd();
  let l: ApprovalListing | undefined;
  try {
    const result = await runBynk(["approve", "--list", "--format", "json"], cwd);
    if (result.code !== 0) {
      void vscode.window.showInformationMessage(
        `Bynk: nothing to review. ${result.stderr.trim() || "Run `bynk fetch` first."}`,
      );
      return;
    }
    l = parseApprovalListing(result.stdout);
  } catch (e) {
    if (e instanceof BynkNotFoundError) await reportBynkNotFound();
    else void vscode.window.showErrorMessage(`Bynk: could not list npm requirements: ${String(e)}`);
    return;
  }
  await show(cwd, l);
}

/** Publish `raw`, then draw it in the (single) review panel. */
async function show(cwd: string, raw: ApprovalListing): Promise<void> {
  const annotated = await publish(cwd, raw, false);
  if (!panel) {
    panel = vscode.window.createWebviewPanel("bynkApprove", "Bynk: npm Requirements", vscode.ViewColumn.Active, {
      enableScripts: true,
    });
    panel.onDidDispose(() => {
      panel = undefined;
    });
    panel.webview.onDidReceiveMessage((m: unknown) => {
      if (isActionMessage(m)) void onAction(m.action, m.args);
    });
  }
  panel.webview.html = renderActionHtml(panel.webview, "Bynk: npm Requirements", renderApproveHtml(annotated), APPROVE_CSS);
  panel.reveal();
}

/** Ask whether to approve all `names`: a modal naming them. */
async function confirmApproveAll(names: string[]): Promise<boolean> {
  const all = `Approve all ${names.length}`;
  const choice = await vscode.window.showWarningMessage(
    `Approve all ${names.length} npm packages?`,
    {
      modal: true,
      detail:
        `${names.join(", ")}\n\nApproving lets each package, and whatever it installs, run on the next \`npm install\`. ` +
        "Approve them one at a time if you have not reviewed every one.",
    },
    all,
  );
  return choice === all;
}

async function onAction(
  action: string,
  args: Record<string, string>,
  confirmAll: (names: string[]) => Promise<boolean> = confirmApproveAll,
): Promise<void> {
  if (!current) return;
  const { cwd } = current;
  switch (action) {
    case "refresh":
      await openReview();
      break;
    case "open":
      if (args.url && /^https:\/\/www\.npmjs\.com\/package\//.test(args.url)) {
        void vscode.env.openExternal(vscode.Uri.parse(args.url));
      }
      break;
    case "approve": {
      // Only a package the current listing has pending, and only the ranges
      // the review showed.
      const name = args.package;
      const shown = pendingPackages(current.listing).find((p) => p.package === name);
      if (!name || !shown) return;
      if (!(await unchanged(cwd, [shown]))) return;
      await approve(cwd, [name]);
      break;
    }
    case "approveAll": {
      const shown = pendingPackages(current.listing);
      if (shown.length === 0) return;
      if (!(await confirmAll(shown.map((p) => p.package)))) return;
      if (!(await unchanged(cwd, shown))) return;
      // By name, never `--all`: the modal named exactly these packages.
      await approve(cwd, shown.map((p) => p.package));
      break;
    }
  }
}

/** Whether the driver still lists exactly `shown` as pending for those
 *  packages. A `fetch` or `update` run elsewhere while the review was open can
 *  add a range or a package the user never saw; if so, redraw the review and
 *  approve nothing. */
async function unchanged(cwd: string, shown: PendingPackage[]): Promise<boolean> {
  const fresh = await listing(cwd);
  const key = (ps: PendingPackage[]) => ps.map((p) => `${p.package}@${p.rows.map((r) => r.range).sort().join(",")}`).sort().join("\n");
  const now = fresh ? pendingPackages(fresh).filter((p) => shown.some((s) => s.package === p.package)) : [];
  if (fresh && key(now) === key(shown)) return true;
  void vscode.window.showWarningMessage(
    "Bynk: what awaits approval changed while you were reviewing it. Nothing was approved; review it again.",
  );
  await openReview();
  return false;
}

/** Run `bynk approve <package>` for each of `names`, then redraw from the
 *  driver. */
async function approve(cwd: string, names: string[]): Promise<void> {
  try {
    for (const name of names) {
      const result = await runBynk(approveArgs(name), cwd);
      if (result.code !== 0) {
        void vscode.window.showErrorMessage(
          `Bynk: approving \`${name}\` failed. ${result.stderr.trim() || `bynk exited with ${result.code}.`}`,
        );
        await openReview();
        return;
      }
    }
    void vscode.window.showInformationMessage(
      `Bynk: approved ${names.map((n) => `\`${n}\``).join(", ")}.`,
    );
  } catch (e) {
    if (e instanceof BynkNotFoundError) await reportBynkNotFound();
    else void vscode.window.showErrorMessage(`Bynk: approval failed: ${String(e)}`);
    return;
  }
  await openReview();
}
