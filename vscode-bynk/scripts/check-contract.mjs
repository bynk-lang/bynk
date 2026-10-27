// Check the extension against the compiler's published editor contract, for the
// server release `bynkServerVersion` pins:
//   - every reserved keyword is highlighted by the TextMate grammar;
//   - the custom semantic-token types/modifiers match package.json;
//   - every snippet still parses (`bynkc fmt -` as the parse oracle).
// The checks live in contract-lib.mjs. This script fetches the inputs.
//
//   node scripts/check-contract.mjs [--contract FILE] [--bynkc FILE]
//
// --contract / --bynkc (or BYNK_EDITOR_CONTRACT / BYNKC) use local files instead
// of downloading the pinned release's, e.g. to check against a build from source.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import {
  legendMismatches,
  missingKeywords,
  snippetSkeletons,
} from "./contract-lib.mjs";
import { asset, pin, root, target } from "./release-assets.mjs";

const read = (p) => fs.readFileSync(path.join(root, p), "utf8");

function arg(name, env) {
  const i = process.argv.indexOf(name);
  if (i === -1) return process.env[env];
  const value = process.argv[i + 1];
  // A flag without its value must not quietly fall back to the download: the
  // caller asked to check a local build.
  if (value === undefined || value.startsWith("--")) {
    console.error(`${name} needs a value`);
    process.exit(2);
  }
  return value;
}

const pkg = JSON.parse(read("package.json"));

let contractFile = arg("--contract", "BYNK_EDITOR_CONTRACT");
if (!contractFile) contractFile = await asset(`bynk-editor-contract-${pin}.json`);
const contract = JSON.parse(fs.readFileSync(contractFile, "utf8"));
console.log(`checking against ${contractFile}${contract.version ? ` (${contract.version})` : ""}`);

const failures = [];

const missing = missingKeywords(contract, JSON.parse(read("syntaxes/bynk.tmLanguage.json")));
if (missing.length)
  failures.push(
    `reserved keywords not highlighted in syntaxes/bynk.tmLanguage.json: ${missing.join(", ")}\n` +
      "  Add each to a `keywords`/`types` match pattern so the editor does not lag the language.",
  );

failures.push(...legendMismatches(contract, pkg).map((m) => `package.json ${m}`));

let bynkc = arg("--bynkc", "BYNKC");
if (!bynkc) {
  try {
    bynkc = await asset(`bynkc-${target()}`);
  } catch (e) {
    throw new Error(`${e.message}; pass --bynkc to use a local build`);
  }
}
for (const [name, source] of snippetSkeletons(JSON.parse(read("snippets/bynk.json")))) {
  const r = spawnSync(bynkc, ["fmt", "-"], { input: source, encoding: "utf8" });
  if (r.error) throw r.error;
  if (r.status !== 0)
    failures.push(
      `snippet \`${name}\` no longer parses against the current grammar\n--- skeleton ---\n${source}\n--- errors ---\n${r.stderr || r.stdout}`,
    );
}

if (failures.length) {
  console.error(`\n${failures.length} contract failure(s):\n`);
  for (const f of failures) console.error(`- ${f}\n`);
  process.exit(1);
}
console.log("extension matches the editor contract");
