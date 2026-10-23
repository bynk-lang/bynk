import * as fs from "fs";
import * as path from "path";
import { spawnSync } from "child_process";

import { runTests } from "@vscode/test-electron";

// Pin the VS Code build under test. An unpinned run resolves "stable" over the
// network on every invocation — the flaky "Resolving version…" step that can
// fail with ECONNRESET and reddens CI for reasons unrelated to the change. A
// concrete version makes that resolve local and the download cacheable. Pinned
// to a recent stable (what "stable" was already resolving to); CI overrides via
// `VSCODE_TEST_VERSION` and caches the matching download. Bump deliberately.
const VSCODE_VERSION = process.env.VSCODE_TEST_VERSION ?? "1.140.0";

// A failure sooner than this after launch is a window that never came up, not a
// test failure (see the retry in `main`).
const STARTUP_WINDOW_MS = 60_000;

// Bootstraps a real VS Code instance with the extension under
// `--extensionDevelopmentPath` plus the fixture workspace, then runs the Mocha
// suite inside the extension host. See test/suite/index.ts for the runner.
async function main() {
  try {
    // The extension root (manifest + bundled `out/extension.js`).
    const extensionDevelopmentPath = path.resolve(__dirname, "../../");
    // The compiled Mocha entry point (out/test/suite/index.js).
    const extensionTestsPath = path.resolve(__dirname, "./suite/index");
    // The fixture project the host opens (a `bynk.toml` + `src/`).
    const workspace = path.resolve(
      __dirname,
      "../../test/fixtures/workspace",
    );

    // The extension resolves `bynkc-lsp` from the `bynk.executablePath`
    // setting, then PATH, then a cached download. Prepend the repo's release
    // dir so activation connects to a locally-built server deterministically —
    // tests must never hit the download-on-activate path.
    const serverDir = path.resolve(__dirname, "../../../target/release");
    assertReleaseServer(serverDir);
    process.env.PATH = `${serverDir}${path.delimiter}${process.env.PATH ?? ""}`;

    // #2353: on a CI runner the window can hang at start-up
    // ("CodeWindow: detected unresponsive", ~16s in, before any test runs) and
    // a re-run clears it. `--disable-gpu` and `--disable-dev-shm-usage` remove
    // the usual causes under xvfb; a failure inside the start-up window is
    // retried once, a failure after it is a test failure and is not.
    const run = () =>
      runTests({
        version: VSCODE_VERSION,
        extensionDevelopmentPath,
        extensionTestsPath,
        // Open the fixture workspace; `--disable-extensions` keeps third-party
        // extensions out of the host (the extension under test still loads).
        launchArgs: [
          workspace,
          "--disable-extensions",
          "--disable-gpu",
          "--disable-dev-shm-usage",
        ],
      });
    const started = Date.now();
    try {
      await run();
    } catch (err) {
      if (Date.now() - started > STARTUP_WINDOW_MS) throw err;
      console.error("VS Code failed during start-up; retrying once:", err);
      await run();
    }
  } catch (err) {
    console.error("Failed to run integration tests:", err);
    process.exit(1);
  }
}

// #2179: prepending `target/release` only helps if a server is there. Without
// one, PATH lookup fell through to whatever `bynkc-lsp` the machine had (a stale
// `~/.cargo/bin` install predating `bynk/explain`), and the suite failed on
// requests that server did not know. Refuse to start instead, and refuse a
// release build whose version is not this workspace's.
function assertReleaseServer(serverDir: string): void {
  const exe = process.platform === "win32" ? "bynkc-lsp.exe" : "bynkc-lsp";
  const server = path.join(serverDir, exe);
  const build = "cargo build --release -p bynk-lsp -p bynkc -p bynk";
  if (!fs.existsSync(server)) {
    throw new Error(`no language server at ${server}; build it first with \`${build}\``);
  }
  const want = workspaceVersion(path.resolve(serverDir, "../../Cargo.toml"));
  const run = spawnSync(server, ["--version"], { encoding: "utf8", timeout: 10_000 });
  // `bynkc-lsp 0.377.1`. A token comparison rather than a version regex: this
  // is an external process's output (see `parseVersion` in src/server.ts for
  // why an unanchored digit pattern is avoided on such input).
  const reported = (run.stdout ?? "").split(/\s+/).map((t) => t.replace(/^v/, ""));
  if (want !== undefined && !reported.includes(want)) {
    throw new Error(
      `${server} reports ${JSON.stringify((run.stdout ?? "").trim() || run.error?.message)}, ` +
        `but the workspace is ${want}; rebuild it with \`${build}\``,
    );
  }
}

/** `[workspace.package] version` from the root Cargo.toml, or undefined if it
 *  can't be read (the version check is then skipped, not failed). */
function workspaceVersion(cargoToml: string): string | undefined {
  let text: string;
  try {
    text = fs.readFileSync(cargoToml, "utf8");
  } catch {
    return undefined;
  }
  let inSection = false;
  for (const line of text.split(/\r?\n/)) {
    const header = /^\s*\[([^\]]*)\]/.exec(line);
    if (header) {
      inSection = header[1].trim() === "workspace.package";
      continue;
    }
    const v = inSection ? /^\s*version\s*=\s*"([^"]*)"/.exec(line) : null;
    if (v) return v[1];
  }
  return undefined;
}

main();
