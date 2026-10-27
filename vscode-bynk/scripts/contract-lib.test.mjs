// node --test scripts/contract-lib.test.mjs
import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import {
  legendMismatches,
  missingKeywords,
  snippetSkeletons,
  stripSnippetPlaceholders,
  wholeWordPresent,
  wrapForParse,
} from "./contract-lib.mjs";

test("whole-word matching respects word boundaries", () => {
  assert.ok(wholeWordPresent('"match": "\\\\b(if|else)\\\\b"', "else"));
  assert.ok(!wholeWordPresent("elsewhere", "else"));
  assert.ok(!wholeWordPresent("my_else", "else"));
  assert.ok(wholeWordPresent("(a|else|b)", "else"));
});

test("a reserved keyword the grammar lacks is reported", () => {
  const contract = { reservedKeywords: ["if", "invariant"] };
  assert.deepEqual(missingKeywords(contract, "(if|else)"), ["invariant"]);
  assert.deepEqual(missingKeywords(contract, "(if|invariant)"), []);
});

const contract = {
  semanticTokens: { customTypes: ["agent", "actor"], customModifiers: ["refined"] },
};
const pkg = (types, mods) => ({
  contributes: {
    semanticTokenTypes: types.map((id) => ({ id })),
    semanticTokenModifiers: mods.map((id) => ({ id })),
  },
});

test("the legend check passes when package.json mirrors the server", () => {
  assert.deepEqual(legendMismatches(contract, pkg(["actor", "agent"], ["refined"])), []);
});

test("the legend check reports both directions", () => {
  const out = legendMismatches(contract, pkg(["agent", "stale"], ["refined"]));
  assert.equal(out.length, 2);
  assert.match(out.join("\n"), /emits `actor` but package\.json does not declare it/);
  assert.match(out.join("\n"), /declares `stale` but the server does not emit it/);
});

test("placeholders strip to a skeleton", () => {
  assert.equal(stripSnippetPlaceholders("context ${1:my.orders}\n\n$0"), "context my.orders\n\n");
  assert.equal(stripSnippetPlaceholders("x ${1|a,b,c|} y"), "x a y");
  // A bare tab stop closing an expression block becomes `()`; elsewhere, empty.
  assert.equal(stripSnippetPlaceholders("fn f() -> Unit {\n  $0\n}"), "fn f() -> Unit {\n  ()\n}");
  assert.equal(stripSnippetPlaceholders("fn f(${1}) {\n  $0\n}"), "fn f() {\n  \n}");
});

test("scaffolds are wrapped by kind", () => {
  assert.equal(wrapForParse("context", "context a"), "context a");
  assert.match(wrapForParse("type", "type T = Int"), /^context Scaffold\n\ntype T = Int\n$/);
  assert.match(wrapForParse("on http", "on http(x) {}"), /^context Scaffold\n\nservice Scaffold \{/);
  assert.match(wrapForParse("case", "x"), /^suite Scaffold \{\n/);
});

test("the real snippet catalogue yields a skeleton per snippet", () => {
  const snippets = JSON.parse(
    fs.readFileSync(new URL("../snippets/bynk.json", import.meta.url), "utf8"),
  );
  const skeletons = snippetSkeletons(snippets);
  assert.equal(skeletons.length, Object.keys(snippets).length);
  for (const [, source] of skeletons) assert.doesNotMatch(source, /\$\d|\$\{/);
});
