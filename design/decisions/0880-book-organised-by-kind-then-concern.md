# 0880 — The Book is organised by kind first, then by concern

- **Status:** Accepted (v0.640.12)

**Context.** The Book was organised concern-first: after the tutorial, one
Guides section per concern (the type system, program structure, effects and
capabilities, and seven more), each holding that concern's explanation pages
and its how-to pages side by side. As the Book grew, that made Guides its
largest part, 114 pages, with "Effects & capabilities" alone listing 29, and
the explanation pages scattered through ten sections with no reading order.
Each concern's landing page already separated the two kinds under an
**Understand** heading, but the sidebar did not. A reader could not see which
part to read to learn an idea and which to read to get a task done, and a new
page had no clear home beyond its concern.

**Decision.** The top level of the Book is the kind of page, following
[Diátaxis](https://diataxis.fr/): **Start** and **Learn Bynk** are read in
order; **Concepts** holds the explanation pages; **Guides** holds the how-to
pages; the **Reference**, **Specification** and **Troubleshooting** are kept
whole for looking things up. Concern is the second level: Concepts groups its
pages by concern, each concern's landing page as its overview, and Guides
groups its pages by the task a reader is doing (data and types, handling
requests, storing data, reliability, and so on), not by the language feature
the task uses. The contents, `site/src/SUMMARY.md`, decide where a page shows;
a page's file stays in its concern's folder, so the regroup moves no URL.

Rejected: keeping the concern-first sections and only adding headings within
them (the explanation pages would stay scattered, and Guides would stay one
114-page part); one top-level part per task group (twenty top-level parts);
writing a new overview page for each task group (content the structure doesn't
need: Starlight groups need no page).

**Consequences.** Concepts can be read in order, like a second book. Guides
stays large but is split into twelve groups a reader can scan by task. A new
page's part follows from its kind, and its group from its concern or task. A
Concepts page's URL still says `/book/guides/` until the web presence track's
Reference consolidation moves the Concepts pages to `/book/concepts/`. The
page that described the concern-first organisation, *How these docs are
organised*, is retired; the Book's front page says how the Book is laid out.
Proposed in #3865, as decision D1 of the web presence track (#3861).
