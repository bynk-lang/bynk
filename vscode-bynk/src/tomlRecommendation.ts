// #2096: suggest a schema-aware TOML extension the first time a `bynk.toml` is
// opened without one. A recommendation, not a dependency: the extension works
// the same without it, `bynk.toml` just gets no completion, hover or validation.
// "Don't show again" is remembered across sessions.

import * as vscode from "vscode";

import {
  TOML_EXTENSION_ID,
  TOML_RECOMMENDATION_DISMISSED,
  shouldRecommendTomlExtension,
} from "./manifestSchema";

export function registerTomlRecommendation(context: vscode.ExtensionContext): void {
  let shownThisSession = false;

  const consider = (doc: vscode.TextDocument): void => {
    const show = shouldRecommendTomlExtension({
      fileName: doc.fileName,
      tomlExtensionInstalled: vscode.extensions.getExtension(TOML_EXTENSION_ID) !== undefined,
      dismissed: context.globalState.get<boolean>(TOML_RECOMMENDATION_DISMISSED, false),
      shownThisSession,
    });
    if (!show) return;
    shownThisSession = true;
    void recommend(context);
  };

  context.subscriptions.push(vscode.workspace.onDidOpenTextDocument(consider));
  for (const doc of vscode.workspace.textDocuments) consider(doc);
}

async function recommend(context: vscode.ExtensionContext): Promise<void> {
  const install = "Show Even Better TOML";
  const never = "Don't Show Again";
  const choice = await vscode.window.showInformationMessage(
    "Bynk: install a TOML extension that reads JSON Schemas (such as Even Better TOML) " +
      "for completion, hover and validation in bynk.toml.",
    install,
    never,
  );
  if (choice === install) {
    await vscode.commands.executeCommand("extension.open", TOML_EXTENSION_ID);
  } else if (choice === never) {
    await context.globalState.update(TOML_RECOMMENDATION_DISMISSED, true);
  }
}
