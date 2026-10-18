// #2098: unit coverage for `explain-render` (src/webview/explain-render.ts),
// the pure core of the "Bynk: Explanation" webview — the heading, the rendered
// blurb and example, the "Read more" link, and the HTML-safety posture it
// inherits from `doc-render`.

import * as assert from "assert";

import { createMarkdownRenderer } from "../../src/webview/doc-render";
import { escapeHtml, renderExplanationHtml } from "../../src/webview/explain-render";
import type { Explanation } from "../../src/webview/explainTypes";

// The shape `bynk/explain` returns for `bynk.resolve.unknown_type`.
const EXPLANATION: Explanation = {
  title: "bynk.resolve.unknown_type",
  markdown:
    "A type name was referenced that does not exist.\n\n## Example\n\n" +
    "```bynk\nfn greet(u: Usr) -> Text { … }   // ✗ no type `Usr`\n```\n",
  href: "https://bynk-lang.org/book/reference/types/",
};

describe("explain-render", () => {
  it("renders the code as the heading, the blurb, and the example as a code block", () => {
    const html = renderExplanationHtml(createMarkdownRenderer(), EXPLANATION);
    assert.ok(html.startsWith('<h1 class="code"><code>bynk.resolve.unknown_type</code></h1>'), html);
    assert.ok(html.includes("<p>A type name was referenced that does not exist.</p>"), html);
    assert.ok(html.includes("<h2>Example</h2>"), html);
    assert.ok(html.includes('<pre><code class="language-bynk">'), html);
    assert.ok(html.includes("fn greet(u: Usr) -&gt; Text"), html);
  });

  it("links the Book page as 'Read more'", () => {
    const html = renderExplanationHtml(createMarkdownRenderer(), EXPLANATION);
    assert.ok(
      html.endsWith(
        '<p class="more"><a href="https://bynk-lang.org/book/reference/types/">Read more in the Book</a></p>',
      ),
      html,
    );
  });

  it("omits the link for a non-http(s) href", () => {
    const html = renderExplanationHtml(createMarkdownRenderer(), {
      ...EXPLANATION,
      href: "command:workbench.action.quit",
    });
    assert.ok(!html.includes("<a "), html);
  });

  it("never passes raw HTML through, in the title or the Markdown", () => {
    const html = renderExplanationHtml(createMarkdownRenderer(), {
      title: "<img src=x onerror=alert(1)>",
      markdown: "Danger <script>alert(2)</script>",
      href: 'https://example.com/"onmouseover="x',
    });
    assert.ok(!html.includes("<img"), html);
    assert.ok(!html.includes("<script>"), html);
    assert.ok(!html.includes('"onmouseover="'), html);
  });

  it("escapes every HTML-significant character", () => {
    assert.strictEqual(escapeHtml(`<a href="x">'&'</a>`), "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;");
  });
});
