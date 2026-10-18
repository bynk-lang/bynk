// Bundle the web test suite (#2102) for the browser extension host: one CommonJS
// file with `vscode` external, the shape @vscode/test-web loads from
// `extensionTestsPath`. The Node-side runner (test/web/runTest.ts) is compiled by
// `npm run compile-tests` like the rest of test/.
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname } from "node:path";

import * as esbuild from "esbuild";

const root = new URL("..", import.meta.url).pathname;

// Mocha's browser build is a UMD script, but mocha's package.json says
// `"type": "module"`, so esbuild bundles it as ESM, in the bundle's own scope.
// There its `typeof module !== "undefined"` branch fires and
// `module.exports = factory()` replaces the suite's exports (and `run` with it).
// Loading it from a namespace with no package.json makes esbuild treat it as
// CommonJS, with a module object of its own; the build still sets the `mocha`
// global the suite uses.
const mochaPath = createRequire(`${root}package.json`).resolve("mocha/mocha.js");
const mochaAsCommonJs = {
  name: "mocha-as-commonjs",
  setup(build) {
    build.onResolve({ filter: /^mocha\/mocha$/ }, () => ({ path: mochaPath, namespace: "mocha-cjs" }));
    build.onLoad({ filter: /.*/, namespace: "mocha-cjs" }, async () => ({
      contents: await readFile(mochaPath, "utf8"),
      loader: "js",
      resolveDir: dirname(mochaPath),
    }));
  },
};

await esbuild.build({
  absWorkingDir: root,
  entryPoints: ["test/web/suite/index.ts"],
  outfile: "out/test/web/suite/index.js",
  bundle: true,
  format: "cjs",
  platform: "browser",
  target: "es2022",
  external: ["vscode"],
  sourcemap: true,
  plugins: [mochaAsCommonJs],
});
console.log("built out/test/web/suite/index.js");
