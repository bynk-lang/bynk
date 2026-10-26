// #2463: end-to-end coverage for the language-model tools. Each tool is
// invoked the way a chat agent would — `vscode.lm.invokeTool`, outside a chat
// request — against the fixture project and the real language server, and its
// text is checked for the toolchain's own answer.

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import * as vscode from "vscode";

import { useFakeBynk } from "./fakeBynk";

const EXT_ID = "bynk-lang.bynk-vscode";

const TOOLS = [
  "bynk_explain_diagnostic",
  "bynk_get_diagnostics",
  "bynk_describe_symbol",
  "bynk_get_documentation",
  "bynk_get_architecture",
  "bynk_get_wire_contract",
  "bynk_check",
  "bynk_search_book",
];

async function invoke(name: string, input: object): Promise<string> {
  const result = await vscode.lm.invokeTool(name, { input, toolInvocationToken: undefined });
  return result.content
    .filter((p): p is vscode.LanguageModelTextPart => p instanceof vscode.LanguageModelTextPart)
    .map((p) => p.value)
    .join("");
}

/** Invoke until `ok` holds: the server analyses asynchronously. */
async function invokeUntil(
  name: string,
  input: object,
  ok: (text: string) => boolean,
  timeoutMs = 40_000,
): Promise<string> {
  const start = Date.now();
  for (;;) {
    let text = "";
    try {
      text = await invoke(name, input);
    } catch (e) {
      text = String(e);
    }
    if (ok(text)) return text;
    if (Date.now() - start > timeoutMs) {
      throw new Error(`timed out after ${timeoutMs}ms waiting on ${name}; last result:\n${text}`);
    }
    await new Promise((r) => setTimeout(r, 250));
  }
}

describe("Bynk language-model tools (#2463)", () => {
  const workspace = path.resolve(__dirname, "../../../test/fixtures/workspace");
  const broken = path.join(workspace, "src", "lm_broken.bynk");
  const service = path.join(workspace, "src", "lm_service.bynk");
  const serviceSource =
    "context fixture.lm_service\n\n" +
    "type Qty = Int where Positive\n\n" +
    "service api from http {\n" +
    '  on GET("/qty/:n") (n: Qty) -> Effect[HttpResult[Qty]] by v: Visitor {\n' +
    "    Ok(n)\n" +
    "  }\n" +
    "}\n";

  before(async function () {
    this.timeout(120_000);
    fs.writeFileSync(broken, "commons fixture.lm_broken\n\nfn f(u: Usr) -> Int {\n  1\n}\n");
    fs.writeFileSync(service, serviceSource);
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} is installed`);
    await ext.activate();
    await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(vscode.Uri.file(service)));
    await vscode.workspace.openTextDocument(vscode.Uri.file(broken));
  });

  after(async () => {
    await vscode.commands.executeCommand("workbench.action.closeAllEditors");
    fs.rmSync(broken, { force: true });
    fs.rmSync(service, { force: true });
  });

  it("registers every tool, tagged `bynk`", () => {
    const registered = vscode.lm.tools.filter((t) => t.tags.includes("bynk")).map((t) => t.name);
    for (const name of TOOLS) assert.ok(registered.includes(name), `${name} in ${registered.join(", ")}`);
  });

  it("bynk_get_diagnostics reports the server's diagnostics with 1-based locations and codes", async function () {
    this.timeout(60_000);
    const text = await invokeUntil("bynk_get_diagnostics", { file: "src/lm_broken.bynk" }, (t) =>
      t.includes("bynk.resolve.unknown_type"),
    );
    assert.match(text, /- src\/lm_broken\.bynk:3:\d+ error\[bynk\.resolve\.unknown_type\]/);
  });

  it("bynk_explain_diagnostic returns the toolchain's explanation and Book link", async function () {
    this.timeout(60_000);
    const text = await invokeUntil("bynk_explain_diagnostic", { code: "bynk.resolve.unknown_type" }, (t) =>
      t.startsWith("# `bynk.resolve.unknown_type`"),
    );
    assert.ok(text.includes("Book: https://bynk-lang.org/book/reference/types/"), text);
  });

  it("bynk_describe_symbol finds a symbol by name on a line, with its definition", async function () {
    this.timeout(60_000);
    const text = await invokeUntil(
      "bynk_describe_symbol",
      { file: "src/text.bynk", line: 10, name: "shout" },
      (t) => t.includes("Defined at:"),
    );
    assert.ok(text.includes("shout"), text);
    assert.ok(text.includes("Defined at: src/text.bynk:5"), text);
  });

  it("bynk_describe_symbol narrows a name search to `file` when `line` is absent", async function () {
    this.timeout(60_000);
    const found = await invokeUntil("bynk_describe_symbol", { file: "src/text.bynk", name: "shout" }, (t) =>
      t.includes("Defined at:"),
    );
    assert.ok(found.includes("Defined at: src/text.bynk:5"), found);
    const absent = await invoke("bynk_describe_symbol", { file: "src/lm_service.bynk", name: "shout" });
    assert.strictEqual(absent, "No Bynk symbol named `shout` in src/lm_service.bynk.\n");
  });

  it("bynk_get_documentation lists a unit's declarations", async function () {
    this.timeout(60_000);
    const text = await invokeUntil("bynk_get_documentation", { file: "src/text.bynk" }, (t) =>
      t.includes("`greet`"),
    );
    assert.ok(text.startsWith("# commons `fixture.text`"), text);
    assert.ok(text.includes("`shout`"), text);
  });

  it("bynk_get_architecture summarises the project's units and services", async function () {
    this.timeout(60_000);
    const text = await invokeUntil("bynk_get_architecture", { file: "src/lm_service.bynk" }, (t) =>
      t.includes("`fixture.lm_service`"),
    );
    assert.ok(text.includes("- service `api`: 1 handler(s)"), text);
  });

  it("bynk_get_wire_contract describes the handler on a line", async function () {
    this.timeout(60_000);
    const text = await invokeUntil("bynk_get_wire_contract", { file: "src/lm_service.bynk", line: 6 }, (t) =>
      t.includes("GET /qty/:n"),
    );
    assert.ok(text.includes("Request: `n: Qty`."), text);
    assert.ok(text.includes("- `Qty`"), text);
  });

  it("bynk_check runs `bynk check` over the project", async function () {
    this.timeout(120_000);
    const text = await invoke("bynk_check", { file: "src/text.bynk" });
    assert.match(text, /from `bynk check`/);
    assert.ok(text.includes("bynk.resolve.unknown_type"), text);
  });

  it("bynk_check reports a run that fails without diagnostics as not passing", async function () {
    this.timeout(120_000);
    // A project whose manifest doesn't parse: `bynk check` fails before it
    // reaches a source file. Whatever it prints, the tool must not call it clean.
    const dir = fs.realpathSync(fs.mkdtempSync(path.join(workspace, ".lm-")));
    try {
      fs.writeFileSync(path.join(dir, "bynk.toml"), "[project\nname = \n");
      fs.mkdirSync(path.join(dir, "src"));
      fs.writeFileSync(path.join(dir, "src", "a.bynk"), "commons a\n");
      let text: string;
      try {
        text = await invoke("bynk_check", { file: path.join(dir, "src", "a.bynk") });
      } catch (e) {
        text = String(e);
      }
      assert.ok(!text.includes("No Bynk diagnostics"), text);
      assert.match(text, /did not pass|error/, text);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  it("bynk_search_book answers from the installed toolchain's Book, labelled with its version", async function () {
    this.timeout(60_000);
    const fake = await useFakeBynk();
    try {
      const text = await invoke("bynk_search_book", { query: "refined types" });
      assert.ok(text.includes("Source: the Bynk Book v9.9.9, from the installed toolchain."), text);
      assert.ok(text.includes("Types › Refined types"), text);
      assert.ok(text.includes("https://bynk-lang.org/book/reference/types/"), text);
      assert.ok(fake.read().some((e) => e.argv?.[0] === "book"), "ran `bynk book`");
    } finally {
      await fake.restore();
    }
  });

  it("refuses a file outside the workspace", async () => {
    const outside = path.join(fs.realpathSync(os.tmpdir()), `bynk-lm-outside-${process.pid}.bynk`);
    fs.writeFileSync(outside, "commons outside\n");
    try {
      await assert.rejects(
        () => invoke("bynk_get_documentation", { file: outside }),
        /No file .* in this workspace/,
      );
    } finally {
      fs.rmSync(outside, { force: true });
    }
  });

  it("explains a bad input to the agent instead of failing silently", async () => {
    await assert.rejects(
      () => invoke("bynk_get_documentation", { file: "src/no_such_file.bynk" }),
      /No file `src\/no_such_file\.bynk`/,
    );
  });
});
