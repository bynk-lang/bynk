#!/usr/bin/env node
// The npm security gate CI runs over each package tree with a committed
// lockfile (`.github/workflows/ci.yml`, job `lint`).
//
//   node scripts/npm-audit-gate.mjs <tree> [-- <extra npm audit args>]
//
// It runs `npm audit --json` in <tree> and fails on any high or critical
// advisory, exactly as `npm audit --audit-level=high` did, except for advisories
// listed for that tree in `scripts/npm-audit-allowlist.json`.
//
// The allowlist exists for one situation: an advisory with **no patched
// release** (so there is nothing to upgrade or pin to) that would otherwise fail
// every PR touching the tree. Each entry names the advisory, the tree, why it is
// accepted, and a `review_by` date. Past that date the entry stops exempting and
// the gate fails, so the exemption cannot quietly outlive its reason. An entry
// that no longer matches anything is reported, so it gets removed once the fix
// lands.

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const [tree, sep, ...extra] = process.argv.slice(2);
if (!tree || (sep !== undefined && sep !== "--")) {
  console.error("usage: npm-audit-gate.mjs <tree> [-- <extra npm audit args>]");
  process.exit(2);
}

const here = dirname(fileURLToPath(import.meta.url));
const allowlist = JSON.parse(readFileSync(join(here, "npm-audit-allowlist.json"), "utf8"))
  .advisories.filter((a) => a.tree === tree);
const today = new Date().toISOString().slice(0, 10);

let raw;
try {
  raw = execFileSync("npm", ["audit", "--json", ...extra], {
    cwd: join(here, "..", tree),
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
  });
} catch (e) {
  // `npm audit` exits non-zero whenever it finds anything; the JSON is still on stdout.
  raw = e.stdout;
}
const report = JSON.parse(raw);
if (report.error) {
  console.error(`npm audit failed in ${tree}: ${JSON.stringify(report.error)}`);
  process.exit(1);
}

// Each advisory, once, with the packages it reaches.
const advisories = new Map();
for (const vuln of Object.values(report.vulnerabilities ?? {})) {
  for (const via of vuln.via) {
    if (typeof via !== "object" || !["high", "critical"].includes(via.severity)) continue;
    const id = via.url.split("/").pop();
    advisories.set(id, { id, name: via.name, severity: via.severity, title: via.title });
  }
}

let failed = false;
for (const adv of advisories.values()) {
  const entry = allowlist.find((a) => a.id === adv.id);
  const label = `${adv.severity} ${adv.id} (${adv.name}): ${adv.title}`;
  if (!entry) {
    console.log(`::error::${tree}: ${label}`);
    failed = true;
  } else if (entry.review_by < today) {
    console.log(`::error::${tree}: ${label} — allowlist entry expired on ${entry.review_by}; recheck for a patched release`);
    failed = true;
  } else {
    console.log(`::warning::${tree}: ${label} — allowed until ${entry.review_by}: ${entry.reason}`);
  }
}
for (const entry of allowlist) {
  if (!advisories.has(entry.id)) {
    console.log(`::warning::${tree}: allowlist entry ${entry.id} no longer matches any advisory — remove it`);
  }
}
console.log(`${tree}: ${advisories.size} high/critical advisory(ies), ${failed ? "gate FAILED" : "gate passed"}`);
process.exit(failed ? 1 : 0);
