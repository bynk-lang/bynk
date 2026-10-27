import * as fs from "fs";
import * as os from "os";
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
    // setting, then PATH, then a cached download. Prepend a directory holding
    // the server so activation connects to it deterministically — tests must
    // never hit the download-on-activate path. By default that is the repo's
    // release dir (a server built from this tree); with BYNK_TEST_BIN_DIR it is
    // a directory of the pinned release's binaries (`npm run fetch:toolchain`
    // stages `.bynk-test-bin`), which needs nothing from the compiler's
    // repository.
    const binDir = process.env.BYNK_TEST_BIN_DIR;
    const serverDir = binDir
      ? path.resolve(binDir)
      : path.resolve(__dirname, "../../../target/release");
    if (binDir) {
      const pin = pinnedVersion(path.join(extensionDevelopmentPath, "package.json"));
      assertServer(serverDir, pin, "npm run fetch:toolchain");
    } else {
      const build = "cargo build --release -p bynk-lsp -p bynkc -p bynk";
      assertServer(
        serverDir,
        workspaceVersion(path.resolve(serverDir, "../../Cargo.toml")),
        build,
      );
    }
    process.env.PATH = `${serverDir}${path.delimiter}${process.env.PATH ?? ""}`;

    // #2353: on a CI runner the window can hang at start-up
    // ("CodeWindow: detected unresponsive", ~16s in, before any test runs) and
    // a re-run clears it. `--disable-gpu` and `--disable-dev-shm-usage` remove
    // the usual causes under xvfb. The suite writes a marker when it starts, so
    // only a run that never reached it is retried (once); a failure after that
    // is a test failure and surfaces as is.
    const marker = path.join(os.tmpdir(), `bynk-vscode-started-${process.pid}`);
    process.env.BYNK_TEST_STARTED_MARKER = marker;
    const run = () => {
      fs.rmSync(marker, { force: true });
      return runTests({
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
    };
    try {
      await run();
    } catch (err) {
      if (fs.existsSync(marker)) throw err;
      console.error("VS Code failed before the suite started; retrying once:", err);
      await run();
    }
  } catch (err) {
    console.error("Failed to run integration tests:", err);
    process.exit(1);
  }
}

// #2179: prepending a server dir only helps if a server is there. Without one,
// PATH lookup fell through to whatever `bynkc-lsp` the machine had (a stale
// `~/.cargo/bin` install predating `bynk/explain`), and the suite failed on
// requests that server did not know. Refuse to start instead, and refuse a
// server whose version is not `want` (the workspace's for a build from this
// tree, the pin's for a downloaded release). `want` undefined skips the version
// check (a version that can't be read is not a failure).
function assertServer(serverDir: string, want: string | undefined, fix: string): void {
  const exe = process.platform === "win32" ? "bynkc-lsp.exe" : "bynkc-lsp";
  const server = path.join(serverDir, exe);
  if (!fs.existsSync(server)) {
    throw new Error(`no language server at ${server}; build it first with \`${fix}\``);
  }
  const run = spawnSync(server, ["--version"], { encoding: "utf8", timeout: 10_000 });
  // `bynkc-lsp 0.377.1`. A token comparison rather than a version regex: this
  // is an external process's output (see `parseVersion` in src/server.ts for
  // why an unanchored digit pattern is avoided on such input).
  const reported = (run.stdout ?? "").split(/\s+/).map((t) => t.replace(/^v/, ""));
  if (want !== undefined && !reported.includes(want)) {
    throw new Error(
      `${server} reports ${JSON.stringify((run.stdout ?? "").trim() || run.error?.message)}, ` +
        `but ${want} is expected; rebuild it with \`${fix}\``,
    );
  }
}

/** `bynkServerVersion` from the extension's package.json, without its `v`. */
function pinnedVersion(packageJson: string): string {
  const v = JSON.parse(fs.readFileSync(packageJson, "utf8")).bynkServerVersion as unknown;
  if (typeof v !== "string") throw new Error(`${packageJson} has no bynkServerVersion`);
  return v.replace(/^v/, "");
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
