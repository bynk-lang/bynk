import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, sep } from "node:path";
import { fileURLToPath } from "node:url";

// 1B.3b (#4047, Decision D): a test double is a `makeIntegration*` factory, and it lives under
// `src/testing/`, where a test build imports it. No production runtime module defines one, so a
// project's emitted `storage`, `agent` or `vectorize` module carries none. `makeTestState` and
// `InMemoryStorage` are not doubles: the Bundle target's `StateRegistry` is built on them, so they
// stay in `storage.ts`.

const src = join(dirname(fileURLToPath(import.meta.url)), "..", "src");

function typescriptFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? typescriptFiles(path) : path.endsWith(".ts") ? [path] : [];
  });
}

// A double is exported as a function or as a `const` (an arrow or a bound function).
const DOUBLE = /^export (?:(?:async )?function|const) (makeIntegration\w+)/gm;

test("no production runtime module defines a makeIntegration* test double", () => {
  const offenders: string[] = [];
  for (const file of typescriptFiles(src)) {
    if (file.startsWith(join(src, "testing") + sep)) continue;
    for (const m of readFileSync(file, "utf8").matchAll(DOUBLE)) {
      offenders.push(`${file.slice(src.length + 1)}: ${m[1]}`);
    }
  }
  assert.deepEqual(offenders, []);
});

test("the test doubles are defined under src/testing/", () => {
  let found = 0;
  for (const file of typescriptFiles(join(src, "testing"))) {
    found += [...readFileSync(file, "utf8").matchAll(DOUBLE)].length;
  }
  assert.ok(found >= 18, `expected the integration doubles under src/testing/, found ${found}`);
});

test("makeTestState and InMemoryStorage stay in storage.ts: the Bundle target's agent storage", () => {
  const storage = readFileSync(join(src, "storage.ts"), "utf8");
  assert.match(storage, /^export function makeTestState\(/m);
  assert.match(storage, /^export class InMemoryStorage\b/m);
});
