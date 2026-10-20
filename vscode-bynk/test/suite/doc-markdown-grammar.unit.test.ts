// Doc comments (`---` blocks and `--- text` lines) highlight their Markdown by
// including VS Code's built-in `text.html.markdown` grammar. As with
// markdown-injection.unit.test.ts there is no tokenizer harness, so this pins
// the declarative contract that keeps a doc-block closing where the compiler
// closes it (ADR 0449: on a marker of the opener's length):
//
// - one block rule per marker length, each opening and closing on that length;
// - every code fence inside a block also ends just before that block's marker,
//   since an open child rule stops the parent's end being tried;
// - prose uses only Markdown's single-line rules, never its begin/while block
//   rules, which would run past the marker.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

// out/test/suite -> the extension root.
const ROOT = path.resolve(__dirname, "../../..");

interface Rule {
  begin?: string;
  end?: string;
  match?: string;
  name?: string;
  contentName?: string;
  include?: string;
  patterns?: Rule[];
  captures?: Record<string, { name?: string; patterns?: Rule[] }>;
}

const grammar = JSON.parse(
  fs.readFileSync(path.join(ROOT, "syntaxes/bynk.tmLanguage.json"), "utf8"),
) as { repository: Record<string, Rule> };
const repo = grammar.repository;

function rule(name: string): Rule {
  const r = repo[name];
  assert.ok(r, `the grammar defines ${name}`);
  return r;
}

const includes = (r: Rule): string[] => (r.patterns ?? []).map((p) => p.include ?? "");

// The marker lines each block length must open and close on, and must not.
const LENGTHS: { key: string; closes: string[]; ignores: string[] }[] = [
  { key: "3", closes: ["---", "  ---  "], ignores: ["----", "-----"] },
  { key: "4", closes: ["----"], ignores: ["---", "-----"] },
  { key: "long", closes: ["-----", "--------"], ignores: ["---", "----"] },
];

describe("Markdown in doc comments", () => {
  it("splits the doc-block by marker length", () => {
    assert.deepStrictEqual(includes(rule("doc-block")), [
      "#doc-block-3",
      "#doc-block-4",
      "#doc-block-long",
    ]);
  });

  for (const { key, closes, ignores } of LENGTHS) {
    describe(`doc-block-${key}`, () => {
      const block = rule(`doc-block-${key}`);

      it("keeps the documentation comment scope", () => {
        assert.strictEqual(block.name, "comment.block.documentation.bynk");
      });

      it("opens only on its own marker length", () => {
        const begin = new RegExp(block.begin ?? "");
        for (const line of closes) assert.ok(begin.test(line), `opens on ${JSON.stringify(line)}`);
        for (const line of ignores) assert.ok(!begin.test(line), `not on ${JSON.stringify(line)}`);
      });

      it("holds fences then Markdown prose", () => {
        assert.deepStrictEqual(includes(block), [
          `#doc-fence-bynk-${key}`,
          `#doc-fence-raw-${key}`,
          "#doc-markdown",
        ]);
      });

      for (const fence of [`doc-fence-bynk-${key}`, `doc-fence-raw-${key}`]) {
        it(`${fence} ends before the block's marker`, () => {
          const end = rule(fence).end ?? "";
          const look = /\(\?=\^.*\)$/.exec(end);
          assert.ok(look, `${fence} ends on a look-ahead at the marker: ${end}`);
          const marker = new RegExp(look[0]);
          for (const line of closes)
            assert.ok(marker.test(line), `stops at ${JSON.stringify(line)}`);
          for (const line of ignores)
            assert.ok(!marker.test(line), `runs on past ${JSON.stringify(line)}`);
        });
      }
    });
  }

  it("embeds Bynk in ```bynk fences, raw text in any other", () => {
    for (const { key } of LENGTHS) {
      const bynk = rule(`doc-fence-bynk-${key}`);
      assert.strictEqual(bynk.contentName, "meta.embedded.block.bynk");
      assert.deepStrictEqual(includes(bynk), ["source.bynk"]);
      assert.ok(bynk.begin?.includes("(?i:(bynk)("), `bynk info string: ${bynk.begin}`);
      const raw = rule(`doc-fence-raw-${key}`);
      assert.strictEqual(raw.contentName, "markup.raw.block.markdown");
      assert.strictEqual(raw.patterns, undefined, "no highlighting inside a raw fence");
    }
  });

  it("uses only Markdown's single-line rules for prose", () => {
    const md = includes(rule("doc-markdown")).filter((i) => i.startsWith("text.html.markdown"));
    assert.deepStrictEqual(md, ["text.html.markdown#inline"]);
  });

  it("matches a heading at any indent", () => {
    const heading = (rule("doc-markdown").patterns ?? []).find(
      (p) => p.name === "markup.heading.markdown",
    );
    assert.ok(heading?.match, "doc-markdown has its own heading rule");
    const re = new RegExp(heading.match);
    for (const line of ["# Title", "\t## Title", "      ### Title ###"]) {
      assert.ok(re.test(line), `a heading: ${JSON.stringify(line)}`);
    }
    for (const line of ["#Title", "text # not a heading"]) {
      assert.ok(!re.test(line), `not a heading: ${JSON.stringify(line)}`);
    }
  });

  // #2203: a `--- text` line is a one-line doc; a marker alone opens a block.
  it("matches a --- text line, without its closer, and never a fence", () => {
    const line = rule("doc-line");
    assert.strictEqual(line.name, "comment.line.documentation.bynk");
    assert.deepStrictEqual(line.captures?.["2"]?.patterns, [
      { include: "text.html.markdown#inline" },
    ]);
    const re = new RegExp(line.match ?? "");
    const text = (l: string) => re.exec(l)?.[2];
    assert.strictEqual(text("--- A doc."), "A doc.");
    assert.strictEqual(text("\t--- A doc. ---"), "A doc.");
    assert.strictEqual(text("---- A doc. ----"), "A doc.");
    assert.strictEqual(text("---- A doc ---"), "A doc ---", "a run of another length is text");
    assert.strictEqual(text("--- a---"), "a---");
    for (const l of ["---", "  ----  ", "x --- y", "---x", "--| A doc.", "// A comment."]) {
      assert.ok(!re.test(l), `not a doc line: ${JSON.stringify(l)}`);
    }
  });

  it("opens no doc-block on a --- text line", () => {
    for (const { key } of LENGTHS) {
      const begin = new RegExp(rule(`doc-block-${key}`).begin ?? "");
      for (const l of ["--- A doc.", "---- A doc. ----", "----- text"]) {
        assert.ok(!begin.test(l), `doc-block-${key} does not open on ${JSON.stringify(l)}`);
      }
    }
  });

  it("matches a // comment anywhere, and nothing with dashes", () => {
    const re = new RegExp(rule("line-comment").match ?? "");
    assert.strictEqual(rule("line-comment").name, "comment.line.double-slash.bynk");
    assert.ok(re.test("// a comment"));
    assert.ok(re.test("let x = 1 // trailing"));
    assert.ok(!re.test("-- a retired comment"));
  });
});
