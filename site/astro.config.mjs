// @ts-check
import { fileURLToPath } from "node:url";
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import starlightLinksValidator from "starlight-links-validator";
// The single source of truth for Bynk highlighting is the VS Code TextMate
// grammar — the same file the editor uses (scopeName: source.bynk). Shiki
// consumes it directly, so the site never maintains a second highlighter.
import bynkGrammar from "../vscode-bynk/syntaxes/bynk.tmLanguage.json" with { type: "json" };
// Expand the mdBook authoring directives ({{#grammar}}, {{#grammar-semantics}},
// {{#include}}, [!KIND] callouts) so the Book's source renders under Starlight.
import remarkBynkDirectives from "./src/plugins/remark-bynk-directives.mjs";
// Render the Book's ```mermaid diagrams to inline SVG at build time (offline, no
// client JS). Needs a headless Chromium (CI runs `playwright install chromium`).
import rehypeMermaid from "rehype-mermaid";
// The Book sidebar, generated from the mdBook SUMMARY.md by
// `scripts/generate-sidebar.mjs` (CI runs `--check` to guard drift).
import bookSidebar from "./src/generated/sidebar.json" with { type: "json" };

// `{{#include}}` diagnostics fixtures live in `site/src/diagnostics/` (the
// remark plugin resolves the `diagnostics/…` suffix against this base).
const includeBase = fileURLToPath(new URL("./src", import.meta.url));

// https://astro.build/config
export default defineConfig({
  site: "https://bynk-lang.org",
  markdown: {
    remarkPlugins: [[remarkBynkDirectives, { includeBase }]],
    rehypePlugins: [[rehypeMermaid, { strategy: "inline-svg" }]],
  },
  // Web fonts (Inter for body/UI, Literata for the wordmark and headings) are
  // loaded via a stylesheet <link> in the Head override rather than Astro's
  // `fonts` API: that API's dev pipeline runs during the dev server's content
  // initialisation and left the `docs` collection empty (dev-only 404s), while a
  // plain <link> behaves identically across dev, build, and sync. See
  // src/components/Head.astro and the font bindings in src/styles/brand.css.
  integrations: [
    starlight({
      title: "Bynk",
      // The title is plain text — the word "Bynk" set in the site's display face
      // (Literata) — rather than a bespoke stylised mark; the typographic
      // treatment lives in src/styles/brand.css.
      favicon: "/favicon.svg",
      // The brand: one electric-indigo accent (rust/ayu stay reserved for code).
      customCss: ["./src/styles/brand.css"],
      // Component overrides: the Header adds the persistent cross-surface nav
      // (track §1.1); the Sidebar is scoped to the surface being viewed (Book /
      // By Example / Developer Docs) instead of listing every surface on every
      // page; the Head emits the self-hosted @font-face declarations.
      components: {
        Header: "./src/components/Header.astro",
        Sidebar: "./src/components/Sidebar.astro",
        Head: "./src/components/Head.astro",
      },
      // Build-time internal link checking (the link-check gate): a broken
      // in-site link fails `astro build`. The whole Book is migrated, so it is
      // validated strictly along with everything else.
      // `/docs/api/` is the generated rustdoc tree — built by `cargo doc` in the
      // deploy workflow and copied into `public/docs/api/` (gitignored, absent from
      // local builds), so the internal link-checker must not try to resolve into it.
      plugins: [starlightLinksValidator({ exclude: ["/docs/api/**"] })],
      // Faithful `bynk` highlighting from the editor's own grammar; the fenced
      // language id is `bynk` (the grammar's own name is the display "Bynk").
      expressiveCode: {
        shiki: { langs: [{ ...bynkGrammar, name: "bynk" }] },
      },
      social: [{ icon: "github", label: "GitHub", href: "https://github.com/bynk-lang/bynk" }],
      // The Book sidebar is generated from src/SUMMARY.md; the By Example and
      // Developer Docs surfaces (their own surfaces, not part of the Book) each
      // append their own group. By Example: snippets first (the gentle tier),
      // then the project gallery. Developer Docs: a curated order over the
      // toolchain content re-homed out of the Book's reference/guides/tooling.
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
        {
          label: "Developer Docs",
          items: [
            { label: "Overview", link: "/docs/" },
            {
              label: "Command-line tools",
              items: [
                { label: "CLI (bynkc)", link: "/docs/cli/" },
                { label: "CLI (bynk driver)", link: "/docs/bynk-cli/" },
              ],
            },
            {
              label: "Project & output",
              items: [
                { label: "bynk.toml manifest", link: "/docs/manifest/" },
                { label: "Registry protocol", link: "/docs/registry-protocol/" },
                { label: "Emission", link: "/docs/emission/" },
              ],
            },
            {
              label: "Editor & tooling",
              items: [
                { label: "Overview", link: "/docs/editor-and-tooling/" },
                { label: "Check your environment with bynk doctor", link: "/docs/editor-and-tooling/doctor/" },
                { label: "Format your code with bynk-fmt", link: "/docs/editor-and-tooling/format/" },
                { label: "Set up editor support", link: "/docs/editor-and-tooling/editor-support/" },
                { label: "Debug in VS Code", link: "/docs/editor-and-tooling/debugging/" },
              ],
            },
            {
              label: "Tool reference",
              items: [
                { label: "Overview", link: "/docs/tooling/" },
                { label: "bynk-fmt", link: "/docs/tooling/bynk-fmt/" },
                { label: "bynk-lsp", link: "/docs/tooling/bynk-lsp/" },
                { label: "tree-sitter-bynk", link: "/docs/tooling/tree-sitter-bynk/" },
                { label: "vscode-bynk", link: "/docs/tooling/vscode-bynk/" },
              ],
            },
          ],
        },
      ],
    }),
  ],
});
