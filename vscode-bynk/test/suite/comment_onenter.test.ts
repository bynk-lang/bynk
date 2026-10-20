import * as assert from "assert";

import * as vscode from "vscode";

// Drives a real Enter keystroke through the editor's "type" command, which is
// what actually applies `language-configuration.json`'s `onEnterRules` — a
// static regex simulation (see manifest.test.ts) can't catch a rule whose
// `beforeText`/`afterText` combination never fires in practice (#306 review:
// an inverted `afterText` lookahead made the line-comment rule fire only when
// splitting mid-comment, never at end-of-line, the common case).
async function pressEnterAt(
  content: string,
  line: number,
  character: number,
): Promise<vscode.TextDocument> {
  const doc = await vscode.workspace.openTextDocument({ language: "bynk", content });
  const editor = await vscode.window.showTextDocument(doc);
  const pos = new vscode.Position(line, character);
  editor.selection = new vscode.Selection(pos, pos);
  await vscode.commands.executeCommand("type", { text: "\n" });
  return editor.document;
}

describe("Bynk onEnterRules — comment continuation (#306)", () => {
  afterEach(async () => {
    await vscode.commands.executeCommand("workbench.action.closeAllEditors");
  });

  it("continues a `//` line comment when Enter is pressed at end of line", async () => {
    const doc = await pressEnterAt("// hello", 0, "// hello".length);
    assert.strictEqual(doc.lineCount, 2, "a new line was inserted");
    assert.strictEqual(doc.lineAt(1).text, "// ", "the new line starts a fresh // comment");
  });

  it("continues a trailing `//` comment after code, at end of line", async () => {
    const line = "let x = 1 // note";
    const doc = await pressEnterAt(line, 0, line.length);
    assert.strictEqual(doc.lineAt(1).text, "// ", "the new line starts a fresh // comment");
  });

  it("re-prefixes the tail when Enter splits a `//` comment mid-line", async () => {
    // Cursor lands right after "hello"; the moved remainder keeps its
    // leading space, so appendText's "// " composes with it as "//  world".
    const doc = await pressEnterAt("// hello world", 0, "// hello".length);
    assert.strictEqual(doc.lineAt(0).text, "// hello", "text before the cursor stays put");
    assert.strictEqual(
      doc.lineAt(1).text,
      "//  world",
      "the moved remainder is re-prefixed as a comment",
    );
  });

  it("does not continue a `---` doc-comment fence as a comment", async () => {
    const doc = await pressEnterAt("---", 0, "---".length);
    assert.strictEqual(doc.lineCount, 2, "a new line was inserted");
    assert.strictEqual(
      doc.lineAt(1).text,
      "",
      "the fence rule fires instead of the line-comment rule (no // prefix)",
    );
  });

  it("does not continue a `--- text` one-line doc (#2203)", async () => {
    // A second `--- ` line would be a second doc on the same target
    // (`bynk.parse.multiple_docs`), so a one-line doc gets no continuation.
    const doc = await pressEnterAt("--- A doc.", 0, "--- A doc.".length);
    assert.strictEqual(doc.lineAt(1).text, "", "the new line is not prefixed");
  });

  it("keeps indentation steady across an indented `---` fence", async () => {
    const doc = await pressEnterAt("  ---", 0, "  ---".length);
    assert.strictEqual(doc.lineAt(1).text, "  ", "the new line keeps the fence's indentation");
  });
});
