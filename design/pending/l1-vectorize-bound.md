---
level: patch
changelog: A Vectorize index listing that stops before its end (100 full pages, or a page that repeats an earlier one) now fails the deploy, naming why, instead of answering with the indexes it saw, so an index it could not see is never read as absent
---

## ADR: a-cut-short-listing-is-an-error
title: An account listing cut short before its end is an error, never a shorter list
summary: the Vectorize index listing, like the queues lookup since 1D.4d, fails when the page cap or a repeated page stops it before the account says it has ended

**Context.** 1D.4c (#4568) bounded the Vectorize index listing, which the API reference documents no paging for: at most `MAX_PAGES` (100) pages, and a stop at a page that brings no new name. A listing stopped that way still answered with the names it had seen. An index past the stop then read as absent. Today that fails safe (its create is refused and reported), but a later `--prune` reading "absent" as "forget it" would leave a live index unrecorded. 1D.4d (#4582) already made the queues lookup fail in the same case.

**Decision.** When the listing is stopped while the account still says there is more (`total_pages` beyond the page, or a full page with none given), by the cap or by a repeated page, `vectorize_names` fails with `the account's Vectorize index listing could not be read to its end (<why>), so which indexes exist is not known`. A short or empty last page is the listing's end, and its names are the answer, as before.

**Consequences.** No lookup over an account listing answers "absent" for what it could not see. A deploy against a listing that misbehaves this way stops, saying why, rather than going on with a partial view. The request sequences are unchanged.
