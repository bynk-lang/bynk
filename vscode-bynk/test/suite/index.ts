import * as fs from "fs";
import * as path from "path";

import { glob } from "glob";
import Mocha from "mocha";

// The extension-host entry point: discover and run every compiled `*.test.js`
// spec under this directory with Mocha. Invoked by @vscode/test-electron via
// `extensionTestsPath` (see test/runTest.ts).
export async function run(): Promise<void> {
  // #2353: tell the launcher the host came up, so it retries only a window that
  // never did (see test/runTest.ts).
  const marker = process.env.BYNK_TEST_STARTED_MARKER;
  if (marker) fs.writeFileSync(marker, "");
  const mocha = new Mocha({ ui: "bdd", color: true, timeout: 60_000 });
  const testsRoot = __dirname;

  const files = await glob("**/*.test.js", { cwd: testsRoot });
  for (const file of files) {
    mocha.addFile(path.resolve(testsRoot, file));
  }

  await new Promise<void>((resolve, reject) => {
    try {
      mocha.run((failures) => {
        if (failures > 0) {
          reject(new Error(`${failures} test(s) failed.`));
        } else {
          resolve();
        }
      });
    } catch (err) {
      reject(err);
    }
  });
}
