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
//!
//! The release pages (slice 3) are the one exception to listing by hand: a
//! `- {release pages}` line lists every page in its parent entry's folder,
//! Unreleased first, then each release newest first, so cutting a release
//! needs no edit here.
//!
//!   node --test scripts/generate-sidebar.test.mjs    the generator's own tests

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

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
export function slugFor(pagePath) {
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
 * - `- {release pages}` stands for every page in its parent entry's folder
 *   (slice 3); the parent's own page is not repeated.
 * - `---` separators are ignored.
 * A line that starts like an item but isn't one (`- [Title] (path.md)`) is
 * reported in `malformed` rather than dropped.
 */
export function parseSummary(text) {
  const lines = text.split("\n");
  let prefix = null;
  const parts = [];
  const malformed = [];
  let current = null;

  for (const [i, line] of lines.entries()) {
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
    const expand = /^(\s*)-\s+\{release pages\}\s*$/.exec(line);
    if (expand) {
      const depth = expand[1].length / 2;
      const parent = [...(current?.entries ?? [])].reverse().find((e) => e.depth < depth);
      if (!parent?.path) {
        malformed.push({ line: i + 1, text: line.trim(), why: "needs a parent entry with a page" });
        continue;
      }
      current.entries.push({ depth, label: null, path: null, expand: path.posix.dirname(parent.path) });
      continue;
    }
    if (/^\s*-\s+\[/.test(line)) {
      malformed.push({ line: i + 1, text: line.trim(), why: "is not `- [Title](path.md)`" });
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
  return { prefix, parts, malformed };
}

/** A release page's version, `v0-639-1.md` → [0, 639, 1]; Unreleased sorts
 *  first. (Release files use dashes: Astro drops dots from a file name's URL.) */
function releaseKey(file) {
  const m = /^v(\d+)-(\d+)-(\d+)\.md$/.exec(path.posix.basename(file));
  return m ? m.slice(1).map(Number) : [Infinity, 0, 0];
}

/** The pages a `{release pages}` line stands for: every page in `dir` but its
 *  index, Unreleased first, then each release newest first. */
export function releasePages(dir, pages) {
  return pages
    .filter((p) => path.posix.dirname(p) === dir && path.posix.basename(p) !== "index.md")
    .sort((a, b) => {
      const [x, y] = [releaseKey(a), releaseKey(b)];
      return y[0] - x[0] || y[1] - x[1] || y[2] - x[2] || a.localeCompare(b);
    });
}

const releaseLabel = (file) =>
  path.posix.basename(file) === "unreleased.md"
    ? "Unreleased"
    : path.posix.basename(file, ".md").replaceAll("-", ".");

/** Build a nesting tree from a part's flat, indentation-tagged entries. */
function buildTree(entries) {
  const root = { children: [] };
  const stack = [{ depth: -1, node: root }];
  for (const entry of entries) {
    while (stack[stack.length - 1].depth >= entry.depth) stack.pop();
    const node = { label: entry.label, path: entry.path, expand: entry.expand, children: [] };
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
function toEntry(node, pages) {
  const items = node.children.flatMap((child) =>
    child.expand
      ? releasePages(child.expand, pages).map((p) => ({ label: releaseLabel(p), link: slugFor(p) }))
      : [toEntry(child, pages)],
  );
  if (node.path === null) {
    if (items.length === 0)
      throw new Error(`site/src/SUMMARY.md: the group "${node.label}" has no pages beneath it.`);
    return { label: node.label, collapsed: true, items };
  }
  if (items.length === 0) return { label: node.label, link: slugFor(node.path) };
  return { label: node.label, collapsed: true, items: [{ label: "Overview", link: slugFor(node.path) }, ...items] };
}

/** A part's children → its group items, collapsing the common case where a part
 *  wraps a single index page (e.g. `# Guides` → `[Guides](guides/index.md)` + its
 *  subsections) so the sidebar reads "Guides › …" rather than "Guides › Guides". */
function partItems(children, pages) {
  if (children.length === 1 && children[0].path !== null && children[0].children.length > 0) {
    const entry = toEntry(children[0], pages);
    // A single page whose children all came from an empty `{release pages}`
    // folder is a plain link: keep it rather than lose the part's contents.
    return entry.items ?? [entry];
  }
  return children.map((child) => toEntry(child, pages));
}

export function buildSidebar(text, pages = bookPages()) {
  const { prefix, parts } = parseSummary(text);
  const sidebar = [];
  if (prefix) sidebar.push({ label: prefix.label, link: "/book/" });
  for (const part of parts) {
    sidebar.push({ label: part.label, items: partItems(buildTree(part.entries), pages) });
  }
  return sidebar;
}

/** Every page under the Book's content directory, as paths relative to it. */
export function bookPages(dir = BOOK, rel = "") {
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
 * beside `foo/index.md`), and any malformed contents line. The prefix chapter
 * is the Book root, `index.md`, whatever file name SUMMARY gives it, so listing
 * `index.md` again under a part counts as listing it twice. A `{release pages}`
 * line lists the pages it stands for.
 */
export function completeness(text, pages = bookPages()) {
  const { prefix, parts, malformed } = parseSummary(text);
  const entries = parts.flatMap((p) =>
    p.entries.flatMap((e) => (e.expand ? releasePages(e.expand, pages) : e.path ? [e.path] : [])),
  );
  const routes = [...(prefix ? ["/book/"] : []), ...entries.map(routeOf)];
  const listed = new Set(routes);
  const onDisk = new Map(pages.map((p) => [routeOf(p), p]));
  return {
    unlisted: [...onDisk].filter(([route]) => !listed.has(route)).map(([, p]) => p).sort(),
    missing: entries.filter((e) => !onDisk.has(routeOf(e))),
    listedTwice: repeated(routes),
    sharedRoutes: repeated(pages.map(routeOf)).map((route) =>
      pages.filter((p) => routeOf(p) === route),
    ),
    malformed,
  };
}

export function reportCompleteness(text, pages = bookPages()) {
  const { unlisted, missing, listedTwice, sharedRoutes, malformed } = completeness(text, pages);
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
  if (malformed.length)
    problems.push(
      "These site/src/SUMMARY.md lines aren't contents entries:\n" +
        malformed.map((m) => `  line ${m.line}: ${m.text} (${m.why})`).join("\n"),
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

// Run as a script, not when the tests import it.
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2));
}
