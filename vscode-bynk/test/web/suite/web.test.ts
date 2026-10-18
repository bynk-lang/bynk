// Web extension smoke tests (#2102), run in a browser VS Code by
// @vscode/test-web against test/fixtures/web. They exercise the real path:
// the `browser` entry, the worker, and the bynk-wasm module it fetches.

import * as vscode from "vscode";

const EXT_ID = "bynk.bynk-vscode";

/**
 * Create (or delete) a file the way the Explorer does, through a workspace
 * edit, so the file-operation events fire. The test host's filesystem doesn't
 * report writes to file watchers.
 */
async function createFile(uri: vscode.Uri, text: string): Promise<void> {
  const edit = new vscode.WorkspaceEdit();
  edit.createFile(uri, { overwrite: true, contents: new TextEncoder().encode(text) });
  if (!(await vscode.workspace.applyEdit(edit))) throw new Error(`creating ${uri.toString()}`);
}

async function deleteFile(uri: vscode.Uri): Promise<void> {
  const edit = new vscode.WorkspaceEdit();
  edit.deleteFile(uri, { ignoreIfNotExists: true });
  await vscode.workspace.applyEdit(edit);
}

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

  it("reports a project file's errors without opening it (slice 2)", async () => {
    // test/fixtures/web has a bynk.toml, so it is loaded as a project and every
    // file is analysed, open or not.
    const diags = await diagnosticsFor(fixture("src/app/broken.bynk"), (d) => d.length > 0);
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
    // under the playground's `browser` platform. The KV unit is clean, so "no
    // diagnostics" alone can't tell a pass from an analysis that hasn't run:
    // wait for the same project analysis to report broken.bynk first. (A type
    // error in the KV unit itself would not do: it stops the analysis before
    // the platform lock that reports `vendor_required`.)
    const doc = await open("src/kv/index.bynk");
    await diagnosticsFor(fixture("src/app/broken.bynk"), (d) => d.length > 0);
    const diags = vscode.languages.getDiagnostics(doc.uri);
    ok(diags.length === 0, `no diagnostics on the KV unit: ${JSON.stringify(diags.map((d) => d.code))}`);
  });

  it("resolves `uses` across the project's files (slice 2)", async () => {
    // web.bynk uses app.text (text.bynk). Its own deliberate type error shows
    // the analysis ran.
    const doc = await open("src/app/web.bynk");
    const diags = await diagnosticsFor(doc.uri, (d) => d.length > 0);
    ok(
      !diags.some((d) => String(d.code).startsWith("bynk.resolve.") || d.code === "bynk.uses.unknown_commons"),
      `app.text resolves: ${JSON.stringify(diags.map((d) => d.code))}`,
    );
    ok(
      diags.some((d) => d.code === "bynk.types.return_mismatch"),
      `the file's own type error is reported: ${JSON.stringify(diags.map((d) => d.code))}`,
    );
    // `greeting(42)` misuses text.bynk's `greeting`, which only checks when
    // app.text is loaded: single-file mode hides it as an unresolved name.
    const misuse = positionOf(doc, "greeting(42)").line;
    ok(
      diags.some((d) => d.range.start.line === misuse),
      `the cross-file misuse is reported: ${JSON.stringify(diags.map((d) => [d.code, d.range.start.line]))}`,
    );
  });

  it("loads an adapter's binding module with the project (slice 2)", async () => {
    // idempotency.bynk names `binding "./idempotency.binding.ts"`; unloaded, it
    // draws `bynk.adapter.no_binding` ("could not be read").
    const doc = await open("src/idempotency.bynk");
    await diagnosticsFor(fixture("src/app/broken.bynk"), (d) => d.length > 0);
    const diags = vscode.languages.getDiagnostics(doc.uri);
    ok(diags.length === 0, `no diagnostics on the adapter: ${JSON.stringify(diags.map((d) => d.message))}`);
  });

  it("re-reads binding modules when an adapter's binding changes (slice 2)", async () => {
    const doc = await open("src/idempotency.bynk");
    const original = doc.getText();
    const replaceAll = async (text: string) => {
      const edit = new vscode.WorkspaceEdit();
      edit.replace(doc.uri, new vscode.Range(doc.positionAt(0), doc.positionAt(doc.getText().length)), text);
      ok(await vscode.workspace.applyEdit(edit), "the edit applies");
    };
    const noBinding = (d: vscode.Diagnostic[]) => d.some((x) => x.code === "bynk.adapter.no_binding");
    const moved = fixture("src/idem2.binding.ts");
    try {
      // Typed, not saved: a binding the project doesn't have yet.
      await replaceAll(original.replace("./idempotency.binding.ts", "./idem2.binding.ts"));
      await diagnosticsFor(doc.uri, noBinding);
      // Creating that module is picked up, and the adapter checks clean.
      const binding = await vscode.workspace.fs.readFile(fixture("src/idempotency.binding.ts"));
      await createFile(moved, new TextDecoder().decode(binding));
      await diagnosticsFor(doc.uri, (d) => !noBinding(d));
      // Deleting it brings the error back.
      await deleteFile(moved);
      await diagnosticsFor(doc.uri, noBinding);
    } finally {
      await replaceAll(original);
      await deleteFile(moved);
    }
    await diagnosticsFor(doc.uri, (d) => !noBinding(d));
  });

  it("keeps a new file under a skipped directory out of the project (slice 2)", async () => {
    // Inside the `src` include, so only the walk's skip rule keeps it out.
    const skipped = fixture("src/target/gen.bynk");
    const fresh = fixture("src/app/fresh.bynk");
    try {
      // Written first: if the watcher event were applied, it would be analysed
      // by the time the second file's diagnostics land.
      await createFile(skipped, "this is not bynk\n");
      await createFile(
        fresh,
        'context app.fresh\n\nservice f {\n  on call() -> Effect[Int] {\n    "not an int"\n  }\n}\n',
      );
      await diagnosticsFor(fresh, (d) => d.length > 0);
      const diags = vscode.languages.getDiagnostics(skipped);
      ok(diags.length === 0, `src/target/gen.bynk is not analysed: ${JSON.stringify(diags.map((d) => d.code))}`);
    } finally {
      await deleteFile(fresh);
      await deleteFile(skipped);
    }
  });

  it("hovers a name from another file of the project (slice 2)", async () => {
    const doc = await open("src/app/web.bynk");
    // `subject` has type `Subject`, declared in text.bynk.
    const pos = positionOf(doc, "greeting(subject)", "greeting(".length + 1);
    const hovers = await vscode.commands.executeCommand<vscode.Hover[]>(
      "vscode.executeHoverProvider",
      doc.uri,
      pos,
    );
    const text = hovers
      .flatMap((h) => h.contents)
      .map((c) => (typeof c === "string" ? c : c.value))
      .join("\n");
    ok(text.includes("Subject"), `hover shows the other file's type: ${text}`);
  });

  it("hides the single-file cascade outside a project (DECISION B)", async () => {
    // An untitled document is in no project, so it is analysed on its own.
    const web = await vscode.workspace.openTextDocument(fixture("src/app/web.bynk"));
    const doc = await vscode.workspace.openTextDocument({ language: "bynk", content: web.getText() });
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

  it("resolves a sibling `consumes` across the project (slice 2)", async () => {
    // commerce/orders.bynk consumes commerce.payment, in another file of the
    // project; slice 1 hid the single-file errors, the project resolves it.
    const doc = await open("src/commerce/orders.bynk");
    const diags = await diagnosticsFor(doc.uri, (d) => d.length > 0);
    ok(
      !diags.some((d) => /^bynk\.(consumes|resolve)\./.test(String(d.code))),
      `no cross-context noise: ${JSON.stringify(diags.map((d) => d.code))}`,
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
