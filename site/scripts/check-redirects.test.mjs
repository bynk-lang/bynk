// Tests for check-redirects.mjs (web presence track, link rule L2): the rule
// parser, the first-match matcher, and the checks on small in-memory sites, so
// a change to the checker can't quietly stop a check from firing.
//
//   node --test site/scripts/check-redirects.test.mjs

import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { applyRedirects, checkRedirects, distRoutes, parseRedirects, resolves, sampleCodes } from "./check-redirects.mjs";

const rules = (/** @type {string} */ text) => {
  const parsed = parseRedirects(text);
  assert.deepEqual(parsed.malformed, []);
  return parsed.rules;
};
const target = (/** @type {string} */ text, /** @type {string} */ path) => applyRedirects(rules(text), path)?.target ?? null;

// --- parsing ---------------------------------------------------------------

test("comment lines and blank lines are ignored; the status defaults to 302", () => {
  const parsed = parseRedirects("# a comment\n  # indented comment\n\n/a /b 301\n/c /d\n");
  assert.deepEqual(parsed.rules, [
    { from: "/a", to: "/b", code: 301, line: 4 },
    { from: "/c", to: "/d", code: 302, line: 5 },
  ]);
  assert.deepEqual(parsed.malformed, []);
});

// Cloudflare only treats a line that starts with `#` as a comment; it would
// drop a rule with a trailing comment, so the checker must not pass it.
test("a trailing comment makes the rule malformed", () => {
  const { rules: parsed, malformed } = parseRedirects("/c /d 301 # moved\n/e /f # moved\n");
  assert.deepEqual(parsed, []);
  assert.deepEqual(malformed.map((m) => m.line), [1, 2]);
});

test("a #fragment in a target is kept", () => {
  assert.equal(target("/old/ /new/#part 301\n", "/old/"), "/new/#part");
  assert.ok(resolves(new Set(["/new/"]), "/new/#part"));
});

test("malformed lines are reported with their line number", () => {
  const { malformed } = parseRedirects("/a\n/b /c 999\nrel /x 301\n/a/*/b /c 301\n/ok /fine 301 extra\n");
  assert.deepEqual(malformed.map((m) => m.line), [1, 2, 3, 4, 5]);
});

// --- matching --------------------------------------------------------------

test("an exact rule matches only its own path, trailing slash included", () => {
  const text = "/old/ /new/ 301\n";
  assert.equal(target(text, "/old/"), "/new/");
  assert.equal(target(text, "/old"), null);
  assert.equal(target(text, "/old/x/"), null);
});

test("a splat captures the rest of the path into :splat", () => {
  const text = "/book/spec/* /reference/spec/:splat 301\n";
  assert.equal(target(text, "/book/spec/grammar/"), "/reference/spec/grammar/");
  assert.equal(target(text, "/book/spec/a/b"), "/reference/spec/a/b");
  assert.equal(target(text, "/book/spec/"), "/reference/spec/");
  assert.equal(target(text, "/book/spec"), null);
});

test("a splat target without :splat sends everything to one page", () => {
  assert.equal(target("/t/* /diagnostics/ 302\n", "/t/anything/at/all/"), "/diagnostics/");
});

test("a :code placeholder matches exactly one segment", () => {
  const text = "/d/:code /reference/diagnostics/:code/ 302\n";
  assert.equal(target(text, "/d/bynk.type.mismatch"), "/reference/diagnostics/bynk.type.mismatch/");
  assert.equal(target(text, "/d/bynk.type.mismatch/"), null);
  assert.equal(target(text, "/d/a/b"), null);
  assert.equal(target(text, "/d/"), null);
});

test("the first matching rule wins", () => {
  const text = "/docs/cli /reference/tooling/bynk-cli/ 301\n/docs/* /reference/tooling/:splat 301\n";
  assert.equal(target(text, "/docs/cli"), "/reference/tooling/bynk-cli/");
  assert.equal(target(text, "/docs/lsp/"), "/reference/tooling/lsp/");
});

test("regex metacharacters in a path match literally", () => {
  assert.equal(target("/a.b /c/ 301\n", "/a.b"), "/c/");
  assert.equal(target("/a.b /c/ 301\n", "/axb"), null);
});

// --- resolving -------------------------------------------------------------

test("a target resolves to a page, its slashless form, an exempt prefix or an external URL", () => {
  const routes = new Set(["/reference/"]);
  assert.ok(resolves(routes, "/reference/"));
  assert.ok(resolves(routes, "/reference"));
  assert.ok(resolves(routes, "/reference/api/bynk_syntax/"));
  assert.ok(resolves(routes, "https://example.com/"));
  assert.ok(!resolves(routes, "/reference/gone/"));
});

// --- the checks, on a small site -------------------------------------------

const SITE = new Set([
  "/",
  "/book/",
  "/reference/",
  "/reference/spec/",
  "/reference/spec/grammar/",
  "/reference/tooling/",
  "/reference/tooling/bynk-cli/",
  "/reference/tooling/lsp/",
  "/reference/diagnostics/",
  "/reference/diagnostics/bynk.a/",
  "/reference/diagnostics/bynk.b/",
  "/404",
  "/llms.txt",
]);
const SAMPLES = { samples: { code: ["bynk.a", "bynk.b"] } };
const GOOD = `
/book/spec /reference/spec/ 301
/book/spec/* /reference/spec/:splat 301
/d/:code /reference/diagnostics/:code/ 302
/docs /reference/tooling/ 301
/docs/tooling /reference/tooling/ 301
/docs/api /reference/api/ 301
/docs/api/* /reference/api/:splat 301
/docs/tooling/* /reference/tooling/:splat 301
/docs/* /reference/tooling/:splat 301
`;
const problemsFor = (/** @type {string} */ text) => checkRedirects(rules(text), SITE, SAMPLES);

test("a sound set of rules has no problems", () => {
  assert.deepEqual(problemsFor(GOOD), []);
});

test("a rule for a live page is reported as shadowing it", () => {
  const problems = problemsFor(`${GOOD}/reference/tooling/lsp/ /book/ 301\n`);
  assert.equal(problems.length, 1);
  assert.match(problems[0].problem, /shadows the live page \/reference\/tooling\/lsp\//);
});

test("a rule for a live page's slashless form shadows it too", () => {
  const problems = problemsFor(`/book /reference/ 301\n${GOOD}`);
  assert.match(problems[0].problem, /shadows the live page \/book\/ \(a request for \/book is redirected\)/);
});

test("a splat prefix without a slash does not take pages from a longer folder", () => {
  // Were /newer/x/ taken as a page under `/new`, its "old path" /older/x/
  // would be tried for the splat rule and land on the missing /gone/.
  const site = new Set(["/new/", "/newer/x/"]);
  const text = "/older/x/ /gone/ 301\n/old /new/ 301\n/old* /new:splat 301\n";
  const problems = checkRedirects(rules(text), site, SAMPLES);
  assert.deepEqual(problems.map((p) => p.line), [1], problems.map((p) => p.problem).join("\n"));
});

test("shadow reports are capped per rule", () => {
  const site = new Set(Array.from({ length: 20 }, (_, i) => `/reference/p${i}/`));
  const problems = checkRedirects(rules("/reference/* /book/ 301\n"), new Set([...site, "/book/"]), SAMPLES);
  assert.equal(problems.filter((p) => p.problem.startsWith("shadows")).length, 5);
  assert.ok(problems.some((p) => p.problem === "… and 35 more live paths it shadows"));
});

test("a splat over a live folder is reported", () => {
  const problems = problemsFor(`/reference/spec/* /book/ 301\n${GOOD}`);
  assert.ok(problems.some((p) => /shadows the live page \/reference\/spec\/grammar\//.test(p.problem)));
});

test("an allowed shadow is not reported", () => {
  const text = `${GOOD}/reference/tooling/lsp/ /book/ 301\n`;
  assert.deepEqual(checkRedirects(rules(text), SITE, { ...SAMPLES, allowShadow: new Set(["/reference/tooling/lsp/"]) }), []);
});

test("an exact rule after a splat that covers it never applies", () => {
  const text = GOOD.replace("/docs/tooling /reference/tooling/ 301\n", "") + "/docs/tooling /reference/tooling/ 301\n";
  const problems = problemsFor(text);
  assert.ok(problems.some((p) => /never applies: the earlier rule on line \d+ \(\/docs\/\*\)/.test(p.problem)));
});

test("an exact rule to a missing page is reported", () => {
  const problems = problemsFor(`${GOOD}/old/ /reference/gone/ 301\n`);
  assert.deepEqual(problems.map((p) => p.problem), ["/old/ → /reference/gone/, which is not a page"]);
});

test("a splat whose target folder moved is reported", () => {
  const problems = problemsFor(GOOD.replace("/book/spec/* /reference/spec/:splat", "/book/spec/* /reference/specification/:splat"));
  assert.ok(problems.some((p) => /\/book\/spec\/ → \/reference\/specification\/, which is not a page/.test(p.problem)));
});

// The slice 5 review (#3944): without their own rules, `/docs/tooling` and
// `/docs/api` fell through to the `/docs/*` splat and landed on no page.
test("a slashless folder that falls through to a wider splat is reported", () => {
  const text = GOOD.replace("/docs/tooling /reference/tooling/ 301\n", "").replace("/docs/api /reference/api/ 301\n", "");
  const problems = problemsFor(text).map((p) => p.problem);
  assert.ok(problems.some((p) => /^\/docs\/tooling → \/reference\/tooling\/tooling \(by the rule on line \d+, \/docs\/\*\), which is not a page$/.test(p)), problems.join("\n"));
  assert.ok(problems.some((p) => p.startsWith("/docs/api → /reference/tooling/api")), problems.join("\n"));
});

test("a slashless folder that matches no rule is reported", () => {
  const problems = problemsFor(GOOD.replace("/book/spec /reference/spec/ 301\n", "")).map((p) => p.problem);
  assert.deepEqual(problems, ["/book/spec matches no rule and is not a page"]);
});

test("a :code rule is tried with each sample code", () => {
  const site = new Set([...SITE].filter((r) => r !== "/reference/diagnostics/bynk.b/"));
  const problems = checkRedirects(rules(GOOD), site, SAMPLES).map((p) => p.problem);
  assert.deepEqual(problems, ["/d/bynk.b → /reference/diagnostics/bynk.b/, which is not a page"]);
});

test("a placeholder with no sample values is reported", () => {
  const problems = checkRedirects(rules("/x/:slug /reference/ 301\n"), SITE, SAMPLES).map((p) => p.problem);
  assert.deepEqual(problems, ["no sample values to test :slug"]);
});

test("the rustdoc targets are exempt", () => {
  assert.deepEqual(problemsFor("/docs/api /reference/api/ 301\n/docs/api/* /reference/api/:splat 301\n"), []);
});

// --- the route model and the sample codes -----------------------------------

test("distRoutes maps index.html to the folder, x.html to /x and assets to themselves", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "redirects-"));
  try {
    for (const rel of ["index.html", "book/index.html", "book/a/index.html", "404.html", "llms.txt", "_astro/x.css"]) {
      fs.mkdirSync(path.dirname(path.join(dir, rel)), { recursive: true });
      fs.writeFileSync(path.join(dir, rel), "");
    }
    assert.deepEqual([...distRoutes(dir)].sort(), ["/", "/404", "/_astro/x.css", "/book/", "/book/a/", "/llms.txt"]);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("sampleCodes takes the first, middle and last registry codes", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "redirects-"));
  try {
    const file = path.join(dir, "diagnostics.json");
    fs.writeFileSync(file, JSON.stringify(["a", "b", "c", "d", "e"].map((code) => ({ code }))));
    assert.deepEqual(sampleCodes(file), ["a", "c", "e"]);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
