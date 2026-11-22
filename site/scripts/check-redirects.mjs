// @ts-check
//! Check `public/_redirects` (the Cloudflare Pages redirect rules for
//! bynk-lang.org) against the built site in `dist/` (web presence track, link
//! rule L2). Each move of a docs folder adds rules here, and until now each PR
//! checked them by hand; this simulates the rules the way Cloudflare applies
//! them (first match wins) and asserts that
//!
//!   1. no rule's left-hand path is a page that still exists: a redirect that
//!      shadows a live page hides it, and one for a page that came back is stale;
//!   2. no rule is unreachable, because an earlier rule matches every path it would;
//!   3. every rule's target is a page in `dist/`. For a `/*` splat, every page
//!      under the target folder is mapped back to its old path (with and without
//!      its trailing slash), and that old path must land on a page; for a
//!      `:code` placeholder, a few codes from `src/generated/diagnostics.json`
//!      are tried.
//!
//! `/reference/api/` (the rustdoc, built only in the deploy workflow) is exempt
//! as a target.
//!
//! Those checks only know the pages that exist now. `src/data/published-routes.txt`
//! remembers every page the site has published (#4010); it only grows. So that
//! a move cannot quietly drop a page, it also asserts that
//!
//!   4. every published route is still a page, or is redirected (with and
//!      without its trailing slash) to one;
//!   5. every built page is in the list, so the list stays complete.
//!
//!   node scripts/check-redirects.mjs           report problems; non-zero if any
//!   node scripts/check-redirects.mjs --update  add the built pages to the list
//!   node scripts/check-redirects.mjs --dist <dir> --redirects <file> --published <file>
//!
//! The `site` CI job runs this after `astro build`; the matcher's tests are in
//! check-redirects.test.mjs.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));

/** Targets under these prefixes are not in `dist/` in CI, only in deploys. */
export const EXEMPT_TARGETS = ["/reference/api/"];

/**
 * Left-hand paths allowed to shadow a live page. Empty: add one only with a
 * reason, when a live page genuinely must be redirected away.
 * @type {Set<string>}
 */
export const ALLOW_SHADOW = new Set();

/** How many problems to print for one rule before summarising the rest. */
const PER_RULE_LIMIT = 5;

/**
 * @typedef {{ from: string, to: string, code: number, line: number }} Rule
 */

/**
 * Parse a `_redirects` file: one `from to [code]` rule per line; lines that
 * start with `#` and blank lines are ignored. Cloudflare has no trailing
 * comments, so a `#` after a rule makes too many fields and the line is
 * malformed (Cloudflare would drop it). Malformed lines are returned, not thrown.
 * @param {string} text
 * @returns {{ rules: Rule[], malformed: { line: number, text: string, reason: string }[] }}
 */
export function parseRedirects(text) {
  /** @type {Rule[]} */
  const rules = [];
  const malformed = [];
  text.split("\n").forEach((raw, i) => {
    const line = i + 1;
    const body = raw.trim();
    if (body === "" || body.startsWith("#")) return;
    const fields = body.split(/\s+/);
    if (fields.length < 2 || fields.length > 3) {
      malformed.push({ line, text: raw, reason: "expected `from to [code]`" });
      return;
    }
    const [from, to, codeText = "302"] = fields;
    const code = Number(codeText);
    if (![200, 301, 302, 303, 307, 308].includes(code)) {
      malformed.push({ line, text: raw, reason: `unknown status ${codeText}` });
      return;
    }
    if (!from.startsWith("/")) {
      malformed.push({ line, text: raw, reason: "the left-hand path must start with /" });
      return;
    }
    const stars = from.split("*").length - 1;
    if (stars > 1 || (stars === 1 && !from.endsWith("*"))) {
      malformed.push({ line, text: raw, reason: "a splat must be a single trailing *" });
      return;
    }
    rules.push({ from, to, code, line });
  });
  return { rules, malformed };
}

const escapeRe = (/** @type {string} */ s) => s.replace(/[.+?^${}()|[\]\\]/g, "\\$&");

/**
 * Compile a rule's left-hand path to a regex. `*` (trailing only) captures the
 * rest of the path as `splat`; a `:name` segment captures one path segment.
 * Everything else matches literally, trailing slash included, as Cloudflare does.
 * @param {string} from
 */
export function compilePattern(from) {
  let src = "";
  const re = /:([A-Za-z_][A-Za-z0-9_]*)|\*/g;
  let last = 0;
  for (const m of from.matchAll(re)) {
    src += escapeRe(from.slice(last, m.index));
    src += m[0] === "*" ? "(?<splat>.*)" : `(?<${m[1]}>[^/]+)`;
    last = /** @type {number} */ (m.index) + m[0].length;
  }
  src += escapeRe(from.slice(last));
  return new RegExp(`^${src}$`);
}

/**
 * The placeholder names (other than the splat) a left-hand path uses.
 * @param {string} from
 */
export function placeholders(from) {
  return [...from.matchAll(/:([A-Za-z_][A-Za-z0-9_]*)/g)].map((m) => m[1]);
}

/**
 * Apply the rules to a request path, first match wins.
 * @param {Rule[]} rules
 * @param {string} pathname
 * @returns {{ rule: Rule, index: number, target: string } | null}
 */
export function applyRedirects(rules, pathname) {
  for (let index = 0; index < rules.length; index++) {
    const rule = rules[index];
    const m = compilePattern(rule.from).exec(pathname);
    if (!m) continue;
    const groups = m.groups ?? {};
    const target = rule.to.replace(/:([A-Za-z_][A-Za-z0-9_]*)/g, (whole, name) =>
      name in groups ? groups[name] : whole,
    );
    return { rule, index, target };
  }
  return null;
}

/**
 * The routes a built site serves: `a/b/index.html` → `/a/b/`, `a/c.html` →
 * `/a/c`, any other file at its own path.
 * @param {string} dist
 * @returns {Set<string>}
 */
export function distRoutes(dist) {
  const routes = new Set();
  /** @param {string} dir @param {string} rel */
  const walk = (dir, rel) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const relPath = `${rel}/${entry.name}`;
      if (entry.isDirectory()) walk(path.join(dir, entry.name), relPath);
      else if (entry.name === "index.html") routes.add(`${rel}/`);
      else if (entry.name.endsWith(".html")) routes.add(relPath.slice(0, -".html".length));
      else routes.add(relPath);
    }
  };
  walk(dist, "");
  return routes;
}

/**
 * Does a redirect target land on something the site serves? A target without
 * its trailing slash counts when the slashed page exists: Cloudflare's own
 * trailing-slash redirect takes it the rest of the way.
 * @param {Set<string>} routes
 * @param {string} target
 */
export function resolves(routes, target) {
  if (/^https?:\/\//.test(target)) return true;
  const pathname = target.replace(/[?#].*$/, "");
  if (EXEMPT_TARGETS.some((p) => pathname === p.slice(0, -1) || pathname.startsWith(p))) return true;
  if (routes.has(pathname)) return true;
  return !pathname.endsWith("/") && routes.has(`${pathname}/`);
}

/**
 * A request path that a rule's left-hand side matches, built by filling its
 * splat and placeholders with sample values.
 * @param {string} from
 * @param {Record<string, string>} values
 * @param {string} [splat]
 */
export function fillPattern(from, values, splat = "") {
  return from
    .replace(/:([A-Za-z_][A-Za-z0-9_]*)/g, (whole, name) => values[name] ?? whole)
    .replace(/\*$/, splat);
}

/**
 * The routes that are pages: every page Astro builds is a folder with an
 * `index.html` (`/a/`). Assets (`/llms.txt`, Pagefind's fragments) and the
 * `404` page are not.
 * @param {Iterable<string>} routes
 */
export function pagesOf(routes) {
  return [...routes].filter((r) => r.endsWith("/"));
}

/**
 * Parse the published-routes list: one route per line; `#` comment lines and
 * blank lines are ignored.
 * @param {string} text
 */
export function parsePublished(text) {
  return text
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l !== "" && !l.startsWith("#"));
}

export const PUBLISHED_HEADER = `# Every page bynk-lang.org has published (web presence track, link rule L2;
# #4010). site/scripts/check-redirects.mjs fails CI if a route here is no
# longer a page and _redirects does not send it to one, and if a built page is
# missing here. The list only grows: after adding a page, run
#
#   npm run build && node scripts/check-redirects.mjs --update
#
# and commit the result. Remove a line only to retire a URL on purpose, with
# nothing to send its readers to.
`;

/**
 * Write the list: the routes, sorted, under the header.
 * @param {Iterable<string>} routes
 */
export function formatPublished(routes) {
  return `${PUBLISHED_HEADER}\n${[...new Set(routes)].sort().join("\n")}\n`;
}

/**
 * Check the published routes against the build and the rules: each must still
 * be a page or redirect to one, and each built page must be listed.
 * @param {Rule[]} rules
 * @param {Set<string>} routes
 * @param {string[]} published
 * @returns {{ problem: string }[]}
 */
export function checkPublished(rules, routes, published) {
  /** @type {{ problem: string }[]} */
  const problems = [];
  const listed = new Set(published);
  for (const route of published) {
    if (routes.has(route)) continue;
    const forms = route.endsWith("/") && route !== "/" ? [route, route.slice(0, -1)] : [route];
    for (const form of forms) {
      const hit = applyRedirects(rules, form);
      if (hit === null) {
        problems.push({ problem: `${form} was published, is no longer a page, and no rule redirects it` });
      } else if (!resolves(routes, hit.target)) {
        problems.push({
          problem: `${form} was published, is no longer a page, and the rule on line ${hit.rule.line} (${hit.rule.from}) sends it to ${hit.target}, which is not a page`,
        });
      }
    }
  }
  const unlisted = pagesOf(routes).filter((r) => !listed.has(r)).sort();
  if (unlisted.length > 0) {
    const shown = unlisted.slice(0, PER_RULE_LIMIT).join(", ");
    const more = unlisted.length > PER_RULE_LIMIT ? ` and ${unlisted.length - PER_RULE_LIMIT} more` : "";
    problems.push({
      problem: `${unlisted.length} built page(s) missing from the published list (${shown}${more}); run \`node scripts/check-redirects.mjs --update\` and commit it`,
    });
  }
  return problems;
}

/**
 * Check the rules against the routes a built site serves.
 * @param {Rule[]} rules
 * @param {Set<string>} routes
 * @param {{ samples?: Record<string, string[]>, allowShadow?: Set<string> }} [options]
 *   `samples`: values to try for each `:name` placeholder.
 * @returns {{ line: number, rule: string, problem: string }[]}
 */
export function checkRedirects(rules, routes, options = {}) {
  const samples = options.samples ?? {};
  const allowShadow = options.allowShadow ?? ALLOW_SHADOW;
  /** @type {{ line: number, rule: string, problem: string }[]} */
  const problems = [];
  const show = (/** @type {Rule} */ r) => `${r.from} ${r.to} ${r.code}`;
  const report = (/** @type {Rule} */ r, /** @type {string} */ problem) =>
    problems.push({ line: r.line, rule: show(r), problem });

  // 1. A rule must not shadow a live page. A request for `/x` is redirected
  //    before Cloudflare's own `/x` → `/x/`, so a rule for the slashless form
  //    hides the page too.
  const pages = pagesOf(routes);
  /** @type {Map<Rule, number>} */
  const shadowed = new Map();
  for (const page of pages) {
    const forms = page.endsWith("/") && page !== "/" ? [page, page.slice(0, -1)] : [page];
    for (const form of forms) {
      const hit = applyRedirects(rules, form);
      if (hit && !allowShadow.has(hit.rule.from)) {
        const n = (shadowed.get(hit.rule) ?? 0) + 1;
        shadowed.set(hit.rule, n);
        if (n <= PER_RULE_LIMIT) report(hit.rule, `shadows the live page ${page} (a request for ${form} is redirected)`);
      }
    }
  }
  for (const [rule, n] of shadowed) {
    if (n > PER_RULE_LIMIT) report(rule, `… and ${n - PER_RULE_LIMIT} more live paths it shadows`);
  }

  rules.forEach((rule, index) => {
    const names = placeholders(rule.from);
    const missing = names.filter((n) => !(samples[n]?.length));
    if (missing.length > 0) {
      report(rule, `no sample values to test :${missing.join(", :")}`);
      return;
    }
    const valueSets = names.length === 0
      ? [{}]
      : samples[names[0]].map((_, i) => Object.fromEntries(names.map((n) => [n, samples[n][i % samples[n].length]])));

    // 2. Reachable: a path this rule matches must reach this rule first.
    const probe = fillPattern(rule.from, valueSets[0], rule.from.endsWith("*") ? "sample/" : "");
    const first = applyRedirects(rules, probe);
    if (first && first.index !== index) {
      report(rule, `never applies: the earlier rule on line ${first.rule.line} (${first.rule.from}) matches ${probe} first`);
      return;
    }

    // 3. Targets resolve.
    /** @type {string[]} */
    const tried = [];
    if (rule.from.endsWith("*") && rule.to.includes(":splat")) {
      // Every page under the target folder, mapped back to its old path.
      const toPrefix = rule.to.slice(0, rule.to.indexOf(":splat"));
      const fromPrefix = rule.from.slice(0, -1);
      for (const values of valueSets) {
        const toFilled = fillPattern(toPrefix, values);
        const fromFilled = fillPattern(fromPrefix, values);
        for (const page of pages) {
          // `/new` as a prefix must not pick up `/newer/…`.
          if (!page.startsWith(toFilled)) continue;
          if (!toFilled.endsWith("/") && page !== toFilled && page[toFilled.length] !== "/") continue;
          const old = fromFilled + page.slice(toFilled.length);
          tried.push(old);
          if (old.endsWith("/") && old.length > 1) tried.push(old.slice(0, -1));
        }
        // The folder itself, and a page that only a deploy has (e.g. the rustdoc).
        tried.push(fromFilled, fromFilled.replace(/\/$/, ""));
      }
    } else {
      for (const values of valueSets) {
        tried.push(fillPattern(rule.from, values, rule.from.endsWith("*") ? "sample/" : ""));
      }
    }

    let failures = 0;
    for (const old of new Set(tried)) {
      if (old === "") continue;
      // Whichever rule catches the old path (an earlier exact rule, or a wider
      // splat after this one), the reader must land on a page.
      const hit = applyRedirects(rules, old);
      let problem = null;
      if (hit === null) {
        if (!resolves(routes, old)) problem = `${old} matches no rule and is not a page`;
      } else if (!resolves(routes, hit.target)) {
        const via = hit.index === index ? "" : ` (by the rule on line ${hit.rule.line}, ${hit.rule.from})`;
        problem = `${old} → ${hit.target}${via}, which is not a page`;
      }
      if (problem) {
        failures++;
        if (failures <= PER_RULE_LIMIT) report(rule, problem);
      }
    }
    if (failures > PER_RULE_LIMIT) {
      report(rule, `… and ${failures - PER_RULE_LIMIT} more paths that land on no page`);
    }
  });

  return problems.sort((a, b) => a.line - b.line);
}

/**
 * A few diagnostic codes to try for `:code`: the first, one from the middle and
 * the last of the registry.
 * @param {string} file
 */
export function sampleCodes(file) {
  /** @type {{ code: string }[]} */
  const entries = JSON.parse(fs.readFileSync(file, "utf8"));
  const codes = entries.map((e) => e.code);
  return [...new Set([codes[0], codes[Math.floor(codes.length / 2)], codes[codes.length - 1]])];
}

function main() {
  const args = process.argv.slice(2);
  const opt = (/** @type {string} */ name, /** @type {string} */ fallback) => {
    const i = args.indexOf(name);
    return i >= 0 && args[i + 1] ? path.resolve(args[i + 1]) : fallback;
  };
  const dist = opt("--dist", path.join(HERE, "../dist"));
  const redirectsFile = opt("--redirects", path.join(HERE, "../public/_redirects"));
  const publishedFile = opt("--published", path.join(HERE, "../src/data/published-routes.txt"));
  const diagnostics = path.join(HERE, "../src/generated/diagnostics.json");

  if (!fs.existsSync(dist)) {
    console.error(`check-redirects: no built site at ${dist}; run \`npm run build\` first.`);
    process.exit(2);
  }
  const { rules, malformed } = parseRedirects(fs.readFileSync(redirectsFile, "utf8"));
  const routes = distRoutes(dist);
  const published = fs.existsSync(publishedFile) ? parsePublished(fs.readFileSync(publishedFile, "utf8")) : [];

  if (args.includes("--update")) {
    const before = new Set(published);
    const next = [...published, ...pagesOf(routes)];
    fs.writeFileSync(publishedFile, formatPublished(next));
    const added = new Set(next).size - before.size;
    console.log(`published-routes.txt: ${added} route(s) added, ${new Set(next).size} in all.`);
    return;
  }

  /** @type {{ line?: number, rule?: string, problem: string }[]} */
  const problems = [
    ...malformed.map((m) => ({ line: m.line, rule: m.text.trim(), problem: m.reason })),
    ...checkRedirects(rules, routes, { samples: { code: sampleCodes(diagnostics) } }),
    ...checkPublished(rules, routes, published),
  ];

  if (problems.length > 0) {
    console.error(`_redirects has ${problems.length} problem(s):`);
    for (const p of problems) {
      console.error(p.rule ? `  line ${p.line}: ${p.rule}\n    ${p.problem}` : `  ${p.problem}`);
    }
    process.exit(1);
  }
  console.log(
    `_redirects: ${rules.length} rules checked against ${routes.size} built routes; ` +
      `${published.length} published routes all still reachable.`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
