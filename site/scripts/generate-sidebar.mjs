// @ts-check
//! Generate (or verify) the Starlight sidebar from `site/src/SUMMARY.md`
//! (documentation track). `SUMMARY.md` is the Book's authored table of
//! contents; this turns it into the committed `site/src/generated/sidebar.json`
//! that `astro.config.mjs` imports, so the sidebar tracks the Book's own order
//! and grouping rather than directory structure.
//!
//!   node scripts/generate-sidebar.mjs            regenerate sidebar.json
//!   node scripts/generate-sidebar.mjs --check     fail (non-zero) if out of date,
//!                                                 or if the contents are incomplete
//!
//! The `site` CI job runs `--check`; a `SUMMARY.md` edit that isn't regenerated
//! fails the build. Mirrors the repo's generate→commit→drift-guard pattern.
//!
//! The contents must also be complete (web presence track, slice 1): every page
//! under `src/content/docs/book/` is listed in `SUMMARY.md`, and every entry
//! names a page. The sidebar comes from `SUMMARY.md`, not the folders, so a page
//! nobody lists exists but no sidebar shows it. `--check` fails naming each one;
//! a plain run warns.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const SUMMARY = path.join(HERE, "../src/SUMMARY.md");
const OUT = path.join(HERE, "../src/generated/sidebar.json");
const BOOK = path.join(HERE, "../src/content/docs/book");

/** Sidebar labels are plain text, so drop the inline-code backticks the Book's
 *  SUMMARY uses (e.g. "`bynk.toml` manifest" → "bynk.toml manifest"). */
function clean(label) {
  return label.replace(/`/g, "");
}

/** A Book page path (relative to the Book root) → its Starlight route under /book/. */
function slugFor(pagePath) {
  let s = pagePath.replace(/\.md$/, "").replace(/(?:^|\/)index$/, "");
  return s === "" ? "/book/" : `/book/${s}/`;
}

/**
 * Parse SUMMARY.md into { prefix, parts }.
 * - `# Part` headers (after the `# Summary` title) open a part.
 * - a bare `[Title](path.md)` before any part is the prefix chapter (Book root).
 * - `- [Title](path.md)` items nest by indentation (2 spaces per level).
 * - `- Label` (no link) is a group with no page of its own: the items nested
 *   beneath it are its contents (web presence track, slice 2).
 * - `---` separators are ignored.
 */
function parseSummary(text) {
  const lines = text.split("\n");
  let prefix = null;
  const parts = [];
  let current = null;

  for (const line of lines) {
    if (/^#\s+Summary\s*$/.test(line)) continue;
    const part = /^#\s+(.+?)\s*$/.exec(line);
    if (part) {
      current = { label: part[1], entries: [] };
      parts.push(current);
      continue;
    }
    const item = /^(\s*)-\s+\[(.+?)\]\((.+?)\)\s*$/.exec(line);
    if (item) {
      current?.entries.push({ depth: item[1].length / 2, label: clean(item[2]), path: item[3] });
      continue;
    }
    const group = /^(\s*)-\s+([^\[\s].*?)\s*$/.exec(line);
    if (group) {
      current?.entries.push({ depth: group[1].length / 2, label: clean(group[2]), path: null });
      continue;
    }
    const bare = /^\[(.+?)\]\((.+?)\)\s*$/.exec(line);
    if (bare && !current) prefix = { label: clean(bare[1]), path: bare[2] };
  }
  return { prefix, parts };
}

/** Build a nesting tree from a part's flat, indentation-tagged entries. */
function buildTree(entries) {
  const root = { children: [] };
  const stack = [{ depth: -1, node: root }];
  for (const entry of entries) {
    while (stack[stack.length - 1].depth >= entry.depth) stack.pop();
    const node = { label: entry.label, path: entry.path, children: [] };
    stack[stack.length - 1].node.children.push(node);
    stack.push({ depth: entry.depth, node });
  }
  return root.children;
}

/** A tree node → a Starlight sidebar entry. A node with children becomes a group
 *  whose own page is surfaced as a leading "Overview" link (stock Starlight groups
 *  carry no link of their own). A label with no page is a plain group. Groups
 *  inside a part start collapsed, so a part's groups can be scanned at a glance;
 *  Starlight opens the one holding the current page. */
function toEntry(node) {
  if (node.path === null) {
    if (node.children.length === 0)
      throw new Error(`site/src/SUMMARY.md: the group "${node.label}" has no pages beneath it.`);
    return { label: node.label, collapsed: true, items: node.children.map(toEntry) };
  }
  if (node.children.length === 0) return { label: node.label, link: slugFor(node.path) };
  return {
    label: node.label,
    collapsed: true,
    items: [{ label: "Overview", link: slugFor(node.path) }, ...node.children.map(toEntry)],
  };
}

/** A part's children → its group items, collapsing the common case where a part
 *  wraps a single index page (e.g. `# Guides` → `[Guides](guides/index.md)` + its
 *  subsections) so the sidebar reads "Guides › …" rather than "Guides › Guides". */
function partItems(children) {
  if (children.length === 1 && children[0].path !== null && children[0].children.length > 0) {
    const only = children[0];
    return [{ label: "Overview", link: slugFor(only.path) }, ...only.children.map(toEntry)];
  }
  return children.map(toEntry);
}

function buildSidebar(text) {
  const { prefix, parts } = parseSummary(text);
  const sidebar = [];
  if (prefix) sidebar.push({ label: prefix.label, link: "/book/" });
  for (const part of parts) {
    sidebar.push({ label: part.label, items: partItems(buildTree(part.entries)) });
  }
  return sidebar;
}

/** Every page under the Book's content directory, as paths relative to it. */
function bookPages(dir = BOOK, rel = "") {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const sub = rel ? `${rel}/${entry.name}` : entry.name;
    if (entry.isDirectory()) return bookPages(path.join(dir, entry.name), sub);
    return /\.mdx?$/.test(entry.name) ? [sub] : [];
  });
}

/** A page's route, whatever its extension: SUMMARY names `01-first-program.md`
 *  for a page written as `.mdx`, and both are the same route. */
const routeOf = (pagePath) => slugFor(pagePath.replace(/\.mdx$/, ".md"));

/** Values that occur more than once, each once, in first-seen order. */
function repeated(values) {
  const seen = new Set();
  const twice = new Set();
  for (const v of values) (seen.has(v) ? twice : seen).add(v);
  return [...twice];
}

/**
 * Compare SUMMARY.md with the pages on disk (web presence track, slices 1–2).
 * Returns the pages no contents lists (`unlisted`), the SUMMARY entries with
 * no page (`missing`), the pages SUMMARY lists more than once (`listedTwice`),
 * and the routes two files on disk both claim (`sharedRoutes`, e.g. `foo.md`
 * beside `foo/index.md`). The prefix chapter is the Book root, `index.md`,
 * whatever file name SUMMARY gives it.
 */
function completeness(text, pages = bookPages()) {
  const { prefix, parts } = parseSummary(text);
  const entries = parts.flatMap((p) => p.entries.map((e) => e.path)).filter((p) => p !== null);
  const listed = new Set(entries.map(routeOf));
  if (prefix) listed.add("/book/");
  const onDisk = new Map(pages.map((p) => [routeOf(p), p]));
  return {
    unlisted: [...onDisk].filter(([route]) => !listed.has(route)).map(([, p]) => p).sort(),
    missing: entries.filter((e) => !onDisk.has(routeOf(e))),
    listedTwice: repeated(entries.map(routeOf)),
    sharedRoutes: repeated(pages.map(routeOf)).map((route) =>
      pages.filter((p) => routeOf(p) === route),
    ),
  };
}

function reportCompleteness(text) {
  const { unlisted, missing, listedTwice, sharedRoutes } = completeness(text);
  const problems = [];
  if (unlisted.length)
    problems.push(
      "These Book pages are in no contents, so no sidebar shows them. List each in\n" +
        "site/src/SUMMARY.md:\n" +
        unlisted.map((p) => `  site/src/content/docs/book/${p}`).join("\n"),
    );
  if (missing.length)
    problems.push(
      "These site/src/SUMMARY.md entries name no page:\n" + missing.map((p) => `  ${p}`).join("\n"),
    );
  if (listedTwice.length)
    problems.push(
      "These pages are listed more than once in site/src/SUMMARY.md; list each once:\n" +
        listedTwice.map((r) => `  ${r}`).join("\n"),
    );
  if (sharedRoutes.length)
    problems.push(
      "These Book files would serve the same page; keep one of each pair:\n" +
        sharedRoutes.map((files) => `  ${files.join(" and ")}`).join("\n"),
    );
  return problems;
}

function main(argv) {
  const check = argv.includes("--check");
  const text = fs.readFileSync(SUMMARY, "utf8");
  const sidebar = buildSidebar(text);
  const rendered = JSON.stringify(sidebar, null, 2) + "\n";
  const problems = reportCompleteness(text);
  if (check) {
    const current = fs.existsSync(OUT) ? fs.readFileSync(OUT, "utf8") : "";
    if (current !== rendered) {
      problems.unshift(
        "site/src/generated/sidebar.json is out of date with site/src/SUMMARY.md.\n" +
          "Regenerate with: node site/scripts/generate-sidebar.mjs",
      );
    }
    if (problems.length) {
      console.error(problems.join("\n\n"));
      process.exit(1);
    }
    console.log("sidebar.json is up to date, and every Book page is in SUMMARY.md once.");
    return;
  }
  if (problems.length) console.warn(problems.join("\n\n") + "\n");
  fs.mkdirSync(path.dirname(OUT), { recursive: true });
  fs.writeFileSync(OUT, rendered);
  console.log(`wrote sidebar.json (${sidebar.length} top-level entries).`);
}

main(process.argv.slice(2));
