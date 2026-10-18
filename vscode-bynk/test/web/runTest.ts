import * as path from "path";

import { runTests } from "@vscode/test-web";

// Launch VS Code for the Web in a headless Chromium with this extension loaded
// through its `browser` entry, open test/fixtures/web, and run the bundled
// Mocha suite (out/test/web/suite/index.js) in the browser extension host
// (#2102). Needs Playwright's Chromium (`npx playwright install chromium`) and
// network access for the VS Code web build, which test-web caches in
// `.vscode-test-web/`.
async function main() {
  try {
    const extensionDevelopmentPath = path.resolve(__dirname, "../../../");
    const extensionTestsPath = path.resolve(__dirname, "./suite/index.js");
    const folderPath = path.resolve(__dirname, "../../../test/fixtures/web");

    await runTests({
      browserType: "chromium",
      // `stable`, not test-web's `insiders` default: the build users get on
      // vscode.dev / github.dev.
      quality: "stable",
      extensionDevelopmentPath,
      extensionTestsPath,
      folderPath,
      headless: process.env.BYNK_WEB_TEST_HEADED !== "1",
    });
  } catch (err) {
    console.error("Failed to run web extension tests:", err);
    process.exit(1);
  }
}

main();
