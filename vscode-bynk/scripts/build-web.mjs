// Build the web extension (#2102): the browser entry (`out/web/extension.js`),
// its language-server worker (`out/web/server.js`), and the `bynk-wasm` module
// the worker fetches (`out/web/bynk_wasm_bg.wasm`).
//
// The wasm glue + module come from `npm run build:wasm`, which runs the
// repo's scripts/build-wasm.sh into `wasm/` (gitignored). They are a build
// artefact, never committed, the same as the playground's `src/vendor/`.
import { copyFileSync, existsSync, mkdirSync } from "node:fs";

import * as esbuild from "esbuild";

const root = new URL("..", import.meta.url).pathname;
const glue = `${root}wasm/bynk_wasm.js`;
const module = `${root}wasm/bynk_wasm_bg.wasm`;

if (!existsSync(glue) || !existsSync(module)) {
  console.error(
    "build:web needs the bynk-wasm glue in wasm/. Run `npm run build:wasm` first " +
      "(needs the wasm32-unknown-unknown target and wasm-bindgen-cli).",
  );
  process.exit(1);
}

const minify = process.env.BYNK_WEB_NO_MINIFY !== "1";
const common = {
  absWorkingDir: root,
  bundle: true,
  platform: "browser",
  target: "es2022",
  minify,
  sourcemap: !minify,
};

// The extension host loads the browser entry as CommonJS, with `vscode`
// provided by the host.
await esbuild.build({
  ...common,
  entryPoints: ["src/web/extension.ts"],
  outfile: "out/web/extension.js",
  format: "cjs",
  external: ["vscode"],
});

// The worker is a classic script. The glue's only `import.meta.url` use is its
// default module path, which the worker never takes (it passes the bytes it
// fetched from `initializationOptions.wasmUrl`).
await esbuild.build({
  ...common,
  entryPoints: ["src/web/server.ts"],
  outfile: "out/web/server.js",
  format: "iife",
  alias: { "bynk-wasm": glue },
  logOverride: { "empty-import-meta": "silent" },
});

mkdirSync(`${root}out/web`, { recursive: true });
copyFileSync(module, `${root}out/web/bynk_wasm_bg.wasm`);
console.log("built out/web/ (extension.js, server.js, bynk_wasm_bg.wasm)");
