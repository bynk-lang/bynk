// #2095: `Bynk: Doctor`, an environment readiness checklist.
//
// Runs `bynk doctor --format json` in the project and renders the verdict in a
// webview (DECISION A; host-rendered through `cliView.ts`, the shared CSP and
// nonce posture), grouped by capability, with each gap's remedy as buttons:
// run the command in a visible terminal after a confirmation, open a link or a
// setting, or copy the remedy (DECISION B: never silently). It is not run on
// activation (DECISION C): failures elsewhere offer it, through `offerDoctor`,
// scoped with `--only` where the failing capability is known. When `bynk`
// itself cannot be started the doctor cannot run either, so that case gets a
// fixed message instead (`reportBynkNotFound`).

import * as vscode from "vscode";

import { BynkNotFoundError, findProjectRoot, runBynk } from "./cli";
import { isActionMessage, renderActionHtml } from "./cliView";
import { DOCTOR_CSS, DoctorReport, offeredCommands, parseDoctorReport, renderDoctorHtml } from "./doctorReport";
import { bynkPath } from "./tasks";
import { targetTriple } from "./server";

/** A `bynk doctor --only` capability. */
export type Capability = "compile" | "test" | "deploy" | "editor" | "build";

const CAPABILITIES: readonly string[] = ["compile", "test", "deploy", "editor", "build"];

let panel: vscode.WebviewPanel | undefined;
let current: { report: DoctorReport; cwd: string; only?: string } | undefined;

export function registerDoctor(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("bynk.doctor", (args?: { only?: string }) =>
      runDoctor(typeof args?.only === "string" && CAPABILITIES.includes(args.only) ? args.only : undefined),
    ),
  );
}

/** Show `message` as an error with a "Run Doctor" action, scoped to `only`. */
export async function offerDoctor(message: string, only?: Capability): Promise<void> {
  const choice = await vscode.window.showErrorMessage(message, "Run Doctor");
  if (choice) await vscode.commands.executeCommand("bynk.doctor", only ? { only } : undefined);
}

/** The fixed message for a `bynk` that cannot be started at all, where the
 *  doctor (which runs through `bynk`) cannot help. */
export async function reportBynkNotFound(detail?: string): Promise<void> {
  const download = "Install Toolchain";
  const install = "Install Guide";
  const settings = "Open Settings";
  const choice = await vscode.window.showErrorMessage(
    `Bynk: could not run \`${bynkPath()}\`${detail ? ` (${detail})` : ""}. Install the Bynk toolchain, or point the \`bynk.bynkPath\` setting at the \`bynk\` executable.`,
    ...(targetTriple() ? [download] : []),
    install,
    settings,
  );
  if (choice === download) {
    void vscode.commands.executeCommand("bynk.installToolchain");
  } else if (choice === install) {
    void vscode.env.openExternal(vscode.Uri.parse("https://bynk-lang.org/book/introduction/install/"));
  } else if (choice === settings) {
    void vscode.commands.executeCommand("workbench.action.openSettings", "bynk.bynkPath");
  }
}

async function runDoctor(only?: string): Promise<void> {
  const root = await findProjectRoot();
  const cwd = root?.fsPath ?? vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? process.cwd();
  const args = ["doctor", "--format", "json", ...(only ? ["--only", only] : []), "."];
  let result;
  try {
    result = await vscode.window.withProgress(
      { location: vscode.ProgressLocation.Notification, title: "Bynk: checking your setup…", cancellable: true },
      (_p, token) => runBynk(args, cwd, token),
    );
  } catch (e) {
    if (e instanceof BynkNotFoundError) await reportBynkNotFound();
    else void vscode.window.showErrorMessage(`Bynk: the doctor failed: ${String(e)}`);
    return;
  }
  let report: DoctorReport;
  try {
    // `--only` exits non-zero when its tools are missing; the report is still there.
    report = parseDoctorReport(result.stdout);
  } catch {
    void vscode.window.showErrorMessage(
      `Bynk: \`bynk doctor\` gave no report. ${result.stderr.trim() || `It exited with ${result.code}.`}`,
    );
    return;
  }
  current = { report, cwd, only };
  if (!panel) {
    panel = vscode.window.createWebviewPanel("bynkDoctor", "Bynk Doctor", vscode.ViewColumn.Active, {
      enableScripts: true,
    });
    panel.onDidDispose(() => {
      panel = undefined;
      current = undefined;
    });
    panel.webview.onDidReceiveMessage((m: unknown) => {
      if (isActionMessage(m)) void onAction(m.action, m.args);
    });
  }
  panel.webview.html = renderActionHtml(panel.webview, "Bynk Doctor", renderDoctorHtml(report, { only }), DOCTOR_CSS);
  panel.reveal();
}

async function onAction(action: string, args: Record<string, string>): Promise<void> {
  if (!current) return;
  switch (action) {
    case "rerun":
      await runDoctor(current.only);
      break;
    case "copy":
      if (args.text) {
        await vscode.env.clipboard.writeText(args.text);
        void vscode.window.showInformationMessage("Copied the remedy.");
      }
      break;
    case "open":
      if (args.url && /^https?:\/\//.test(args.url)) void vscode.env.openExternal(vscode.Uri.parse(args.url));
      break;
    case "setting":
      if (args.setting?.startsWith("bynk.")) {
        void vscode.commands.executeCommand("workbench.action.openSettings", args.setting);
      }
      break;
    case "run": {
      // Only a command the current report offers, and only after the user has
      // seen it: remedies install software (DECISION B).
      const command = args.command;
      if (!command || !offeredCommands(current.report).has(command)) return;
      const run = "Run in Terminal";
      const choice = await vscode.window.showWarningMessage(
        `Run \`${command}\`?`,
        { modal: true, detail: "It runs in a new terminal in your project, where you can watch it." },
        run,
      );
      if (choice !== run) return;
      const terminal = vscode.window.createTerminal({ name: "Bynk Doctor", cwd: current.cwd });
      terminal.show();
      terminal.sendText(command);
      break;
    }
  }
}
