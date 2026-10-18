// The web extension's manifest contract (#2102): every command the web entry
// (src/web/extension.ts) doesn't register is hidden on the web by a `!isWeb`
// clause wherever the manifest surfaces it, so vscode.dev never offers a Bynk
// command that fails with "command not found". Reads package.json from disk (no
// `vscode`), so it runs under plain Mocha as well as in the extension host.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

/** The commands src/web/extension.ts registers. Keep in step with it. */
const WEB_COMMANDS = new Set(["bynk.showServerOutput"]);

/**
 * Whether a `when` clause hides its item on the web: it carries `!isWeb`, or
 * it is `false` (hidden everywhere, like #2114's status-bar-only
 * `bynk.devServerMenu`).
 */
const hiddenOnWeb = (when: string | undefined): boolean =>
  when === "false" || (when?.includes("!isWeb") ?? false);

interface Gated {
  command?: string;
  when?: string;
}

const manifest = JSON.parse(
  fs.readFileSync(path.resolve(__dirname, "../../../package.json"), "utf8"),
);
const contributes = manifest.contributes;

describe("Bynk manifest — web extension (#2102)", () => {
  it("declares the browser entry point next to the Node one", () => {
    assert.strictEqual(manifest.browser, "./out/web/extension.js");
    assert.strictEqual(manifest.main, "./out/extension.js");
  });

  it("gives every Node-only command a palette entry hidden on the web", () => {
    const palette: Gated[] = contributes.menus.commandPalette;
    for (const { command } of contributes.commands as { command: string }[]) {
      if (WEB_COMMANDS.has(command)) continue;
      const entry = palette.find((m) => m.command === command);
      assert.ok(entry, `${command} has a commandPalette entry (or it shows on the web)`);
      assert.ok(hiddenOnWeb(entry.when), `${command}'s palette entry is hidden on the web`);
    }
  });

  it("hides #2114's dev-server commands on the web", () => {
    const palette: Gated[] = contributes.menus.commandPalette;
    for (const command of ["bynk.startDevServer", "bynk.stopDevServer", "bynk.restartDevServer"]) {
      const entry = palette.find((m) => m.command === command);
      assert.ok(entry?.when?.includes("!isWeb"), `${command} carries !isWeb`);
    }
  });

  it("hides every Node-only menu item and keybinding on the web", () => {
    const surfaces: [string, Gated[]][] = [
      ...Object.entries(contributes.menus as Record<string, Gated[]>),
      ["keybindings", contributes.keybindings as Gated[]],
    ];
    for (const [where, items] of surfaces) {
      for (const item of items) {
        if (!item.command || WEB_COMMANDS.has(item.command)) continue;
        assert.ok(
          hiddenOnWeb(item.when),
          `${item.command} in ${where} is hidden on the web (when: ${item.when})`,
        );
      }
    }
  });

  it("does not hide the web's own commands", () => {
    const palette: Gated[] = contributes.menus.commandPalette;
    for (const command of WEB_COMMANDS) {
      const entry = palette.find((m) => m.command === command);
      assert.ok(!entry?.when?.includes("!isWeb"), `${command} stays visible on the web`);
    }
  });

  it("hides the views, and their welcome content, that need bynk-lsp on the web", () => {
    // #2112's Project sidebar is built from `bynk-lsp` requests the web worker
    // doesn't answer. With every view hidden, VS Code hides the container too.
    const views = Object.values((contributes.views ?? {}) as Record<string, { id: string; when?: string }[]>).flat();
    for (const view of views) {
      assert.ok(view.when?.includes("!isWeb"), `view ${view.id} is hidden on the web`);
    }
    for (const welcome of (contributes.viewsWelcome ?? []) as { view: string; when?: string }[]) {
      assert.ok(welcome.when?.includes("!isWeb"), `${welcome.view}'s welcome is hidden on the web`);
    }
  });

  it("hides the walkthrough, whose steps run Node-only commands, on the web", () => {
    for (const walkthrough of contributes.walkthroughs as { id: string; when?: string }[]) {
      assert.strictEqual(walkthrough.when, "!isWeb", `${walkthrough.id} is hidden on the web`);
    }
  });
});
