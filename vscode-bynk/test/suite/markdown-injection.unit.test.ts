// #2099: ```bynk fences in Markdown are highlighted by a TextMate injection
// grammar. The extension has no grammar-tokenizer harness (and VS Code exposes
// no token API to the extension host), so the contract pinned here is the
// declarative one: the manifest wires the injection into Markdown and maps the
// embedded scope to the `bynk` language, and the grammar file targets
// `text.html.markdown`, recognises both fence characters with the `bynk` info
// string only (DECISION A), and embeds `source.bynk`.
//
// It reads the files from disk rather than through `vscode.extensions`, so it
// needs no extension host.

import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";

// out/test/suite -> the extension root.
const ROOT = path.resolve(__dirname, "../../..");

interface GrammarContribution {
  language?: string;
  scopeName: string;
  path: string;
  injectTo?: string[];
  embeddedLanguages?: Record<string, string>;
}

interface Rule {
  begin?: string;
  end?: string;
  while?: string;
  name?: string;
  contentName?: string;
  include?: string;
  patterns?: Rule[];
  beginCaptures?: Record<string, { name: string }>;
  endCaptures?: Record<string, { name: string }>;
}

function readJson<T>(rel: string): T {
  return JSON.parse(fs.readFileSync(path.join(ROOT, rel), "utf8")) as T;
}

function injection(): GrammarContribution {
  const pkg = readJson<{ contributes: { grammars: GrammarContribution[] } }>("package.json");
  const entry = pkg.contributes.grammars.find((g) => g.scopeName === "markdown.bynk.codeblock");
  assert.ok(entry, "package.json contributes the markdown.bynk.codeblock grammar");
  return entry;
}

function fenceRule(): Rule {
  const g = readJson<{
    scopeName: string;
    injectionSelector: string;
    patterns: Rule[];
    repository: Record<string, Rule>;
  }>(injection().path);
  assert.strictEqual(g.scopeName, "markdown.bynk.codeblock");
  assert.deepStrictEqual(g.patterns, [{ include: "#fenced_code_block_bynk" }]);
  const rule = g.repository.fenced_code_block_bynk;
  assert.ok(rule, "the grammar defines fenced_code_block_bynk");
  return rule;
}

describe("Markdown injection grammar (#2099)", () => {
  it("is an injection into Markdown, not a language grammar", () => {
    const entry = injection();
    assert.strictEqual(entry.language, undefined, "an injection grammar names no language");
    assert.deepStrictEqual(entry.injectTo, ["text.html.markdown"]);
    assert.ok(
      fs.existsSync(path.join(ROOT, entry.path)),
      `${entry.path} exists (vsce would otherwise package a dangling grammar)`,
    );
  });

  it("maps the embedded block to the bynk language", () => {
    // Without this, comment toggling and bracket matching inside the fence
    // use Markdown's configuration instead of Bynk's.
    assert.deepStrictEqual(injection().embeddedLanguages, {
      "meta.embedded.block.bynk": "bynk",
    });
  });

  it("injects with left priority into text.html.markdown", () => {
    const g = readJson<{ injectionSelector: string }>(injection().path);
    // `L:` makes the rule win over Markdown's own fenced_code_block_unknown.
    assert.strictEqual(g.injectionSelector, "L:text.html.markdown");
  });

  it("opens on ``` and ~~~ fences with the bynk info string only", () => {
    const rule = fenceRule();
    assert.strictEqual(rule.name, "markup.fenced_code.block.markdown");
    assert.ok(rule.begin, "the fence rule has a begin pattern");
    assert.ok(rule.begin.includes("(`{3,}|~{3,})"), `both fence characters: ${rule.begin}`);
    // DECISION A: the language id, no aliases.
    assert.ok(rule.begin.includes("(?i:(bynk)("), `bynk is the only identifier: ${rule.begin}`);
    assert.strictEqual(rule.beginCaptures?.["4"]?.name, "fenced_code.block.language.markdown");
    // The close must repeat the opening fence (backreference), so a ~~~ block
    // is not closed by ``` and vice versa.
    assert.ok(rule.end?.includes("(\\3)"), `closing fence backreferences the opener: ${rule.end}`);
  });

  it("embeds source.bynk as meta.embedded.block.bynk", () => {
    const body = fenceRule().patterns ?? [];
    assert.strictEqual(body.length, 1);
    assert.strictEqual(body[0].contentName, "meta.embedded.block.bynk");
    assert.deepStrictEqual(body[0].patterns, [{ include: "source.bynk" }]);
  });

  it("embeds the scope the main grammar actually declares", () => {
    const main = readJson<{ scopeName: string }>("syntaxes/bynk.tmLanguage.json");
    assert.strictEqual(main.scopeName, "source.bynk");
  });
});
