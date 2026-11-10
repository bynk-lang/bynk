// The YAML subset parser against a filtered yaml-test-suite corpus (#3214), and
// against the `yaml` package as an oracle (a dev dependency, never shipped).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";
import { __bynkYamlParse } from "../src/yaml.ts";

const dir = join(dirname(fileURLToPath(import.meta.url)), "yaml-corpus");
const manifest = JSON.parse(readFileSync(join(dir, "manifest.json"), "utf8")) as {
  pass: string[];
  refused: { [id: string]: string };
  lenient: string[];
};
const read = (id: string, file: string): string => readFileSync(join(dir, id, file), "utf8");

test("every accepted corpus case decodes to exactly its expected JSON", () => {
  assert.ok(manifest.pass.length >= 150, "the corpus has shrunk");
  for (const id of manifest.pass) {
    const r = __bynkYamlParse(read(id, "in.yaml"));
    assert.equal(r.tag, "Ok", `${id}: ${r.tag === "Err" ? r.error.message : ""}`);
    assert.deepEqual((r as { value: unknown }).value, JSON.parse(read(id, "in.json")), id);
  }
});

test("every refused corpus case stays refused, with its kind", () => {
  for (const [id, kind] of Object.entries(manifest.refused)) {
    const r = __bynkYamlParse(read(id, "in.yaml"));
    assert.equal(r.tag, "Err", `${id} is now accepted: move it to \`pass\` after checking it`);
    assert.equal((r as { error: { kind: string } }).error.kind, kind, id);
  }
});

test("the known leniencies do not grow", () => {
  for (const id of manifest.lenient) {
    assert.equal(__bynkYamlParse(read(id, "in.yaml")).tag, "Ok", `${id} is now refused: remove it from \`lenient\``);
  }
});

// Documents inside the accepted subset: the parser and the `yaml` package (core
// schema, YAML 1.2) must agree on the value.
const SUBSET: string[] = [
  "a: 1\nb: [x, y]\nc: {d: true}\n",
  "- 1\n- 2.5\n- ~\n- 'q'\n- \"w\\n\"\n",
  "on: off\nyes: no\nn: y\n",
  "t: 2001-12-14\nu: 1_000\no: 0o17\nh: 0x1F\ne: 1e3\n",
  "s: |\n  a\n  b\nf: >-\n  c\n  d\nk: |+\n  x\n\n",
  "# c\nkey: value # trailing\nother: 'x # y'\n",
  "l:\n- a\n- b: 1\n  c: 2\n- - x\n",
  "multi: one\n  two\n\n  three\nq: \"one\n  two\"\n",
  "---\ndoc: true\n...\n",
  "[a, [b, c], {d: e}]\n",
  "a: {}\nb: []\nc:\nd: ''\n",
  "'quoted key': 1\n\"also\": 2\n",
];

test("agrees with the `yaml` package on the accepted subset", () => {
  for (const text of SUBSET) {
    const mine = __bynkYamlParse(text);
    assert.equal(mine.tag, "Ok", `${JSON.stringify(text)}`);
    const theirs = parse(text, { schema: "core", version: "1.2" }) as unknown;
    assert.deepEqual((mine as { value: unknown }).value, theirs, JSON.stringify(text));
  }
});

// A deterministic mutation fuzz: whatever the bytes, the parser returns a value
// or a `DecodeError` (it never throws) and does so quickly.
test("mutated corpus inputs never throw and never hang", () => {
  let seed = 0x9e3779b9;
  const rnd = (n: number): number => {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
    return seed % n;
  };
  const ids = [...manifest.pass, ...Object.keys(manifest.refused), ...manifest.lenient];
  const junk = [" ", "\t", "\n", "- ", ": ", "#", "&a ", "*a", "!t ", "[", "]", "{", "}", '"', "'", "|", ">", "---\n", "...\n", ",", "\\", "\u00e9", "\r"];
  const started = Date.now();
  for (let round = 0; round < 4; round++) {
    for (const id of ids) {
      let text = read(id, "in.yaml");
      for (let k = 0; k < 1 + rnd(4); k++) {
        const at = rnd(text.length + 1);
        const op = rnd(3);
        if (op === 0) text = text.slice(0, at) + junk[rnd(junk.length)] + text.slice(at);
        else if (op === 1) text = text.slice(0, at) + text.slice(at + 1 + rnd(3));
        else text = text.slice(0, at);
      }
      const r = __bynkYamlParse(text);
      assert.ok(r.tag === "Ok" || r.tag === "Err", JSON.stringify(text));
    }
  }
  assert.ok(Date.now() - started < 20000, "the fuzz run was slow");
});

test("pathological inputs finish quickly", () => {
  const cases = [
    "a: " + "x ".repeat(200000),
    "- ".repeat(100000),
    "[".repeat(100000) + "]".repeat(100000),
    "a:\n" + " b:\n".repeat(50000),
    '"' + "\\u00e9".repeat(100000) + '"',
    "|\n" + "  x\n".repeat(100000),
    Array.from({ length: 50000 }, (_, i) => `k${i}: ${i}`).join("\n"),
    // A quoted value folded over many lines (a regression for a quadratic fold, #3275).
    'a: "' + "a\n".repeat(200000) + '"\n',
    "a: '" + "a\n".repeat(200000) + "'\n",
    // A long run of spaces, then text, then a line break, inside a quoted value.
    'a: "' + " ".repeat(500000) + "x\n" + '"\n',
    // Long whitespace runs elsewhere: a block-scalar header, a plain value, a flow scalar.
    "a: |" + " ".repeat(500000) + "x\n  y\n",
    "a: " + "x" + " ".repeat(500000) + "y\n",
    "a: [" + "x".repeat(10) + " ".repeat(500000) + "y]\n",
    // Many continuation lines of a plain, a folded and a flow scalar.
    "a: " + "word\n  ".repeat(100000) + "end\n",
    "a: >\n" + "  folded line\n".repeat(100000),
    "a: [" + "x\n".repeat(100000) + "]\n",
    // Many colons in one plain scalar.
    "a: " + "b:".repeat(200000) + "\n",
  ];
  for (const text of cases) {
    const t = Date.now();
    const r = __bynkYamlParse(text);
    assert.ok(r.tag === "Ok" || r.tag === "Err");
    assert.ok(Date.now() - t < 5000, `slow: ${text.slice(0, 20)}`);
  }
});
