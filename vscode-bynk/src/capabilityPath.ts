// #2469: "Show capability path" — the server's code action on a `given` entry,
// which lists every chain from the declaration to the capability's use.
//
// The action carries a client-side command rather than an edit, so the server
// offers it only to a client that advertises `experimental.capabilityPathCommand`
// (see `capabilityPathClientFeature`). The command's arguments are plain JSON —
// the entry's URI and position, then the sites of every step — and the editor's
// own references peek (`editor.action.showReferences`) wants real `Uri`,
// `Position` and `Location` objects, so the command revives them.

import * as vscode from "vscode";
import type { StaticFeature } from "vscode-languageclient/node";

import { parseArguments } from "./capabilityPathCore";

/** The command id; the server's code action names it (`SHOW_COMMAND`). */
export const SHOW_CAPABILITY_PATH_COMMAND = "bynk.showCapabilityPath";

/** The client capability that tells the server this client runs
 *  `bynk.showCapabilityPath`, so it should offer the action. */
export function capabilityPathClientFeature(): StaticFeature {
  return {
    fillClientCapabilities(capabilities) {
      capabilities.experimental = {
        ...(capabilities.experimental as Record<string, unknown> | undefined),
        capabilityPathCommand: true,
      };
    },
    initialize() {},
    getState: () => ({ kind: "static" }),
    clear() {},
  };
}

export function registerCapabilityPath(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.commands.registerCommand(
      SHOW_CAPABILITY_PATH_COMMAND,
      async (uri?: unknown, position?: unknown, locations?: unknown) => {
        const args = parseArguments(uri, position, locations);
        if (!args) {
          return;
        }
        // The peek wants real `Uri`/`Position`/`Location` objects; the command's
        // arguments arrive as plain JSON.
        await vscode.commands.executeCommand(
          "editor.action.showReferences",
          vscode.Uri.parse(args.uri),
          new vscode.Position(args.position.line, args.position.character),
          args.locations.map(
            (l) =>
              new vscode.Location(
                vscode.Uri.parse(l.uri),
                new vscode.Range(
                  l.range.start.line,
                  l.range.start.character,
                  l.range.end.line,
                  l.range.end.character,
                ),
              ),
          ),
        );
      },
    ),
  );
}
