// #2472 integration test: dependencies and the registry in the editor, inside
// the extension host and against the stand-in `bynk` (test/fixtures/fake-bynk).
// The stand-in prints the driver's JSON documents and never touches a network,
// so what these tests pin is what the extension asks the driver and what it does
// with the answers: the Dependencies node, the upgrade lens (looked up once per
// session, silent offline), Add Dependency…, Fetch Dependencies and a
// plan-first Publish….

import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { useFakeBynk, waitFor, type FakeLogEntry } from "./fakeBynk";

interface Entry {
  label: string;
  description?: string;
  children?: Entry[];
}

describe("Dependencies in the editor (#2472)", () => {
  let dir: string;
  let fake: Awaited<ReturnType<typeof useFakeBynk>>;
  // The manifest the lenses read lives in a temp directory, not in the shared
  // fixture workspace: rewriting that one wakes the Test Explorer's file watcher
  // and its discovery runs would leak into other suites' fake-driver logs.
  let manifest: string;
  const saved = {
    showInputBox: vscode.window.showInputBox,
    showQuickPick: vscode.window.showQuickPick,
    showWarningMessage: vscode.window.showWarningMessage,
  };
  const win = vscode.window as unknown as Record<string, unknown>;

  const argvs = (verb: string): string[][] =>
    fake
      .read()
      .filter((e: FakeLogEntry) => e.argv?.[0] === verb)
      .map((e) => e.argv!);

  async function lenses(): Promise<vscode.CodeLens[]> {
    return (
      (await vscode.commands.executeCommand<vscode.CodeLens[]>(
        "vscode.executeCodeLensProvider",
        vscode.Uri.file(manifest),
      )) ?? []
    );
  }

  before(async function () {
    this.timeout(60_000);
    dir = fs.mkdtempSync(path.join(os.tmpdir(), "bynk-deps-"));
    fake = await useFakeBynk();
    manifest = path.join(dir, "bynk.toml");
    fs.writeFileSync(
      manifest,
      `[project]\nname = "deps-fixture"\n\n[dependencies]\nmoney = { package = "acme/money", version = "^0.3.1" }\nother = { package = "acme/other", version = "^1.0.0" }\n`,
    );
    const deps = path.join(dir, "deps.json");
    fs.writeFileSync(
      deps,
      JSON.stringify({
        dependencies: [
          { alias: "money", package: "acme/money", dev: false, kind: "registry", requested: "^0.3.1", source: "acme-internal", locked: "0.3.1" },
          { alias: "other", package: "acme/other", dev: false, kind: "registry", requested: "^1.0.0", source: "acme-internal", locked: "1.0.0" },
        ],
      }),
    );
    process.env.BYNK_FAKE_DEPS = deps;
    await vscode.extensions.getExtension("bynk-lang.bynk-vscode")?.activate();
  });

  after(async () => {
    for (const [k, v] of Object.entries(saved)) win[k] = v;
    delete process.env.BYNK_FAKE_DEPS;
    delete process.env.BYNK_FAKE_PUBLISH;
    await vscode.commands.executeCommand("workbench.action.closeAllEditors");
    await fake.restore();
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it("lists resolved dependencies in the Project sidebar", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(30_000);
    let node: Entry | undefined;
    const deadline = Date.now() + 20_000;
    while (!node && Date.now() < deadline) {
      const tree = await vscode.commands.executeCommand<Entry[]>("bynk._projectTreeSnapshot", 2, true);
      node = tree.find((e) => e.label === "Dependencies");
      if (!node) await new Promise((r) => setTimeout(r, 500));
    }
    assert.ok(node, "a Dependencies node");
    assert.strictEqual(node.description, "2");
    const money = node.children?.find((c) => c.label === "money");
    assert.ok(money?.description?.includes("0.3.1") && money.description.includes("acme-internal"), JSON.stringify(money));
  });

  it("offers an upgrade lens once per package, and none for a registry that can't answer", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(30_000);
    await vscode.workspace.openTextDocument(vscode.Uri.file(manifest));
    let titles: string[] = [];
    const deadline = Date.now() + 20_000;
    while (!titles.includes("0.3.1 → 0.4.0 available") && Date.now() < deadline) {
      titles = (await lenses()).map((l) => l.command?.title ?? "");
      if (!titles.includes("0.3.1 → 0.4.0 available")) await new Promise((r) => setTimeout(r, 300));
    }
    assert.deepStrictEqual(titles, ["0.3.1 → 0.4.0 available"], "acme/other is unknown to the registry: no lens");
    await lenses();
    await lenses();
    const searched = argvs("search").map((a) => a[1]);
    assert.strictEqual(searched.filter((p) => p === "acme/money").length, 1, "looked up once");
    assert.strictEqual(searched.filter((p) => p === "acme/other").length, 1, "a failure is not retried");
  });

  it("upgrading rewrites the range and fetches", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(30_000);
    const found = (await lenses()).find((l) => l.command?.title.includes("→"));
    assert.ok(found?.command);
    const before = argvs("fetch").length;
    await vscode.commands.executeCommand(found.command.command, ...(found.command.arguments ?? []));
    assert.ok(fs.readFileSync(manifest, "utf8").includes('version = "^0.4.0"'));
    assert.ok(fs.readFileSync(manifest, "utf8").includes('version = "^1.0.0"'), "the other entry is untouched");
    await waitFor(() => argvs("fetch").length > before, "the fetch");
    assert.deepStrictEqual(argvs("fetch").at(-1), ["fetch", "--format", "json"]);
  });

  it("Add Dependency… searches through the driver, then adds the chosen version", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(30_000);
    win.showInputBox = async () => "acme/money";
    let offered: string[] = [];
    win.showQuickPick = async (items: readonly (vscode.QuickPickItem & { range: string })[]) => {
      offered = items.map((i) => i.label);
      return items[1]; // 0.3.1, not the latest
    };
    await vscode.commands.executeCommand("bynk.addDependency");
    assert.deepStrictEqual(offered, ["0.4.0", "0.3.1"], "newest first; the yanked 0.5.0 is not offered");
    assert.deepStrictEqual(argvs("add").at(-1), ["add", "acme/money@^0.3.1", "--format", "json"]);
  });

  it("Add Dependency… adds nothing when the lookup fails", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(30_000);
    win.showInputBox = async () => "acme/unknown";
    win.showQuickPick = async () => assert.fail("no versions to pick");
    const errors: string[] = [];
    const showErrorMessage = vscode.window.showErrorMessage;
    win.showErrorMessage = async (m: string) => (errors.push(m), undefined);
    const adds = argvs("add").length;
    try {
      await vscode.commands.executeCommand("bynk.addDependency");
    } finally {
      win.showErrorMessage = showErrorMessage;
    }
    assert.strictEqual(argvs("add").length, adds);
    assert.ok(errors.some((e) => e.includes("could not look up acme/unknown")), errors.join("|"));
  });

  it("Fetch Dependencies runs bynk fetch", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(30_000);
    const before = argvs("fetch").length;
    await vscode.commands.executeCommand("bynk.fetchDependencies");
    assert.strictEqual(argvs("fetch").length, before + 1);
  });

  it("Publish… plans first, and publishes only after a confirmation", async function () {
    if (process.platform === "win32") this.skip();
    this.timeout(30_000);
    const asked: { message: string; detail?: string }[] = [];
    let answer: string | undefined;
    win.showWarningMessage = async (message: string, options?: { detail?: string }) => {
      asked.push({ message, detail: options?.detail });
      return answer;
    };

    // Declined: the plan ran, the publish did not.
    answer = undefined;
    await vscode.commands.executeCommand("bynk.publish");
    assert.strictEqual(asked.length, 1);
    const detail = asked[0].detail ?? "";
    for (const part of ["acme/fixture", "0.1.0", "acme-internal", "sha256-fixture", "src/main.bynk"]) {
      assert.ok(detail.includes(part), `${part} in: ${detail}`);
    }
    let publishes = argvs("publish");
    assert.deepStrictEqual(publishes, [["publish", "--dry-run", "--format", "json"]]);

    // Confirmed: the plan, then the real publish.
    answer = "Publish";
    await vscode.commands.executeCommand("bynk.publish");
    publishes = argvs("publish");
    assert.deepStrictEqual(publishes.slice(1), [
      ["publish", "--dry-run", "--format", "json"],
      ["publish", "--expect-checksum", "sha256-fixture", "--format", "json"],
    ]);
  });
});
