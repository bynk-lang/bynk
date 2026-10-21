// Unit coverage for the version-comparison helpers in src/server.ts (#484:
// warn actionably when a stale `bynkc-lsp` resolved from PATH is older than
// the extension's pinned target).

import * as assert from "assert";

import {
  compareVersions,
  expectedServerVersion,
  parseVersion,
  serverAssetName,
  toolAssetName,
} from "../../src/server";

describe("parseVersion", () => {
  it("extracts MAJOR.MINOR.PATCH from a `--version` line", () => {
    assert.deepStrictEqual(parseVersion("bynkc-lsp 0.129.0"), [0, 129, 0]);
  });

  it("extracts from a bare or `v`-prefixed version string", () => {
    assert.deepStrictEqual(parseVersion("v0.132.1"), [0, 132, 1]);
    assert.deepStrictEqual(parseVersion("0.132.1"), [0, 132, 1]);
  });

  it("returns undefined when no version pattern is present", () => {
    assert.strictEqual(parseVersion("not a version"), undefined);
    assert.strictEqual(parseVersion(""), undefined);
  });

  it("extracts the first match and ignores trailing text", () => {
    assert.deepStrictEqual(parseVersion("0.129.0-dev+adversarial"), [0, 129, 0]);
  });

  it("stays fast on a long dot-free digit run (no unbounded regex backtracking)", () => {
    const adversarial = "9".repeat(50_000);
    const start = Date.now();
    assert.strictEqual(parseVersion(adversarial), undefined);
    assert.ok(Date.now() - start < 500, "parseVersion must run in linear time");
  });
});

describe("compareVersions", () => {
  it("orders by major, then minor, then patch", () => {
    assert.strictEqual(compareVersions([0, 129, 0], [0, 132, 1]), -1);
    assert.strictEqual(compareVersions([0, 132, 1], [0, 129, 0]), 1);
    assert.strictEqual(compareVersions([1, 0, 0], [0, 999, 999]), 1);
    assert.strictEqual(compareVersions([0, 132, 0], [0, 132, 1]), -1);
  });

  it("returns 0 for equal versions", () => {
    assert.strictEqual(compareVersions([0, 132, 1], [0, 132, 1]), 0);
  });
});

describe("expectedServerVersion (#1673)", () => {
  it("holds a downloaded or cached server to the pin, the last shipped release", () => {
    assert.strictEqual(expectedServerVersion("downloaded", "v0.290.0", "0.303.3"), "0.290.0");
    assert.strictEqual(expectedServerVersion("cached", "v0.290.0", "0.303.3"), "0.290.0");
  });

  it("holds a server from PATH or the setting to the extension's own version", () => {
    assert.strictEqual(expectedServerVersion("path", "v0.290.0", "0.303.3"), "0.303.3");
    assert.strictEqual(expectedServerVersion("setting", "v0.290.0", "0.303.3"), "0.303.3");
  });
});

describe("toolAssetName", () => {
  it("names the raw per-platform binary assets release.yml publishes", () => {
    assert.strictEqual(toolAssetName("bynk", "x86_64-unknown-linux-gnu"), "bynk-x86_64-unknown-linux-gnu");
    assert.strictEqual(toolAssetName("bynkc", "aarch64-apple-darwin"), "bynkc-aarch64-apple-darwin");
    assert.strictEqual(toolAssetName("bynk", "x86_64-pc-windows-msvc"), "bynk-x86_64-pc-windows-msvc.exe");
  });

  it("keeps the server's asset name unchanged", () => {
    assert.strictEqual(serverAssetName("x86_64-unknown-linux-gnu"), "bynkc-lsp-x86_64-unknown-linux-gnu");
    assert.strictEqual(serverAssetName("x86_64-pc-windows-msvc"), "bynkc-lsp-x86_64-pc-windows-msvc.exe");
  });
});
