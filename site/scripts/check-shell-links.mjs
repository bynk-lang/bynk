#!/usr/bin/env node
// Check the links the page shell and the home page draw from components
// (web presence slice 8): the theme's header and site map, and the home
// page's sections. starlight-links-validator checks only links written in
// Markdown, so these would otherwise go unchecked.
//
// Reads the built site (`dist/`, so run it after `astro build`): every
// site-relative href in the home page and in one Book page must name a built
// page, and a `#fragment` must name an id on that page. Links under /blog/
// are the blog's (another build), so they are skipped. A link that a
// `_redirects` rule sends on to a built page still works, so it passes with a
// note to update it at its source (the theme's site map can trail a rename
// until the theme's next release, as it did in slice 12).
//
//   node scripts/check-shell-links.mjs [dist]

import fs from "node:fs";
import path from "node:path";

const dist = path.resolve(process.argv[2] ?? "dist");
const PAGES = ["index.html", "book/index.html"];

/** The built file a site path is served from, or null. */
function fileFor(pathname) {
  const p = decodeURIComponent(pathname).replace(/^\//, "");
  for (const candidate of [p, path.join(p, "index.html")]) {
    const abs = path.join(dist, candidate);
    if (fs.existsSync(abs) && fs.statSync(abs).isFile()) return abs;
  }
  return null;
}

/** `_redirects`' exact rules (no splats), source path → destination. */
const redirects = new Map();
const redirectsFile = path.join(dist, "_redirects");
if (fs.existsSync(redirectsFile)) {
  for (const line of fs.readFileSync(redirectsFile, "utf8").split("\n")) {
    const [from, to] = line.trim().split(/\s+/);
    if (from?.startsWith("/") && !from.includes("*") && to) redirects.set(from, to);
  }
}

const hrefs = (html) => [...html.matchAll(/<a\b[^>]*\bhref="([^"]+)"/g)].map((m) => m[1].replaceAll("&amp;", "&"));

const problems = [];
const notes = [];
let checked = 0;
for (const page of PAGES) {
  const html = fs.readFileSync(path.join(dist, page), "utf8");
  for (const href of new Set(hrefs(html))) {
    if (!href.startsWith("/") || href.startsWith("//") || href.startsWith("/blog/") || href === "/blog") continue;
    const [withQuery, fragment] = href.split("#");
    const pathname = withQuery.split("?")[0];
    checked++;
    let file = fileFor(pathname);
    const to = redirects.get(pathname);
    if (!file && to?.startsWith("/") && (file = fileFor(to.split("#")[0]))) {
      notes.push(`${page}: ${href} redirects to ${to}; link to that at the source`);
    }
    if (!file) {
      problems.push(`${page}: ${href} — no such page`);
      continue;
    }
    if (fragment && !fs.readFileSync(file, "utf8").includes(`id="${fragment}"`)) {
      problems.push(`${page}: ${href} — no #${fragment} on that page`);
    }
  }
}

if (notes.length) console.log(`Note:\n  ${notes.join("\n  ")}`);
if (problems.length) {
  console.error(`The shell's links: ${problems.length} broken\n  ${problems.join("\n  ")}`);
  process.exit(1);
}
console.log(`The shell's links resolve (${checked} checked on ${PAGES.length} pages; /blog/ links are the blog's).`);
