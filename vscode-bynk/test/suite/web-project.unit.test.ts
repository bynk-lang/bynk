// Unit coverage for the web worker's project model (#2102 slice 2): which
// project a document belongs to, its project-relative path, the sources sent
// to `bynk_analyze_project`, and where project diagnostics are filed.

import * as assert from "assert";

import { bindingModules, diagnosticsByPath, Project, projectFor } from "../../src/web/project";

const ROOT = "vscode-test-web://mount/proj";
const project = (): Project =>
  new Project({
    root: `${ROOT}/`,
    files: [
      { path: "bynk.toml", uri: `${ROOT}/bynk.toml`, text: "[project]" },
      { path: "src/a.bynk", uri: `${ROOT}/src/a.bynk`, text: "disk a" },
    ],
  });

describe("web Project", () => {
  it("knows which URIs are under its root", () => {
    const p = project();
    assert.strictEqual(p.root, ROOT);
    assert.ok(p.contains(`${ROOT}/src/b.bynk`));
    assert.ok(!p.contains(`${ROOT}-other/src/b.bynk`));
    assert.ok(!p.contains("untitled:Untitled-1"));
  });

  it("maps a URI to its project-relative path, decoding a new file's", () => {
    const p = project();
    assert.strictEqual(p.pathOf(`${ROOT}/src/a.bynk`), "src/a.bynk");
    assert.strictEqual(p.pathOf(`${ROOT}/src/my%20file.bynk`), "src/my file.bynk");
    assert.strictEqual(p.pathOf("untitled:x"), undefined);
  });

  it("overlays open documents on the files read from disk", () => {
    const p = project();
    const { files, uris } = p.sources([
      { uri: `${ROOT}/src/a.bynk`, text: "live a" },
      { uri: `${ROOT}/src/new.bynk`, text: "unsaved" },
      { uri: "untitled:x", text: "elsewhere" },
    ]);
    assert.deepStrictEqual(files, {
      "bynk.toml": "[project]",
      "src/a.bynk": "live a",
      "src/new.bynk": "unsaved",
    });
    assert.strictEqual(uris.get("src/new.bynk"), `${ROOT}/src/new.bynk`);
  });

  it("applies file changes and deletions", () => {
    const p = project();
    p.update({ path: "src/b.bynk", uri: `${ROOT}/src/b.bynk`, text: "b" });
    p.update({ path: "src/a.bynk", uri: `${ROOT}/src/a.bynk`, text: null });
    assert.deepStrictEqual(Object.keys(p.sources([]).files).sort(), ["bynk.toml", "src/b.bynk"]);
  });
});

describe("web bindingModules", () => {
  it("resolves an adapter's binding against its directory", () => {
    const adapter = 'adapter idem {\n  binding "./idem.binding.ts"\n}\n';
    assert.deepStrictEqual(bindingModules("src/a/idem.bynk", adapter), ["src/a/idem.binding.ts"]);
    assert.deepStrictEqual(
      bindingModules("src/a/idem.bynk", adapter.replace("./", "../lib/")),
      ["src/lib/idem.binding.ts"],
    );
    // Escaping the project root, or not an adapter at all: nothing.
    assert.deepStrictEqual(bindingModules("idem.bynk", adapter.replace("./", "../")), []);
    assert.deepStrictEqual(bindingModules("src/c.bynk", 'context c\n  binding "./x.ts"\n'), []);
  });
});

describe("web projectFor", () => {
  it("picks the innermost project", () => {
    const outer = project();
    const inner = new Project({ root: `${ROOT}/nested`, files: [] });
    assert.strictEqual(projectFor([outer, inner], `${ROOT}/nested/src/x.bynk`), inner);
    assert.strictEqual(projectFor([outer, inner], `${ROOT}/src/x.bynk`), outer);
    assert.strictEqual(projectFor([outer, inner], "untitled:x"), undefined);
  });
});

describe("web diagnosticsByPath", () => {
  it("groups by path and files project-level diagnostics under bynk.toml", () => {
    const d = (path: string | null) => ({
      path,
      from: 0,
      to: 0,
      severity: "error",
      category: "c",
      message: "m",
    });
    const grouped = diagnosticsByPath([d("src/a.bynk"), d(null), d("src/a.bynk")]);
    assert.strictEqual(grouped.get("src/a.bynk")?.length, 2);
    assert.strictEqual(grouped.get("bynk.toml")?.length, 1);
  });
});
