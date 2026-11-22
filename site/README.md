# Bynk Site — The documentation & web presence for Bynk

An [Astro](https://astro.build/) + [Starlight](https://starlight.astro.build/) static site. It replaced the former mdBook book and deploys to `bynk-lang.org`. The framework decision is recorded in ADR 0141 (`../design/decisions/0141-documentation-framework.md`).

## What it holds

- **The Book** (`src/content/docs/book/`): the tutorials, concepts and guides. Its contents are `src/SUMMARY.md`.
- **Bynk by Example** (`src/content/docs/by-example/`): project pages built from `../examples/` and a snippets tier. The `<Example>` component reads real source, so a page cannot drift from what the compiler checks, and runnable blocks link to (or embed) the playground.
- **The Reference** (`src/content/docs/reference/`): the language reference, architecture, adapters, tooling (the CLI, manifest, emission and each tool, plus generated crate API docs), the diagnostics, the normative specification and the releases. Its contents are `src/REFERENCE.md`.

Every fenced `bynk` block is compiled by the `doc_examples` gate in `bynk-tests/tests/`, so the code shown is checked by CI.

## How it fits together

- **Brand.** The colours, fonts, header, wordmark and code themes come from [`@bynk-lang/site-theme`](https://github.com/bynk-lang/site-theme), a Starlight plugin shared with the blog and published to GitHub Packages. The site keeps only its `Sidebar` override (scoped to the surface being viewed) and `src/styles/site.css` (the landing hero). A brand change is made in the theme and taken here as a version bump.
- **Highlighting.** ` ```bynk ` code fences are highlighted by Shiki/Expressive Code, fed a TextMate grammar at `src/grammars/bynk.tmLanguage.json` (scope `source.bynk`). It is a copy of the VS Code extension's grammar ([bynk-lang/vscode-bynk](https://github.com/bynk-lang/vscode-bynk)); `bynkc`'s `textmate_keywords` test holds it to the reserved keywords, and it is refreshed from the extension when that changes. The theme ships the same grammar; the site's copy is the one registered, and CI fails if the two differ. So a grammar change releases the theme first, then bumps it here and refreshes the site's copy in one pull request.
- **Search.** [Pagefind](https://pagefind.app/) — local and static, built automatically by `astro build`. On bynk-lang.org it also searches the blog's index (the deploy sets `BYNK_MERGE_SEARCH`; local builds search this site alone), and the blog's search covers this site. Keep both on the same Pagefind minor version: CI warns when they differ, and the deploy then leaves the merge off.
- **Link checking.** A build-time internal link check (`starlight-links-validator`) fails the build on a broken in-site link.
- **Toolchain.** Astro 7 + Starlight, Node ≥ 22.12 (Astro 7's floor). A committed `package-lock.json` pins it.

## Local development

- `npm install` — install deps. The theme is on GitHub Packages, which needs a token even to read: put a classic token with only `read:packages` in your user-level `~/.npmrc` as `//npm.pkg.github.com/:_authToken=YOUR_TOKEN` (`site/.npmrc` already points `@bynk-lang` there). CI and the deploy use the workflow token (`packages: read`); Dependabot uses the `PACKAGES_READ_TOKEN` Dependabot secret.
- `npm run dev` — Astro dev server (it prints the local URL — `http://localhost:4321` by default).
- `npm run build` — static build into `site/dist/`; this also builds the Pagefind index and runs the internal link-checker.
- `npm run preview` — serve the built `dist/` locally.

## Deploy

The deploy is automated by `.github/workflows/deploy-site.yml`. It builds `site/dist/` and uploads it with `wrangler pages deploy` to a single Cloudflare Pages project. It runs on push to `main` (when `site/**` or the tmLanguage grammar changes) and on manual `workflow_dispatch` (Actions tab → "Deploy the docs site" → Run workflow).

The maintainer does a one-time account setup:

1. Create a Cloudflare **Pages** project of type **Direct Upload** named **`bynk-lang`** — this name is what the workflow's `--project-name` flag targets; keep them in sync if you rename.
2. Attach the custom domain `bynk-lang.org` (the apex) to the `bynk-lang` project. Cloudflare's custom-domain flow creates the DNS record when the zone is Cloudflare-managed.
3. The Cloudflare secrets — `CLOUDFLARE_API_TOKEN` (scoped to Account → Cloudflare Pages → Edit) and `CLOUDFLARE_ACCOUNT_ID` — are already configured as repo secrets (first added for the playground deploy, which now lives in [bynk-lang/playground](https://github.com/bynk-lang/playground)). No new secrets are needed.
4. Trigger the first deploy — push to `main`, or run the workflow manually.

Until the project and secrets exist, the workflow still builds `site/dist/` and simply skips the upload — it reports a notice rather than failing.