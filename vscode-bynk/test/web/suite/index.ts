// The web test entry point (#2102): runs inside the browser extension host, so
// it uses Mocha's browser build. Bundled by scripts/build-web-tests.mjs; invoked
// by @vscode/test-web via `extensionTestsPath` (see test/web/runTest.ts).
import "mocha/mocha";

export function run(): Promise<void> {
  return new Promise((resolve, reject) => {
    // `spec`, not the browser default (`html`): the extension host is a worker with
    // no `document`, and spec writes to the console test-web relays.
    mocha.setup({ ui: "bdd", reporter: "spec", timeout: 60_000 });
    // Registers its `describe` blocks against the global set up above.
    require("./web.test");
    try {
      mocha.run((failures) => {
        if (failures > 0) reject(new Error(`${failures} web test(s) failed.`));
        else resolve();
      });
    } catch (err) {
      reject(err);
    }
  });
}
