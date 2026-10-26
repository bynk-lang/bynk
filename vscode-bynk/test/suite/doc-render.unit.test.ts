// #847: unit coverage for `doc-render` (src/webview/doc-render.ts) — the pure
// core of the documentation webview, testable without a live webview/DOM,
// exactly as `mermaid-gen.unit.test.ts` covers #846. Rendered-page correctness
// (heading click-to-code, the undocumented toggle, link interception) needs a
// real browser and is out of reach here; this pins the Markdown render's
// security posture and the small pure helpers.

import * as assert from "assert";

import {
  createMarkdownRenderer,
  docLinkIndex,
  headingTag,
  isExternalHttpLink,
  renderDocMarkdown,
} from "../../src/webview/doc-render";

describe("doc-render", () => {
  it("renders basic Markdown (emphasis, inline code)", () => {
    const md = createMarkdownRenderer();
    const html = renderDocMarkdown(md, "A **bold** word and `code`.");
    assert.ok(html.includes("<strong>bold</strong>"), html);
    assert.ok(html.includes("<code>code</code>"), html);
  });

  it("renders a fenced signature block as a code block", () => {
    const md = createMarkdownRenderer();
    const html = renderDocMarkdown(md, "```bynk\nservice Api {\n}\n```\n");
    assert.ok(html.includes("<pre>"), html);
    assert.ok(html.includes("<code"), html);
    assert.ok(html.includes("service Api"), html);
  });

  it("does NOT pass raw HTML from a doc comment through to markup (html: false)", () => {
    const md = createMarkdownRenderer();
    // A doc comment containing a script/img must never reach the DOM as markup.
    const html = renderDocMarkdown(
      md,
      'Danger: <script>alert(1)</script> and <img src=x onerror=alert(2)>.',
    );
    assert.ok(!html.includes("<script>"), html);
    assert.ok(!html.includes("<img"), html);
    // It is rendered as escaped text instead.
    assert.ok(html.includes("&lt;script&gt;"), html);
  });

  it("does NOT auto-linkify a bare URL (linkify: false)", () => {
    const md = createMarkdownRenderer();
    const html = renderDocMarkdown(md, "See https://example.com for details.");
    assert.ok(!html.includes("<a "), html);
  });

  it("renders an explicit Markdown link as an anchor (gated at click time)", () => {
    const md = createMarkdownRenderer();
    const html = renderDocMarkdown(md, "See [the docs](https://example.com).");
    assert.ok(html.includes('href="https://example.com"'), html);
  });

  it("renders a CommonMark <url> autolink as an anchor even with linkify off", () => {
    // `linkify: false` only suppresses *bare* URLs; an angle-bracket autolink is
    // core CommonMark. It still flows through the same http(s) click gate.
    const md = createMarkdownRenderer();
    const html = renderDocMarkdown(md, "See <https://example.com>.");
    assert.ok(html.includes('href="https://example.com"'), html);
    assert.ok(isExternalHttpLink("https://example.com"));
  });

  it("maps nesting depth to heading tags, clamped at h4", () => {
    assert.strictEqual(headingTag(0), "h2");
    assert.strictEqual(headingTag(1), "h3");
    assert.strictEqual(headingTag(2), "h4");
    assert.strictEqual(headingTag(5), "h4");
  });

  it("admits only absolute http(s) links to the external allow-list", () => {
    assert.ok(isExternalHttpLink("http://example.com"));
    assert.ok(isExternalHttpLink("https://example.com/x"));
    assert.ok(!isExternalHttpLink("command:bynk.doThing"));
    assert.ok(!isExternalHttpLink("file:///etc/passwd"));
    assert.ok(!isExternalHttpLink("/relative/path"));
    assert.ok(!isExternalHttpLink("vscode://extension"));
  });

  it("renders a resolved intra-doc link as an anchor and recovers its index (#2462)", () => {
    const md = createMarkdownRenderer();
    const html = renderDocMarkdown(md, "See [Limiter](#bynk-doc-link-3).");
    assert.ok(html.includes('<a href="#bynk-doc-link-3">Limiter</a>'), html);
    assert.strictEqual(docLinkIndex("#bynk-doc-link-3"), 3);
    assert.strictEqual(docLinkIndex("#other"), null);
    assert.strictEqual(docLinkIndex("https://example.com/#bynk-doc-link-3"), null);
  });
});
