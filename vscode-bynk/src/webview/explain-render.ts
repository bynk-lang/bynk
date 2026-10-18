// #2098: the pure rendering core of the "Bynk: Explanation" webview — no
// `vscode`, no DOM, so the unit tests can pin it. It reuses the documentation
// view's Markdown renderer (`doc-render.ts`), and so its security posture:
// raw HTML in the Markdown renders as text, and the only link is the "Read
// more" one built here from `href`, emitted only for an http(s) URL. Clicks on
// it are still gated through the host's allow-list (`openExternalHref`).

import type { MarkdownIt } from "markdown-it";

import { isExternalHttpLink, renderDocMarkdown } from "./doc-render";
import type { Explanation } from "./explainTypes";

/** Escape text for an HTML text node or a double-quoted attribute. */
export function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

/** The explanation page body: the code as the heading, the rendered blurb and
 *  example, then "Read more in the Book" for an http(s) `href`. */
export function renderExplanationHtml(md: MarkdownIt, e: Explanation): string {
  const heading = `<h1 class="code"><code>${escapeHtml(e.title)}</code></h1>`;
  const body = `<div class="body">${renderDocMarkdown(md, e.markdown)}</div>`;
  const more = isExternalHttpLink(e.href)
    ? `<p class="more"><a href="${escapeHtml(e.href)}">Read more in the Book</a></p>`
    : "";
  return heading + body + more;
}
