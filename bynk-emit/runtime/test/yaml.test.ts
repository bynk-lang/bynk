// `Yaml.decode[T]` (#3214): the YAML core-schema subset parser.
import { test } from "node:test";
import assert from "node:assert/strict";
import type { JsonValue } from "../src/boundary.ts";
import { YAML_MAX_BYTES, YAML_MAX_DEPTH, __bynkYamlParse } from "../src/yaml.ts";

function ok(text: string): JsonValue {
  const r = __bynkYamlParse(text);
  assert.equal(r.tag, "Ok", r.tag === "Err" ? `${r.error.kind}: ${r.error.message}` : "");
  return (r as { value: JsonValue }).value;
}

function err(text: string): { kind: string; path: string; message: string } {
  const r = __bynkYamlParse(text);
  assert.equal(r.tag, "Err", `expected a refusal, got ${JSON.stringify((r as { value: unknown }).value)}`);
  return (r as { error: { kind: string; path: string; message: string } }).error;
}

test("core-schema scalar resolution", () => {
  const cases: [string, JsonValue][] = [
    ["~", null],
    ["null", null],
    ["Null", null],
    ["NULL", null],
    ["true", true],
    ["True", true],
    ["TRUE", true],
    ["false", false],
    ["False", false],
    ["FALSE", false],
    ["0", 0],
    ["-12", -12],
    ["+7", 7],
    ["007", 7],
    ["0o17", 15],
    ["0x1F", 31],
    ["1.5", 1.5],
    ["-.5", -0.5],
    ["1e3", 1000],
    ["1.5E-2", 0.015],
    ["5.", 5],
    // The YAML 1.1 hazards stay strings under the 1.2 core schema.
    ["on", "on"],
    ["off", "off"],
    ["yes", "yes"],
    ["no", "no"],
    ["y", "y"],
    ["n", "n"],
    ["Yes", "Yes"],
    ["2001-12-14", "2001-12-14"],
    ["1_000", "1_000"],
    ["0b101", "0b101"],
    ["0o8", "0o8"],
    ["1.2.3", "1.2.3"],
    ["nUll", "nUll"],
    ["-", "-"],
    ["hello world", "hello world"],
    ["a: b".replace(": ", ":"), "a:b"],
    ["http://example.com/x", "http://example.com/x"],
  ];
  for (const [text, want] of cases) assert.deepEqual(ok(`v: ${text}\n`), { v: want }, text);
});

test("quoted scalars are always strings", () => {
  assert.deepEqual(ok(`a: "true"\nb: 'null'\nc: "12"\nd: ''\n`), { a: "true", b: "null", c: "12", d: "" });
  assert.deepEqual(ok(`a: 'it''s'\nb: "q\\"uote"\n`), { a: "it's", b: 'q"uote' });
});

test("double-quoted escapes", () => {
  assert.deepEqual(ok(`a: "x\\ty\\n\\u00e9\\x41\\U0001F600\\\\"\n`), { a: "x\ty\n\u00e9A\u{1F600}\\" });
  assert.equal(err(`a: "\\q"\n`).kind, "Malformed");
  assert.equal(err(`a: "\\u12"\n`).kind, "Malformed");
  assert.equal(err(`a: "\\ud800"\n`).kind, "Malformed");
});

test("the settings-file example", () => {
  const doc = [
    "# Which features the App runs here, and how.",
    "triage:",
    "  enabled: true",
    "  labels: [bug, enhancement, documentation]   # the only labels it may suggest",
    "nudges:",
    "  enabled: true",
    "  after_days: 3",
    "",
  ].join("\n");
  assert.deepEqual(ok(doc), {
    triage: { enabled: true, labels: ["bug", "enhancement", "documentation"] },
    nudges: { enabled: true, after_days: 3 },
  });
});

test("block mappings and sequences", () => {
  assert.deepEqual(ok("a:\n  b:\n    c: 1\n  d: [1, 2]\ne: x\n"), { a: { b: { c: 1 }, d: [1, 2] }, e: "x" });
  assert.deepEqual(ok("- 1\n- a: 1\n  b: 2\n- - x\n  - y\n-\n  z: 1\n"), [1, { a: 1, b: 2 }, ["x", "y"], { z: 1 }]);
  // A sequence at the same indent as its key.
  assert.deepEqual(ok("k:\n- 1\n- 2\nm: 3\n"), { k: [1, 2], m: 3 });
  assert.deepEqual(ok("k:\n"), { k: null });
  assert.deepEqual(ok("- \n- b\n"), [null, "b"]);
  assert.deepEqual(ok("? : x".slice(0, 0) + "a: 1\n\n\nb: 2\n"), { a: 1, b: 2 });
});

test("flow collections", () => {
  assert.deepEqual(ok("a: {x: 1, y: [true, ~], 'z w': \"q\"}\n"), { a: { x: 1, y: [true, null], "z w": "q" } });
  assert.deepEqual(ok("[1, 2, 3]\n"), [1, 2, 3]);
  assert.deepEqual(ok("{a, b: 2}\n"), { a: null, b: 2 });
  assert.deepEqual(ok("[a, b,]\n"), ["a", "b"]);
  assert.deepEqual(ok("a: [\n  1,\n  2, # two\n  3\n]\nb: 1\n"), { a: [1, 2, 3], b: 1 });
  assert.deepEqual(ok("a: {}\nb: []\n"), { a: {}, b: [] });
  assert.deepEqual(ok("a: [x y, z]\n"), { a: ["x y", "z"] });
  assert.deepEqual(ok('a: {"k":v}\n'), { a: { k: "v" } });
  assert.equal(err("a: [1, 2\n").kind, "Malformed");
  assert.equal(err("a: {x: 1\n").kind, "Malformed");
  assert.equal(err("a: [x: 1]\n").kind, "UnsupportedSyntax");
});

test("comments", () => {
  assert.deepEqual(ok("# c\na: 1 # c\nb: 'x # y' # c\nc: x#y\n"), { a: 1, b: "x # y", c: "x#y" });
  assert.equal(ok("# only a comment\n"), null);
  assert.equal(ok(""), null);
});

test("multi-line scalars", () => {
  assert.deepEqual(ok("a: one\n  two\n\n  three\nb: 1\n"), { a: "one two\nthree", b: 1 });
  assert.deepEqual(ok('a: "one\n  two\n\n  three"\n'), { a: "one two\nthree" });
  assert.deepEqual(ok("a: 'x\n  y'\n"), { a: "x y" });
  assert.deepEqual(ok('a: "x\\\n   y"\n'), { a: "xy" });
});

test("block scalars", () => {
  assert.deepEqual(ok("a: |\n  line 1\n  line 2\nb: 1\n"), { a: "line 1\nline 2\n", b: 1 });
  assert.deepEqual(ok("a: |-\n  x\n  y\n\n\nb: 1\n"), { a: "x\ny", b: 1 });
  assert.deepEqual(ok("a: |+\n  x\n\n\nb: 1\n"), { a: "x\n\n\n", b: 1 });
  assert.deepEqual(ok("a: >\n  one\n  two\n\n  three\n"), { a: "one two\nthree\n" });
  assert.deepEqual(ok("a: >-\n  one\n    indented\n  two\n"), { a: "one\n  indented\ntwo" });
  assert.deepEqual(ok("a: |2\n    x\n   y\n"), { a: "  x\n y\n" });
  assert.deepEqual(ok("- |\n  a\n- >\n  b\n"), ["a\n", "b\n"]);
  assert.deepEqual(ok("a: |\n  # not a comment\n  x\n"), { a: "# not a comment\nx\n" });
  assert.deepEqual(ok("a: |\nb: 1\n"), { a: "", b: 1 });
  assert.equal(ok("|\n  top\n"), "top\n");
  assert.equal(err("a: |x\n  y\n").kind, "Malformed");
});

test("a single document, with markers", () => {
  assert.deepEqual(ok("---\na: 1\n"), { a: 1 });
  assert.deepEqual(ok("---\na: 1\n...\n"), { a: 1 });
  assert.deepEqual(ok("--- # c\na: 1\n"), { a: 1 });
  assert.equal(ok("--- text\n"), "text");
  assert.equal(ok("--- |\n  b\n"), "b\n");
  assert.equal(err("a: 1\n---\nb: 2\n").kind, "MultipleDocuments");
  assert.equal(err("---\na: 1\n---\nb: 2\n").kind, "MultipleDocuments");
  assert.equal(err("a: 1\n...\n---\nb: 2\n").kind, "MultipleDocuments");
  assert.equal(err("a: 1\n---\n").kind, "MultipleDocuments");
});

test("anchors, aliases, merge keys, tags, directives, complex keys are refused", () => {
  const cases: [string, string, string][] = [
    ["a: &x 1\n", "UnsupportedAnchor", "$.a"],
    ["&x a: 1\n", "UnsupportedAnchor", "$"],
    ["- &x 1\n", "UnsupportedAnchor", "$[0]"],
    ["a: [&x 1]\n", "UnsupportedAnchor", "$.a[0]"],
    ["a: &x\n  b: 1\n", "UnsupportedAnchor", "$.a"],
    ["a: *x\n", "UnsupportedAlias", "$.a"],
    ["- *x\n", "UnsupportedAlias", "$[0]"],
    ["a: {b: *x}\n", "UnsupportedAlias", "$.a.b"],
    ["<<: {a: 1}\n", "UnsupportedMergeKey", "$.<<"],
    ["a:\n  <<: x\n", "UnsupportedMergeKey", "$.a.<<"],
    ["{<<: x}\n", "UnsupportedMergeKey", "$.<<"],
    ["a: !!str 1\n", "UnsupportedTag", "$.a"],
    ["a: !custom x\n", "UnsupportedTag", "$.a"],
    ["!!map\na: 1\n", "UnsupportedTag", "$"],
    ["a: [!t x]\n", "UnsupportedTag", "$.a[0]"],
    ["%YAML 1.2\n---\na: 1\n", "UnsupportedSyntax", "$"],
    ["? a\n: 1\n", "UnsupportedSyntax", "$"],
    ["a: .inf\n", "UnsupportedValue", "$.a"],
    ["a: -.INF\n", "UnsupportedValue", "$.a"],
    ["a: .nan\n", "UnsupportedValue", "$.a"],
    ["a: 1\na: 2\n", "DuplicateKey", "$.a"],
    ["{a: 1, a: 2}\n", "DuplicateKey", "$.a"],
  ];
  for (const [text, kind, path] of cases) {
    const e = err(text);
    assert.equal(e.kind, kind, `${JSON.stringify(text)}: ${e.message}`);
    assert.equal(e.path, path, JSON.stringify(text));
  }
  // A quoted `<<` is only a string key.
  assert.deepEqual(ok('"<<": 1\n'), { "<<": 1 });
  // `&`, `*`, `!` inside a plain value are ordinary characters.
  assert.deepEqual(ok("a: x&y *z q!r\n"), { a: "x&y *z q!r" });
});

test("a refusal names its line and column", () => {
  const e = err("a: 1\nb:\n  c: &x 2\n");
  assert.equal(e.kind, "UnsupportedAnchor");
  assert.equal(e.path, "$.b.c");
  assert.match(e.message, /\(line 3, column 6\)$/);
  assert.match(err("a: 1\n\tb: 2\n").message, /line 2/);
});

test("syntax errors are Malformed", () => {
  for (const bad of [
    "a: b: c\n",
    "a: 1\n b: 2\n",
    "a: 1\n  - x\n",
    "- a\nb: 1\n",
    "a: [\n",
    'a: "unterminated\n',
    "a: @x\n",
    "a: `x\n",
    "\tkey: 1\n",
    "a:\n\t- x\n",
    "a: 1\n- b\n",
    "key\n: value\n",
  ]) {
    const e = err(bad);
    assert.equal(e.kind, "Malformed", `${JSON.stringify(bad)} -> ${e.kind}: ${e.message}`);
  }
});

test("keys are strings", () => {
  assert.deepEqual(ok("1: a\ntrue: b\nnull: c\n'x y': d\n"), { "1": "a", true: "b", null: "c", "x y": "d" });
});

test("a __proto__ key is data, not a prototype", () => {
  const v = ok("__proto__: {x: 1}\nb: 2\n") as { [k: string]: JsonValue };
  assert.deepEqual(Object.keys(v), ["__proto__", "b"]);
  assert.equal(Object.getPrototypeOf(v), Object.prototype);
  assert.equal(({} as { x?: number }).x, undefined);
});

test("CRLF and a byte-order mark are accepted", () => {
  assert.deepEqual(ok("\uFEFFa: 1\r\nb:\r\n  - x\r\n"), { a: 1, b: ["x"] });
});

test("nesting deeper than the cap is refused", () => {
  const nest = (n: number): string => "[".repeat(n) + "]".repeat(n);
  assert.deepEqual(ok(nest(YAML_MAX_DEPTH)) !== undefined, true);
  const e = err(nest(YAML_MAX_DEPTH + 1));
  assert.equal(e.kind, "LimitExceeded");
  const block = (n: number): string => Array.from({ length: n }, (_, k) => `${" ".repeat(k)}- `).join("").trimEnd();
  const b = (n: number): string => Array.from({ length: n }, (_, k) => `${" ".repeat(k * 2)}k:`).join("\n") + " 1\n";
  ok(b(YAML_MAX_DEPTH));
  assert.equal(err(b(YAML_MAX_DEPTH + 1)).kind, "LimitExceeded");
  assert.equal(err(`${block(YAML_MAX_DEPTH + 1)} x\n`).kind, "LimitExceeded");
});

test("input over the size cap is refused up front", () => {
  const big = `a: ${"x".repeat(YAML_MAX_BYTES)}\n`;
  const e = err(big);
  assert.equal(e.kind, "LimitExceeded");
  assert.equal(e.path, "$");
  // The cap counts UTF-8 bytes, not code units.
  const wide = `a: ${"\u00e9".repeat(YAML_MAX_BYTES / 2)}\n`;
  assert.equal(err(wide).kind, "LimitExceeded");
  ok(`a: ${"x".repeat(YAML_MAX_BYTES - 10)}\n`);
});

test("the nesting limit is hit before the stack is", () => {
  const e = err("[".repeat(100000));
  assert.equal(e.kind, "LimitExceeded");
});

test("only a space or a tab is whitespace; other Unicode spaces are content", () => {
  // JS `trim` would strip these; YAML keeps them.
  assert.deepEqual(ok("a: x\u00a0\n"), { a: "x\u00a0" });
  assert.deepEqual(ok("a: \u00a0x\n"), { a: "\u00a0x" });
  assert.deepEqual(ok("a: x\u2028\n"), { a: "x\u2028" });
  // A line holding only a no-break space is content, not a blank line.
  assert.deepEqual(ok("a: |\n  x\n  \u00a0\n  y\n"), { a: "x\n\u00a0\ny\n" });
  assert.deepEqual(ok('a: "x\u00a0\n  y"\n'), { a: "x\u00a0 y" });
  // Literal whitespace before a fold is still trimmed; an escaped one is kept.
  assert.deepEqual(ok('a: "x  \n  y"\n'), { a: "x y" });
  assert.deepEqual(ok('a: "x\\t \n  y"\n'), { a: "x\t y" });
});

test("a folded quoted scalar keeps its content over many lines", () => {
  const n = 1000;
  const lines = Array.from({ length: n }, (_, i) => `w${i}`);
  assert.deepEqual(ok(`a: "${lines.join("\n  ")}"\n`), { a: lines.join(" ") });
  assert.deepEqual(ok(`a: '${lines.join("\n  ")}'\n`), { a: lines.join(" ") });
});
