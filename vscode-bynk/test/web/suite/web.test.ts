// Web extension smoke tests (#2102), run in a browser VS Code by
// @vscode/test-web against test/fixtures/web. They exercise the real path:
// the `browser` entry, the worker, and the bynk-wasm module it fetches.

import * as vscode from "vscode";

const EXT_ID = "bynk.bynk-vscode";

// No Node `assert` in a browser bundle.
function ok(cond: unknown, msg: string): asserts cond {
  if (!cond) throw new Error(msg);
}

function fixture(rel: string): vscode.Uri {
  const folder = vscode.workspace.workspaceFolders?.[0];
  ok(folder, "the fixture folder is open");
  return vscode.Uri.joinPath(folder.uri, rel);
}

async function open(rel: string): Promise<vscode.TextDocument> {
  const doc = await vscode.workspace.openTextDocument(fixture(rel));
  await vscode.window.showTextDocument(doc);
  return doc;
}

/** Wait until `pred` holds for the document's diagnostics, or time out. */
async function diagnosticsFor(
  uri: vscode.Uri,
  pred: (d: vscode.Diagnostic[]) => boolean,
  timeoutMs = 45_000,
): Promise<vscode.Diagnostic[]> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const diags = vscode.languages.getDiagnostics(uri);
    if (pred(diags)) return diags;
    if (Date.now() > deadline) {
      throw new Error(
        `timed out waiting for diagnostics on ${uri.toString()}; last: ${JSON.stringify(
          diags.map((d) => [d.code, d.message]),
        )}`,
      );
    }
    await new Promise((r) => setTimeout(r, 250));
  }
}

/** The position just after the last occurrence of `needle` in `doc`. */
function positionOf(doc: vscode.TextDocument, needle: string, delta = 0): vscode.Position {
  const at = doc.getText().lastIndexOf(needle);
  ok(at >= 0, `fixture contains ${needle}`);
  return doc.positionAt(at + delta);
}

describe("Bynk web extension", () => {
  before(async () => {
    const ext = vscode.extensions.getExtension(EXT_ID);
    ok(ext, `${EXT_ID} is installed`);
    await ext.activate();
  });

  it("registers only the web commands", async () => {
    const commands = await vscode.commands.getCommands(true);
    ok(commands.includes("bynk.showServerOutput"), "bynk.showServerOutput is registered");
    for (const nodeOnly of ["bynk.runTests", "bynk.debugTests", "bynk.downloadServer"]) {
      ok(!commands.includes(nodeOnly), `${nodeOnly} is not registered on the web`);
    }
  });

  it("reports a type error from the wasm analyser", async () => {
    const doc = await open("src/app/broken.bynk");
    const diags = await diagnosticsFor(doc.uri, (d) => d.length > 0);
    ok(
      diags.some((d) => d.severity === vscode.DiagnosticSeverity.Error && d.source === "bynk"),
      `a bynk error is reported: ${JSON.stringify(diags.map((d) => d.code))}`,
    );
    // The span lands on the offending expression, not at 0:0.
    const span = diags[0].range;
    ok(span.start.line > 0, `the diagnostic is positioned: ${JSON.stringify(span)}`);
  });

  it("analyses for cloudflare, as bynk-lsp does", async () => {
    // A `consumes bynk.cloudflare` unit draws `bynk.target.vendor_required`
    // under the playground's `browser` platform.
    // The KV unit is clean, so "no diagnostics" alone can't tell a pass from an
    // analysis that hasn't run. Open it, then a document that is opened after
    // it and has an error: the worker analyses documents in the order their
    // edits arrive, so once the marker's diagnostics land, the KV unit's have.
    // (A type error in the KV unit itself would not do: it stops the analysis
    // before the platform lock that reports `vendor_required`.)
    const doc = await open("src/kv/index.bynk");
    const marker = await vscode.workspace.openTextDocument({
      language: "bynk",
      content: (await vscode.workspace.openTextDocument(fixture("src/app/broken.bynk"))).getText(),
    });
    await diagnosticsFor(marker.uri, (d) => d.length > 0);
    const diags = vscode.languages.getDiagnostics(doc.uri);
    ok(diags.length === 0, `no diagnostics on the KV unit: ${JSON.stringify(diags.map((d) => d.code))}`);
  });

  it("hides the single-file cascade of an unresolved `uses` (DECISION B)", async () => {
    // The fixture also has a deliberate type error, which the filter keeps, so
    // its arrival shows the analysis ran.
    const doc = await open("src/app/web.bynk");
    const diags = await diagnosticsFor(doc.uri, (d) => d.length > 0);
    ok(
      !diags.some((d) => String(d.code).startsWith("bynk.resolve.") || d.code === "bynk.uses.unknown_commons"),
      `no cross-unit noise: ${JSON.stringify(diags.map((d) => d.code))}`,
    );
    ok(
      diags.some((d) => d.code === "bynk.types.return_mismatch"),
      `the file's own type error is kept: ${JSON.stringify(diags.map((d) => d.code))}`,
    );
  });

  it("hovers an expression's type", async () => {
    const doc = await open("src/app/demo.bynk");
    // The tail `now` (after the multi-byte `é` earlier in the file).
    const pos = positionOf(doc, "now", 1);
    const hovers = await vscode.commands.executeCommand<vscode.Hover[]>(
      "vscode.executeHoverProvider",
      doc.uri,
      pos,
    );
    const text = hovers
      .flatMap((h) => h.contents)
      .map((c) => (typeof c === "string" ? c : c.value))
      .join("\n");
    ok(text.includes("Instant"), `hover shows the type: ${text}`);
  });

  it("completes capability methods", async () => {
    const doc = await open("src/app/demo.bynk");
    const pos = positionOf(doc, "Clock.", "Clock.".length);
    const list = await vscode.commands.executeCommand<vscode.CompletionList>(
      "vscode.executeCompletionItemProvider",
      doc.uri,
      pos,
    );
    const labels = list.items.map((i) => (typeof i.label === "string" ? i.label : i.label.label));
    ok(labels.includes("now"), `completion offers Clock.now: ${JSON.stringify(labels.slice(0, 20))}`);
  });
});
