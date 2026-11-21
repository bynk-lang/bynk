# Web presence — the Book reorganised, Reference consolidated, one brand across docs and blog

- **Status:** Settling. Spine issue not yet opened; this doc arrives by a
  settling draft PR. No slice is accepted.
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
  information architecture, and the URL and redirect policy.
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
2. **The URL policy.** Which URLs move, how old ones keep working, and what may
   never break.
3. **One brand.** A shared theme, built from the design system's tokens, used by
   the docs site and the blog.

### 1.2 Why a track (the ADR 0076 trigger)

- **It spans many increments.** Some slices are mechanical (contents, redirects,
  a generator change); four are new tutorial chapters.
- **Its surface is not settled.** The structure is a proposal; §14 lists what
  remains open.
- **It makes a hard-to-reverse commitment.** The compiler emits about 790
  documentation links (§2.2), and released compilers keep emitting them. Every
  URL this track moves must redirect for good.

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
| `bynk-syntax/src/diagnostics.rs` | ~610 links into `/book/reference/`, ~129 into `/book/guides/`, ~50 into `/book/spec/`; also generates `reference/diagnostics.md` and holds the `bynk explain` texts |
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
- No link breaks: every moved URL redirects permanently, and the compiler's own
  links point at the new places.
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
2. **Learn Bynk**: ten chapters, one running project:
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
3. **Concepts**: the explanation pages, gathered from the guides and About:
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

## 5. URLs and redirects

The load-bearing commitment.

- **R1. A URL that has been published redirects permanently.** Released
  compilers and editor extensions emit `/book/...` links indefinitely, so a
  redirect is never removed.
- **R2. Redirects live in `site/public/_redirects`** (Cloudflare Pages), using
  splat rules where a whole tree moves (`/book/reference/* /reference/:splat 301`)
  and single rules otherwise. Astro's `redirects` option stays for the one
  existing entry and is not grown.
- **R3. A moved page's links are rewritten at source in the same slice**:
  `diagnostics.rs`, `keywords.rs`, `first_party_docs.rs`, `xtask`, tests, CI path
  filters and `llms.txt`. The redirect covers old compilers, not this one.
- **R4. A redirect test**: a committed list of every URL published before the
  move, checked in CI to resolve (directly or by one redirect) in the built
  site.
- **R5. Guides keep their URLs.** Regrouping happens in `SUMMARY.md` only.
  Whether Concepts pages move to `/book/concepts/` is open (§14).

## 6. Diagnostics, one page per code

The Diagnostic index is already generated from `bynk-syntax/src/diagnostics.rs`,
which also holds the curated `bynk explain` texts. Extend the same generator to
emit a page per explained code at `/reference/diagnostics/<code>/`: the summary,
the explanation, the compiled example pair, and the fix. Fold each
troubleshooting page's content into its code's `Explain` entry, then retire
`book/troubleshooting/` behind redirects. `bynk explain`, the editor's Explain
panel and the website then show the same text from one source.

## 7. Releases

Replace the single changelog page with one page per minor version under
`/reference/releases/`, plus an index. `xtask stamp` writes the row into the
current minor's page instead of `reference/changelog.md`. *Upgrading* stays a
Reference page. A notable release can also get a blog post that links to its
page.

## 8. One brand

- **A theme package** at `site/theme/`, published as `@bynk-lang/site-theme`:
  CSS custom properties generated from the design system's tokens (dark and
  light), the shared header and footer, the `<head>` with fonts, and the Bynk
  TextMate grammar. The docs site uses it from the workspace; the blog depends
  on a released version.
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
- **Redirects** (R4).
- Existing checks continue: link validation, British English, glossary links,
  spec sections, `llms.txt` links, the docs-consistency tests.

## 10. Migration and coexistence

Slices land in an order where the site is whole after each one. Structure-only
slices (1–2) move no URL. The Reference move (slice 5) is the single large
mechanical slice, and its redirects land in the same PR. The theme slices (6–8)
are independent of the Book slices and can run in parallel. The tutorial
chapters (9–12) are independent of each other once slice 2 has landed.

The back-half feature freeze (`bynk` features, from v0.639.1) does not block
this track: no slice changes language or compiler behaviour. Slices that touch
`bynk-syntax` or `xtask` change only documentation links, the docs generator
and release tooling.

## 11. Risks

- **Broken compiler links.** Mitigated by R1 to R4.
- **Tutorial drift.** Four new chapters extend one running project; each
  chapter's code is compiled by `doc_examples` like the existing ones.
- **Two contents files diverge.** Both are generated by the same script under
  the same `--check`.
- **Blog lags the theme.** The blog pins a theme version; a theme release notes
  breaking changes.

## 12. Slices

| # | Slice | Moves URLs | Depends on |
|---|---|---|---|
| 0 | Settle this doc; land the two ADRs (§13) | No | — |
| 1 | Contents complete: list the 18 missing pages; add the completeness check | No | 0 |
| 2 | Regroup the sidebar into Start, Learn Bynk, Concepts, Guides; replace "How these docs are organised" with a short Book front page | No | 1 |
| 3 | Releases: per-minor pages, `xtask stamp` writes them; redirect the changelog URL | Yes | 0 |
| 4 | Diagnostics: generated per-code pages; fold in troubleshooting; redirects | Yes | 0 |
| 5 | Reference consolidation: move `book/reference`, `book/spec` and `/docs/` to `/reference/`; its own contents; sidebar surfaces; source link rewrite; redirect test | Yes | 3, 4 |
| 6 | Theme package from the design system tokens; docs site adopts it | No | 0 |
| 7 | Blog adopts the theme; cross-site search | No | 6 |
| 8 | Site shell: header, footer and home page per the mockups | No | 5, 6 |
| 9 | Learn Bynk 5: Handle failure with `Result` | No | 2 |
| 10 | Learn Bynk 6: Reach out with capabilities | No | 2 |
| 11 | Learn Bynk 8: Split into contexts | No | 2 |
| 12 | Learn Bynk 10: Deploy it; renumber chapters 5–9 | Yes (renumbered chapter files) | 9–11 |
| 13 | Retirement: Project pages for contributing and roadmap; close the spine | Yes | all |

## 13. Decisions (ADRs to land up front)

- **D1. Information architecture.** Top level by kind (Learn, Concepts,
  Guides, Reference), concern within each. Supersedes the concern-first
  organisation described in *How these docs are organised*: concern-first made
  Guides a mix of explanation and how-to with no reading order.
- **D2. URL and redirect policy.** R1 to R5 (§5).

Recorded in their slices: one theme package in this repo (slice 6); diagnostics
pages generated from the registry (slice 4); per-minor release pages
(slice 3).

## 14. Open questions

1. **Do Concepts pages move to `/book/concepts/`?** Moving gives honest URLs
   and breadcrumbs, but costs redirects and compiler link rewrites for any that
   `diagnostics.rs` targets. Keeping them where they are costs nothing now.
2. **The running project for Learn Bynk.** Extend chapter 2's service through
   all ten chapters, or choose a project from `examples/` that already has
   contexts, an agent and a deploy story?
3. **Releases granularity.** Per minor version (about 640 pages so far) or
   grouped by release tag per the release discipline?
4. **Does the Specification keep `/book/spec/` under a redirect, or move to
   `/reference/spec/`?** Recommendation: move, for one Reference surface.
5. **Where the theme package is published** (npm or GitHub Packages), and how
   the blog is told about a new version.
6. **Light theme default.** The design system makes dark the default; reading
   pages may suit light better. Follow the reader's system setting?
