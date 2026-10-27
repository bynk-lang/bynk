// #2465: the `@bynk` chat participant in the extension host. A participant's
// handler can't be driven headlessly (the chat UI supplies the request), so the
// model-driven flow is covered by `chatCore.unit.test.ts`; this checks the
// wiring that is the extension's own: the contribution, and that **Apply fix**
// applies a proposal only to the lines it was made against.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import * as vscode from "vscode";

const EXT_ID = "bynk-lang.bynk-vscode";

describe("@bynk chat participant (#2465)", () => {
  const file = path.resolve(__dirname, "../../../test/fixtures/workspace/src/chat_fix.bynk");
  const source = "commons fixture.chat_fix\n\nfn f(u: Usr) -> Int {\n  1\n}\n";

  before(async function () {
    this.timeout(120_000);
    fs.writeFileSync(file, source);
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    await ext.activate();
  });

  after(async () => {
    await vscode.commands.executeCommand("workbench.action.closeAllEditors");
    fs.rmSync(file, { force: true });
  });

  it("contributes @bynk with its four commands", () => {
    const ext = vscode.extensions.getExtension(EXT_ID)!;
    const [participant] = ext.packageJSON.contributes.chatParticipants;
    assert.strictEqual(participant.name, "bynk");
    assert.deepStrictEqual(
      participant.commands.map((c: { name: string }) => c.name),
      ["explain", "learn", "architecture", "fix"],
    );
    return vscode.commands.getCommands(true).then((all) => assert.ok(all.includes("bynk.chat.applyFix")));
  });

  const fix = (original: string) => ({
    path: "src/chat_fix.bynk",
    startLine: 3,
    endLine: 3,
    original,
    replacement: "fn f(u: User) -> Int {",
  });

  it("Apply fix refuses lines that changed since the proposal", async () => {
    const doc = await vscode.workspace.openTextDocument(vscode.Uri.file(file));
    await vscode.commands.executeCommand("bynk.chat.applyFix", doc.uri.toString(), fix("fn g(u: Usr) -> Int {"));
    assert.strictEqual(doc.getText(), source);
  });

  it("Apply fix refuses a range past the end of a shorter file", async () => {
    const doc = await vscode.workspace.openTextDocument(vscode.Uri.file(file));
    await vscode.commands.executeCommand("bynk.chat.applyFix", doc.uri.toString(), {
      ...fix("fn f(u: Usr) -> Int {"),
      startLine: 40,
      endLine: 50,
    });
    assert.strictEqual(doc.getText(), source);
  });

  it("Apply fix replaces the proposed lines", async () => {
    const doc = await vscode.workspace.openTextDocument(vscode.Uri.file(file));
    await vscode.commands.executeCommand("bynk.chat.applyFix", doc.uri.toString(), fix("fn f(u: Usr) -> Int {"));
    assert.strictEqual(doc.getText(), source.replace("Usr", "User"));
  });
});
