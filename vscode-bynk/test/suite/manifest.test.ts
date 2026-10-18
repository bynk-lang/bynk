import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import * as vscode from "vscode";

const EXT_ID = "bynk.bynk-vscode";

interface MenuItem {
  command?: string;
  when?: string;
  group?: string;
  submenu?: string;
}
interface Keybinding {
  command: string;
  key?: string;
  mac?: string;
  when?: string;
}

function contributes(): {
  menus: Record<string, MenuItem[]>;
  keybindings: Keybinding[];
} {
  const ext = vscode.extensions.getExtension(EXT_ID);
  assert.ok(ext, `extension ${EXT_ID} is installed`);
  const c = ext.packageJSON.contributes ?? {};
  return {
    menus: c.menus ?? {},
    keybindings: c.keybindings ?? [],
  };
}

// The editor-currency slice-5 UI surface: the menus, keybindings, and
// language-configuration enrichment are declarative, so their contract is the
// manifest itself. These pin that the contributions exist and stay gated —
// a dropped `when` clause would flood every workspace's command palette with
// Bynk commands, the regression this guards.
describe("Bynk manifest — UI surface (slice 5)", () => {
  it("gates the Bynk commands in the command palette", () => {
    const { menus } = contributes();
    const palette = menus.commandPalette ?? [];
    const gate = "editorLangId == bynk || bynk.hasProject";

    // #2102: Node-only commands are also hidden in the web extension.
    for (const command of [
      "bynk.newContext",
      "bynk.openProjectConfig",
      "bynk.runTests",
      "bynk.debugTests",
      "bynk.restartServer",
      "bynk.downloadServer",
    ]) {
      const entry = palette.find((m) => m.command === command);
      assert.ok(entry, `${command} has a commandPalette entry`);
      assert.strictEqual(
        entry.when,
        `(${gate}) && !isWeb`,
        `${command} is gated on a Bynk context, off the web`,
      );
    }
    // The web entry registers this one too, so it carries no `!isWeb`.
    const output = palette.find((m) => m.command === "bynk.showServerOutput");
    assert.strictEqual(output?.when, gate, "bynk.showServerOutput is gated on a Bynk context");

    // The bootstrap command must stay available in a non-Bynk workspace, so it
    // is not gated on a Bynk context, only hidden on the web (#2102), where
    // there is no filesystem to scaffold into.
    const newProject = palette.find((m) => m.command === "bynk.newProject");
    assert.strictEqual(newProject?.when, "!isWeb", "bynk.newProject is gated only off the web");
  });

  it("offers run/debug from the editor title and context menus", () => {
    const { menus } = contributes();
    for (const where of ["editor/title/run", "editor/context"]) {
      const items = menus[where] ?? [];
      for (const command of ["bynk.runTests", "bynk.debugTests"]) {
        const entry = items.find((m) => m.command === command);
        assert.ok(entry, `${command} appears in ${where}`);
        assert.strictEqual(
          entry.when,
          "editorLangId == bynk && !isWeb",
          `${command} in ${where} is scoped to bynk editors, off the web`,
        );
      }
    }
  });

  it("offers New Context / Open Project Config in the explorer", () => {
    const { menus } = contributes();
    const items = menus["explorer/context"] ?? [];
    for (const command of ["bynk.newContext", "bynk.openProjectConfig"]) {
      const entry = items.find((m) => m.command === command);
      assert.ok(entry, `${command} appears in explorer/context`);
      assert.ok(
        entry.when?.includes("bynk.hasProject"),
        `${command} is gated on a Bynk project`,
      );
    }
  });

  it("binds Run/Debug Tests shortcuts, scoped to bynk editors", () => {
    const { keybindings } = contributes();
    for (const command of ["bynk.runTests", "bynk.debugTests"]) {
      const binding = keybindings.find((k) => k.command === command);
      assert.ok(binding, `${command} has a keybinding`);
      assert.ok(binding.key && binding.mac, `${command} binds both key and mac`);
      assert.strictEqual(
        binding.when,
        "editorLangId == bynk && !isWeb",
        `${command} keybinding is scoped to bynk editors, off the web`,
      );
    }
  });

  it("enriches the language configuration with wordPattern and onEnterRules", () => {
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    const configPath = path.join(ext.extensionPath, "language-configuration.json");
    const config = JSON.parse(fs.readFileSync(configPath, "utf8"));

    assert.ok(
      typeof config.wordPattern === "string" && config.wordPattern.length > 0,
      "a wordPattern is defined",
    );
    // The pattern must compile as a regex.
    assert.doesNotThrow(() => new RegExp(config.wordPattern));

    assert.ok(
      Array.isArray(config.onEnterRules) && config.onEnterRules.length > 0,
      "onEnterRules are defined",
    );
  });

  it("disambiguates -- line-comment continuation from a --- block-comment fence", () => {
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    const configPath = path.join(ext.extensionPath, "language-configuration.json");
    const config = JSON.parse(fs.readFileSync(configPath, "utf8"));

    const rules: Array<{
      beforeText: string;
      afterText?: string;
      action: { indent: string; appendText?: string };
    }> = config.onEnterRules;

    const lineCommentRule = rules.find((r) => r.action.appendText === "-- ");
    assert.ok(lineCommentRule, "a line-comment continuation rule is defined");
    const lineRe = new RegExp(lineCommentRule.beforeText);
    assert.ok(lineRe.test("-- a line comment"), "matches a line-comment-only line");
    assert.ok(lineRe.test("let x = 1 -- trailing comment"), "matches a trailing comment");
    assert.ok(!lineRe.test("---"), "does not match a bare block-comment fence");
    assert.ok(!lineRe.test("--- doc block start"), "does not match an opening fence");

    const fenceRule = rules.find(
      (r) => !r.action.appendText && /^\^\\s\*---/.test(r.beforeText),
    );
    assert.ok(fenceRule, "a block-comment fence rule is defined");
    const fenceRe = new RegExp(fenceRule.beforeText);
    assert.ok(fenceRe.test("---"), "matches a bare fence");
    assert.ok(fenceRe.test("  ----"), "matches an indented longer fence");
    assert.ok(!fenceRe.test("-- not a fence"), "does not match a line comment");

    // #1888: a `--|` doc line continues as a doc line, and its rule comes
    // before the `--` rule, which would match it too (VS Code takes the first).
    const docLineIndex = rules.findIndex((r) => r.action.appendText === "--| ");
    assert.ok(docLineIndex >= 0, "a doc-line continuation rule is defined");
    const docLineRe = new RegExp(rules[docLineIndex].beforeText);
    assert.ok(docLineRe.test("--| A doc."), "matches a doc line");
    assert.ok(docLineRe.test("\t--| An indented doc."), "matches an indented doc line");
    assert.ok(!docLineRe.test("x: Int, --| a comment"), "does not match `--|` after code");
    assert.ok(!docLineRe.test("-- a comment"), "does not match a line comment");
    assert.ok(
      docLineIndex < rules.indexOf(lineCommentRule),
      "the doc-line rule is tried before the line-comment rule",
    );
  });
});
