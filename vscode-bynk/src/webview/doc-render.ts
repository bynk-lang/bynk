// #847: the pure Markdown-rendering core of the documentation webview — the
// one piece testable without a live webview/DOM, exactly as `mermaid-gen.ts` is
// for #846. No `vscode`, no DOM: it configures a vendored `markdown-it` and
// renders a doc entry's Markdown to a sanitised HTML string.
//
// Security posture (issue #847's "untrusted Markdown in doc comments" risk):
// `html: false` drops any raw HTML in a doc comment to text, so a `<script>`
// or `<img onerror=…>` written in a `---` block can never reach the DOM as
// markup. `linkify: false` means a *bare* URL stays literal text (an explicit
// `[text](url)` or a CommonMark `<url>` autolink still becomes an anchor).
// Whatever renders as a link, the webview gates its clicks through the host's
// http(s) allow-list (`webviewHost.openExternalHref`) — so the security posture
// does not depend on how few things markdown-it turns into anchors. Combined
// with the page's `default-src 'none'` CSP (no inline scripts, no external
// fetches), the render cannot execute or exfiltrate.

// markdown-it 15 ships its own declarations (no `@types/markdown-it`): the
// default export is a callable factory, and `MarkdownIt` is the instance type.
import markdownIt, { type MarkdownIt } from "markdown-it";

/** A `markdown-it` configured for the doc webview: HTML disabled, no
 *  auto-linkification of bare URLs (an explicit `[text](url)`/`<url>` still
 *  links — every link is gated at click time regardless), typographic
 *  replacements off (a doc comment's literal punctuation should render
 *  verbatim). */
export function createMarkdownRenderer(): MarkdownIt {
  return markdownIt({
    html: false,
    linkify: false,
    typographer: false,
    breaks: false,
  });
}

/** Render a doc entry's Markdown (a fenced `bynk` signature, optionally
 *  followed by doc-comment prose) to an HTML string, HTML-disabled. */
export function renderDocMarkdown(md: MarkdownIt, source: string): string {
  return md.render(source);
}

/** The heading tag for a declaration at nesting `depth` — the page title is an
 *  `h1`, top-level declarations `h2`, their members `h3`, deeper members clamp
 *  at `h4` (HTML has no `h7`, and the page never nests that far anyway). */
export function headingTag(depth: number): string {
  const level = Math.min(2 + depth, 4);
  return `h${level}`;
}

/** Is `href` a link the host may open externally? Only absolute http(s) — the
 *  same allow-list `webviewHost.openExternalHref` enforces, checked here too so
 *  the webview never even posts a `command:`/`file:` href. Mirrored, not shared,
 *  because this module is browser-context (no `vscode.Uri`). */
export function isExternalHttpLink(href: string): boolean {
  return /^https?:\/\//i.test(href);
}

/** #2462: the index `N` of a `#bynk-doc-link-N` href the server wrote for a
 *  resolved intra-doc link, or `null` for any other href. */
export function docLinkIndex(href: string): number | null {
  const m = /^#bynk-doc-link-(\d+)$/.exec(href);
  return m ? Number(m[1]) : null;
}
