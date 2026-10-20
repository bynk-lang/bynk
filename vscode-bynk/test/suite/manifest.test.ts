import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import * as vscode from "vscode";

const EXT_ID = "bynk-lang.bynk-vscode";

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
      "bynk.explainDiagnostic",
      // #2114: the dev server runs `bynk dev`, a local process.
      "bynk.startDevServer",
      "bynk.stopDevServer",
      "bynk.restartDevServer",
      // #2118: deploy runs `bynk deploy`, a local process.
      "bynk.deploy",
      "bynk.showDeployPlan",
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
    // #2121: Doctor checks the local toolchain, so it is likewise ungated on the
    // desktop and hidden on the web.
    const doctor = palette.find((m) => m.command === "bynk.doctor");
    assert.strictEqual(doctor?.when, "!isWeb", "bynk.doctor is gated only off the web");
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

  // #2091: a custom token type with no `semanticTokenScopes` entry gets no
  // colour under a theme that has no rule for it, so every declared type
  // needs a TextMate fallback.
  it("maps every custom semantic token type to a fallback scope", () => {
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    const c = ext.packageJSON.contributes ?? {};
    const types: Array<{ id: string }> = c.semanticTokenTypes ?? [];
    const scopes: Record<string, string[]> =
      (c.semanticTokenScopes ?? []).find(
        (s: { language?: string }) => s.language === "bynk",
      )?.scopes ?? {};

    assert.ok(types.length > 0, "custom semantic token types are declared");
    for (const { id } of types) {
      assert.ok(
        Array.isArray(scopes[id]) && scopes[id].length > 0,
        `semantic token type ${id} has a semanticTokenScopes fallback`,
      );
    }
  });

  // #2092: the asset and tiered-suite snippets ship with the extension.
  it("ships the asset and tiered-suite snippets", () => {
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    const contributed: Array<{ language: string; path: string }> =
      ext.packageJSON.contributes?.snippets ?? [];
    const entry = contributed.find((s) => s.language === "bynk");
    assert.ok(entry, "a bynk snippets file is contributed");
    const snippets: Record<string, { prefix: string; body: string[] }> = JSON.parse(
      fs.readFileSync(path.join(ext.extensionPath, entry.path), "utf8"),
    );

    for (const [name, prefix, start] of [
      ["asset", "asset", "asset "],
      ["asset at", "asset", "asset "],
      ["suite as", "suite as", "suite "],
      ["case as", "case as", "case "],
    ]) {
      const snippet = snippets[name];
      assert.ok(snippet, `the ${name} snippet exists`);
      assert.strictEqual(snippet.prefix, prefix, `${name} is offered on \`${prefix}\``);
      assert.ok(snippet.body[0].startsWith(start), `${name} expands to a ${start.trim()} declaration`);
    }
    for (const name of ["suite as", "case as"]) {
      assert.ok(
        snippets[name].body[0].includes("${2|unit,integration,system|}"),
        `${name} offers the three tiers as a choice`,
      );
    }
    assert.ok(snippets["asset at"].body[0].includes(" at \""), "asset at has an `at` path");
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

  it("disambiguates // line-comment continuation from a --- doc fence (#2203)", () => {
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    const configPath = path.join(ext.extensionPath, "language-configuration.json");
    const config = JSON.parse(fs.readFileSync(configPath, "utf8"));
    assert.strictEqual(config.comments.lineComment, "//", "Toggle Line Comment uses //");

    const rules: Array<{
      beforeText: string;
      afterText?: string;
      action: { indent: string; appendText?: string };
    }> = config.onEnterRules;

    const lineCommentRule = rules.find((r) => r.action.appendText === "// ");
    assert.ok(lineCommentRule, "a line-comment continuation rule is defined");
    const lineRe = new RegExp(lineCommentRule.beforeText);
    assert.ok(lineRe.test("// a line comment"), "matches a line-comment-only line");
    assert.ok(lineRe.test("let x = 1 // trailing comment"), "matches a trailing comment");
    assert.ok(!lineRe.test("---"), "does not match a bare doc fence");
    assert.ok(!lineRe.test("--- A one-line doc."), "does not match a one-line doc");
    assert.ok(!lineRe.test('let u = "https://example.com"'), "does not match a URL");

    const fenceRule = rules.find(
      (r) => !r.action.appendText && /^\^\\s\*---/.test(r.beforeText),
    );
    assert.ok(fenceRule, "a doc fence rule is defined");
    const fenceRe = new RegExp(fenceRule.beforeText);
    assert.ok(fenceRe.test("---"), "matches a bare fence");
    assert.ok(fenceRe.test("  ----"), "matches an indented longer fence");
    assert.ok(!fenceRe.test("--- A one-line doc."), "does not match a one-line doc");

    // #2203: nothing continues a doc line, `--- ` or the retired `--| `.
    assert.ok(
      !rules.some((r) => r.action.appendText?.startsWith("--")),
      "no rule continues a doc line",
    );
  });

  // #2093: the live model views' entry points, settings and restore hooks.
  it("puts the model views on the editor title bar, scoped to bynk editors", () => {
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    const { menus } = contributes();
    const commands: Array<{ command: string; icon?: string }> = ext.packageJSON.contributes.commands;
    for (const command of [
      "bynk.showSequenceDiagram",
      "bynk.showDocumentation",
      "bynk.showArchitectureMap",
      "bynk.showWireContract",
      "bynk.showAgentState",
    ]) {
      const entry = (menus["editor/title"] ?? []).find((m) => m.command === command);
      assert.ok(entry, `${command} appears in editor/title`);
      assert.strictEqual(
        entry.when,
        "editorLangId == bynk && !isWeb",
        `${command} is scoped to bynk editors, off the web`,
      );
      assert.ok(entry.group?.startsWith("navigation"), `${command} is a title-bar button`);
      assert.ok(commands.find((c) => c.command === command)?.icon, `${command} has an icon`);
    }
  });

  // #2097: the project sidebar's container, view and actions.
  it("contributes the Bynk activity-bar project view and its actions", () => {
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    const c = ext.packageJSON.contributes;
    const container = (c.viewsContainers?.activitybar ?? []).find((v: { id: string }) => v.id === "bynk");
    assert.ok(container, "a `bynk` activity-bar container (DECISION B)");
    assert.ok(fs.existsSync(path.join(ext.extensionPath, container.icon)), "its icon ships");
    const view = (c.views?.bynk ?? []).find((v: { id: string }) => v.id === "bynk.project");
    assert.ok(view, "a Project view in it");
    assert.strictEqual(view.when, "bynk.hasProject && !isWeb", "shown only in a Bynk project, off the web");

    const { menus } = contributes();
    const title = (menus["view/title"] ?? []).map((m) => m.command);
    assert.ok(title.includes("bynk.project.refresh"), "a refresh title action");
    assert.ok(title.includes("bynk.project.showArchitectureMap"), "a Show Architecture Map title action");
    const inline = (menus["view/item/context"] ?? []).filter((m) => m.group?.startsWith("inline"));
    for (const [command, item] of [
      ["bynk.project.sequenceDiagram", "bynkHandler"],
      ["bynk.project.wireContract", "bynkHandler"],
      ["bynk.project.showDocumentation", "bynkUnit"],
    ]) {
      assert.ok(
        inline.some((m) => m.command === command && m.when?.includes(`viewItem == ${item}`)),
        `${command} is an inline action on ${item} nodes`,
      );
    }
    // The tree-node commands take a node argument, so they stay out of the palette.
    const palette = menus.commandPalette ?? [];
    for (const command of ["bynk.project.sequenceDiagram", "bynk.project.wireContract", "bynk.project.showDocumentation"]) {
      assert.strictEqual(palette.find((m) => m.command === command)?.when, "false", `${command} is palette-hidden`);
    }
  });

  it("declares the live-view settings and restore activation", () => {
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    const pkg = ext.packageJSON;
    const props = pkg.contributes.configuration.properties;
    assert.deepStrictEqual(props["bynk.views.refresh"].enum, ["onSave", "onType", "manual"]);
    assert.strictEqual(props["bynk.views.refresh"].default, "onSave", "DECISION A: on save");
    assert.strictEqual(props["bynk.viewsCodeLens"].default, false, "DECISION D: lens off");
    for (const viewType of [
      "bynkSequenceDiagram",
      "bynkDocumentation",
      "bynkArchitectureMap",
      "bynkWireContract",
      "bynkAgentState",
    ]) {
      assert.ok(
        pkg.activationEvents.includes(`onWebviewPanel:${viewType}`),
        `a reload can restore ${viewType}`,
      );
    }
  });
});
