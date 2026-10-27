// Stage the bynk-wasm module + glue from the pinned release into `wasm/`, where
// `build-web.mjs` expects them, instead of building them from the compiler's
// source (`npm run build:wasm`, which needs the compiler's repository, a Rust
// toolchain with the wasm32 target, and wasm-bindgen-cli). The release's
// `bynk-web-<tag>.tar.gz` is the same artefact the playground builds from.
//
//   node scripts/fetch-web-assets.mjs

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { asset, pin, root } from "./release-assets.mjs";

const tarball = await asset(`bynk-web-${pin}.tar.gz`);
const out = path.join(root, "wasm");
fs.rmSync(out, { recursive: true, force: true });
fs.mkdirSync(out, { recursive: true });
// One top-level directory, `bynk-web-<tag>/`; `--strip-components` drops it.
const r = spawnSync("tar", ["-xzf", tarball, "-C", out, "--strip-components=1"], { encoding: "utf8" });
if (r.error || r.status !== 0) throw new Error(`cannot unpack ${tarball}: ${r.error?.message ?? r.stderr}`);
for (const f of ["bynk_wasm.js", "bynk_wasm_bg.wasm"])
  if (!fs.existsSync(path.join(out, f))) throw new Error(`${tarball} has no ${f}`);
console.log(`staged the ${pin} bynk-wasm module in wasm/`);
