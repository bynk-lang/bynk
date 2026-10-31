// @ts-check
//! Fail if a section number is used by more than one heading in the normative
//! specification (`src/content/docs/book/spec/*.md`). Spec sections are cited by
//! number (§7.3.4e), so a repeated number makes a citation ambiguous, and a
//! section copied by mistake can contradict its original. The link checker cannot
//! see either case. A duplicated §7.3.4e did exactly that (#2610, #2611).
//!
//! A section number is the `§…` token that opens a Markdown heading, for example
//! `### §4.8.9b trace_expr` → `§4.8.9b`. Numbers are compared across every
//! chapter, because each chapter's numbers carry its own prefix. Headings inside
//! code fences are ignored.
//!
//!   node scripts/check-spec-sections.mjs     report repeats; non-zero if any
//!
//! The `site` CI job runs this.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const SPEC = path.join(HERE, "../src/content/docs/book/spec");

/** @type {Map<string, string[]>} section number → `file:line` of each heading */
const seen = new Map();

for (const file of fs.readdirSync(SPEC).filter((f) => f.endsWith(".md")).sort()) {
  const lines = fs.readFileSync(path.join(SPEC, file), "utf8").split("\n");
  // The open fence's marker (e.g. "````"). Per CommonMark, a fence closes only
  // on a line of the same character, at least as long, with no info string.
  /** @type {string | null} */
  let fence = null;
  lines.forEach((line, i) => {
    const marker = line.match(/^ {0,3}(`{3,}|~{3,})(.*)$/);
    if (fence === null) {
      if (marker) fence = marker[1];
      else {
        const heading = line.match(/^#{1,6}\s+(§[^\s{]+)/);
        if (heading) record(heading[1], `${file}:${i + 1}`);
      }
      return;
    }
    if (
      marker &&
      marker[1][0] === fence[0] &&
      marker[1].length >= fence.length &&
      marker[2].trim() === ""
    ) {
      fence = null;
    }
  });
}

/** @param {string} number @param {string} where */
function record(number, where) {
  const list = seen.get(number);
  if (list) list.push(where);
  else seen.set(number, [where]);
}

const repeats = [...seen].filter(([, where]) => where.length > 1);
if (repeats.length > 0) {
  console.error("Spec section numbers used by more than one heading:");
  for (const [number, where] of repeats) {
    console.error(`  ${number}: ${where.join(", ")}`);
  }
  console.error(
    "\nGive each heading its own number, or remove the duplicate section.",
  );
  process.exit(1);
}
console.log(`spec sections: ${seen.size} numbered headings, no repeats`);
