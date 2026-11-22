# 0882 — The compiler links a diagnostic by its code, at /d/<code>, and the site decides where that goes

- **Status:** Accepted (v0.640.15)

**Context.** The toolchain linked a diagnostic to the Book in two ways. A code
with a curated explanation linked the Book page and heading that `Explain`
named (`page` and `anchor`, 798 pairs in `bynk-syntax`); any other code linked
its category's heading on the generated diagnostic index. The editor showed a
link only for an explained code. So the compiler held the site's structure: a
page moved or a heading renamed meant a compiler change, and a link printed by
an installed toolchain broke when the site was reorganised, which the web
presence track (#3861) is about to do. The track's link policy (L1–L3) says a
link that leaves the repository names a thing, not a page.

**Decision.** The toolchain links every code as
`https://bynk-lang.org/d/<code>` (`diagnostics::href`, `Explain::href`), and a
string that is not a registered code as `/d/`, the index. The site redirects
`/d/<code>` (a 302, so the target can move) to a page it builds for every
registry code, at `/book/reference/diagnostics/<code>/`. The page shows the
code's summary and severity, its explanation and example where it has one (or
says it has none), its category on the index, and the Book pages that explain
it. `Explain` loses `page` and `anchor`; the code-to-page links move to the
site's `site/src/data/diagnostic-links.json`. The compiler renders the pages'
data (`render_json`, committed as `site/src/generated/diagnostics.json`), and
a docs-consistency test fails when that file is stale or when a link names a
code outside the registry or a page or heading that does not exist. The LSP
gives every registered code a `codeDescription`, not only explained ones.

Rejected: keeping `page` and `anchor` in the compiler and adding the
redirect. The compiler would still need a change for every move. Rejected:
pointing `/d/<code>` at the Book page directly, with no page per code. Codes
without an explanation (about a tenth) would have nowhere to go, and a code
explained in two places could link only one.

**Consequences.** A link printed by any released toolchain keeps working
while the site is reorganised: only `_redirects` and the data file change.
Every code now has a link, in the editor too. Writing a section that explains
a code means adding its link to the data file, which the test checks. The
build makes about 900 more pages (a local build went from 28 to 36 seconds).
`bynk explain` prints the `/d/` link in place of the Book page's. Proposed in
#3876.
