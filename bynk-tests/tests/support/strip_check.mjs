// Strip-only verification harness for the in-browser track — the strip-only
// emission invariant (ADR 0136). Given a root directory, EVERY `.ts` file under
// it must be erasable by pure type-stripping: that is what lets bynkc's emitted
// output run under Node `--experimental-strip-types` (the `--inspect` debug path)
// and, ultimately, in the browser with no transpiler in the loop.
//
// `node:module`'s `stripTypeScriptTypes(code, { mode: 'strip' })` is the precise
// oracle — it performs exactly the strip-only transform Node applies when it runs
// a `.ts` file, throwing `ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX` on the non-erasable
// constructs (constructor parameter properties, `enum`, `namespace`). It works on
// a source string, so there is no execution and no import resolution — and the
// whole tree is checked in one process, paying Node startup once rather than
// per file. (`node --experimental-strip-types --check` is *not* a substitute: a
// leading `type`/`declare` statement trips its module-detection and false-fails,
// even though those strip cleanly.)
//
// Usage: node strip_check.mjs <root-dir>
//   exit 0 — every `.ts` strips cleanly
//   exit 1 — at least one file is non-erasable (FAIL lines printed to stdout)
//   exit 2 — `stripTypeScriptTypes` unavailable (Node < 22.13): caller should skip
import * as mod from "node:module";
import { readFileSync, readdirSync } from "node:fs";
import { join, relative } from "node:path";

const strip = mod.stripTypeScriptTypes;
if (typeof strip !== "function") {
  console.error("stripTypeScriptTypes unavailable (needs Node >= 22.13)");
  process.exit(2);
}

const root = process.argv[2];
if (!root) {
  console.error("usage: strip_check.mjs <root-dir>");
  process.exit(2);
}

function walk(dir, acc) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, entry.name);
    if (entry.isDirectory()) walk(p, acc);
    else if (entry.name.endsWith(".ts")) acc.push(p);
  }
  return acc;
}

const files = walk(root, []);
let failed = 0;
for (const file of files) {
  try {
    strip(readFileSync(file, "utf8"), { mode: "strip" });
  } catch (e) {
    failed++;
    const reason = (e.message || "").split("\n")[0];
    console.log(`FAIL\t${relative(root, file)}\t${e.code || e.name}\t${reason}`);
  }
}
console.log(`CHECKED\t${files.length}`);
process.exit(failed > 0 ? 1 : 0);
