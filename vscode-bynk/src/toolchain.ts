// Provisioning for the `bynk` driver and `bynkc` compiler (the language server
// is `server.ts`'s). Without this a user who installs the extension still needs
// a Rust toolchain and `cargo install` before check, test, `bynk dev` or deploy
// will run.
//
// The policy mirrors the server's: anything the user supplied wins. A
// `bynk.bynkPath`, or `bynk` already on PATH, is left alone and nothing is
// downloaded. Otherwise the extension fetches the checksum-verified release
// binaries into its global storage, uses them for everything it runs, and
// appends their directory to the PATH of integrated terminals so `bynk dev` and
// `bynk deploy` work there too. Appended, not prepended: a `bynk` the user
// installs later takes precedence.

import * as vscode from "vscode";

import {
  cachedToolchain,
  downloadToolchain,
  targetTriple,
  userSuppliedBynk,
} from "./server";

/** The managed `bynk` driver in use, when the user supplied none. */
let managedBynk: string | undefined;

/** The extension-managed `bynk` driver to run, or `undefined` when the user's
 *  own (`bynk.bynkPath` or PATH) is in charge. */
export function managedBynkPath(): string | undefined {
  return managedBynk;
}

/** Point the extension, and the PATH of new terminals, at the cached toolchain
 *  when the user supplied no `bynk`. Returns whether a managed one is in use. */
function applyManaged(context: vscode.ExtensionContext): boolean {
  const cached = userSuppliedBynk() ? undefined : cachedToolchain(context);
  managedBynk = cached?.bynk;
  const terminals = context.environmentVariableCollection;
  terminals.clear();
  if (cached) {
    terminals.description = "Puts the Bynk toolchain downloaded by the Bynk extension on PATH.";
    terminals.append("PATH", (process.platform === "win32" ? ";" : ":") + cached.dir);
  }
  return cached !== undefined;
}

/** The install in flight, so a second request (the command, or the button on
 *  the "could not run bynk" error, during the activation fetch) joins it
 *  instead of starting a competing download. */
let inFlight: Promise<InstallResult> | undefined;

type InstallResult = { ok: true } | { ok: false; message: string };

/** Make a `bynk` available: nothing to do when the user has one or a managed
 *  copy is cached already, otherwise download it. A failure is returned as a
 *  message (and logged) rather than thrown; `interactive` decides whether the
 *  caller shows it. */
export function ensureToolchain(
  context: vscode.ExtensionContext,
  output: vscode.OutputChannel,
): Promise<InstallResult> {
  inFlight ??= install(context, output).finally(() => {
    inFlight = undefined;
  });
  return inFlight;
}

async function install(
  context: vscode.ExtensionContext,
  output: vscode.OutputChannel,
): Promise<InstallResult> {
  if (userSuppliedBynk() || applyManaged(context)) return { ok: true };
  if (!targetTriple()) {
    return {
      ok: false,
      message:
        `no prebuilt Bynk toolchain for ${process.platform}/${process.arch}; ` +
        "build it with `cargo install --path bynk --path bynkc` and set `bynk.bynkPath`",
    };
  }
  try {
    await downloadToolchain(context, output);
  } catch (e) {
    output.appendLine(`[toolchain] download failed: ${String(e)}`);
    return { ok: false, message: String(e instanceof Error ? e.message : e) };
  }
  applyManaged(context);
  return { ok: true };
}

export function registerToolchain(
  context: vscode.ExtensionContext,
  output: vscode.OutputChannel,
): void {
  applyManaged(context);
  context.subscriptions.push(
    vscode.commands.registerCommand("bynk.installToolchain", async () => {
      if (userSuppliedBynk()) {
        void vscode.window.showInformationMessage(
          "Bynk: a `bynk` is already available (the `bynk.bynkPath` setting or your PATH), so there is nothing to install.",
        );
        return;
      }
      const result = await ensureToolchain(context, output);
      if (result.ok) {
        void vscode.window.showInformationMessage(
          "Bynk: toolchain ready. New terminals have `bynk` and `bynkc` on PATH.",
        );
      } else {
        const pick = await vscode.window.showErrorMessage(
          `Bynk: couldn't install the toolchain (${result.message}).`,
          "Show Output",
        );
        if (pick) output.show();
      }
    }),
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("bynk.bynkPath")) {
        applyManaged(context);
        autoInstall(context, output);
      }
    }),
  );
  autoInstall(context, output);
}

/** Fetch the toolchain in the background, as the server is: on activation, and
 *  again when `bynk.bynkPath` is cleared. Quiet on failure (a release cut before
 *  these binaries were published has none): the first command that needs `bynk`
 *  offers the install. */
function autoInstall(
  context: vscode.ExtensionContext,
  output: vscode.OutputChannel,
): void {
  if (
    userSuppliedBynk() ||
    !vscode.workspace.getConfiguration("bynk").get<boolean>("toolchain.autoInstall", true)
  ) {
    return;
  }
  void ensureToolchain(context, output).then((r) => {
    if (!r.ok) output.appendLine(`[toolchain] not installed: ${r.message}`);
  });
}
