// @ts-check
//! Verify every `/book/` and `/reference/` link in the curated `public/llms.txt` index
//! resolves to a real source page (documentation track). `llms.txt` is
//! hand-authored, so it can drift when a page is renamed or moved; this confirms
//! each route maps to a committed source page. `/book/` routes resolve against the
//! Book, `/reference/` routes against the Reference tree. External
//! links and in-code `](…)` (e.g. type signatures) match neither prefix, so they
//! are naturally skipped.
//!
//!   node scripts/check-llms-links.mjs     report broken links; non-zero if any
//!
//! The `site` CI job runs this.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const LLMS = path.join(HERE, "../public/llms.txt");
// Surface prefix → its content root. A route's leading segment selects the root.
const ROOTS = {
  "/book/": path.join(HERE, "../src/content/docs/book"),
  "/reference/": path.join(HERE, "../src/content/docs/reference"),
};

const text = fs.readFileSync(LLMS, "utf8");
const broken = [];
for (const m of text.matchAll(/\]\((\/(?:book|reference)\/[^)#]*)(?:#[^)]*)?\)/g)) {
  const route = m[1];
  // `/reference/api/…` is the generated rustdoc tree (built by cargo doc in the deploy
  // workflow), not a committed content page — nothing to resolve here.
  if (route.startsWith("/reference/api/")) continue;
  const prefix = `/${route.split("/")[1]}/`;
  const root = ROOTS[prefix];
  const slug = route.slice(prefix.length).replace(/\/$/, "");
  const candidates =
    slug === ""
      ? ["index.md", "index.mdx"]
      : [`${slug}.md`, `${slug}.mdx`, `${slug}/index.md`, `${slug}/index.mdx`];
  if (!candidates.some((rel) => fs.existsSync(path.join(root, rel)))) {
    broken.push(route);
  }
}

if (broken.length > 0) {
  console.error(`llms.txt has ${broken.length} broken /book/ link(s):`);
  for (const r of broken) console.error(`  ${r}`);
  process.exit(1);
}
console.log("llms.txt links all resolve.");
