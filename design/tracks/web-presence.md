# Web presence — the Book reorganised, Reference consolidated, one brand across docs and blog

- **Status:** Settled in #3859; the open questions were settled with the
  maintainer on 21 November 2026 (§14). Spine issue: #3861, which carries the
  live slice status.
- **Realises:** the brief of the retired documentation track and
  [ADR 0141](../decisions/0141-documentation-framework.md): *one coherent web
  presence under one domain and design system*. That track stood the site up;
  this one gives it a shape a reader can navigate and a brand that covers the
  blog too.
- **Posture:** Feature track per
  [ADR 0076](../decisions/0076-feature-track-posture.md). It qualifies on two
  axes (§1.2).
- **Governing records:** [ADR 0141](../decisions/0141-documentation-framework.md)
  (Astro + Starlight, Cloudflare Pages). Two ADRs land up front (§13): the
  information architecture, and the link policy (compiler links name codes,
  never pages).
- **Design reference:** page mockups for the home page, a Book chapter, an
  example, a reference page and a blog post, and boards for the Book audit and
  the proposed structure, are linked from the spine issue. They use the Bynk
  Playground design system (warm-green surfaces, amber and teal, Instrument
  Sans and JetBrains Mono).

## 1. The theme

### 1.1 What this track settles

bynk-lang.org has four kinds of reader content: a book you learn from, examples
you run, reference you look things up in, and a blog that explains the thinking.
Today they are arranged by history rather than by use. The Book holds all four
kinds of documentation plus the changelog, the specification, troubleshooting
and contributing notes; Developer Docs is a separate reference surface; the blog
is a second site with its own copy of the brand.

This track settles three things:

1. **The information architecture.** The site has four doors (Book, Examples,
   Reference, Blog). The Book is the journey: Start, Learn Bynk, Concepts,
   Guides. Everything looked up moves to Reference.
2. **The link policy.** The compiler links to documentation by diagnostic code,
   never by page, so the docs can be reorganised without touching the
   compiler. Moved pages get simple folder-level redirects.
3. **One brand.** A shared theme, built from the design system's tokens, used by
   the docs site and the blog.

### 1.2 Why a track (the ADR 0076 trigger)

- **It spans many increments.** Some slices are mechanical (contents, redirects,
  a generator change); four are new tutorial chapters.
- **Its surface needed settling.** The structure was a proposal with open
  questions; §14 records how they were settled.
- **It changes a contract between the compiler and the site.** Today the
  compiler holds a page and a heading anchor for each of its 802 curated
  explanations (§2.2). This track replaces them with one URL per diagnostic
  code that the site promises to serve.

## 2. What's true today

### 2.1 The Book, measured

Book and Developer Docs together: about 490,000 words across 220 pages
(November 2026, v0.640).

| Part | Pages | Words | Note |
|---|---:|---:|---|
| Reference (without changelog) | 30 | 135,000 | |
| Changelog | 1 | 108,000 | 22% of all words, one page |
| Guides | 114 | 97,000 | "Effects & capabilities" alone lists 29 |
| Specification | 15 | 70,000 | |
| Developer Docs | 19 | 53,000 | a separate surface (`/docs/`) |
| Troubleshooting | 21 | 7,700 | overlaps the Diagnostic index |
| Tutorials | 6 | 6,100 | about 1% of the words |
| About | 4 | 5,400 | |
| Contributing | 4 | 4,700 | compiler contributors, not readers |
| Getting started | 5 | 3,400 | includes "How these docs are organised" |

What gets in the way:

1. **The learning path stops early.** Six chapters, then 114 guide pages.
   Failure handling, capabilities, contexts and deployment are never taught in
   sequence.
2. **The changelog lives in Reference**, as the Book's largest page.
3. **Guide sections are uneven.** "Effects & capabilities" mixes the capability
   model and join semantics with recipes for R2, Postgres and Vectorize.
   "Events" has one page.
4. **Explanation is hard to find.** Philosophy, model and "Understand" pages sit
   inside how-to lists.
5. **18 pages are missing from `SUMMARY.md`**: 14 guides (among them *Handle
   money correctly*, *Accept a form*, *Roll back a deployment*) and 4
   troubleshooting pages.
6. **Two homes for one error.** Troubleshooting pages cover codes the generated
   Diagnostic index also covers.
7. **Reference is split** between the Book and Developer Docs.
8. **Other readers' material is mixed in**: compiler contributing, the roadmap,
   and a page that explains how the docs are organised.

### 2.2 What depends on Book paths

Moving pages is not only a site change. These read or emit Book paths:

| Dependant | Depends on |
|---|---|
| `bynk-syntax/src/diagnostics.rs` | 802 `Explain` entries, each with a `page` and `anchor` (about 610 into `/book/reference/`, 129 into `/book/guides/`, 50 into `/book/spec/`). `Explain::href` turns them into the editor's `codeDescription` link, the URL `bynk explain` prints and the MCP tools' links; `href` falls back to an anchor in the diagnostic index. Also generates `reference/diagnostics.md` |
| `bynk/src/book.rs`, `bynk/src/mcp/tools.rs` | `bynk book` builds page URLs from the Book bundled at build time, so they follow the content; the MCP tools hard-code the diagnostic index URL |
| `bynk-syntax/src/keywords.rs` | `reference/keywords.md` |
| `bynk-ide/src/first_party_docs.rs` | `reference/bynk-capabilities.md` and its URL |
| `xtask` (`stamp`, `release_notes`, `grammar`, `site_page`) | `reference/changelog.md`, `reference/upgrading.md`, `reference/grammar-appendix.md`, `/book/` |
| `xtask/tests/docs_consistency/*`, `xtask/tests/upgrading_page.rs` | diagnostics, keywords and grammar reference pages |
| `bynk-tests` (`doc_examples`, `types_reference_methods`, `wire_reference`) | the Book's code blocks and named reference pages |
| `site/scripts/*` | sidebar generation, `llms-full`, glossary, spec-section and British-English checks |
| `.github/workflows/ci.yml` | path filters on `site/src/content/docs/book/**` |
| `site/public/llms.txt` | hand-written `/book/` links |

The sidebar is generated from `site/src/SUMMARY.md`, not from directories.
**Regrouping the sidebar therefore moves no URL.** Only moving files does.

## 3. Goals and non-goals

**Goals**

- A reader can tell from the sidebar which part to read: learn in order,
  understand an idea, do a task, or look something up.
- Learn Bynk teaches every core concern once, in order, in one running project.
- Every page is in the contents, and a check keeps it that way.
- One home per diagnostic code: its meaning and its fix together.
- One Reference surface, covering the language and the tooling.
- The compiler never names a docs page. Its documentation links are
  `https://bynk-lang.org/d/<code>`, served by the site for every code, so a
  docs reorganisation needs no compiler change.
- Moving a folder does not break inbound links: the blog's 18 links into the
  Book and the repository's own links keep working.
- The docs site and the blog share one theme from one source, built from the
  design system's tokens, with search that covers both.

**Non-goals**

- Rewriting existing guide or reference prose beyond what a move needs.
- Versioned documentation (a version picker over past releases). Worth doing
  later; out of scope here.
- Merging the blog into this repository. It stays in `bynk-lang/blog`, served
  at `/blog/` by its proxy Worker.
- Redesigning the playground.

## 4. Target information architecture

### 4.1 The site

| Door | Path | Holds |
|---|---|---|
| Home | `/` | What Bynk is, the four doors, latest posts |
| Book | `/book/` | Start, Learn Bynk, Concepts, Guides |
| Examples | `/by-example/` | Snippets and projects (unchanged) |
| Reference | `/reference/` | Language, architecture, adapters, tooling, diagnostics, specification, releases |
| Blog | `/blog/` | Posts and series (`bynk-lang/blog`) |

Contributing, versioning and roadmap become Project pages linked from the
footer.

### 4.2 The Book

1. **Start**: What is Bynk? (absorbs *Why Bynk exists*), Anatomy of a Bynk
   service, Coming from TypeScript, Install.
2. **Learn Bynk**: ten chapters, one running project: the HTTP service begun in
   chapter 2 grows through every chapter after it, so the existing chapters
   stay and the four new ones extend the same code:
   1. Compile your first program
   2. Build a small HTTP service
   3. Model your data with types
   4. Make illegal states unrepresentable
   5. Handle failure with `Result` (new)
   6. Reach out with capabilities (new)
   7. Add a stateful agent
   8. Split into contexts (new)
   9. Test it
   10. Deploy it (new)
3. **Concepts**, at `/book/concepts/`: the explanation pages, moved from the
   guides and About:
   the type-system philosophy, literal admission, the capability model, sends,
   joins and timeouts, the agent model, invariants as contracts, events, actors
   and access, localisation, the testing philosophy, why compile to TypeScript,
   and the two comparison pages.
4. **Guides**: how-to pages only, grouped by what you are building: data and
   types; handling requests; callers and access; storing data; calling out; AI
   and search; reliability; agents and state; testing; projects and deployment;
   localisation.

Each tutorial chapter ends with links onward: its concept page, its guides, its
reference page and an example.

### 4.3 Reference

One surface with its own contents, sections in this order: Language,
Architecture, Adapters, Tooling (today's Developer Docs), Diagnostics,
Specification, Releases. The Specification stays normative and whole.

## 5. Links and redirects

Bynk is pre-1.0 and has almost no users to protect from a moved page, so this policy
optimises for a docs structure that can keep changing.

- **L1. The compiler links by code.** Every documentation link the toolchain
  emits for a diagnostic is `https://bynk-lang.org/d/<code>`: the editor's
  clickable code, `bynk explain`, and the MCP tools. The compiler holds no page
  paths and no heading anchors for diagnostics.
- **L2. The site serves every code.** `/d/<code>` is one rule in
  `site/public/_redirects`, `/d/* /reference/diagnostics/:splat 302`, to a
  generated page that exists for every registry code (§6). The code is the
  lookup key, so there is no table to maintain; if the diagnostic pages ever
  move, only this rule changes. A temporary (302) redirect keeps browsers from
  caching the target.
- **L3. Other fixed links are few and short.** The handful of other URLs the
  toolchain hard-codes (the diagnostic index, *Upgrading*, the first-party
  capabilities page) use stable short paths of the same kind. `bynk book`
  already derives its URLs from the bundled Book and needs nothing. There is
  no general key registry.
- **L4. A folder move gets one splat redirect**
  (`/book/reference/* /reference/:splat 301`) for inbound links from the blog,
  the repository and search engines. There are no per-page guarantees and no
  redirect test; a single moved page gets a single rule only where something
  is known to link to it. Pages caps `_redirects` at 2,000 static and 100
  dynamic rules, which this policy stays far below.
- **L5. Links inside the repository are rewritten in the same slice**:
  `keywords.rs`, `first_party_docs.rs`, `xtask`, tests, CI path filters,
  `llms.txt`, and the blog's links in its own next release.
- **L6. Guides keep their URLs.** Regrouping happens in `SUMMARY.md` only.
  Concepts pages move to `/book/concepts/` once slice 4 has landed, with one
  rule each (§14).

## 6. Diagnostics, one page per code

The Diagnostic index is already generated from `bynk-syntax/src/diagnostics.rs`,
which also holds the curated `bynk explain` texts. Extend the same generator to
emit a page for **every** registry code at `/reference/diagnostics/<code>/`:
the summary and severity, and, for an explained code, the explanation, the
compiled example pair and the fix. An unexplained code's page says so, so
`/d/<code>` (L2) never 404s.

Each page ends with links to the reference sections that govern the code.
Those targets are docs structure, so they move out of the compiler:
`Explain` loses `page` and `anchor`, and the site keeps a data file
(`site/src/data/diagnostic-links.json`, code → one or more site paths). The
site build fails when a registry code has no page or a link does not resolve,
which keeps the drift guard the `page`/`anchor` fields provide today.
`Explain::href` and `diagnostics::href` return `/d/<code>`.

Fold each troubleshooting page's content into its code's `Explain` entry,
then retire `book/troubleshooting/` behind one splat redirect. `bynk explain`,
the editor's Explain panel and the website then show the same text from one
source.

## 7. Releases

Replace the single changelog page with one page per release tag under
`/reference/releases/<tag>/` (93 tags so far), plus an index, newest first.
Each page lists the increments the release contains. `xtask stamp` writes each
increment's row into `/reference/releases/unreleased/`; cutting a release
renames that page to the new tag and starts an empty one, as a step of the
release process in the release discipline. *Upgrading* stays a
Reference page. A notable release can also get a blog post that links to its
page.

## 8. One brand

- **A theme package** in its own repository,
  [bynk-lang/site-theme](https://github.com/bynk-lang/site-theme), published to
  GitHub Packages as `@bynk-lang/site-theme`: CSS custom properties generated
  from the design system's tokens (dark and light), the shared header, the
  `<head>` with fonts, and the Bynk TextMate grammar, as a Starlight plugin.
  The docs site and the blog both depend on a released version. (It was planned
  at `site/theme/` in this repository; it moved out so it could ship while this
  repository was busy.) The blog's CI reads it
  with its `GITHUB_TOKEN` (`packages: read`), and local installs need a token
  with `read:packages` in `~/.npmrc`; the blog's README says how.
- **The reader's colour scheme decides.** The site follows
  `prefers-color-scheme`, with a toggle that overrides it and is remembered.
  With no preference it falls back to dark, the design system's default. Both
  themes are first-class: every token has a light and a dark value.
- **The palette changes.** The indigo accent is retired. Amber marks the single
  primary action and the current page; teal is for links and success. This
  reconciles the site with the bento-blocks mark, as the design system's README
  asks.
- **Search across both sites.** Each site keeps its own Pagefind index; the
  docs site merges the blog's (Pagefind `mergeIndex`), so one search box covers
  both.

## 9. Checks

- **Contents completeness.** `generate-sidebar.mjs --check` also fails when a
  page under `book/` (or `reference/`) is in no contents file.
- **Diagnostic pages.** Every registry code has a page, and every target in
  `diagnostic-links.json` resolves (§6).
- Existing checks continue: link validation, British English, glossary links,
  spec sections, `llms.txt` links, the docs-consistency tests.

## 10. Migration and coexistence

Slices land in an order where the site is whole after each one. Structure-only
slices (1–2) move no URL. Slice 5 is a plain move of pages: the Reference
section with splat redirects, and the Concepts pages to `/book/concepts/` with
one rule each. Once slice 4 has landed, the compiler names none of these
pages, so the move does not touch it. The theme slices (6–7)
are independent of the Book slices and can run in parallel. The site shell
(slice 8) waits for slice 5, because its header, footer and home page link to
the `/reference/` URLs that slice 5 creates. The tutorial
chapters (9–11) are independent of each other once slice 2 has landed.

**Chapter files carry no number.** A new chapter's file is named by its
subject (`tutorials/failure-and-result.md`, `tutorials/capabilities.md`,
`tutorials/contexts.md`, `tutorials/deploy.md`), and its place in the order
comes from `SUMMARY.md` alone, so slices 9–11 move no URL and collide with no
existing file. Slice 12 renames the six existing numbered files
(`01-first-program.md` and so on) to unnumbered names, with one redirect each,
so a chapter's URL never changes when chapters are added again.

The back-half feature freeze ([ADR 0876](../decisions/0876-back-half-feature-freeze.md))
allows design work, so this doc can settle during it. Most slices change only
the site. Three touch the toolchain: slice 3 changes `xtask stamp`, and slice 4
changes the URL the editor, `bynk explain` and the MCP tools show, and the
`Explain` struct. None changes the language. Under ADR 0876, whether slices 3
and 4 may be built before the freeze lifts is the maintainer's decision.

## 11. Risks

- **A code with no page.** Mitigated by generating a page for every registry
  code and failing the site build when one is missing (§6).
- **Broken inbound links.** Mitigated by a splat redirect per moved folder
  (L4).
- **Tutorial drift.** Four new chapters extend one running project; each
  chapter's code is compiled by `doc_examples` like the existing ones.
- **Two contents files diverge.** Both are generated by the same script under
  the same `--check`.
- **Blog lags the theme.** The blog pins a theme version; a theme release notes
  breaking changes.
- **Package access.** GitHub Packages needs a token even to read, so a missing
  `read:packages` token stops a blog install. The blog's README and CI
  configuration carry the setup.

## 12. Slices

| # | Slice | Moves URLs | Depends on |
|---|---|---|---|
| 0 | Settle this doc; land the two ADRs (§13) | No | — |
| 1 | Contents complete: list the 18 missing pages; add the completeness check | No | 0 |
| 2 | Regroup the sidebar into Start, Learn Bynk, Concepts, Guides; replace "How these docs are organised" with a short Book front page | No | 1 |
| 3 | Releases: one page per release tag and an `unreleased` page that `xtask stamp` writes; a release cut renames it; redirect the changelog URL | Yes | 0 |
| 4 | Diagnostics: a generated page for every code; the `/d/<code>` contract (L1–L3); `page`/`anchor` move from `Explain` to the site's data file; fold in troubleshooting | Yes | 0 |
| 5 | Reference consolidation: move `book/reference`, `book/spec` and `/docs/` to `/reference/`; its own contents; sidebar surfaces; splat redirects. Move the Concepts pages to `/book/concepts/`, one rule each. Repository links rewritten (L5) | Yes | 2, 3, 4 |
| 6 | Theme package from the design system tokens, published to GitHub Packages; light and dark following the reader's setting; docs site adopts it | No | 0 |
| 7 | Blog adopts the theme; cross-site search | No | 6 |
| 8 | Site shell: header, footer and home page per the mockups | No | 5, 6 |
| 9 | Learn Bynk 5: Handle failure with `Result` (`tutorials/failure-and-result.md`) | No | 2 |
| 10 | Learn Bynk 6: Reach out with capabilities (`tutorials/capabilities.md`) | No | 2 |
| 11 | Learn Bynk 8: Split into contexts (`tutorials/contexts.md`) | No | 2 |
| 12 | Learn Bynk 10: Deploy it (`tutorials/deploy.md`); rename the six numbered chapter files to unnumbered names, with a redirect each (§10) | Yes (six renamed chapter files) | 9–11 |
| 13 | Retirement: Project pages for contributing and roadmap; close the spine | Yes | all |

## 13. Decisions (ADRs to land up front)

- **D1. Information architecture.** Top level by kind (Learn, Concepts,
  Guides, Reference), concern within each. Supersedes the concern-first
  organisation described in *How these docs are organised*: concern-first made
  Guides a mix of explanation and how-to with no reading order.
- **D2. Compiler links name codes, never pages.** L1 to L3 (§5): the
  toolchain links diagnostics as `/d/<code>`, the site serves a page for every
  code, and the code-to-section mapping belongs to the docs. Folder moves get
  splat redirects (L4); nothing promises a published URL forever before 1.0.

Recorded in their slices: one theme package, in its own repository (slice 6); diagnostics
pages generated from the registry (slice 4); one release page per tag
(slice 3).

## 14. Settled questions

Settled with the maintainer on 21 November 2026.

1. **Concepts pages move to `/book/concepts/`.** URLs and breadcrumbs match
   the sidebar. The move waits for slice 4, after which the compiler no longer
   links to them, and lands in slice 5 with one rule per page.
2. **Learn Bynk grows chapter 2's service.** The existing chapters stay; the
   four new chapters extend the same service, so no chapter is rewritten
   around a different project.
3. **One release page per release tag**, not per minor version: about 90
   pages so far rather than about 640, matching how the release discipline
   cuts releases. Increments between tags collect on an `unreleased` page.
4. **The theme package is published to GitHub Packages**, under the
   `bynk-lang` organisation. Reading it needs a token, which §8 and §11 cover.
5. **The site follows the reader's colour scheme**, with a remembered toggle
   and dark as the fallback.
