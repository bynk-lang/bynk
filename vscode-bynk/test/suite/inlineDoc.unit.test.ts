// Unit coverage for the #849 doc-Markdown tokenizer (src/inlineDoc.ts) — the
// pure function the inline-doc decoration surface applies. VS Code exposes no API
// to read back applied `TextEditorDecorationType` ranges, so the range logic is
// pinned here directly, adversarial cases included: emphasis that must NOT fire
// (snake_case, whitespace-hugging, bullets), block pairing, and unclosed blocks.

import * as assert from "assert";

import {
  docDecorations,
  type DocDecorationKind,
  type DocDecorationRange,
} from "../../src/inlineDoc";

/** Compact a range to `kind@line:start-end` for readable assertions. */
function fmt(r: DocDecorationRange): string {
  return `${r.kind}@${r.line}:${r.startChar}-${r.endChar}`;
}

function decos(text: string): string[] {
  return docDecorations(text).map(fmt);
}

/** Wrap body lines in a doc block starting at line 0, so `---` is line 0 and the
 *  first body line is line 1. */
function block(...body: string[]): string {
  return ["---", ...body, "---"].join("\n");
}

describe("docDecorations — block scoping", () => {
  it("decorates nothing outside a doc block", () => {
    assert.deepStrictEqual(decos("# not a heading\n**not bold**"), []);
  });

  it("decorates a heading inside a block, not the markers", () => {
    // `---`(0) `# Title`(1) `---`(2). Heading colours from `#` to EOL.
    assert.deepStrictEqual(decos(block("# Title")), ["heading@1:0-7"]);
  });

  it("handles two separate blocks with code between them", () => {
    const text = [
      "---",
      "# One",
      "---",
      "fn f() {}",
      "---",
      "# Two",
      "---",
    ].join("\n");
    assert.deepStrictEqual(decos(text), ["heading@1:0-5", "heading@5:0-5"]);
  });

  it("drops speculative decorations inside an unclosed final block", () => {
    // Second `---` opens a block that never closes → its body is dropped.
    const text = ["---", "# Kept", "---", "code", "---", "# Dropped"].join("\n");
    assert.deepStrictEqual(decos(text), ["heading@1:0-6"]);
  });

  it("treats a `---` inside a would-be block as the closing marker", () => {
    // This mirrors the lexer: a `---` line always terminates the block, so the
    // text after it is code, not doc — nothing there is decorated.
    const text = ["---", "**doc**", "---", "**code**"].join("\n");
    assert.deepStrictEqual(decos(text), ["strong@1:0-7"]);
  });

  it("accepts markers with extra dashes and surrounding whitespace", () => {
    const text = ["  ----  ", "*hi*", "----"].join("\n");
    assert.deepStrictEqual(decos(text), ["emphasis@1:0-4"]);
  });

  it("closes only on a marker of the opener's length (#1885)", () => {
    // The `---` lines are body text of the `----` block, so the heading after
    // them is still doc; the code after the `----` close is not.
    const text = ["----", "---", "# Inner", "---", "----", "**code**"].join(
      "\n",
    );
    assert.deepStrictEqual(decos(text), ["heading@2:0-7"]);
  });

  it("treats a block whose closer has another length as unclosed", () => {
    const text = ["----", "# Dropped", "---"].join("\n");
    assert.deepStrictEqual(decos(text), []);
  });
});

describe("docDecorations — `--- text` doc lines (#2203)", () => {
  it("decorates a doc line's text at its own columns", () => {
    // `  --- ` is a 6-column prefix; `**hi**` follows it.
    assert.deepStrictEqual(decos("  --- **hi** there"), ["strong@0:6-12"]);
    assert.deepStrictEqual(decos("--- # Title"), ["heading@0:4-11"]);
  });

  it("leaves the closer out of a heading", () => {
    assert.deepStrictEqual(decos("--- # Title ---"), ["heading@0:4-11"]);
    // A run of another length is text.
    assert.deepStrictEqual(decos("--- # Title ----"), ["heading@0:4-16"]);
  });

  it("leaves a `//` comment, a `---` after code and a retired `--|` alone", () => {
    assert.deepStrictEqual(decos("// **not a doc**"), []);
    assert.deepStrictEqual(decos("x: Int, --- **not a doc**"), []);
    assert.deepStrictEqual(decos("--| **not a doc**"), []);
    assert.deepStrictEqual(decos("---**not a doc**"), []);
  });
});

describe("docDecorations — headings", () => {
  it("colours from the first `#`, excluding leading indent", () => {
    assert.deepStrictEqual(decos(block("   ## Indented")), ["heading@1:3-14"]);
  });

  it("requires whitespace after the hashes (`#Nope` is not a heading)", () => {
    assert.deepStrictEqual(decos(block("#Nope")), []);
  });

  it("supports a bare `#` at end of line", () => {
    assert.deepStrictEqual(decos(block("#")), ["heading@1:0-1"]);
  });

  it("does not treat 7+ hashes as a heading", () => {
    assert.deepStrictEqual(decos(block("####### too deep")), []);
  });

  it("does not also emphasise inside a heading line", () => {
    // Heading owns its line — the `**strong**` is not separately decorated.
    assert.deepStrictEqual(decos(block("# A **b** c")), ["heading@1:0-11"]);
  });
});

describe("docDecorations — strong and emphasis", () => {
  const cases: [string, string[]][] = [
    ["**bold**", ["strong@1:0-8"]],
    ["*italic*", ["emphasis@1:0-8"]],
    ["__bold__", ["strong@1:0-8"]],
    ["_italic_", ["emphasis@1:0-8"]],
    ["a **b** c *d* e", ["strong@1:2-7", "emphasis@1:10-13"]],
    // Emphasis mid-line keeps correct columns.
    ["see **this** now", ["strong@1:4-12"]],
  ];
  for (const [body, expected] of cases) {
    it(`renders ${JSON.stringify(body)}`, () => {
      assert.deepStrictEqual(decos(block(body)), expected);
    });
  }
});

describe("docDecorations — emphasis false positives (must NOT fire)", () => {
  const noFire: string[] = [
    "snake_case_name", // `_` inside a word: not italic
    "a_b_c",
    "* bullet item", // `* ` with trailing space: not emphasis
    "a * b * c", // arithmetic-looking prose: whitespace-hugging
    "** empty **", // whitespace right inside the delimiters
    "just one * asterisk",
    "trailing_", // no right-hand content
    "____", // empty strong
  ];
  for (const body of noFire) {
    it(`does not decorate ${JSON.stringify(body)}`, () => {
      assert.deepStrictEqual(decos(block(body)), []);
    });
  }
});

describe("docDecorations — UTF-16 columns", () => {
  it("counts astral emoji as their UTF-16 width before an emphasis run", () => {
    // "😀" is 2 UTF-16 code units, so `*x*` starts at column 3 (emoji + space).
    const body = "😀 *x*";
    const [r] = docDecorations(block(body));
    assert.strictEqual(r.kind as DocDecorationKind, "emphasis");
    assert.strictEqual(r.startChar, 3);
    assert.strictEqual(r.endChar, 6);
  });
});

describe("docDecorations — CRLF", () => {
  it("ends a heading range before a trailing CR", () => {
    const text = "---\r\n# Title\r\n---\r\n";
    assert.deepStrictEqual(decos(text), ["heading@1:0-7"]);
  });
});
