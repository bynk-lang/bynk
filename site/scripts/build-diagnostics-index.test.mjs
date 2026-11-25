// @ts-check
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { readEntries, renderIndex } from "./build-diagnostics-index.mjs";

const entry = (/** @type {string} */ code, /** @type {string} */ category, severity = "error", summary = `About ${code}.`) => ({
  code,
  summary,
  severity,
  category,
});

test("the index groups codes by category title, codes sorted within each", () => {
  const page = renderIndex([
    entry("bynk.type.b", "Types"),
    entry("bynk.actor.z", "Actors"),
    entry("bynk.type.a", "Types"),
  ]);
  const headings = [...page.matchAll(/^## (.+)$/gm)].map((m) => m[1]);
  assert.deepEqual(headings, ["Actors", "Types"]);
  assert.ok(page.indexOf("bynk.type.a") < page.indexOf("bynk.type.b"));
});

test("the index counts every code and badges warnings and hints, not errors", () => {
  const page = renderIndex([
    entry("bynk.a.x", "A"),
    entry("bynk.a.y", "A", "warning"),
    entry("bynk.a.z", "A", "hint"),
  ]);
  assert.match(page, /There are \*\*3\*\* codes in total\./);
  assert.match(page, /- \[`bynk\.a\.x`\]\(\/reference\/diagnostics\/bynk\.a\.x\/\) — About bynk\.a\.x\./);
  assert.match(page, /bynk\.a\.y`\]\(\/reference\/diagnostics\/bynk\.a\.y\/\) — <span class="diagnostic-severity">Warning<\/span> About/);
  assert.match(page, /<span class="diagnostic-severity">Hint<\/span> About bynk\.a\.z/);
});

test("the page is a Starlight page with a title and the filter script", () => {
  const page = renderIndex([entry("bynk.a.x", "A")]);
  assert.ok(page.startsWith("---\ntitle: Diagnostic index\n---\n"));
  assert.ok(page.includes('<script type="module" src="/diagnostic-filter.js"></script>'));
});

test("entries are read from the per-code files, sorted by code", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "diag-index-"));
  try {
    for (const code of ["bynk.b.y", "bynk.a.x"]) {
      fs.writeFileSync(path.join(dir, `${code}.json`), JSON.stringify(entry(code, "A")));
    }
    fs.writeFileSync(path.join(dir, "notes.txt"), "ignored");
    assert.deepEqual(readEntries(dir).map((e) => e.code), ["bynk.a.x", "bynk.b.y"]);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("retired codes get their own section, sorted, with what replaced them", () => {
  const page = renderIndex(
    [entry("bynk.a.x", "A")],
    [
      { code: "bynk.z.old", version: "1.2.3", why: "It went.", replacement: null },
      { code: "bynk.b.old", version: "1.0.0", why: "Unreachable now.", replacement: "the body cap (#3921)" },
    ],
  );
  assert.match(page, /^## Retired codes$/m);
  assert.ok(page.indexOf("bynk.b.old") < page.indexOf("bynk.z.old"));
  assert.match(page, /`bynk\.b\.old` — retired in v1\.0\.0\. Unreachable now\. Replaced by the body cap \(#3921\)\./);
  assert.match(page, /`bynk\.z\.old` — retired in v1\.2\.3\. It went\.\n/);
});

test("no tombstones, no Retired codes section", () => {
  assert.doesNotMatch(renderIndex([entry("bynk.a.x", "A")]), /Retired codes/);
});
