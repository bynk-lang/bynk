// The files are parsed in this one process with `vm.SourceTextModule` (parse
// only: nothing is linked or evaluated), which is V8's own ES-module parser, so
// a surviving TypeScript construct fails exactly as it would under `node
// --check`. The caller runs this with `--experimental-vm-modules`. (#3387: the
// check used to spawn `node --check` per file; at ~6,800 files the ~65 ms
// process startup each dominated the corpus-wide test.)
//
// Usage: node --experimental-vm-modules js_check.mjs <root-dir>
//   exit 0 — every `.js` parses; exit 1 — at least one failed (FAIL lines on stdout)
import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";
import vm from "node:vm";

const root = process.argv[2];
if (!root) {
  console.error("usage: js_check.mjs <root-dir>");
  process.exit(2);
}

function walk(dir, acc) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, entry.name);
    if (entry.isDirectory()) walk(p, acc);
    else if (entry.name.endsWith(".js")) acc.push(p);
  }
  return acc;
}

const files = walk(root, []);
let failed = 0;

for (const file of files) {
  try {
    new vm.SourceTextModule(readFileSync(file, "utf8"), { identifier: file });
  } catch (err) {
    failed++;
    console.log(`FAIL\t${relative(root, file)}\t${String(err && err.message).split("\n")[0].trim()}`);
  }
}

console.log(`CHECKED\t${files.length}`);
process.exit(failed > 0 ? 1 : 0);
