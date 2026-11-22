// @ts-check
//! Generate (or verify) the Starlight sidebars from their contents files
//! (documentation track). Each tree has an authored table of contents:
//! `site/src/SUMMARY.md` for the Book (`/book/`) and, since web presence slice 5,
//! `site/src/REFERENCE.md` for the Reference (`/reference/`). This turns each into
//! a committed sidebar under `site/src/generated/` that `astro.config.mjs`
//! imports, so a sidebar tracks its contents' order and grouping rather than
//! directory structure.
//!
//!   node scripts/generate-sidebar.mjs            regenerate sidebar.json
//!   node scripts/generate-sidebar.mjs --check     fail (non-zero) if out of date,
//!                                                 or if the contents are incomplete
//!
//! The `site` CI job runs `--check`; a `SUMMARY.md` edit that isn't regenerated
//! fails the build. Mirrors the repo's generate→commit→drift-guard pattern.
//!
//! The contents must also be complete (web presence track, slice 1): every page
//! under a tree's folder (`src/content/docs/book/`, `src/content/docs/reference/`)
//! is listed in that tree's contents file, and every entry names a page. A
//! sidebar comes from its contents file, not the folders, so a page nobody
//! lists exists but no sidebar shows it. `--check` fails naming each one;
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
const BOOK = path.join(HERE, "../src/content/docs/book");

/** The trees, each with its contents file, folder, route base and sidebar. */
export const TREES = [
  {
    name: "Book",
    contents: "site/src/SUMMARY.md",
    dir: BOOK,
    base: "/book/",
    out: path.join(HERE, "../src/generated/sidebar.json"),
  },
  {
    name: "Reference",
    contents: "site/src/REFERENCE.md",
    dir: path.join(HERE, "../src/content/docs/reference"),
    base: "/reference/",
    out: path.join(HERE, "../src/generated/reference-sidebar.json"),
  },
];
const BOOK_TREE = TREES[0];

/** Sidebar labels are plain text, so drop the inline-code backticks the Book's
 *  SUMMARY uses (e.g. "`bynk.toml` manifest" → "bynk.toml manifest"). */
function clean(label) {
  return label.replace(/`/g, "");
}

/** A page path (relative to its tree's root) → its Starlight route under `base`. */
export function slugFor(pagePath, base = "/book/") {
  let s = pagePath.replace(/\.md$/, "").replace(/(?:^|\/)index$/, "");
  return s === "" ? base : `${base}${s}/`;
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
function toEntry(node, pages, tree) {
  const items = node.children.flatMap((child) =>
    child.expand
      ? releasePages(child.expand, pages).map((p) => ({ label: releaseLabel(p), link: slugFor(p, tree.base) }))
      : [toEntry(child, pages, tree)],
  );
  if (node.path === null) {
    if (items.length === 0)
      throw new Error(`${tree.contents}: the group "${node.label}" has no pages beneath it.`);
    return { label: node.label, collapsed: true, items };
  }
  const link = slugFor(node.path.replace(/\.mdx$/, ".md"), tree.base);
  if (items.length === 0) return { label: node.label, link };
  return { label: node.label, collapsed: true, items: [{ label: "Overview", link }, ...items] };
}

/** A part's children → its group items, collapsing the common case where a part
 *  wraps a single index page (e.g. `# Guides` → `[Guides](guides/index.md)` + its
 *  subsections) so the sidebar reads "Guides › …" rather than "Guides › Guides". */
function partItems(children, pages, tree) {
  if (children.length === 1 && children[0].path !== null && children[0].children.length > 0) {
    const entry = toEntry(children[0], pages, tree);
    // A single page whose children all came from an empty `{release pages}`
    // folder is a plain link: keep it rather than lose the part's contents.
    return entry.items ?? [entry];
  }
  return children.map((child) => toEntry(child, pages, tree));
}

export function buildSidebar(text, pages = bookPages(), tree = BOOK_TREE) {
  const { prefix, parts } = parseSummary(text);
  const sidebar = [];
  if (prefix) sidebar.push({ label: prefix.label, link: tree.base });
  for (const part of parts) {
    sidebar.push({ label: part.label, items: partItems(buildTree(part.entries), pages, tree) });
  }
  return sidebar;
}

/** Every page under a tree's content directory (the Book's by default), as
 *  paths relative to it. */
export function bookPages(dir = BOOK, rel = "") {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const sub = rel ? `${rel}/${entry.name}` : entry.name;
    if (entry.isDirectory()) return bookPages(path.join(dir, entry.name), sub);
    return /\.mdx?$/.test(entry.name) ? [sub] : [];
  });
}

/** A page's route, whatever its extension: SUMMARY names `01-first-program.md`
 *  for a page written as `.mdx`, and both are the same route. */
const routeOf = (pagePath, base = "/book/") => slugFor(pagePath.replace(/\.mdx$/, ".md"), base);

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
export function completeness(text, pages = bookPages(), base = "/book/") {
  const { prefix, parts, malformed } = parseSummary(text);
  const entries = parts.flatMap((p) =>
    p.entries.flatMap((e) => (e.expand ? releasePages(e.expand, pages) : e.path ? [e.path] : [])),
  );
  const route = (p) => routeOf(p, base);
  const routes = [...(prefix ? [base] : []), ...entries.map(route)];
  const listed = new Set(routes);
  const onDisk = new Map(pages.map((p) => [route(p), p]));
  return {
    unlisted: [...onDisk].filter(([r]) => !listed.has(r)).map(([, p]) => p).sort(),
    missing: entries.filter((e) => !onDisk.has(route(e))),
    listedTwice: repeated(routes),
    sharedRoutes: repeated(pages.map(route)).map((r) => pages.filter((p) => route(p) === r)),
    malformed,
  };
}

/** The folder a tree's pages live in, from the repository root. */
const folderOf = (tree) => path.relative(path.join(HERE, "../.."), tree.dir);

export function reportCompleteness(text, pages = bookPages(), tree = BOOK_TREE) {
  const { unlisted, missing, listedTwice, sharedRoutes, malformed } = completeness(text, pages, tree.base);
  const problems = [];
  if (unlisted.length)
    problems.push(
      `These ${tree.name} pages are in no contents, so no sidebar shows them. List each in\n` +
        `${tree.contents}:\n` +
        unlisted.map((p) => `  ${folderOf(tree)}/${p}`).join("\n"),
    );
  if (missing.length)
    problems.push(`These ${tree.contents} entries name no page:\n` + missing.map((p) => `  ${p}`).join("\n"));
  if (listedTwice.length)
    problems.push(
      `These pages are listed more than once in ${tree.contents}; list each once:\n` +
        listedTwice.map((r) => `  ${r}`).join("\n"),
    );
  if (sharedRoutes.length)
    problems.push(
      `These ${tree.name} files would serve the same page; keep one of each pair:\n` +
        sharedRoutes.map((files) => `  ${files.join(" and ")}`).join("\n"),
    );
  if (malformed.length)
    problems.push(
      `These ${tree.contents} lines aren't contents entries:\n` +
        malformed.map((m) => `  line ${m.line}: ${m.text} (${m.why})`).join("\n"),
    );
  return problems;
}

function main(argv) {
  const check = argv.includes("--check");
  const problems = [];
  for (const tree of TREES) {
    const text = fs.readFileSync(path.join(HERE, "../..", tree.contents), "utf8");
    const pages = bookPages(tree.dir);
    const sidebar = buildSidebar(text, pages, tree);
    const rendered = JSON.stringify(sidebar, null, 2) + "\n";
    const out = path.relative(path.join(HERE, "../.."), tree.out);
    problems.push(...reportCompleteness(text, pages, tree));
    if (check) {
      const current = fs.existsSync(tree.out) ? fs.readFileSync(tree.out, "utf8") : "";
      if (current !== rendered)
        problems.unshift(
          `${out} is out of date with ${tree.contents}.\n` +
            "Regenerate with: node site/scripts/generate-sidebar.mjs",
        );
      continue;
    }
    fs.mkdirSync(path.dirname(tree.out), { recursive: true });
    fs.writeFileSync(tree.out, rendered);
    console.log(`wrote ${out} (${sidebar.length} top-level entries).`);
  }
  if (check) {
    if (problems.length) {
      console.error(problems.join("\n\n"));
      process.exit(1);
    }
    console.log("The sidebars are up to date, and every page is in its tree's contents once.");
    return;
  }
  if (problems.length) console.warn(problems.join("\n\n") + "\n");
}

// Run as a script, not when the tests import it.
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2));
}
