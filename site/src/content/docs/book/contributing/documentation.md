---
title: Working on the docs
---
This Book lives in `site/` and is built with [Astro](https://astro.build/) and
[Starlight](https://starlight.astro.build/), deployed to Cloudflare Pages at
[bynk-lang.org](https://bynk-lang.org). The pages themselves are Markdown under
`site/src/content/docs/book/`; the authoring conventions below — grammar
includes, diagnostic transcripts, callouts, diagrams — are expanded at build
time by a small remark plugin, so writing a page is mostly writing prose.

It is organised **by concern** (see
[How these docs are organised](/book/introduction/how-these-docs-are-organised/)):
a guided tutorial spine, then one **Guides** section per concern that co-locates
that topic's explanation and how-to pages, with the **Reference** and
**Specification** kept whole as lookup catalogues. [Diátaxis](https://diataxis.fr/)
still governs each page — one mode per page — it just sits inside a concern now.

## Build and preview locally

Everything runs from `site/`:

```sh
npm install            # one-time
npm run dev            # live preview at localhost:4321
npm run build          # full production build (the gate CI runs)
npm run preview        # serve the built site to spot-check it
```

`npm run build` is `astro build`. It expands the authoring directives, renders
the diagrams, builds the Pagefind search index, and runs the
[`starlight-links-validator`](https://github.com/HiDeoo/starlight-links-validator)
plugin, so a broken in-site link **fails the build**. The link validator is
wired in `site/astro.config.mjs` alongside the directive plugin, the diagram
renderer, and the `bynk` syntax highlighter.

> [!TIP]
> Diagrams render through a headless Chromium. If a build complains it cannot
> launch a browser, install one once with `npx playwright install chromium`
> (CI runs the same command).

Highlighting for `bynk` fenced blocks comes from a TextMate grammar
(`site/src/grammars/bynk.tmLanguage.json`), a copy of the VS Code extension's
([bynk-lang/vscode-bynk](https://github.com/bynk-lang/vscode-bynk)), so code on the page matches the editor.
Refresh the copy from the extension when its grammar changes.

## The sidebar

The sidebar is **generated**, not hand-maintained. `site/src/SUMMARY.md` is the
Book's authored table of contents; `site/scripts/generate-sidebar.mjs` turns it
into the committed `site/src/generated/sidebar.json` that `astro.config.mjs`
imports. After editing `SUMMARY.md`, regenerate and commit:

```sh
node site/scripts/generate-sidebar.mjs            # rewrite sidebar.json
node site/scripts/generate-sidebar.mjs --check     # CI: fail if it drifted
```

The `--check` mode runs in CI, so a `SUMMARY.md` edit that you forget to
regenerate fails the build.

## Diagrams and callouts

**Diagrams.** Write a fenced ` ```mermaid ` block. At build time `rehype-mermaid`
renders it to **inline SVG** — offline, no client JS, no CDN. **Accessibility
rule — required:** every diagram carries a *caption* and a *text equivalent* in
the surrounding prose. No information may live only in a picture; a reader who
cannot see the diagram must still get the full meaning from the text.

**Callouts.** Write a GitHub-style alert blockquote; the directive plugin turns
it into a Starlight aside. Exactly four kinds, each with a fixed meaning — use
them for what they say, not for decoration:

| Callout | Aside | Means |
|---|---|---|
| `> [!NOTE]` | note | an aside or clarification |
| `> [!TIP]` | tip | a better or faster way |
| `> [!WARNING]` | caution | easy to get wrong; proceed carefully |
| `> [!DANGER]` | danger | will break, or is forbidden |

```text
> [!WARNING]
> Body text — ordinary Markdown, rendered normally.
```

## Embedding a grammar production

A reference page can embed one grammar production by name. Put a line whose only
content is the directive:

```text
{{#grammar http_handler}}
```

The plugin replaces it with an EBNF code block holding that production, rendered
from the committed `site/src/generated/grammar.json` (the same source the
[grammar appendix](/book/reference/grammar-appendix/) is built from) so it cannot
drift from the parser. That JSON is itself a drift-guarded render of the grammar
— `bynk-grammar/tests/generated_grammar_json.rs` regenerates and checks it — so
the rendered production is **generated, never hand-edited**. An unknown rule name
throws at build time, so a typo cannot silently vanish.

## Embedding a construct's static semantics

A production says what *parses*; the diagnostics say what is *legal beyond
parsing*. Embed the diagnostics that constrain a construct with:

```text
{{#grammar-semantics http_handler}}
```

The plugin replaces it with a bullet list of the governing diagnostics, read
from `site/src/generated/grammar-semantics.json` — the rule → diagnostics map.
That file is generated from the `grammar_symbol` field on each diagnostic in the
registry and drift-guarded by `bynkc/tests/generated_site_json.rs`, which also
checks every `grammar_symbol` names a real grammar rule. A construct with no
diagnostics yields a neutral line rather than failing, since an unconstrained
production is legitimate; to add or change a mapping, edit `grammar_symbol` and
re-bless.

The annotated reference ([`reference/grammar.md`](/book/reference/grammar/)) must
cover every production: `bynkc/tests/grammar_coverage.rs` asserts that each
embeddable grammar rule has exactly one `{{#grammar <rule>}}` entry with a
matching `{#rule-<rule>}` heading anchor (a custom heading id — see below), and
that every directive argument names a real rule. So a new production cannot ship
without a documented entry, and the diagnostic index's **Construct** column
deep-links to `grammar#rule-<rule>` and always resolves.

> [!NOTE]
> The plugin honours custom heading ids: `## Heading {#custom-id}` renders a
> heading whose anchor is `custom-id`. The grammar reference's `{#rule-…}`
> anchors are exactly this, which is how the cross-page deep links stay stable.

## Showing a real diagnostic

To show what the compiler actually says when it refuses a program — verbatim, not
paraphrased — add a deliberately failing fixture and `{{#include}}` both it and
its captured transcript:

1. Write a standalone failing program at `site/src/diagnostics/<id>.bynk` (a
   `commons` or `context` block, like a doc example, but one that must error).
2. Run `BYNK_BLESS=1 cargo test -p bynkc --test it doc_diagnostics::`. This compiles
   the fixture, asserts it fails, and writes the real diagnostic — colour-free,
   with a stable `<id>.bynk` label — to `site/src/diagnostics/<id>.txt`.
3. On the page, show the source in a `bynk,fail` fence and the transcript in a
   `text` fence, each holding a single `{{#include}}` line pointing at the
   fixture and its transcript — e.g. `{{#include ../../diagnostics/<id>.bynk}}`.
   The plugin resolves the `diagnostics/…` suffix against `site/src/` (the
   `includeBase` set in `astro.config.mjs`), so only the `diagnostics/` part of
   the path matters. See
   [the agent model](/book/guides/agents-and-state/the-agent-model/) for a live
   example to copy.

The `.txt` transcripts are **generated — never hand-edit them**;
`doc_diagnostics` (run in CI) re-derives them from `bynkc` and fails if the
committed copy drifts, and fails if a fixture ever starts compiling. The fixtures
live outside the Book pages, so the doc-example gate skips a fenced block whose
body is only an `{{#include}}` (it is display-only; the fixture's own compile is
what `doc_diagnostics` checks).

> [!NOTE]
> The plugin strips the comma list from a fence tag — `bynk,fail` highlights as
> `bynk` — so the failing-example flag is conveyed by the prose and the paired
> error transcript, not by a special render.

### The before/after device

On **explanation** pages, pair the refusal with the bug it prevents — the most
persuasive shape in the Book. Two panels:

- **The bug that ships.** A short, idiomatic `typescript` block that genuinely
  compiles *with* the exact bug Bynk targets. Tag it `typescript` (the
  doc-example gate ignores it) and keep it honest — it must really compile.
- **The program that won't build.** The Bynk equivalent via the mechanism above:
  a `bynk,fail` fixture include, then the generated transcript.

Weave it into the prose where the page already *asserts* the bug, so the
demonstration replaces the assertion rather than bolting on. Keep it to
explanation pages; reference stays dry. See
[the type-system philosophy](/book/guides/type-system/philosophy/) for the
device in use.

## The guardrails

Several mechanisms keep the docs honest. Some run inside the site build; the
rest are Rust drift-guard tests and Node `--check` scripts that CI runs.

1. **Every example compiles.** `bynkc/tests/doc_examples.rs` extracts every
   fenced ```` ```bynk ```` block from `site/src/content/docs/book/**` and
   compiles it — `commons` blocks in-process, `context` blocks as a temp
   project, and a block with neither header inside a synthetic `commons` (or
   `context`). Annotate blocks that should not be compiled as-is:
   - ```` ```bynk,fragment ```` — a partial snippet (a single member, a statement,
     or code naming types declared elsewhere on the page). It must still
     **parse**, inside the unit, agent, service, function or test body it comes
     from;
   - ```` ```bynk,fail ```` — a negative example that must fail to compile;
     ```` ```bynk,fail=<code> ```` also requires that diagnostic code, so a page
     explaining one error cannot drift onto another;
   - ```` ```bynk,ignore ```` — pseudo-syntax only (`<Name>` placeholders, `…`
     elisions), which is not Bynk at all.

   Bynk uses `//` for comments; a retired `--` comment fails the gate
   (`bynk.lex.legacy_comment`).

2. **Generated reference is generated.** Four reference pages are emitted from the
   compiler/grammar and guarded by tests, so they cannot drift:

   | Page | Source | Test |
   |---|---|---|
   | `reference/diagnostics.md` | `bynkc/src/diagnostics.rs` registry | `diagnostics_registry.rs` |
   | `reference/keywords.md` | `bynkc/src/keywords.rs` (lexer keywords) | `keywords_reference.rs` |
   | `reference/cli.md` | the clap command tree (`bynkc/src/cli.rs`) | `cli_reference.rs` |
   | `reference/grammar-appendix.md` | `tree-sitter-bynk/src/grammar.json` | `grammar_reference.rs` |

   Each test renders the page and asserts it matches the committed file. They
   carry a *"GENERATED FILE — do not edit by hand"* banner. Regenerate after a
   relevant change:

   ```sh
   BYNK_BLESS=1 cargo test -p bynkc --test it -- diagnostics_registry:: \
                                                   keywords_reference:: \
                                                   cli_reference:: \
                                                   grammar_reference::
   ```

   The two generated JSON artifacts behind the directives —
   `site/src/generated/grammar.json` and `grammar-semantics.json` — are guarded
   the same way (`bynk-grammar`'s `generated_grammar_json` and `bynkc`'s
   `generated_site_json`); re-bless them with `BYNK_BLESS=1` too.

3. **Link checking.** `starlight-links-validator` validates internal links on
   every `astro build`. Separately, `site/scripts/check-llms-links.mjs` confirms
   each `/book/` route named in the hand-authored `llms.txt` index still maps to
   a real page.

4. **Drift checks.** `node site/scripts/generate-sidebar.mjs --check` and
   `node site/scripts/build-llms-full.mjs --check` fail CI if the committed
   `sidebar.json` or `llms-full.txt` no longer matches its source (see
   *Machine-readable Book*, below).

5. **Version banners.** `bynkc/tests/doc_version.rs` checks the Book's
   current-version banners; `scripts/bump-version.sh` rewrites them, so there is
   no manual bump step (see *Docs ship with the feature*).

## Machine-readable Book

Two files under `site/public/` give tools a text view of the Book:

- **`llms.txt`** is the **hand-authored** curated index — a short primer plus
  links into the Book. Because it is hand-authored it can drift, so
  `check-llms-links.mjs` link-checks its `/book/` routes.
- **`llms-full.txt`** is **generated** by `site/scripts/build-llms-full.mjs`: the
  whole Book concatenated in sidebar order with the authoring directives
  expanded against the same committed data the site uses, so it cannot drift from
  what renders. Regenerate with the script and commit; `--check` guards it in CI.

```sh
node site/scripts/build-llms-full.mjs            # rewrite public/llms-full.txt
node site/scripts/build-llms-full.mjs --check     # CI: fail if out of date
```

## Style

- **One Diátaxis mode per page.** No explanation inside a tutorial; no how-to
  steps inside reference. Link outward to siblings instead of duplicating.
- **British English.** Run `site/scripts/check-british-english.sh` to flag US
  spellings in the Book's prose (it ignores fenced code). It is an advisory lint,
  not a CI gate — treat British spelling as a review-time convention.
- **Document the present.** Write what compiles today; mark planned features as
  planned.
- **Payloadless sums use the `enum` form.** A sum whose variants all carry no
  payload is written `enum { A, B, C }` in examples — it is the canonical spelling
  ([`types`](/book/reference/types/#sum-types)). Reserve the `| A | B` pipe form
  for sums where a variant carries a payload (or the grammar reference teaching the
  `sum_type` production itself). The two are the same type; the convention keeps
  examples uniform.

### The four voices

"One mode per page" is also a rule about *voice*: the same fact should sound
different in each mode. Here is "an agent's state must be zeroable", written four
ways — read them as a tuning fork before you draft a page.

- **Tutorial** (warm, "we", a guaranteed path): "We'll give the counter a
  `count` field. Bynk needs a starting value for a brand-new key, so every state
  field must have a zero — `Int`'s is `0`, so we're set. Run it and watch a fresh
  counter read `0`."
- **How-to** (imperative, goal-first): "Keep every agent state field zeroable:
  use a type with an implicit zero (`Int`, `Bool`, `String`, `Option[T]`), or
  give the field an initialiser (`field: T = value`)."
- **Reference** (neutral, terse): "Each agent `state` field must be zeroable — it
  has an implicit zero value or an initialiser. A non-zeroable field without an
  initialiser is rejected (`bynk.agents.non_zeroable_state_field`)."
- **Explanation** (discursive, a view): "Why insist on a zero? A fresh key has no
  stored state, and no constructor was required first, so the agent must still
  come into being with a defined value. Zeroability is how Bynk makes 'never seen
  before' honest instead of undefined."

## Glossary first-use linking

The [glossary](/book/reference/glossary/) gives each load-bearing term a stable
anchor, `#term-<slug>` (e.g. `#term-refined-type`). On a reader-facing page, link
the **first** occurrence of a glossary term to its entry —
`[refined type](/book/reference/glossary/#term-refined-type)` — and only the first;
never inside a heading, a code fence, or on the glossary page itself.

`site/scripts/check-glossary-links.sh` is an **advisory** lint: it lists, per page,
glossary terms that appear with no link to their entry, so first-use linking can
be caught up page by page. It prints findings and exits 0 (set
`GLOSSARY_LINK_STRICT=1` to exit non-zero on findings). It deliberately does not
auto-link — terms are common words, so a human decides each first use.

## The language specification

The [Bynk Language Specification](/book/spec/) lives in
`site/src/content/docs/book/spec/`. It is the **normative** definition of the
**current language**, updated in place per increment, distinct in register from
the friendly [grammar reference](/book/reference/grammar/): the reference is
per-construct lookup, the spec is the complete citable definition. The two share
their generated facts.

It is **translation-defined** — syntax by the grammar, static semantics by the
`bynk.*` well-formedness rules, dynamic meaning by emission plus the runtime
contract — and it **reuses the existing machinery**: it embeds `{{#grammar}}`
productions and `{{#grammar-semantics}}` diagnostics just like the reference (the
rendered output is shared from one source, so there is no drift), and every
example is covered by the doc-example gate. It adds nothing of its own.

**Keeping the spec current.** A language or grammar increment updates the
**affected spec chapters** and records each language-defining call as a
**decision record** in `design/decisions/` — it does *not* spawn a standalone
instalment document. An increment's *design draft* is a **transient proposal** in
`design/proposals/`: merged for sign-off before implementation, consumed by it,
and deleted by the PR that lands the increment (the lifecycle is documented in
that directory's README). Much of the spec stays current for free: the
`{{#grammar}}` productions (§3/§4/§11) and the `{{#grammar-semantics}}` diagnostic
links (§5) re-render from the grammar and the registry, so syntax and the
diagnostic catalogue never drift. The **prose** is hand-maintained — when
behaviour changes, review §5 (static semantics), §6 (the type system), §7 / §7.4
(emission and the runtime library), §8 (compilation), §10 (conformance), and
Appendix B (version history).

**Verify against the compiler.** Specification and reference claims are checked
against the **actual compiler** — the emitter, the checker, and the fixtures —
never against older design documents. Cite the historical design notes for
*intent* only, never for current behaviour.

## Docs ship with the feature

Treat docs as part of an increment's definition of done (see
[Testing & fixtures](/book/contributing/testing/)): update the affected reference
(regenerating the generated pages), update the affected
[specification](/book/spec/) chapters (per *Keeping the spec current* above), and
add a changelog entry. The Book's current-version banners are single-sourced —
`scripts/bump-version.sh` rewrites them (and regenerates `llms-full.txt`) and
`bynkc/tests/doc_version.rs` fails CI on drift, so there is no manual
version-bump step. Finally, check that any touched tutorial or guide still
compiles under the doc-example gate, and run `npm run build` from `site/` to
confirm the page renders and its links resolve.
