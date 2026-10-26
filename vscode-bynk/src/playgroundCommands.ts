// #2480: "Copy Playground Link" and "Open in Playground". Both use only the VS
// Code API, so the desktop and web entries register them alike (the encoding is
// in `playgroundLink.ts`).
//
// What is shared (DECISION B): the selection, when there is one; otherwise the
// whole file, but only for a single-file program. A file inside a `bynk.toml`
// project depends on its sibling units, so sharing it whole would not open in
// the playground; there the command asks for a selection instead, and warns
// when the selection names a unit it does not import.

import * as vscode from "vscode";

import { foreignReferences, playgroundLink } from "./playgroundLink";

/** Whether `uri` sits under a folder holding a `bynk.toml`. */
async function inProject(uri: vscode.Uri): Promise<boolean> {
  let dir = uri;
  for (;;) {
    const parent = vscode.Uri.joinPath(dir, "..");
    if (parent.path === dir.path) return false;
    dir = parent;
    try {
      await vscode.workspace.fs.stat(vscode.Uri.joinPath(dir, "bynk.toml"));
      return true;
    } catch {
      // No manifest here; keep climbing.
    }
  }
}

/** The link for the active editor, or `undefined` after telling the user why not. */
async function linkForActiveEditor(): Promise<string | undefined> {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== "bynk") {
    void vscode.window.showInformationMessage("Open a Bynk file to share it in the playground.");
    return undefined;
  }
  const { document, selection } = editor;
  const selected = document.getText(selection);
  let source: string;
  if (selected.trim() !== "") {
    source = selected;
    const foreign = foreignReferences(document.getText(), selected);
    if (foreign.length > 0) {
      void vscode.window.showWarningMessage(
        `The selection refers to ${foreign.join(", ")} from other files, which the playground ` +
          "link does not include. It may not compile on its own.",
      );
    }
  } else if (await inProject(document.uri)) {
    void vscode.window.showInformationMessage(
      "This file is part of a project, so it depends on other files. Select the code to share.",
    );
    return undefined;
  } else {
    source = document.getText();
  }
  const link = await playgroundLink(source);
  if (!link.ok) {
    void vscode.window.showWarningMessage(
      `That snippet is too large for a playground link (${link.length} of ${link.max} characters ` +
        "once compressed). Select less code.",
    );
    return undefined;
  }
  return link.url;
}

export function registerPlaygroundCommands(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("bynk.copyPlaygroundLink", async () => {
      const url = await linkForActiveEditor();
      if (!url) return;
      await vscode.env.clipboard.writeText(url);
      void vscode.window.showInformationMessage("Playground link copied.");
    }),
    vscode.commands.registerCommand("bynk.openInPlayground", async () => {
      const url = await linkForActiveEditor();
      if (url) await vscode.env.openExternal(vscode.Uri.parse(url, true));
    }),
  );
}
