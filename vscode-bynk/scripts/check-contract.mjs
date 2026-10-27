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

import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  legendMismatches,
  missingKeywords,
  parseSums,
  releaseTarget,
  snippetSkeletons,
} from "./contract-lib.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
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
const pin = pkg.bynkServerVersion;
const releases = `https://github.com/bynk-lang/bynk/releases/download/${pin}`;
const cache = path.join(root, ".contract-cache", pin);

/** The release's SHA256SUMS as `{ name: hash }`. */
async function sums() {
  const res = await fetch(`${releases}/SHA256SUMS`);
  if (!res.ok) throw new Error(`cannot fetch ${releases}/SHA256SUMS: HTTP ${res.status}`);
  return parseSums(await res.text());
}

/** Download release asset `name`, verified against SHA256SUMS, cached per pin. */
async function asset(name, table) {
  const file = path.join(cache, name);
  const hash = table[name];
  if (!hash) throw new Error(`${name} is not in ${pin}'s SHA256SUMS (a pin that predates it?)`);
  const sha = (b) => createHash("sha256").update(b).digest("hex");
  if (fs.existsSync(file) && sha(fs.readFileSync(file)) === hash) return file;
  const res = await fetch(`${releases}/${name}`);
  if (!res.ok) throw new Error(`cannot fetch ${releases}/${name}: HTTP ${res.status}`);
  const bytes = Buffer.from(await res.arrayBuffer());
  if (sha(bytes) !== hash) throw new Error(`${name}: checksum does not match ${pin}'s SHA256SUMS`);
  fs.mkdirSync(cache, { recursive: true });
  fs.writeFileSync(file, bytes, { mode: 0o755 });
  return file;
}

/** The release target triple of this machine. */
function target() {
  const triple = releaseTarget(process.platform, process.arch);
  if (!triple) throw new Error(`no prebuilt Bynk binaries for ${process.platform}/${process.arch}; pass --bynkc`);
  return triple;
}

let table;
const lazySums = async () => (table ??= await sums());

let contractFile = arg("--contract", "BYNK_EDITOR_CONTRACT");
if (!contractFile) contractFile = await asset(`bynk-editor-contract-${pin}.json`, await lazySums());
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
if (!bynkc) bynkc = await asset(`bynkc-${target()}`, await lazySums());
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
