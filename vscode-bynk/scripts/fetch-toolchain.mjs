// Download the pinned release's `bynkc-lsp`, `bynk` and `bynkc` into
// `.bynk-test-bin/` under their plain names, for the integration tests to put
// on PATH (`BYNK_TEST_BIN_DIR=.bynk-test-bin npm test`) instead of binaries
// built from the compiler's source in `target/release`.
//
//   node scripts/fetch-toolchain.mjs

import fs from "node:fs";
import path from "node:path";
import { asset, pin, root, target } from "./release-assets.mjs";

const out = path.join(root, ".bynk-test-bin");
fs.mkdirSync(out, { recursive: true });
const exe = process.platform === "win32" ? ".exe" : "";
for (const tool of ["bynkc-lsp", "bynk", "bynkc"]) {
  const from = await asset(`${tool}-${target()}`);
  const to = path.join(out, `${tool}${exe}`);
  fs.copyFileSync(from, to);
  fs.chmodSync(to, 0o755);
}
console.log(`staged the ${pin} toolchain in ${out}`);
