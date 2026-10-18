// #2096: `bynk.toml` editing support. The schema association is declarative
// (`contributes.tomlValidation`), so its contract is package.json; the TOML
// extension recommendation's policy is a pure function.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

import {
  MANIFEST_SCHEMA_URL,
  isManifestPath,
  shouldRecommendTomlExtension,
} from "../../src/manifestSchema";

// Compiled to out/test/suite/, so the package root is three levels up.
const packageJson = JSON.parse(
  fs.readFileSync(path.resolve(__dirname, "../../../package.json"), "utf8"),
);

describe("bynk.toml schema association", () => {
  it("maps bynk.toml to the published schema", () => {
    const rules: { fileMatch?: string; url?: string }[] =
      packageJson.contributes?.tomlValidation ?? [];
    assert.ok(
      rules.some((r) => r.fileMatch === "bynk.toml" && r.url === MANIFEST_SCHEMA_URL),
      `a tomlValidation rule maps bynk.toml to ${MANIFEST_SCHEMA_URL}`,
    );
  });

  it("recommends the TOML extension rather than depending on it", () => {
    for (const key of ["extensionDependencies", "extensionPack"]) {
      assert.ok(
        !(packageJson[key] ?? []).includes("tamasfe.even-better-toml"),
        `${key} does not require a TOML extension`,
      );
    }
  });
});

describe("isManifestPath", () => {
  it("matches bynk.toml under either separator", () => {
    assert.ok(isManifestPath("/work/shop/bynk.toml"));
    assert.ok(isManifestPath("C:\\work\\shop\\bynk.toml"));
    assert.ok(isManifestPath("bynk.toml"));
  });

  it("rejects other TOML files", () => {
    assert.ok(!isManifestPath("/work/shop/wrangler.toml"));
    assert.ok(!isManifestPath("/work/shop/notbynk.toml"));
    assert.ok(!isManifestPath("/work/shop/bynk.toml.bak"));
  });
});

describe("shouldRecommendTomlExtension", () => {
  const base = {
    fileName: "/work/shop/bynk.toml",
    tomlExtensionInstalled: false,
    dismissed: false,
    shownThisSession: false,
  };

  it("recommends on opening a manifest with no TOML extension", () => {
    assert.ok(shouldRecommendTomlExtension(base));
  });

  it("stays quiet when one is installed, once dismissed, or once shown", () => {
    assert.ok(!shouldRecommendTomlExtension({ ...base, tomlExtensionInstalled: true }));
    assert.ok(!shouldRecommendTomlExtension({ ...base, dismissed: true }));
    assert.ok(!shouldRecommendTomlExtension({ ...base, shownThisSession: true }));
  });

  it("ignores files that aren't a manifest", () => {
    assert.ok(!shouldRecommendTomlExtension({ ...base, fileName: "/work/shop/src/a.bynk" }));
  });
});
