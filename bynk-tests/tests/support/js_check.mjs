// Validate that every `.js` file under a root is syntactically valid (parsed as
// an ES module) — the verification side of the in-browser track's first-class JS
// artefact (slice 1, ADR 0137). `bynkc compile --emit js` type-strips the emitted
// TypeScript via oxc; this confirms the result is runnable JavaScript with no
// residual TypeScript syntax (a surviving annotation would fail `node --check`).
//
// `node --check <file>` is one process per file, so the checks fan out across a
// bounded worker pool in this single driver process. A root `package.json`
// (`type: module`, staged by the caller) makes Node parse the `.js` as ESM
// regardless of Node version.
//
// Usage: node js_check.mjs <root-dir>
//   exit 0 — every `.js` parses; exit 1 — at least one failed (FAIL lines on stdout)
import { readdirSync } from "node:fs";
import { join, relative } from "node:path";
import { execFile } from "node:child_process";

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
let cursor = 0;

function check(file) {
  return new Promise((resolve) => {
    execFile(process.execPath, ["--check", file], (err, _stdout, stderr) => {
      if (err) {
        failed++;
        const reason = (stderr || "").split("\n").find((l) => l.includes("Error")) || "invalid";
        console.log(`FAIL\t${relative(root, file)}\t${reason.trim()}`);
      }
      resolve();
    });
  });
}

async function worker() {
  while (cursor < files.length) {
    await check(files[cursor++]);
  }
}

await Promise.all(Array.from({ length: 8 }, worker));
console.log(`CHECKED\t${files.length}`);
process.exit(failed > 0 ? 1 : 0);
