// v0.78: the Test CodeLens + eager discovery, end to end. Opening a `.bynk` test file
// (without opening the Testing view) should trigger discovery and surface a
// `Run Test | Debug Test` CodeLens at the case declaration. Guarded on `bynk` +
// `bynkc` (discovery is a `bynk test --no-run` compile, #486); the harness
// provisions both.

import * as assert from "assert";
import * as path from "path";
import * as fs from "fs";
import { spawnSync } from "child_process";
import * as vscode from "vscode";

import { assertFixtureChecks } from "./fixtureCheck";

function have(tool: string): boolean {
  try {
    return spawnSync(tool, ["--version"], { stdio: "ignore" }).status === 0;
  } catch {
    return false;
  }
}

describe("Test CodeLens (eager discovery)", () => {
  let dir: string;

  after(() => {
    if (dir) {
      try {
        fs.rmSync(dir, { recursive: true, force: true });
      } catch {
        /* best-effort */
      }
    }
  });

  it("shows Run/Debug lenses at a test case after opening the file", async function () {
    if (!have("bynk") || !have("bynkc")) this.skip();
    this.timeout(60_000);

    // A project inside the workspace folder with one test case.
    const workspace = path.resolve(__dirname, "../../../test/fixtures/workspace");
    dir = fs.realpathSync(fs.mkdtempSync(path.join(workspace, ".cl-")));
    fs.mkdirSync(path.join(dir, "src"));
    fs.mkdirSync(path.join(dir, "tests"));
    fs.writeFileSync(
      path.join(dir, "bynk.toml"),
      "[project]\nname = \"cl\"\n\n[paths]\ninclude = [\"src\", \"tests\"]\n",
    );
    fs.writeFileSync(
      path.join(dir, "src", "calc.bynk"),
      "commons calc {\n  fn dbl(n: Int) -> Int { n + n }\n}\n",
    );
    const testSrc = "suite calc {\n  case \"doubles\" {\n    expect dbl(3) == 6\n  }\n}\n";
    fs.writeFileSync(path.join(dir, "tests", "calc.bynk"), testSrc);
    const caseLine = testSrc.split("\n").findIndex((l) => l.includes('case "doubles"'));
    assertFixtureChecks(dir, "test CodeLens");

    // Open the test file — this is what should trigger eager discovery (no Testing
    // view interaction).
    const uri = vscode.Uri.file(path.join(dir, "tests", "calc.bynk"));
    await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(uri));

    // Poll the CodeLens provider until discovery settles and lenses appear.
    let lenses: vscode.CodeLens[] = [];
    const deadline = Date.now() + 45_000;
    while (Date.now() < deadline) {
      lenses = (await vscode.commands.executeCommand<vscode.CodeLens[]>(
        "vscode.executeCodeLensProvider",
        uri,
      )) ?? [];
      // Only our test lenses (ignore any LSP reference lenses).
      const ours = lenses.filter((l) =>
        ["bynk.runTestCase", "bynk.debugTestCase"].includes(l.command?.command ?? ""),
      );
      if (ours.length >= 2) {
        lenses = ours;
        break;
      }
      await new Promise((r) => setTimeout(r, 500));
    }

    const commands = lenses.map((l) => l.command?.command);
    assert.ok(commands.includes("bynk.runTestCase"), `Run Test lens present, got ${commands}`);
    assert.ok(commands.includes("bynk.debugTestCase"), `Debug Test lens present, got ${commands}`);
    // v0.127: each lens carries the case name, so `▷ Run Test` runs *that* case.
    assert.ok(
      lenses.every((l) => l.command?.arguments?.[0] === "doubles"),
      `lenses carry the case name, got ${lenses.map((l) => l.command?.arguments?.[0])}`,
    );
    // Anchored at the case declaration line.
    assert.ok(
      lenses.every((l) => l.range.start.line === caseLine),
      `lenses at the case line ${caseLine}, got ${lenses.map((l) => l.range.start.line)}`,
    );
    // The per-case commands are registered (invoked only via the lens, not the
    // palette).
    const registered = await vscode.commands.getCommands(true);
    assert.ok(registered.includes("bynk.runTestCase"), "bynk.runTestCase is registered");
    assert.ok(registered.includes("bynk.debugTestCase"), "bynk.debugTestCase is registered");
  });
});
