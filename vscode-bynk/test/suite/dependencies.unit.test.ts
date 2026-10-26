// Unit coverage for the dependency-free core of dependencies in the editor
// (#2472): the driver's JSON documents, the argument lists (plan first for
// publish, every registry question through `bynk`), version comparison, the
// `bynk.toml` scan behind the upgrade lenses, and the publish plan's text.

import * as assert from "assert";

import {
  addArgs,
  compareVersions,
  currentVersion,
  depsArgs,
  fetchArgs,
  parseDeps,
  parsePublishPlan,
  parseSearch,
  pickableVersions,
  publishArgs,
  publishPlanArgs,
  publishPlanText,
  scanManifestDependencies,
  searchArgs,
  upgradeTarget,
  upgradeTitle,
  validPackageName,
} from "../../src/dependenciesCore";

const SEARCH = JSON.stringify({
  package: "acme/money",
  registry: "acme-internal",
  latest: "0.4.0",
  versions: [
    { version: "0.3.1", checksum: "sha256-a", yanked: false, withdrawn: null },
    { version: "0.4.0", checksum: "sha256-b", yanked: false, withdrawn: null },
    { version: "0.5.0", checksum: "sha256-c", yanked: true, withdrawn: null },
    { version: "0.2.0", checksum: "sha256-d", yanked: true, withdrawn: "malware" },
  ],
});

describe("dependencies core (#2472)", () => {
  it("builds argument lists that go through the driver, plan first for publish", () => {
    assert.deepStrictEqual(depsArgs(), ["deps", "--format", "json"]);
    assert.deepStrictEqual(searchArgs("acme/money"), ["search", "acme/money", "--format", "json"]);
    assert.deepStrictEqual(addArgs("acme/money", "^0.4.0"), ["add", "acme/money@^0.4.0", "--format", "json"]);
    assert.deepStrictEqual(addArgs("acme/money"), ["add", "acme/money", "--format", "json"]);
    assert.deepStrictEqual(fetchArgs(), ["fetch", "--format", "json"]);
    assert.deepStrictEqual(publishPlanArgs(), ["publish", "--dry-run", "--format", "json"]);
    assert.deepStrictEqual(publishArgs("sha256-abc"), ["publish", "--expect-checksum", "sha256-abc", "--format", "json"]);
    assert.ok(!publishArgs("sha256-abc").includes("--dry-run"));
  });

  it("validates a package name as org/package", () => {
    assert.ok(validPackageName("acme/money"));
    assert.ok(validPackageName("acme-co/money_2"));
    for (const bad of ["money", "acme/", "/money", "acme/money@1", "acme money/x", "a/b/c", "--flag/x", ""]) {
      assert.ok(!validPackageName(bad), bad);
    }
  });

  it("parses bynk search, and refuses anything else", () => {
    const found = parseSearch(SEARCH)!;
    assert.strictEqual(found.latest, "0.4.0");
    assert.strictEqual(found.versions.length, 4);
    assert.strictEqual(found.versions[3].withdrawn, "malware");
    assert.strictEqual(parseSearch("not json"), undefined);
    assert.strictEqual(parseSearch("[]"), undefined);
    assert.strictEqual(parseSearch(JSON.stringify({ package: "a/b" })), undefined);
  });

  it("offers only usable versions, newest first", () => {
    const picks = pickableVersions(parseSearch(SEARCH)!).map((v) => v.version);
    assert.deepStrictEqual(picks, ["0.4.0", "0.3.1"]);
  });

  it("parses bynk deps", () => {
    const rows = parseDeps(
      JSON.stringify({
        dependencies: [
          { alias: "money", package: "acme/money", dev: false, kind: "registry", requested: "^0.3", source: "acme-internal", locked: "0.3.1" },
          { alias: "util", package: "acme/util", dev: true, kind: "path", requested: "../util", source: "../util", locked: null },
        ],
      }),
    )!;
    assert.strictEqual(rows[0].locked, "0.3.1");
    assert.strictEqual(rows[1].dev, true);
    assert.strictEqual(rows[1].locked, null);
    assert.strictEqual(parseDeps(JSON.stringify({ dependencies: [{ alias: "x" }] })), undefined);
    assert.strictEqual(parseDeps("nope"), undefined);
  });

  it("compares versions by semver precedence", () => {
    assert.strictEqual(compareVersions("0.4.0", "0.3.1"), 1);
    assert.strictEqual(compareVersions("0.3.1", "0.3.1"), 0);
    assert.strictEqual(compareVersions("1.0.0", "1.0.0-rc.1"), 1);
    assert.strictEqual(compareVersions("1.0.0-rc.2", "1.0.0-rc.10"), -1);
    assert.strictEqual(compareVersions("1.10.0", "1.9.0"), 1);
    assert.ok(Number.isNaN(compareVersions("^1", "1.0.0")));
  });

  it("decides when an upgrade is on offer", () => {
    const found = parseSearch(SEARCH);
    assert.strictEqual(currentVersion("0.3.1", "^0.3"), "0.3.1");
    assert.strictEqual(currentVersion(null, "^0.3.2"), "0.3.2");
    assert.strictEqual(currentVersion(null, "^1"), undefined);
    assert.strictEqual(upgradeTarget("0.3.1", found), "0.4.0");
    assert.strictEqual(upgradeTarget("0.4.0", found), undefined);
    assert.strictEqual(upgradeTarget("0.9.0", found), undefined);
    assert.strictEqual(upgradeTarget(undefined, found), undefined);
    assert.strictEqual(upgradeTarget("0.3.1", undefined), undefined, "offline: no lens");
    assert.strictEqual(upgradeTitle("0.3.1", "0.4.0"), "0.3.1 → 0.4.0 available");
  });

  describe("scanManifestDependencies", () => {
    const toml = [
      "[project]",
      'name = "app"',
      "",
      "[dependencies]",
      'money = { package = "acme/money", version = "^0.3.1" }',
      'local = { package = "acme/local", path = "../local" }',
      '"odd-name" = { version = "~1.2.0", package = "acme/odd" } # trailing',
      "",
      "[dev-dependencies]",
      'tester = { package = "acme/tester", version = "^2" }',
      "",
      "[dependencies.long]",
      'package = "acme/long"',
      'version = "^3.0.0"',
      "",
      "[registries.acme-internal]",
      'index = "https://example.test/"',
    ].join("\n");

    it("finds registry dependencies and the exact range text", () => {
      const deps = scanManifestDependencies(toml);
      assert.deepStrictEqual(
        deps.map((d) => [d.alias, d.package, d.range, d.dev]),
        [
          ["money", "acme/money", "^0.3.1", false],
          ["odd-name", "acme/odd", "~1.2.0", false],
          ["tester", "acme/tester", "^2", true],
          ["long", "acme/long", "^3.0.0", false],
        ],
      );
      const lines = toml.split("\n");
      for (const d of deps) {
        assert.strictEqual(lines[d.rangeLine].slice(d.rangeStart, d.rangeEnd), d.range, d.alias);
      }
    });

    it("skips git and path entries, and handles CRLF", () => {
      const deps = scanManifestDependencies(toml.replace(/\n/g, "\r\n"));
      assert.ok(!deps.some((d) => d.alias === "local"));
      assert.strictEqual(deps.length, 4);
    });

    it("ignores dependency-shaped text elsewhere", () => {
      assert.deepStrictEqual(
        scanManifestDependencies('[package]\nname = "a/b"\nversion = "1.0.0"\n[unstable]\nallow = ["x"]\n'),
        [],
      );
    });
  });

  it("parses a publish plan and states the identity, version, registry and files", () => {
    const plan = parsePublishPlan(
      JSON.stringify({
        package: "acme/money",
        version: "1.2.0",
        checksum: "sha256-abc",
        registry: "acme-internal",
        dry_run: true,
        files: Array.from({ length: 15 }, (_, i) => ({ path: `src/f${i}.bynk`, bytes: 1024 })),
      }),
    )!;
    assert.strictEqual(plan.dryRun, true);
    const text = publishPlanText(plan);
    for (const part of ["acme/money", "1.2.0", "acme-internal", "sha256-abc", "15 (15.0 KiB)", "src/f0.bynk", "… and 3 more"]) {
      assert.ok(text.includes(part), `${part} in:\n${text}`);
    }
    assert.ok(!text.includes("src/f14.bynk"), "only the first files are listed");
    assert.strictEqual(parsePublishPlan("{}"), undefined);
  });
});
