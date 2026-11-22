// @ts-check
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import starlightLinksValidator from "starlight-links-validator";
// The Bynk brand, shared with the blog (web presence slice 6): the design-system
// tokens, Starlight dressed in them, the header and wordmark, the fonts and the
// code themes. See bynk-lang/site-theme.
import bynkSiteTheme from "@bynk-lang/site-theme";
// Bynk highlighting is a TextMate grammar (scopeName: source.bynk), which Shiki
// consumes directly. This is a copy of the VS Code extension's grammar
// (bynk-lang/vscode-bynk, syntaxes/bynk.tmLanguage.json), kept here so the site
// builds without the extension's repository. the `textmate_keywords` test
// holds it to the reserved keywords; refresh it from the extension when its
// grammar changes.
import bynkGrammar from "./src/grammars/bynk.tmLanguage.json" with { type: "json" };
// Expand the mdBook authoring directives ({{#grammar}}, {{#grammar-semantics}},
// {{#include}}, [!KIND] callouts) so the Book's source renders under Starlight.
import remarkBynkDirectives from "./src/plugins/remark-bynk-directives.mjs";
// Render the Book's ```mermaid diagrams to inline SVG at build time (offline, no
// client JS). Needs a headless Chromium (CI runs `playwright install chromium`).
import rehypeMermaid from "rehype-mermaid";
// The Book sidebar, generated from the mdBook SUMMARY.md by
// `scripts/generate-sidebar.mjs` (CI runs `--check` to guard drift).
import bookSidebar from "./src/generated/sidebar.json" with { type: "json" };
// The Reference sidebar, generated from src/REFERENCE.md the same way (web
// presence slice 5): the language reference, architecture, adapters, tooling,
// diagnostics, the specification and the releases.
import referenceSidebar from "./src/generated/reference-sidebar.json" with { type: "json" };

// `{{#include}}` diagnostics fixtures live in `site/src/diagnostics/` (the
// remark plugin resolves the `diagnostics/…` suffix against this base).
const includeBase = fileURLToPath(new URL("./src", import.meta.url));

// The diagrams wear the design system. Mermaid starts from its neutral `base`
// theme, and src/styles/mermaid.css goes in as `themeCSS`: Mermaid scopes it to
// each diagram and puts it after its own rules, and its values are the
// `--bynk-*` tokens, so an inlined diagram follows the page's light or dark
// theme, Starlight's toggle included. (rehype-mermaid's `dark` option would
// emit a light and a dark image under a `prefers-color-scheme` media query,
// which ignores the toggle, and only for the `img-*` strategies.)
//
// Mermaid sizes each box to its label in the headless browser at build time,
// so it must measure in the face the page draws: Instrument Sans, loaded there
// from the npm package (offline) rather than the page's Google Fonts link.
const mermaid = {
  strategy: "inline-svg",
  css: import.meta.resolve("@fontsource-variable/instrument-sans/index.css"),
  mermaidConfig: {
    theme: "base",
    fontFamily: '"Instrument Sans Variable", sans-serif',
    themeCSS: readFileSync(new URL("./src/styles/mermaid.css", import.meta.url), "utf8"),
  },
};

// https://astro.build/config
export default defineConfig({
  site: "https://bynk-lang.org",
  // `/docs/cli/` (the old `bynkc` page, removed in E3-4) is redirected in
  // public/_redirects, before the `/docs/*` splat that moved Developer Docs.
  markdown: {
    remarkPlugins: [[remarkBynkDirectives, { includeBase }]],
    rehypePlugins: [[rehypeMermaid, mermaid]],
  },
  integrations: [
    starlight({
      title: "Bynk",
      favicon: "/favicon.svg",
      // The brand comes from the theme plugin below; this adds only the
      // site's own touches (the landing hero).
      customCss: ["./src/styles/site.css"],
      // The theme supplies the Head (fonts), Header (wordmark and cross-site
      // nav) and SiteTitle. The Sidebar stays the site's own: it is scoped to
      // the surface being viewed (Book / By Example / Reference) instead of
      // listing every surface on every page.
      components: {
        Sidebar: "./src/components/Sidebar.astro",
      },
      // Build-time internal link checking (the link-check gate): a broken
      // in-site link fails `astro build`. The whole Book is migrated, so it is
      // validated strictly along with everything else.
      // `/reference/api/` is the generated rustdoc tree — built by `cargo doc` in the
      // deploy workflow and copied into `public/reference/api/` (gitignored, absent from
      // local builds), so the internal link-checker must not try to resolve into it.
      // A diagnostic code's page (`src/pages/reference/diagnostics/[code].astro`)
      // is a custom page, which the validator cannot see, so links to one are
      // excluded too. The page is built for every registry code, and the
      // `diagnostics_registry` docs-consistency test checks that every such link
      // in the site's content names a registry code, so none can dangle.
      plugins: [
        bynkSiteTheme(),
        starlightLinksValidator({
          exclude: ["/reference/api/**", "/reference/diagnostics/*/"],
        }),
      ],
      // Faithful `bynk` highlighting from the editor's own grammar (the site's
      // copy wins over the theme's; CI checks the two are the same file); the fenced
      // language id is `bynk` (the grammar's own name is the display "Bynk").
      // #2958: a tagged multi-line string (`"""sql`) embeds that language's
      // grammar, so the languages the grammar includes load with it.
      expressiveCode: {
        shiki: {
          langs: [
            {
              ...bynkGrammar,
              name: "bynk",
              embeddedLangsLazy: ["sql", "json", "html", "css", "javascript"],
            },
          ],
        },
      },
      social: [{ icon: "github", label: "GitHub", href: "https://github.com/bynk-lang/bynk" }],
      // The Book sidebar is generated from src/SUMMARY.md and the Reference
      // sidebar from src/REFERENCE.md; By Example appends its own group:
      // snippets first (the gentle tier), then the project gallery.
      sidebar: [
        ...bookSidebar,
        {
          label: "By Example",
          items: [
            { label: "Overview", link: "/by-example/" },
            { label: "Snippets", items: [{ autogenerate: { directory: "by-example/snippets" } }] },
            { label: "Projects", items: [{ autogenerate: { directory: "by-example/projects" } }] },
          ],
        },
        ...referenceSidebar,
      ],
    }),
  ],
});
